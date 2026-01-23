use regex::Regex;

use crate::api::error::ApiError;

/// Validates a source ID
/// - Must contain only alphanumeric characters and hyphens
/// - Must be between 1 and 64 characters
pub fn validate_source_id(id: &str) -> Result<(), ApiError> {
    if id.is_empty() || id.len() > 64 {
        return Err(ApiError::BadRequest(
            "Source ID must be between 1 and 64 characters".to_string(),
        ));
    }

    let re = Regex::new(r"^[a-zA-Z0-9-]+$").unwrap();
    if !re.is_match(id) {
        return Err(ApiError::BadRequest(
            "Source ID must contain only alphanumeric characters and hyphens".to_string(),
        ));
    }

    Ok(())
}

/// Validates and normalizes a domain name
/// - Ensures trailing dot is present
/// - Validates wildcard depth (must be at least 2 layers)
/// - Rejects invalid patterns like `*.com` or `*.`
pub fn validate_and_normalize_domain(name: &str) -> Result<String, ApiError> {
    if name.is_empty() {
        return Err(ApiError::BadRequest(
            "Domain name cannot be empty".to_string(),
        ));
    }

    let mut normalized = name.to_string();

    // Add trailing dot if missing
    if !normalized.ends_with('.') {
        normalized.push('.');
    }

    // Check for wildcard patterns
    if normalized.starts_with("*.") {
        // Count the number of labels (dots minus 1)
        let dot_count = normalized.matches('.').count();

        // For "*.example.com." we have 3 dots, which is 2 labels after the wildcard
        // We need at least 2 labels after the wildcard (e.g., "*.com." is invalid)
        if dot_count < 3 {
            return Err(ApiError::BadRequest(
                "Wildcard domains must have at least 2 labels (e.g., *.example.com)".to_string(),
            ));
        }
    }

    // Basic validation: check for invalid characters
    // DNS names can only contain alphanumeric, dots, hyphens, and wildcards
    let re = Regex::new(r"^(\*\.)?[a-zA-Z0-9.-]+\.$").unwrap();
    if !re.is_match(&normalized) {
        return Err(ApiError::BadRequest(
            "Domain name contains invalid characters".to_string(),
        ));
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_source_id() {
        assert!(validate_source_id("test").is_ok());
        assert!(validate_source_id("test-123").is_ok());
        assert!(validate_source_id("Test-123").is_ok());
        assert!(validate_source_id("").is_err());
        assert!(validate_source_id("test_123").is_err()); // underscore not allowed
        assert!(validate_source_id("test 123").is_err()); // space not allowed
    }

    #[test]
    fn test_validate_domain() {
        assert_eq!(
            validate_and_normalize_domain("example.com").unwrap(),
            "example.com."
        );
        assert_eq!(
            validate_and_normalize_domain("example.com.").unwrap(),
            "example.com."
        );
        assert_eq!(
            validate_and_normalize_domain("*.example.com").unwrap(),
            "*.example.com."
        );
        assert!(validate_and_normalize_domain("*.com").is_err());
        assert!(validate_and_normalize_domain("*.").is_err());
        assert!(validate_and_normalize_domain("").is_err());
    }
}
