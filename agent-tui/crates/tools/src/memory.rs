//! Cross-session memory (extension 3.5 — Reflexion-style lessons).
//!
//! Lessons are short, durable hints the agent has accumulated across
//! sessions. They live in `.agent-tui/memory/lessons.jsonl` as one JSON
//! record per line — append-only, simple to inspect, simple to diff.
//!
//! Retrieval is keyword-based (Jaccard over lowercased word sets). It is
//! intentionally model-free so the memory store works offline and never
//! breaks the cache by silently calling out to an embedding service.
//!
//! Wiring:
//!   * `MemoryStore::load(workspace_root)` opens or creates the file.
//!   * `MemoryStore::add(topic, lesson)` appends a record.
//!   * `MemoryStore::retrieve(query, k)` returns the top-k matching lessons.
//!   * The engine calls `inject_into_system_prompt()` once per turn when
//!     `memory_enabled` is true.

use camino::{Utf8Path, Utf8PathBuf};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::sync::Mutex;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
    pub topic: String,
    pub lesson: String,
    pub timestamp: i64,
}

pub struct MemoryStore {
    path: Utf8PathBuf,
    enabled: bool,
    lessons: Mutex<Vec<Lesson>>,
}

impl MemoryStore {
    pub fn load(workspace_root: &Utf8Path) -> Self {
        let dir = workspace_root.join(".agent-tui").join("memory");
        let path = dir.join("lessons.jsonl");
        let mut lessons = Vec::new();
        if path.exists() {
            if let Ok(f) = std::fs::File::open(path.as_std_path()) {
                for line in BufReader::new(f).lines().map_while(Result::ok) {
                    if line.trim().is_empty() {
                        continue;
                    }
                    if let Ok(l) = serde_json::from_str::<Lesson>(&line) {
                        lessons.push(l);
                    }
                }
            }
        }
        Self {
            path,
            enabled: true,
            lessons: Mutex::new(lessons),
        }
    }

    pub fn disabled() -> Self {
        Self {
            path: Utf8PathBuf::from(""),
            enabled: false,
            lessons: Mutex::new(Vec::new()),
        }
    }

    pub fn add(
        &self,
        topic: impl Into<String>,
        lesson: impl Into<String>,
    ) -> Result<(), MemoryError> {
        if !self.enabled {
            return Ok(());
        }
        let entry = Lesson {
            topic: topic.into(),
            lesson: lesson.into(),
            timestamp: Utc::now().timestamp(),
        };
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent.as_std_path())?;
        }
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path.as_std_path())?;
        let line = serde_json::to_string(&entry)?;
        writeln!(f, "{line}")?;
        self.lessons.lock().unwrap().push(entry);
        Ok(())
    }

    pub fn retrieve(&self, query: &str, k: usize) -> Vec<Lesson> {
        if !self.enabled || k == 0 {
            return Vec::new();
        }
        let q_tokens = tokenize(query);
        if q_tokens.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(f32, Lesson)> = self
            .lessons
            .lock()
            .unwrap()
            .iter()
            .map(|l| {
                let mut text = l.topic.clone();
                text.push(' ');
                text.push_str(&l.lesson);
                let t = tokenize(&text);
                (jaccard(&q_tokens, &t), l.clone())
            })
            .filter(|(s, _)| *s > 0.0)
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().take(k).map(|(_, l)| l).collect()
    }

    pub fn len(&self) -> usize {
        self.lessons.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Build the suffix that gets appended to the system prompt for the
    /// next turn. Returns `None` if there are no relevant lessons.
    pub fn suffix_for(&self, query: &str, k: usize) -> Option<String> {
        let hits = self.retrieve(query, k);
        if hits.is_empty() {
            return None;
        }
        let mut s = String::from("\n\nRelevant lessons from prior sessions:\n");
        for (i, l) in hits.iter().enumerate() {
            s.push_str(&format!("  {}. [{}] {}\n", i + 1, l.topic, l.lesson));
        }
        Some(s)
    }
}

fn tokenize(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .map(|w| w.to_lowercase())
        .collect()
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f32;
    let union = a.union(b).count() as f32;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn add_and_retrieve_persists_across_load() {
        let td = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
        {
            let mem = MemoryStore::load(&root);
            mem.add("rust async", "use tokio::spawn for fire-and-forget tasks")
                .unwrap();
            mem.add("git", "prefer git restore over git checkout for files")
                .unwrap();
            assert_eq!(mem.len(), 2);
        }
        let mem = MemoryStore::load(&root);
        assert_eq!(mem.len(), 2);
        let hits = mem.retrieve("rust tokio task", 5);
        assert!(!hits.is_empty());
        assert!(hits[0].lesson.contains("tokio"));
    }

    #[test]
    fn empty_query_returns_nothing() {
        let td = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
        let mem = MemoryStore::load(&root);
        mem.add("topic", "lesson body here").unwrap();
        assert!(mem.retrieve("", 5).is_empty());
    }

    #[test]
    fn disabled_store_is_noop() {
        let mem = MemoryStore::disabled();
        mem.add("x", "y").unwrap();
        assert!(mem.is_empty());
        assert!(mem.retrieve("anything", 5).is_empty());
        assert!(mem.suffix_for("anything", 5).is_none());
    }

    #[test]
    fn suffix_contains_top_hit() {
        let td = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
        let mem = MemoryStore::load(&root);
        mem.add("retrieval", "use reciprocal rank fusion to combine signals")
            .unwrap();
        let s = mem.suffix_for("fusion retrieval signal", 3).unwrap();
        assert!(s.contains("reciprocal rank fusion"));
        assert!(s.starts_with("\n\nRelevant lessons"));
    }
}
