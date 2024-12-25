use regex::Regex;
use lazy_static::lazy_static;

const COMMENT_CHAR: &str = "#";

lazy_static! {
    static ref HOSTNAME_RE: Regex = {
        Regex::new(r"^(([a-zA-Z0-9]|[a-zA-Z0-9][a-zA-Z0-9_\-]*[a-zA-Z0-9])\.)*([A-Za-z0-9]|[A-Za-z0-9][A-Za-z0-9_\-]*[A-Za-z0-9])$")
            .expect("Hardcoded regex should compile")
    };
}

pub fn parse_blocklist(content: &str) -> Vec<&str> {
    content.lines()
        .flat_map(|line| extract_hostname(line))
        .collect()
}

pub fn parse_allowlist(content: &str) -> Vec<&str> {
    content.lines()
        .map(|line| strip_comments(line).trim())
        .filter(|line| !line.is_empty() && is_hostname(line))
        .collect()
}

fn extract_hostname(line: &str) -> Option<&str> {
    let cleaned = strip_comments(line).trim();
    let tokens = cleaned.split_whitespace().take(2).collect::<Vec<&str>>();
    let hostname = tokens.last()
        .filter(|h| is_hostname(h));

    if hostname.is_none() && !cleaned.is_empty() {
        log::warn!("skipping line: {}", line);
    }

    hostname.copied()
}

fn strip_comments(line: &str) -> &str {
    line.find(COMMENT_CHAR)
        .map(|i| &line[..i])
        .unwrap_or(line)
}

fn is_hostname(token: &str) -> bool {
    HOSTNAME_RE.is_match(token) && token != "localhost"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_blocklist() {
        let input = vec![
            "#comment",
            "d1.example.com # comment",
            "127.0.0.1 d2.example.com #comment",
            "example.com", 
            "127.0.0.1 d3.example.com", 
            "www.zdjecie-facebook-zdj3425jeio.dkonto.pl",
        ].join("\n");

        let expected = vec![
            "d1.example.com",
            "d2.example.com",
            "example.com",
            "d3.example.com",
            "www.zdjecie-facebook-zdj3425jeio.dkonto.pl",
        ];

        assert_eq!(parse_blocklist(&input), expected);
    }
}
