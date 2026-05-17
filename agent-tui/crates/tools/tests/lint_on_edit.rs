//! Integration tests for extension 3.2 — Linter-on-edit ACI.
//!
//! The plan requires asserting three properties:
//!   1. A `write_file` returning Rust code with a syntax error is rejected
//!      with a useful error.
//!   2. A `write_file` returning correct Rust is accepted.
//!   3. A `write_file` of a non-Rust file (no recognised grammar) is
//!      accepted unconditionally.
//!
//! Note on the "missing semicolon" case from the plan: Rust's tree-sitter
//! grammar is forgiving enough that some missing-semicolon cases parse
//! cleanly as expression-final statements. The first test below uses an
//! incomplete-expression form (`let x = ;`) that tree-sitter does flag —
//! this is the SWE-agent ACI's failure mode (broken-but-plausible edits)
//! that we want to catch.

use agent_tui_tools::tools::builtins::WriteFileTool;
use agent_tui_tools::{Tool, ToolContext};
use camino::Utf8PathBuf;
use serde_json::json;

fn ctx() -> (tempfile::TempDir, ToolContext) {
    let td = tempfile::tempdir().unwrap();
    let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
    (td, ToolContext::new(root))
}

#[tokio::test]
async fn broken_rust_is_rejected_with_useful_error() {
    let (_td, c) = ctx();
    let res = WriteFileTool
        .execute(
            json!({
                "path": "lib.rs",
                "content": "fn main() { let x = ; }\n"
            }),
            &c,
        )
        .await;
    let err = res.expect_err("syntax-broken Rust must be rejected");
    let msg = format!("{err}");
    assert!(
        msg.contains("rust") || msg.contains("syntax") || msg.contains("parse"),
        "error should explain the failure, got: {msg}"
    );
    assert!(
        msg.contains("line") || msg.contains(':'),
        "error should include a position hint, got: {msg}"
    );
}

#[tokio::test]
async fn well_formed_rust_is_accepted() {
    let (_td, c) = ctx();
    WriteFileTool
        .execute(
            json!({
                "path": "lib.rs",
                "content": "pub fn add(a: i32, b: i32) -> i32 { a + b }\n"
            }),
            &c,
        )
        .await
        .expect("well-formed Rust must be accepted");
}

#[tokio::test]
async fn unknown_language_is_accepted_unconditionally() {
    let (_td, c) = ctx();
    // `.foo` extension has no grammar registered; even garbage content
    // must pass the syntax check.
    WriteFileTool
        .execute(
            json!({
                "path": "notes.foo",
                "content": "@@@!! this is not any known language @@@\n"
            }),
            &c,
        )
        .await
        .expect("unknown extension must be accepted unconditionally");
}
