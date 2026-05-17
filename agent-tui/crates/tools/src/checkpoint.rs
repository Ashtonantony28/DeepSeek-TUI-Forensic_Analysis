//! Workspace checkpoint storage (extension 3.1).
//!
//! Each checkpoint is a git stash object (created via `git stash create`) plus
//! a metadata record stored in `.agent-tui/checkpoints/index.json`. The stash
//! is never pushed onto the user-visible stash list — only the SHA is kept,
//! so the user's main stash stays clean.
//!
//! When the workspace is not a git repository, the store silently disables
//! itself (`new` succeeds but `create` returns `Disabled`). Engine callers
//! treat that as a no-op rather than an error.

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::sync::Mutex;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CheckpointError {
    #[error("checkpoint store disabled (not a git repository)")]
    Disabled,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("git error: {0}")]
    Git(String),
    #[error("checkpoint not found: {0}")]
    NotFound(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointMeta {
    pub id: String,
    pub stash_sha: String,
    pub turn: u32,
    pub timestamp: i64,
    pub summary: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct IndexFile {
    checkpoints: Vec<CheckpointMeta>,
    next_seq: u64,
}

pub struct CheckpointStore {
    root: Utf8PathBuf,
    dir: Utf8PathBuf,
    index_path: Utf8PathBuf,
    enabled: bool,
    inner: Mutex<IndexFile>,
}

impl CheckpointStore {
    pub fn open(workspace_root: &Utf8Path) -> Self {
        let dir = workspace_root.join(".agent-tui").join("checkpoints");
        let index_path = dir.join("index.json");
        let enabled = workspace_root.join(".git").exists();
        let inner = if index_path.exists() {
            std::fs::read_to_string(index_path.as_std_path())
                .ok()
                .and_then(|s| serde_json::from_str::<IndexFile>(&s).ok())
                .unwrap_or_default()
        } else {
            IndexFile::default()
        };
        Self {
            root: workspace_root.to_owned(),
            dir,
            index_path,
            enabled,
            inner: Mutex::new(inner),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn list(&self) -> Vec<CheckpointMeta> {
        self.inner.lock().unwrap().checkpoints.clone()
    }

    pub fn get(&self, id: &str) -> Option<CheckpointMeta> {
        self.inner
            .lock()
            .unwrap()
            .checkpoints
            .iter()
            .find(|c| c.id == id)
            .cloned()
    }

    /// Create a new checkpoint capturing the current working tree.
    ///
    /// `summary` is a short, human-readable label (typically the tool name +
    /// affected path). `turn` is the current turn number, used to filter the
    /// list in the TUI overlay.
    pub fn create(
        &self,
        turn: u32,
        summary: impl Into<String>,
    ) -> Result<CheckpointMeta, CheckpointError> {
        if !self.enabled {
            return Err(CheckpointError::Disabled);
        }
        let out = run_git(&self.root, &["stash", "create"])?;
        let sha = out.trim().to_string();
        if sha.is_empty() {
            // Nothing to checkpoint (clean tree); use HEAD instead so rollback
            // still has something to refer to.
            let head = run_git(&self.root, &["rev-parse", "HEAD"])
                .map_err(|e| CheckpointError::Git(format!("clean tree and no HEAD: {e}")))?;
            let sha = head.trim().to_string();
            return self.persist(turn, summary.into(), sha);
        }
        // Make the object permanent (otherwise `git gc` would collect it).
        let label = format!("agent-tui checkpoint turn={turn}");
        run_git(
            &self.root,
            &["update-ref", "refs/agent-tui/last-stash", &sha, ""],
        )
        .ok();
        let _ = run_git(
            &self.root,
            &[
                "tag",
                "-f",
                &format!("agent-tui-checkpoint/{sha}"),
                &sha,
                "-m",
                &label,
            ],
        );
        self.persist(turn, summary.into(), sha)
    }

    fn persist(
        &self,
        turn: u32,
        summary: String,
        stash_sha: String,
    ) -> Result<CheckpointMeta, CheckpointError> {
        std::fs::create_dir_all(self.dir.as_std_path())?;
        let mut inner = self.inner.lock().unwrap();
        let seq = inner.next_seq;
        inner.next_seq += 1;
        let id = format!("ck-{seq:04}");
        let meta = CheckpointMeta {
            id: id.clone(),
            stash_sha,
            turn,
            timestamp: chrono_secs(),
            summary,
        };
        inner.checkpoints.push(meta.clone());
        let body = serde_json::to_string_pretty(&*inner)?;
        std::fs::write(self.index_path.as_std_path(), body)?;
        Ok(meta)
    }

    /// Render a unified diff between the workspace as-of the checkpoint and
    /// the current working tree. Read-only.
    pub fn diff(&self, id: &str) -> Result<String, CheckpointError> {
        if !self.enabled {
            return Err(CheckpointError::Disabled);
        }
        let meta = self
            .get(id)
            .ok_or_else(|| CheckpointError::NotFound(id.into()))?;
        run_git(&self.root, &["diff", &meta.stash_sha])
    }

    /// Restore the workspace to the checkpoint state.
    ///
    /// Uses `git stash apply --index <sha>` after a hard reset to HEAD, so
    /// the user's branch pointer stays where it is. Caller is expected to
    /// gate this on user confirmation.
    pub fn rollback(&self, id: &str) -> Result<(), CheckpointError> {
        if !self.enabled {
            return Err(CheckpointError::Disabled);
        }
        let meta = self
            .get(id)
            .ok_or_else(|| CheckpointError::NotFound(id.into()))?;
        // Reset the working tree, then restore.
        run_git(&self.root, &["reset", "--hard", "HEAD"])?;
        // If the SHA is a regular commit (HEAD-fallback case from `create`),
        // checkout works; if it is a stash-style commit, `stash apply` is
        // correct. Try stash apply first, then fall back to a hard reset.
        if run_git(&self.root, &["stash", "apply", "--index", &meta.stash_sha]).is_err() {
            run_git(&self.root, &["reset", "--hard", &meta.stash_sha])?;
        }
        Ok(())
    }
}

fn run_git(cwd: &Utf8Path, args: &[&str]) -> Result<String, CheckpointError> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd.as_std_path())
        .output()?;
    if !out.status.success() {
        return Err(CheckpointError::Git(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn chrono_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repo() -> (tempfile::TempDir, Utf8PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
        run_git(&root, &["init", "-q"]).unwrap();
        run_git(&root, &["config", "user.email", "t@e"]).unwrap();
        run_git(&root, &["config", "user.name", "t"]).unwrap();
        run_git(&root, &["config", "commit.gpgsign", "false"]).unwrap();
        std::fs::write(root.join("a.txt"), "v1\n").unwrap();
        run_git(&root, &["add", "."]).unwrap();
        run_git(&root, &["commit", "-q", "--no-gpg-sign", "-m", "init"]).unwrap();
        (td, root)
    }

    #[test]
    fn checkpoint_and_rollback_roundtrip() {
        let (_td, root) = init_repo();
        let store = CheckpointStore::open(&root);
        assert!(store.enabled());

        // Mutate then checkpoint.
        std::fs::write(root.join("a.txt"), "v2\n").unwrap();
        let cp = store.create(1, "test").unwrap();
        assert!(!cp.stash_sha.is_empty());
        assert_eq!(store.list().len(), 1);

        // Further mutation then rollback.
        std::fs::write(root.join("a.txt"), "v3\n").unwrap();
        store.rollback(&cp.id).unwrap();
        let restored = std::fs::read_to_string(root.join("a.txt").as_std_path()).unwrap();
        assert_eq!(restored, "v2\n");
    }

    #[test]
    fn disabled_without_git() {
        let td = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
        let store = CheckpointStore::open(&root);
        assert!(!store.enabled());
        assert!(matches!(
            store.create(0, "x").unwrap_err(),
            CheckpointError::Disabled
        ));
    }

    #[test]
    fn diff_shows_workspace_changes() {
        let (_td, root) = init_repo();
        let store = CheckpointStore::open(&root);
        std::fs::write(root.join("a.txt"), "v2\n").unwrap();
        let cp = store.create(0, "first").unwrap();
        std::fs::write(root.join("a.txt"), "v3\n").unwrap();
        let diff = store.diff(&cp.id).unwrap();
        assert!(diff.contains("v3"), "diff should mention the new content");
    }
}
