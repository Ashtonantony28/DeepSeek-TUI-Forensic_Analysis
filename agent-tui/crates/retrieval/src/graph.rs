//! Repo dependency graph + PageRank (extension 3.4, Aider-style).
//!
//! Builds a directed import graph across files, then runs power-iteration
//! PageRank to surface "important" files in the repo. Imports are extracted
//! per language with simple regex matchers — good enough for ranking, and
//! cheap enough to rebuild on every workspace scan.
//!
//! Personalised PageRank: when a query is supplied, the random-restart
//! distribution is biased toward files whose names or paths contain the
//! query, matching Aider's behaviour.

use camino::{Utf8Path, Utf8PathBuf};
use ignore::WalkBuilder;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct RepoGraph {
    /// Each file's canonical path inside the repo.
    pub files: Vec<Utf8PathBuf>,
    /// Adjacency list: `edges[i]` is the set of files that `files[i]` imports.
    pub edges: Vec<Vec<usize>>,
    file_index: HashMap<Utf8PathBuf, usize>,
}

impl RepoGraph {
    pub fn empty() -> Self {
        Self { files: Vec::new(), edges: Vec::new(), file_index: HashMap::new() }
    }

    pub fn len(&self) -> usize { self.files.len() }
    pub fn is_empty(&self) -> bool { self.files.is_empty() }

    pub fn index_of(&self, path: &Utf8Path) -> Option<usize> {
        self.file_index.get(path).copied()
    }
}

pub fn build(root: &Utf8Path) -> RepoGraph {
    let mut g = RepoGraph::empty();
    // First pass: collect candidate source files.
    for dent in WalkBuilder::new(root.as_std_path()).build().filter_map(Result::ok) {
        if !dent.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let std_path = dent.path();
        if !is_source(std_path) {
            continue;
        }
        if let Ok(rel) = std_path.strip_prefix(root.as_std_path()) {
            if let Ok(rel) = Utf8PathBuf::from_path_buf(rel.to_path_buf()) {
                let idx = g.files.len();
                g.file_index.insert(rel.clone(), idx);
                g.files.push(rel);
                g.edges.push(Vec::new());
            }
        }
    }

    // Second pass: parse imports.
    for (i, rel) in g.files.clone().iter().enumerate() {
        let abs = root.join(rel);
        let Ok(body) = std::fs::read_to_string(abs.as_std_path()) else { continue };
        let imports = extract_imports(rel, &body);
        for module in imports {
            if let Some(target) = resolve_import(&g, rel, &module) {
                if target != i && !g.edges[i].contains(&target) {
                    g.edges[i].push(target);
                }
            }
        }
    }
    g
}

fn is_source(p: &std::path::Path) -> bool {
    matches!(
        p.extension().and_then(|s| s.to_str()).unwrap_or(""),
        "rs" | "py" | "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "go" | "java"
    )
}

/// PageRank with optional query bias. Returns scores aligned with `g.files`.
pub fn pagerank(g: &RepoGraph, query: Option<&str>, iters: u32) -> Vec<f32> {
    let n = g.files.len();
    if n == 0 {
        return Vec::new();
    }
    let damping: f32 = 0.85;
    let teleport: Vec<f32> = match query {
        None => vec![1.0 / n as f32; n],
        Some(q) => {
            let ql = q.to_lowercase();
            let mut t: Vec<f32> = g
                .files
                .iter()
                .map(|p| if p.as_str().to_lowercase().contains(&ql) { 1.0 } else { 0.0 })
                .collect();
            let sum: f32 = t.iter().sum();
            if sum > 0.0 {
                for v in &mut t { *v /= sum; }
            } else {
                t = vec![1.0 / n as f32; n];
            }
            t
        }
    };
    let mut rank = vec![1.0 / n as f32; n];
    for _ in 0..iters {
        let mut next = vec![0.0_f32; n];
        let mut dangling = 0.0_f32;
        for (i, edges) in g.edges.iter().enumerate() {
            if edges.is_empty() {
                dangling += rank[i];
            } else {
                let share = rank[i] / edges.len() as f32;
                for &j in edges {
                    next[j] += share;
                }
            }
        }
        for i in 0..n {
            next[i] = damping * (next[i] + dangling / n as f32)
                + (1.0 - damping) * teleport[i];
        }
        rank = next;
    }
    rank
}

// ----- import extraction -----

static RUST_USE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\buse\s+([A-Za-z_][A-Za-z0-9_:]*)").unwrap());
static RUST_MOD: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bmod\s+([A-Za-z_][A-Za-z0-9_]+)\s*;").unwrap());
static PY_IMPORT: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*(?:from\s+([A-Za-z0-9_.]+)\s+import|import\s+([A-Za-z0-9_.]+))").unwrap());
static JS_IMPORT: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?:import\s+[^"']*from\s*|require\s*\(\s*)["']([^"']+)["']"#).unwrap());
static GO_IMPORT: Lazy<Regex> = Lazy::new(|| Regex::new(r#"^\s*(?:import\s+)?"([^"]+)""#).unwrap());
static JAVA_IMPORT: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*import\s+([A-Za-z0-9_.]+)\s*;").unwrap());

fn extract_imports(path: &Utf8Path, body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let ext = path.extension().unwrap_or("");
    match ext {
        "rs" => {
            for cap in RUST_USE.captures_iter(body) {
                if let Some(m) = cap.get(1) {
                    let head = m.as_str().split("::").next().unwrap_or("").to_string();
                    if !head.is_empty() { out.push(head); }
                }
            }
            for cap in RUST_MOD.captures_iter(body) {
                if let Some(m) = cap.get(1) { out.push(m.as_str().to_string()); }
            }
        }
        "py" => {
            for line in body.lines() {
                if let Some(cap) = PY_IMPORT.captures(line) {
                    let m = cap.get(1).or_else(|| cap.get(2));
                    if let Some(m) = m {
                        out.push(m.as_str().to_string());
                    }
                }
            }
        }
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" => {
            for cap in JS_IMPORT.captures_iter(body) {
                if let Some(m) = cap.get(1) {
                    out.push(m.as_str().to_string());
                }
            }
        }
        "go" => {
            for line in body.lines() {
                if let Some(cap) = GO_IMPORT.captures(line) {
                    if let Some(m) = cap.get(1) { out.push(m.as_str().to_string()); }
                }
            }
        }
        "java" => {
            for line in body.lines() {
                if let Some(cap) = JAVA_IMPORT.captures(line) {
                    if let Some(m) = cap.get(1) { out.push(m.as_str().to_string()); }
                }
            }
        }
        _ => {}
    }
    out
}

fn resolve_import(g: &RepoGraph, from: &Utf8Path, module: &str) -> Option<usize> {
    // Heuristic: try matching the module against file stems and path tails.
    let tail = module.rsplit_one_of(&[':', '.', '/']);
    let head = tail.as_deref().unwrap_or(module);

    let from_dir = from.parent().unwrap_or(Utf8Path::new(""));
    // 1. Sibling file with same stem.
    for ext in ["rs", "py", "js", "ts", "tsx", "jsx", "go", "java"] {
        let candidate = from_dir.join(format!("{head}.{ext}"));
        if let Some(i) = g.index_of(&candidate) {
            return Some(i);
        }
    }
    // 2. Sibling directory module (Rust mod, Python package).
    for ext in ["rs", "py"] {
        let candidate = from_dir.join(head).join(format!("mod.{ext}"));
        if let Some(i) = g.index_of(&candidate) {
            return Some(i);
        }
        let init = from_dir.join(head).join("__init__.py");
        if let Some(i) = g.index_of(&init) {
            return Some(i);
        }
        let _ = ext;
    }
    // 3. Any file whose stem matches.
    for (i, p) in g.files.iter().enumerate() {
        if p.file_stem().map(|s| s == head).unwrap_or(false) {
            return Some(i);
        }
    }
    None
}

trait RSplitOneOf {
    fn rsplit_one_of(&self, seps: &[char]) -> Option<String>;
}
impl RSplitOneOf for str {
    fn rsplit_one_of(&self, seps: &[char]) -> Option<String> {
        self.rfind(seps).map(|i| self[i + 1..].to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn td() -> (tempfile::TempDir, Utf8PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let p = Utf8PathBuf::from_path_buf(t.path().to_path_buf()).unwrap();
        (t, p)
    }

    #[test]
    fn rust_use_creates_edge() {
        let (_t, root) = td();
        std::fs::create_dir_all(root.join("src").as_std_path()).unwrap();
        std::fs::write(root.join("src/util.rs"), "pub fn x() {}").unwrap();
        std::fs::write(root.join("src/main.rs"), "use util::x;\nfn main(){}").unwrap();
        let g = build(&root);
        let main_i = g.index_of(&Utf8PathBuf::from("src/main.rs")).unwrap();
        let util_i = g.index_of(&Utf8PathBuf::from("src/util.rs")).unwrap();
        assert!(g.edges[main_i].contains(&util_i));
    }

    #[test]
    fn pagerank_returns_normalised_scores() {
        let (_t, root) = td();
        std::fs::create_dir_all(root.join("src").as_std_path()).unwrap();
        std::fs::write(root.join("src/a.rs"), "use b;\nfn x(){}").unwrap();
        std::fs::write(root.join("src/b.rs"), "pub fn y(){}").unwrap();
        std::fs::write(root.join("src/c.rs"), "use b;\nfn z(){}").unwrap();
        let g = build(&root);
        let scores = pagerank(&g, None, 30);
        let b = g.index_of(&Utf8PathBuf::from("src/b.rs")).unwrap();
        let a = g.index_of(&Utf8PathBuf::from("src/a.rs")).unwrap();
        // b is depended on by both a and c → should outrank either of them.
        assert!(scores[b] > scores[a]);
        let sum: f32 = scores.iter().sum();
        assert!((sum - 1.0).abs() < 0.05, "scores should approximately sum to 1, got {sum}");
    }

    #[test]
    fn personalised_pagerank_biases_query_match() {
        let (_t, root) = td();
        std::fs::create_dir_all(root.join("src").as_std_path()).unwrap();
        std::fs::write(root.join("src/auth.rs"), "fn x(){}").unwrap();
        std::fs::write(root.join("src/parser.rs"), "fn y(){}").unwrap();
        let g = build(&root);
        let s = pagerank(&g, Some("auth"), 30);
        let auth = g.index_of(&Utf8PathBuf::from("src/auth.rs")).unwrap();
        let parser = g.index_of(&Utf8PathBuf::from("src/parser.rs")).unwrap();
        assert!(s[auth] > s[parser]);
    }
}
