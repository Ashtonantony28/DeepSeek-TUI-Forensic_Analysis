//! Multi-stage recovery parser for tool inputs (ANALYSIS.md §6).
//!
//! Order:
//!   1. Try raw JSON parse on `input_buffer`.
//!   2. Strip markdown code fences and retry.
//!   3. Double-decode (JSON-encoded string of JSON).
//!   4. Extract first balanced `{...}` (or `[...]`) segment.

use serde_json::Value;

pub fn parse_tool_input(buf: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(buf) {
        // Tools always want an object/array; if we got a bare string,
        // try the double-decode path.
        match &v {
            Value::Object(_) | Value::Array(_) => return Some(v),
            Value::String(s) => {
                if let Ok(inner) = serde_json::from_str::<Value>(s) {
                    if matches!(inner, Value::Object(_) | Value::Array(_)) {
                        return Some(inner);
                    }
                }
            }
            _ => {}
        }
    }
    let stripped = strip_code_fences(buf);
    if stripped != buf {
        if let Ok(v) = serde_json::from_str::<Value>(&stripped) {
            return Some(v);
        }
    }
    if let Ok(inner) = serde_json::from_str::<String>(buf) {
        if let Ok(v) = serde_json::from_str::<Value>(&inner) {
            return Some(v);
        }
    }
    if let Some(seg) = extract_balanced(buf, '{', '}') {
        if let Ok(v) = serde_json::from_str::<Value>(&seg) {
            return Some(v);
        }
    }
    if let Some(seg) = extract_balanced(buf, '[', ']') {
        if let Ok(v) = serde_json::from_str::<Value>(&seg) {
            return Some(v);
        }
    }
    None
}

fn strip_code_fences(s: &str) -> String {
    let trimmed = s.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        // Drop language tag up to the first newline.
        let after_lang = rest.split_once('\n').map(|(_, b)| b).unwrap_or(rest);
        if let Some(end) = after_lang.rfind("```") {
            return after_lang[..end].trim().to_string();
        }
    }
    trimmed.to_string()
}

fn extract_balanced(s: &str, open: char, close: char) -> Option<String> {
    let bytes: Vec<char> = s.chars().collect();
    let mut depth = 0i32;
    let mut start: Option<usize> = None;
    let mut in_str = false;
    let mut esc = false;
    for (i, c) in bytes.iter().enumerate() {
        if in_str {
            if esc {
                esc = false;
            } else if *c == '\\' {
                esc = true;
            } else if *c == '"' {
                in_str = false;
            }
            continue;
        }
        if *c == '"' {
            in_str = true;
            continue;
        }
        if *c == open {
            if depth == 0 {
                start = Some(i);
            }
            depth += 1;
        } else if *c == close {
            depth -= 1;
            if depth == 0 {
                let s_start = start?;
                let chunk: String = bytes[s_start..=i].iter().collect();
                return Some(chunk);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plain_json() {
        let v = parse_tool_input(r#"{"a": 1}"#).unwrap();
        assert_eq!(v, json!({"a":1}));
    }

    #[test]
    fn fenced_json() {
        let s = "```json\n{\"a\":1}\n```";
        let v = parse_tool_input(s).unwrap();
        assert_eq!(v, json!({"a":1}));
    }

    #[test]
    fn double_encoded() {
        let s = r#""{\"a\":1}""#;
        let v = parse_tool_input(s).unwrap();
        assert_eq!(v, json!({"a":1}));
    }

    #[test]
    fn extracts_embedded() {
        let s = "noise before {\"a\":1} noise after";
        let v = parse_tool_input(s).unwrap();
        assert_eq!(v, json!({"a":1}));
    }

    #[test]
    fn returns_none_on_garbage() {
        assert!(parse_tool_input("@@@").is_none());
    }
}
