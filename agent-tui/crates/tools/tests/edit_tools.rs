use agent_tui_tools::tools::builtins::{EditFileTool, ReadFileTool, WriteFileTool};
use agent_tui_tools::{Tool, ToolContext};
use camino::Utf8PathBuf;
use serde_json::json;

fn ctx() -> (tempfile::TempDir, ToolContext) {
    let td = tempfile::tempdir().unwrap();
    let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
    (td, ToolContext::new(root))
}

#[tokio::test]
async fn write_then_read_roundtrips() {
    let (_td, c) = ctx();
    WriteFileTool
        .execute(json!({"path": "a.txt", "content": "hello"}), &c)
        .await
        .unwrap();
    let out = ReadFileTool
        .execute(json!({"path": "a.txt"}), &c)
        .await
        .unwrap();
    assert_eq!(out.content, "hello");
}

#[tokio::test]
async fn write_rust_with_bad_syntax_errors() {
    let (_td, c) = ctx();
    let res = WriteFileTool
        .execute(
            json!({"path": "broken.rs", "content": "fn main() { let x = "}),
            &c,
        )
        .await;
    assert!(res.is_err(), "expected syntax error");
}

#[tokio::test]
async fn write_rust_with_good_syntax_ok() {
    let (_td, c) = ctx();
    WriteFileTool
        .execute(
            json!({"path": "ok.rs", "content": "fn main() { let x = 1; }\n"}),
            &c,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn write_unknown_extension_ok_even_if_garbage() {
    let (_td, c) = ctx();
    WriteFileTool
        .execute(json!({"path": "notes.foo", "content": "@@@!"}), &c)
        .await
        .unwrap();
}

#[tokio::test]
async fn edit_rolls_back_on_syntax_failure() {
    let (_td, c) = ctx();
    let initial = "fn main() { let x = 1; }\n";
    WriteFileTool
        .execute(json!({"path": "lib.rs", "content": initial}), &c)
        .await
        .unwrap();
    let res = EditFileTool
        .execute(
            json!({
                "path": "lib.rs",
                "old_text": "let x = 1;",
                "new_text": "let x = "
            }),
            &c,
        )
        .await;
    assert!(res.is_err());
    let after = std::fs::read_to_string(c.workspace_root.join("lib.rs").as_std_path()).unwrap();
    assert_eq!(after, initial, "edit should have rolled back");
}
