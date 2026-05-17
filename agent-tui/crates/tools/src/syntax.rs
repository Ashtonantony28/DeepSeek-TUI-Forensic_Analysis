//! Tree-sitter syntax check for edit/write tools.
//!
//! Recognised by file extension; unrecognised extensions pass through
//! (the rule from the build plan: "no recognised grammar → accept
//! unconditionally").

use camino::Utf8Path;
use tree_sitter::{Parser, Tree};

#[derive(Debug, Clone)]
pub struct SyntaxError {
    pub language: &'static str,
    pub message: String,
    pub row: usize,
    pub column: usize,
}

/// Returns Ok(()) on success or no-recognised-grammar.
/// Returns Err with diagnostic details if the language is recognised
/// AND the file fails to parse without errors.
pub fn check_file(path: &Utf8Path, source: &str) -> Result<(), SyntaxError> {
    let (lang, name) = match language_for_path(path) {
        Some(p) => p,
        None => return Ok(()),
    };
    let mut parser = Parser::new();
    if parser.set_language(&lang).is_err() {
        return Ok(());
    }
    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => return Ok(()),
    };
    if let Some(err) = first_error_node(&tree) {
        let start = err.start_position();
        return Err(SyntaxError {
            language: name,
            message: format!(
                "{} parse error: unexpected `{}` at {}:{}",
                name,
                err.kind(),
                start.row + 1,
                start.column + 1,
            ),
            row: start.row + 1,
            column: start.column + 1,
        });
    }
    Ok(())
}

fn first_error_node(tree: &Tree) -> Option<tree_sitter::Node<'_>> {
    let mut cursor = tree.walk();
    let mut stack = vec![tree.root_node()];
    while let Some(n) = stack.pop() {
        if n.is_error() || n.is_missing() {
            return Some(n);
        }
        for child in n.children(&mut cursor) {
            stack.push(child);
        }
    }
    None
}

fn language_for_path(path: &Utf8Path) -> Option<(tree_sitter::Language, &'static str)> {
    let ext = path.extension()?.to_lowercase();
    match ext.as_str() {
        "rs" => Some((tree_sitter_rust::language(), "rust")),
        "py" => Some((tree_sitter_python::language(), "python")),
        "js" | "jsx" | "mjs" | "cjs" => Some((tree_sitter_javascript::language(), "javascript")),
        "ts" => Some((tree_sitter_typescript::language_typescript(), "typescript")),
        "tsx" => Some((tree_sitter_typescript::language_tsx(), "tsx")),
        "go" => Some((tree_sitter_go::language(), "go")),
        "c" | "h" => Some((tree_sitter_c::language(), "c")),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => Some((tree_sitter_cpp::language(), "cpp")),
        "java" => Some((tree_sitter_java::language(), "java")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;

    #[test]
    fn good_rust_passes() {
        let path = Utf8PathBuf::from("lib.rs");
        assert!(check_file(&path, "fn main() { let x = 1; }").is_ok());
    }

    #[test]
    fn bad_rust_fails() {
        let path = Utf8PathBuf::from("lib.rs");
        let err = check_file(&path, "fn main() { let x = }").unwrap_err();
        assert_eq!(err.language, "rust");
    }

    #[test]
    fn unknown_ext_passes() {
        let path = Utf8PathBuf::from("notes.foo");
        assert!(check_file(&path, "@@@!").is_ok());
    }

    #[test]
    fn good_python_passes() {
        let path = Utf8PathBuf::from("a.py");
        assert!(check_file(&path, "def f():\n    return 1\n").is_ok());
    }

    #[test]
    fn bad_python_fails() {
        let path = Utf8PathBuf::from("a.py");
        assert!(check_file(&path, "def f(:\n    return 1\n").is_err());
    }
}
