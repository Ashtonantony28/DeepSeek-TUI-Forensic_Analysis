//! AST-aware chunker (extension 3.4 — cAST style).
//!
//! Splits a source file into self-contained units (functions, classes,
//! methods) using language-specific tree-sitter node kinds. Each chunk
//! carries its byte range, line range, kind, and best-effort name so it
//! can be re-displayed with file:line references.
//!
//! Files in unrecognised languages fall back to a single whole-file chunk
//! — this preserves the contract that callers can chunk *any* file even
//! if the result is coarser.

use camino::Utf8Path;
use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Parser};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeChunk {
    pub path: String,
    pub kind: String,
    pub name: Option<String>,
    pub start_line: u32,
    pub end_line: u32,
    pub byte_start: u32,
    pub byte_end: u32,
    pub text: String,
    pub language: String,
}

impl CodeChunk {
    pub fn file_only(path: &Utf8Path, source: &str) -> Self {
        Self {
            path: path.to_string(),
            kind: "file".into(),
            name: None,
            start_line: 1,
            end_line: source.lines().count() as u32,
            byte_start: 0,
            byte_end: source.len() as u32,
            text: source.to_string(),
            language: "text".into(),
        }
    }
}

/// Chunk `source` into top-level definitions for the file's language.
pub fn chunk_file(path: &Utf8Path, source: &str) -> Vec<CodeChunk> {
    let Some((lang, lang_name, queries)) = language_for_path(path) else {
        return vec![CodeChunk::file_only(path, source)];
    };
    let mut parser = Parser::new();
    if parser.set_language(&lang).is_err() {
        return vec![CodeChunk::file_only(path, source)];
    }
    let Some(tree) = parser.parse(source, None) else {
        return vec![CodeChunk::file_only(path, source)];
    };
    let mut out: Vec<CodeChunk> = Vec::new();
    let mut cursor = tree.walk();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        let kind = node.kind();
        if queries.contains(&kind) {
            let start = node.start_position();
            let end = node.end_position();
            let byte_start = node.start_byte();
            let byte_end = node.end_byte().min(source.len());
            let text = source[byte_start..byte_end].to_string();
            let name = extract_name(&node, source);
            out.push(CodeChunk {
                path: path.to_string(),
                kind: kind.to_string(),
                name,
                start_line: (start.row + 1) as u32,
                end_line: (end.row + 1) as u32,
                byte_start: byte_start as u32,
                byte_end: byte_end as u32,
                text,
                language: lang_name.into(),
            });
        } else {
            for child in node.children(&mut cursor) {
                stack.push(child);
            }
        }
    }
    if out.is_empty() {
        return vec![CodeChunk::file_only(path, source)];
    }
    out.sort_by_key(|c| c.byte_start);
    out
}

fn extract_name(node: &Node, source: &str) -> Option<String> {
    // Common pattern across grammars: a child named `name` or `identifier`.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let k = child.kind();
        if k == "name" || k == "identifier" || k == "type_identifier" || k == "field_identifier" {
            let bs = child.start_byte();
            let be = child.end_byte().min(source.len());
            if bs < be {
                return Some(source[bs..be].to_string());
            }
        }
    }
    // Fall back: scan named children for a child whose field name is "name".
    for i in 0..node.named_child_count() {
        if let Some(child) = node.named_child(i) {
            if node.field_name_for_child(i as u32).is_some() {
                let k = child.kind();
                if k == "identifier" || k == "type_identifier" {
                    let bs = child.start_byte();
                    let be = child.end_byte().min(source.len());
                    if bs < be {
                        return Some(source[bs..be].to_string());
                    }
                }
            }
        }
    }
    None
}

type LangSpec = (tree_sitter::Language, &'static str, &'static [&'static str]);

fn language_for_path(path: &Utf8Path) -> Option<LangSpec> {
    let ext = path.extension()?.to_lowercase();
    match ext.as_str() {
        "rs" => Some((
            tree_sitter_rust::language(),
            "rust",
            &[
                "function_item",
                "impl_item",
                "struct_item",
                "enum_item",
                "trait_item",
            ],
        )),
        "py" => Some((
            tree_sitter_python::language(),
            "python",
            &["function_definition", "class_definition"],
        )),
        "js" | "jsx" | "mjs" | "cjs" => Some((
            tree_sitter_javascript::language(),
            "javascript",
            &[
                "function_declaration",
                "class_declaration",
                "method_definition",
                "arrow_function",
            ],
        )),
        "ts" => Some((
            tree_sitter_typescript::language_typescript(),
            "typescript",
            &[
                "function_declaration",
                "class_declaration",
                "method_definition",
                "interface_declaration",
            ],
        )),
        "tsx" => Some((
            tree_sitter_typescript::language_tsx(),
            "tsx",
            &[
                "function_declaration",
                "class_declaration",
                "method_definition",
            ],
        )),
        "go" => Some((
            tree_sitter_go::language(),
            "go",
            &[
                "function_declaration",
                "method_declaration",
                "type_declaration",
            ],
        )),
        "java" => Some((
            tree_sitter_java::language(),
            "java",
            &[
                "class_declaration",
                "method_declaration",
                "interface_declaration",
            ],
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;

    #[test]
    fn rust_file_chunks_into_items() {
        let src = "pub fn a() {}\npub fn b(x: i32) -> i32 { x + 1 }\n";
        let chunks = chunk_file(&Utf8PathBuf::from("x.rs"), src);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].language, "rust");
        assert_eq!(chunks[0].name.as_deref(), Some("a"));
        assert_eq!(chunks[1].name.as_deref(), Some("b"));
        assert!(chunks[1].text.contains("x + 1"));
    }

    #[test]
    fn python_file_chunks_into_defs_and_classes() {
        let src = "def foo():\n    return 1\n\nclass Bar:\n    def baz(self):\n        return 2\n";
        let chunks = chunk_file(&Utf8PathBuf::from("x.py"), src);
        // Class-level chunk + top-level def. `baz` is nested inside `Bar`
        // and won't appear as its own top-level chunk (cAST behaviour).
        assert!(chunks.iter().any(|c| c.name.as_deref() == Some("foo")));
        assert!(chunks.iter().any(|c| c.name.as_deref() == Some("Bar")));
    }

    #[test]
    fn unknown_language_falls_back_to_file_chunk() {
        let chunks = chunk_file(&Utf8PathBuf::from("notes.foo"), "blah");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].kind, "file");
    }
}
