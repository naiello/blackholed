use regex::Regex;
use lazy_static::lazy_static;

const COMMENT_CHAR: &str = "#";

lazy_static! {
    static ref HOSTNAME_RE: Regex = {
        Regex::new(r"^(([a-zA-Z0-9]|[a-zA-Z0-9][a-zA-Z0-9\-]*[a-zA-Z0-9])\.)*([A-Za-z0-9]|[A-Za-z0-9][A-Za-z0-9\-]*[A-Za-z0-9])$")
            .expect("Hardcoded regex should compile")
    };
}

pub fn parse_blocklist(content: &str) -> Vec<&str> {
    content.lines()
        .flat_map(|line| extract_hostname(line))
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
            "example.com", 
            "127.0.0.1 sub2.example.com", 
            "invalid_host"
        ].join("\n");

        let expected = vec![
            "example.com",
            "sub2.example.com",
        ];

        assert_eq!(parse_blocklist(&input), expected);
    }
}
