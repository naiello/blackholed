use anyhow::{bail, Context, Result};
use hickory_server::proto::rr::LowerName;
use std::str::FromStr;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};

/// Parsed host entry from a blocklist/allowlist source
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHost {
    pub name: LowerName,
}

/// Parse a blocklist/allowlist from an AsyncRead source
///
/// Parsing rules:
/// - Lines starting with '#' are comments (ignored)
/// - Lines like "0.0.0.0 blockme.com" - ignore first field, use second
/// - Lines like "blockme.com" - use domain directly
/// - Wildcards like "*.blockme.com" are allowed
/// - Wildcards like "*.com" (less than 2 layers) are dropped
/// - Trailing '.' is added if missing
pub async fn parse_list<R: AsyncBufRead + Unpin>(reader: R) -> Result<Vec<ParsedHost>> {
    let mut reader = BufReader::new(reader);
    let mut hosts = Vec::new();
    let mut line = String::new();
    let mut line_number = 0;

    loop {
        line.clear();
        line_number += 1;

        let bytes_read = reader
            .read_line(&mut line)
            .await
            .context("Failed to read line from source")?;

        if bytes_read == 0 {
            break; // EOF
        }

        match parse_line(&line) {
            Ok(Some(host)) => hosts.push(host),
            Ok(None) => {} // Skip (comment, empty, or invalid wildcard)
            Err(e) => {
                log::warn!(
                    "Failed to parse line {}: {} - {}",
                    line_number,
                    line.trim(),
                    e
                );
            }
        }
    }

    log::debug!("Parsed {} hosts from source", hosts.len());
    Ok(hosts)
}

/// Parse a single line from a blocklist/allowlist
fn parse_line(line: &str) -> Result<Option<ParsedHost>> {
    // Trim whitespace
    let line = line.trim();

    // Skip empty lines
    if line.is_empty() {
        return Ok(None);
    }

    // Skip comments
    if line.starts_with('#') {
        return Ok(None);
    }

    // Split by whitespace and '#' for inline comments
    let parts: Vec<&str> = line
        .split('#')
        .next()
        .unwrap_or("")
        .split_whitespace()
        .collect();

    if parts.is_empty() {
        return Ok(None);
    }

    // Determine which field to use
    let domain_str = if parts.len() >= 2 {
        // Format: "0.0.0.0 blockme.com" or "127.0.0.1 blockme.com"
        // Use the second field
        parts[1]
    } else {
        // Format: "blockme.com"
        parts[0]
    };

    // Validate and normalize the domain
    let normalized = normalize_domain(domain_str)?;

    // Check wildcard depth (must be at least 2 layers deep)
    if is_invalid_wildcard(&normalized) {
        log::trace!("Skipping invalid wildcard: {}", domain_str);
        return Ok(None);
    }

    // Parse as LowerName
    let name = LowerName::from_str(&normalized)
        .with_context(|| format!("Invalid domain name: {}", normalized))?;

    Ok(Some(ParsedHost { name }))
}

/// Normalize a domain by ensuring it has a trailing '.'
fn normalize_domain(domain: &str) -> Result<String> {
    let trimmed = domain.trim();

    if trimmed.is_empty() {
        bail!("Empty domain");
    }

    if trimmed.ends_with('.') {
        Ok(trimmed.to_string())
    } else {
        Ok(format!("{}.", trimmed))
    }
}

/// Check if a wildcard is invalid (less than 2 layers deep)
/// Examples:
/// - "*.com." is invalid (1 layer)
/// - "*.example.com." is valid (2 layers)
fn is_invalid_wildcard(domain: &str) -> bool {
    if !domain.starts_with("*.") {
        return false;
    }

    // Count the number of '.' characters
    // "*.com." has 2 dots (invalid)
    // "*.example.com." has 3 dots (valid)
    let dot_count = domain.chars().filter(|&c| c == '.').count();

    dot_count < 3
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[tokio::test]
    async fn test_parse_simple_domain() {
        let content = "example.com\n";
        let reader = Cursor::new(content);
        let hosts = parse_list(reader).await.unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name.to_string(), "example.com.");
    }

    #[tokio::test]
    async fn test_parse_with_sinkhole_ip() {
        let content = "0.0.0.0 blocked.com\n127.0.0.1 alsoblocked.com\n";
        let reader = Cursor::new(content);
        let hosts = parse_list(reader).await.unwrap();
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].name.to_string(), "blocked.com.");
        assert_eq!(hosts[1].name.to_string(), "alsoblocked.com.");
    }

    #[tokio::test]
    async fn test_parse_with_comments() {
        let content = "# This is a comment\nexample.com\n0.0.0.0 blocked.com # inline comment\n";
        let reader = Cursor::new(content);
        let hosts = parse_list(reader).await.unwrap();
        assert_eq!(hosts.len(), 2);
    }

    #[tokio::test]
    async fn test_parse_wildcard_valid() {
        let content = "*.example.com\n";
        let reader = Cursor::new(content);
        let hosts = parse_list(reader).await.unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name.to_string(), "*.example.com.");
    }

    #[tokio::test]
    async fn test_parse_wildcard_invalid() {
        let content = "*.com\n";
        let reader = Cursor::new(content);
        let hosts = parse_list(reader).await.unwrap();
        assert_eq!(hosts.len(), 0); // Should be dropped
    }

    #[tokio::test]
    async fn test_parse_adds_trailing_dot() {
        let content = "example.com\n";
        let reader = Cursor::new(content);
        let hosts = parse_list(reader).await.unwrap();
        assert!(hosts[0].name.to_string().ends_with('.'));
    }

    #[tokio::test]
    async fn test_parse_empty_lines() {
        let content = "example.com\n\n\nblocked.com\n";
        let reader = Cursor::new(content);
        let hosts = parse_list(reader).await.unwrap();
        assert_eq!(hosts.len(), 2);
    }

    #[tokio::test]
    async fn test_parse_domain_already_has_trailing_dot() {
        let content = "example.com.\n";
        let reader = Cursor::new(content);
        let hosts = parse_list(reader).await.unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name.to_string(), "example.com.");
    }

    #[tokio::test]
    async fn test_parse_multiple_entries() {
        let content = "example.com\nbad.com\nevil.org\n";
        let reader = Cursor::new(content);
        let hosts = parse_list(reader).await.unwrap();
        assert_eq!(hosts.len(), 3);
    }

    #[test]
    fn test_normalize_domain() {
        assert_eq!(normalize_domain("example.com").unwrap(), "example.com.");
        assert_eq!(normalize_domain("example.com.").unwrap(), "example.com.");
        assert_eq!(normalize_domain("  example.com  ").unwrap(), "example.com.");
    }

    #[test]
    fn test_normalize_domain_empty() {
        assert!(normalize_domain("").is_err());
        assert!(normalize_domain("   ").is_err());
    }

    #[test]
    fn test_is_invalid_wildcard() {
        assert!(is_invalid_wildcard("*.com."));
        assert!(!is_invalid_wildcard("*.example.com."));
        assert!(!is_invalid_wildcard("example.com."));
        assert!(!is_invalid_wildcard("*.sub.example.com."));
    }
}
