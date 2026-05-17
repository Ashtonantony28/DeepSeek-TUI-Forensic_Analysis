//! Redact API-key-shaped strings from arbitrary text before logging or
//! sending into the model context.

use regex::Regex;
use std::sync::OnceLock;

static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();

fn patterns() -> &'static [Regex] {
    PATTERNS.get_or_init(|| {
        vec![
            Regex::new(r"sk-ant-[A-Za-z0-9_-]{20,}").unwrap(),
            Regex::new(r"sk-[A-Za-z0-9]{20,}").unwrap(),
            Regex::new(r"gsk_[A-Za-z0-9]{30,}").unwrap(),
            Regex::new(r"xai-[A-Za-z0-9_-]{20,}").unwrap(),
            Regex::new(r"AKIA[0-9A-Z]{16}").unwrap(),
            Regex::new(r"ghp_[A-Za-z0-9]{30,}").unwrap(),
        ]
    })
}

pub fn redact_secrets(input: &str) -> String {
    let mut out = input.to_string();
    for re in patterns() {
        out = re.replace_all(&out, "[REDACTED]").to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_anthropic_key() {
        let s = "X=sk-ant-abcdefghijklmnopqrstuvwxyz0123456789";
        assert!(redact_secrets(s).contains("[REDACTED]"));
    }

    #[test]
    fn passes_innocent_text() {
        assert_eq!(redact_secrets("hello world"), "hello world");
    }
}
