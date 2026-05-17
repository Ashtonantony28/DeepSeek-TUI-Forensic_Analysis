//! Embedding-backed retriever (extension 3.4).
//!
//! Uses Ollama's `/api/embeddings` endpoint by default with
//! `nomic-embed-text`. Vectors are kept in memory and mirrored to
//! `.agent-tui/index/embeddings.json` so incremental rebuilds skip files
//! whose mtime hasn't changed since the last index.
//!
//! When Ollama isn't reachable, `embed` returns an error; callers (the
//! hybrid retriever) silently drop the embeddings retriever and continue
//! with the chunker + graph signals. Plan §risk #14.

use crate::chunker::{chunk_file, CodeChunk};
use camino::{Utf8Path, Utf8PathBuf};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::UNIX_EPOCH;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EmbedError {
    #[error("ollama unreachable: {0}")]
    Network(String),
    #[error("ollama returned non-success status {0}")]
    BadStatus(u16),
    #[error("serde: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("no embedding returned")]
    Empty,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddedChunk {
    pub chunk: CodeChunk,
    pub vector: Vec<f32>,
    pub source_mtime: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmbeddingIndex {
    pub model: String,
    pub chunks: Vec<EmbeddedChunk>,
}

pub struct EmbeddingClient {
    pub base_url: String,
    pub model: String,
    http: reqwest::Client,
}

impl EmbeddingClient {
    pub fn new() -> Self {
        let base_url = std::env::var("OLLAMA_HOST")
            .unwrap_or_else(|_| "http://localhost:11434".into());
        let model = std::env::var("AGENT_TUI_EMBED_MODEL")
            .unwrap_or_else(|_| "nomic-embed-text".into());
        Self {
            base_url,
            model,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .expect("reqwest client"),
        }
    }

    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        #[derive(Serialize)]
        struct Req<'a> { model: &'a str, prompt: &'a str }
        #[derive(Deserialize)]
        struct Resp { embedding: Vec<f32> }
        let url = format!("{}/api/embeddings", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .json(&Req { model: &self.model, prompt: text })
            .send()
            .await
            .map_err(|e| EmbedError::Network(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(EmbedError::BadStatus(status.as_u16()));
        }
        let r: Resp = resp.json().await.map_err(|e| EmbedError::Network(e.to_string()))?;
        if r.embedding.is_empty() {
            return Err(EmbedError::Empty);
        }
        Ok(r.embedding)
    }

    /// Quick liveness check. Used by the hybrid retriever to decide whether
    /// to include the embedding signal at startup.
    pub async fn is_reachable(&self) -> bool {
        let url = format!("{}/api/tags", self.base_url.trim_end_matches('/'));
        self.http
            .get(&url)
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }
}

impl Default for EmbeddingClient {
    fn default() -> Self { Self::new() }
}

pub fn index_path(root: &Utf8Path) -> Utf8PathBuf {
    root.join(".agent-tui").join("index").join("embeddings.json")
}

pub fn load_index(root: &Utf8Path) -> EmbeddingIndex {
    let p = index_path(root);
    if !p.exists() {
        return EmbeddingIndex::default();
    }
    std::fs::read_to_string(p.as_std_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_index(root: &Utf8Path, idx: &EmbeddingIndex) -> Result<(), EmbedError> {
    let p = index_path(root);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent.as_std_path())?;
    }
    let body = serde_json::to_string(idx)?;
    std::fs::write(p.as_std_path(), body)?;
    Ok(())
}

/// Build or update the embedding index for the workspace.
///
/// Files whose mtime hasn't changed since the existing index keep their
/// vectors; new/modified files are re-chunked and re-embedded.
pub async fn build_or_update(
    root: &Utf8Path,
    client: &EmbeddingClient,
) -> Result<EmbeddingIndex, EmbedError> {
    let mut existing = load_index(root);
    if existing.model.is_empty() {
        existing.model = client.model.clone();
    } else if existing.model != client.model {
        existing = EmbeddingIndex { model: client.model.clone(), chunks: Vec::new() };
    }
    let mut by_path: HashMap<Utf8PathBuf, Vec<EmbeddedChunk>> = HashMap::new();
    for ec in existing.chunks.drain(..) {
        by_path
            .entry(Utf8PathBuf::from(&ec.chunk.path))
            .or_default()
            .push(ec);
    }

    let mut next = EmbeddingIndex { model: client.model.clone(), chunks: Vec::new() };
    for dent in WalkBuilder::new(root.as_std_path()).build().filter_map(Result::ok) {
        if !dent.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let std_path = dent.path();
        let Some(rel) = std_path
            .strip_prefix(root.as_std_path())
            .ok()
            .and_then(|p| Utf8PathBuf::from_path_buf(p.to_path_buf()).ok())
        else {
            continue;
        };
        if !is_indexable(&rel) {
            continue;
        }
        let mtime = mtime_secs(std_path);
        if let Some(cached) = by_path.get(&rel) {
            if cached.first().map(|c| c.source_mtime).unwrap_or(0) == mtime {
                next.chunks.extend(cached.iter().cloned());
                continue;
            }
        }
        let Ok(body) = std::fs::read_to_string(std_path) else { continue };
        for chunk in chunk_file(&rel, &body) {
            let prompt = chunk_prompt(&chunk);
            let vector = match client.embed(&prompt).await {
                Ok(v) => v,
                Err(_) => continue,
            };
            next.chunks.push(EmbeddedChunk { chunk, vector, source_mtime: mtime });
        }
    }
    save_index(root, &next)?;
    Ok(next)
}

fn is_indexable(p: &Utf8Path) -> bool {
    matches!(
        p.extension().unwrap_or(""),
        "rs" | "py" | "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "go" | "java" | "md"
    )
}

fn mtime_secs(p: &std::path::Path) -> u64 {
    std::fs::metadata(p)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn chunk_prompt(c: &CodeChunk) -> String {
    // Include a short header so the embedding represents both location and content.
    format!(
        "{} {}:{}-{}\n{}",
        c.kind,
        c.path,
        c.start_line,
        c.end_line,
        &c.text[..c.text.len().min(2_000)]
    )
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0_f32;
    let mut na = 0.0_f32;
    let mut nb = 0.0_f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = (na.sqrt() * nb.sqrt()).max(1e-9);
    dot / denom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load_index_roundtrip() {
        let td = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
        let mut idx = EmbeddingIndex { model: "test".into(), chunks: Vec::new() };
        idx.chunks.push(EmbeddedChunk {
            chunk: crate::chunker::CodeChunk::file_only(&Utf8PathBuf::from("a.rs"), "fn main(){}"),
            vector: vec![0.1, 0.2, 0.3],
            source_mtime: 100,
        });
        save_index(&root, &idx).unwrap();
        let back = load_index(&root);
        assert_eq!(back.chunks.len(), 1);
        assert_eq!(back.model, "test");
    }

    #[test]
    fn cosine_basics() {
        assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(cosine_similarity(&[], &[1.0]), 0.0);
    }

    #[tokio::test]
    async fn embed_fails_gracefully_when_ollama_absent() {
        // Point at a port we don't bind so the request fails fast.
        let mut c = EmbeddingClient::new();
        c.base_url = "http://127.0.0.1:1".into();
        let res = c.embed("hello").await;
        assert!(res.is_err());
        assert!(!c.is_reachable().await);
    }
}
