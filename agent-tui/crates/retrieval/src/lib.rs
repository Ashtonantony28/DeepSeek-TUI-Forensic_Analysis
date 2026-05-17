//! Retrieval abstractions.
//!
//! Phase 2 shipped `RipgrepRetriever`. Phase 3.4 adds three additional
//! signals — an AST chunker (`chunker`), a PageRank repo-graph (`graph`),
//! and Ollama-backed embeddings (`embeddings`) — fused with reciprocal-rank
//! fusion in `HybridRetriever`. All implement the same `Retriever` trait.

pub mod chunker;
pub mod embeddings;
pub mod graph;
pub mod hybrid;

pub use chunker::{chunk_file, CodeChunk};
pub use embeddings::{
    build_or_update as build_or_update_embeddings, cosine_similarity, EmbedError, EmbeddedChunk,
    EmbeddingClient, EmbeddingIndex,
};
pub use graph::{build as build_repo_graph, pagerank, RepoGraph};
pub use hybrid::HybridRetriever;

use async_trait::async_trait;
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalHit {
    pub path: Utf8PathBuf,
    pub line: u32,
    pub snippet: String,
    pub score: f32,
}

#[derive(Debug, Error)]
pub enum RetrievalError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

#[async_trait]
pub trait Retriever: Send + Sync {
    async fn search(&self, query: &str, top_k: usize) -> Result<Vec<RetrievalHit>, RetrievalError>;
}

pub struct RipgrepRetriever {
    pub root: Utf8PathBuf,
}

impl RipgrepRetriever {
    pub fn new(root: Utf8PathBuf) -> Self {
        Self { root }
    }
}

#[async_trait]
impl Retriever for RipgrepRetriever {
    async fn search(&self, query: &str, top_k: usize) -> Result<Vec<RetrievalHit>, RetrievalError> {
        let q = query.to_string();
        let root = self.root.clone();
        let hits = tokio::task::spawn_blocking(move || {
            let mut out: Vec<RetrievalHit> = Vec::new();
            for dent in ignore::WalkBuilder::new(root.as_std_path())
                .build()
                .filter_map(Result::ok)
            {
                if !dent.file_type().is_some_and(|t| t.is_file()) {
                    continue;
                }
                if let Ok(body) = std::fs::read_to_string(dent.path()) {
                    for (i, line) in body.lines().enumerate() {
                        if line.contains(&q) {
                            out.push(RetrievalHit {
                                path: Utf8PathBuf::from_path_buf(dent.path().to_path_buf())
                                    .unwrap_or_else(|p| {
                                        Utf8PathBuf::from(p.to_string_lossy().into_owned())
                                    }),
                                line: (i + 1) as u32,
                                snippet: line.to_string(),
                                score: 1.0,
                            });
                            if out.len() >= top_k {
                                return out;
                            }
                        }
                    }
                }
            }
            out
        })
        .await
        .map_err(|e| RetrievalError::Other(e.to_string()))?;
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ripgrep_finds_hits() {
        let td = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
        std::fs::write(root.join("a.txt"), "hello\nworld\n").unwrap();
        let r = RipgrepRetriever::new(root);
        let hits = r.search("world", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line, 2);
    }
}
