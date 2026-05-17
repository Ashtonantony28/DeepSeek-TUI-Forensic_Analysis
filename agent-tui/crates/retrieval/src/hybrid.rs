//! Hybrid retriever (extension 3.4) — fuses three signals with Reciprocal
//! Rank Fusion (RRF):
//!
//!   1. AST chunker — top-K chunks whose text contains query terms.
//!   2. Repo graph — personalised PageRank seeded with the query.
//!   3. Embeddings — cosine similarity over `nomic-embed-text` vectors,
//!      when an Ollama instance is reachable. Falls back silently when not.
//!
//! RRF: `score(d) = Σ 1 / (k + rank_r(d))` over each ranker `r`. `k=60` is
//! the common default; we keep it.

use crate::chunker::{chunk_file, CodeChunk};
use crate::embeddings::{cosine_similarity, EmbeddingClient, EmbeddingIndex};
use crate::graph::{self, RepoGraph};
use crate::{RetrievalError, RetrievalHit, Retriever};
use async_trait::async_trait;
use camino::{Utf8Path, Utf8PathBuf};
use ignore::WalkBuilder;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

const RRF_K: f32 = 60.0;

pub struct HybridRetriever {
    pub root: Utf8PathBuf,
    pub graph: RepoGraph,
    pub embeddings: Arc<RwLock<Option<EmbeddingIndex>>>,
    pub embed_client: Arc<EmbeddingClient>,
    pub embeddings_available: bool,
}

impl HybridRetriever {
    /// Construct without doing IO. Use `prepare` to actually build the graph
    /// and probe for Ollama before the first query.
    pub fn new(root: Utf8PathBuf) -> Self {
        Self {
            root,
            graph: RepoGraph::empty(),
            embeddings: Arc::new(RwLock::new(None)),
            embed_client: Arc::new(EmbeddingClient::new()),
            embeddings_available: false,
        }
    }

    pub async fn prepare(mut self) -> Self {
        self.graph = graph::build(&self.root);
        self.embeddings_available = self.embed_client.is_reachable().await;
        if self.embeddings_available {
            let loaded = crate::embeddings::load_index(&self.root);
            if !loaded.chunks.is_empty() {
                *self.embeddings.write().await = Some(loaded);
            }
        }
        self
    }

    /// Force a full embedding rebuild. Returns the number of chunks indexed.
    pub async fn reindex_embeddings(&self) -> Result<usize, RetrievalError> {
        if !self.embeddings_available {
            return Err(RetrievalError::Other("embeddings unavailable".into()));
        }
        let idx = crate::embeddings::build_or_update(&self.root, &self.embed_client)
            .await
            .map_err(|e| RetrievalError::Other(e.to_string()))?;
        let count = idx.chunks.len();
        *self.embeddings.write().await = Some(idx);
        Ok(count)
    }
}

#[async_trait]
impl Retriever for HybridRetriever {
    async fn search(&self, query: &str, top_k: usize) -> Result<Vec<RetrievalHit>, RetrievalError> {
        let lex_ranked = rank_lexical(&self.root, query, top_k * 4);
        let graph_ranked = rank_graph(&self.graph, query, top_k * 2);
        let vec_ranked = if self.embeddings_available {
            rank_embeddings(&self.embeddings, &self.embed_client, query, top_k * 2).await
        } else {
            Vec::new()
        };

        let mut scores: HashMap<HitKey, f32> = HashMap::new();
        let mut record: HashMap<HitKey, RetrievalHit> = HashMap::new();
        for ranked in [&lex_ranked, &graph_ranked, &vec_ranked] {
            for (rank, hit) in ranked.iter().enumerate() {
                let key = HitKey::from(hit);
                let contrib = 1.0 / (RRF_K + rank as f32 + 1.0);
                *scores.entry(key.clone()).or_insert(0.0) += contrib;
                record.entry(key).or_insert_with(|| hit.clone());
            }
        }

        let mut fused: Vec<(HitKey, f32)> = scores.into_iter().collect();
        fused.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let mut out: Vec<RetrievalHit> = Vec::with_capacity(top_k);
        for (k, s) in fused.into_iter().take(top_k) {
            if let Some(mut h) = record.remove(&k) {
                h.score = s;
                out.push(h);
            }
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct HitKey {
    path: String,
    line: u32,
}

impl From<&RetrievalHit> for HitKey {
    fn from(h: &RetrievalHit) -> Self {
        Self {
            path: h.path.to_string(),
            line: h.line,
        }
    }
}

// ---- per-signal rankers ----

fn rank_lexical(root: &Utf8Path, query: &str, top_k: usize) -> Vec<RetrievalHit> {
    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<(f32, RetrievalHit)> = Vec::new();
    for dent in WalkBuilder::new(root.as_std_path())
        .build()
        .filter_map(Result::ok)
    {
        if !dent.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Some(rel) = dent
            .path()
            .strip_prefix(root.as_std_path())
            .ok()
            .and_then(|p| Utf8PathBuf::from_path_buf(p.to_path_buf()).ok())
        else {
            continue;
        };
        let Ok(body) = std::fs::read_to_string(dent.path()) else {
            continue;
        };
        for chunk in chunk_file(&rel, &body) {
            let lower = chunk.text.to_lowercase();
            let score = terms.iter().filter(|t| lower.contains(t.as_str())).count() as f32;
            if score == 0.0 {
                continue;
            }
            let snippet = chunk_summary(&chunk);
            out.push((
                score,
                RetrievalHit {
                    path: Utf8PathBuf::from(&chunk.path),
                    line: chunk.start_line,
                    snippet,
                    score,
                },
            ));
        }
    }
    out.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    out.into_iter().take(top_k).map(|(_, h)| h).collect()
}

fn rank_graph(g: &RepoGraph, query: &str, top_k: usize) -> Vec<RetrievalHit> {
    if g.is_empty() {
        return Vec::new();
    }
    let scores = graph::pagerank(g, Some(query), 25);
    let mut indexed: Vec<(usize, f32)> = scores.iter().enumerate().map(|(i, s)| (i, *s)).collect();
    indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    indexed
        .into_iter()
        .take(top_k)
        .map(|(i, s)| RetrievalHit {
            path: g.files[i].clone(),
            line: 1,
            snippet: format!("(graph rank for `{query}`)"),
            score: s,
        })
        .collect()
}

async fn rank_embeddings(
    idx: &Arc<RwLock<Option<EmbeddingIndex>>>,
    client: &EmbeddingClient,
    query: &str,
    top_k: usize,
) -> Vec<RetrievalHit> {
    let guard = idx.read().await;
    let Some(idx) = guard.as_ref() else {
        return Vec::new();
    };
    if idx.chunks.is_empty() {
        return Vec::new();
    }
    let Ok(q_vec) = client.embed(query).await else {
        return Vec::new();
    };
    let mut scored: Vec<(f32, &crate::embeddings::EmbeddedChunk)> = idx
        .chunks
        .iter()
        .map(|c| (cosine_similarity(&q_vec, &c.vector), c))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .take(top_k)
        .filter(|(s, _)| *s > 0.0)
        .map(|(s, ec)| RetrievalHit {
            path: Utf8PathBuf::from(&ec.chunk.path),
            line: ec.chunk.start_line,
            snippet: chunk_summary(&ec.chunk),
            score: s,
        })
        .collect()
}

fn chunk_summary(c: &CodeChunk) -> String {
    let head = c.name.clone().unwrap_or_else(|| c.kind.clone());
    let first = c.text.lines().next().unwrap_or("").trim();
    format!("{head} — {first}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;

    #[tokio::test]
    async fn hybrid_returns_lexical_hits_when_embeddings_unavailable() {
        let td = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
        std::fs::create_dir_all(root.join("src").as_std_path()).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "pub fn authenticate_user(name: &str) -> bool { name == \"root\" }\n",
        )
        .unwrap();
        std::fs::write(root.join("src/util.rs"), "pub fn other() {}\n").unwrap();

        let mut h = HybridRetriever::new(root.clone());
        // Skip prepare() to keep embeddings off in this test.
        h.graph = crate::graph::build(&root);
        let hits = h.search("authenticate", 5).await.unwrap();
        assert!(!hits.is_empty());
        assert!(hits.iter().any(|h| h.path.as_str().contains("lib.rs")));
    }

    #[tokio::test]
    async fn empty_repo_returns_no_hits() {
        let td = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
        let h = HybridRetriever::new(root).prepare().await;
        let hits = h.search("anything", 5).await.unwrap();
        assert!(hits.is_empty());
    }
}
