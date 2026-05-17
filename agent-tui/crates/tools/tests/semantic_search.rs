//! Integration test for extension 3.4 — `semantic_search` tool wired through
//! the hybrid retriever (lexical + graph signals; embeddings are skipped
//! when Ollama isn't running, which is the common case in CI).

use agent_tui_tools::tools::builtins::SemanticSearchTool;
use agent_tui_tools::{Tool, ToolContext, ToolRegistry};
use camino::Utf8PathBuf;
use serde_json::json;

#[tokio::test]
async fn semantic_search_returns_lexical_hits_without_ollama() {
    let td = tempfile::tempdir().unwrap();
    let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
    std::fs::create_dir_all(root.join("src").as_std_path()).unwrap();
    std::fs::write(
        root.join("src/auth.rs"),
        "pub fn authenticate_user(name: &str) -> bool { name == \"root\" }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        "use auth;\nfn main() { auth::authenticate_user(\"x\"); }\n",
    )
    .unwrap();

    let mut ctx = ToolContext::new(root);
    ToolRegistry::enable_hybrid_retrieval(&mut ctx).await;

    let out = SemanticSearchTool
        .execute(json!({"query": "authenticate", "top_k": 5}), &ctx)
        .await
        .expect("semantic_search must succeed even without embeddings");
    assert!(!out.is_error, "semantic_search should not flag an error");
    assert!(
        out.content.contains("auth.rs"),
        "expected auth.rs in the results, got: {}",
        out.content
    );
}

#[tokio::test]
async fn semantic_search_errors_when_retriever_absent() {
    let td = tempfile::tempdir().unwrap();
    let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
    let ctx = ToolContext::new(root);
    let res = SemanticSearchTool
        .execute(json!({"query": "anything"}), &ctx)
        .await;
    assert!(
        res.is_err(),
        "no retriever -> tool should signal NotAvailable"
    );
}
