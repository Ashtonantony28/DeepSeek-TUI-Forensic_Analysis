//! Workspace isolation for SWE-bench evaluation.
//!
//! Three isolation modes:
//!
//! - `Docker`: spin up the official swebench instance image. Preferred when
//!   Docker is available; provides the cleanest environment.
//! - `GitClone`: `git clone` + `git checkout <base_commit>`. Fallback when
//!   Docker is absent. Requires network access to GitHub.
//! - `DryRun`: in-memory/temp-dir operation. No network, no Docker. Used by
//!   `--dry-run` mode to exercise all code paths without real instances.
//!
//! Detection runs at startup (`detect_isolation`) and logs which path is taken.

use crate::dataset::SweInstance;
use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationMode {
    Docker,
    GitClone,
    DryRun,
}

impl std::fmt::Display for IsolationMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IsolationMode::Docker => write!(f, "docker"),
            IsolationMode::GitClone => write!(f, "git-clone"),
            IsolationMode::DryRun => write!(f, "dry-run"),
        }
    }
}

/// Detect which isolation mode is available and log the result.
pub async fn detect_isolation(dry_run: bool) -> IsolationMode {
    if dry_run {
        info!("isolation: dry-run (synthetic instances, no network)");
        return IsolationMode::DryRun;
    }
    let docker_ok = Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    if docker_ok {
        info!("isolation: docker (swebench instance images)");
        IsolationMode::Docker
    } else {
        warn!("isolation: docker unavailable — falling back to git-clone");
        IsolationMode::GitClone
    }
}

pub struct IsolatedWorkspace {
    pub root: Utf8PathBuf,
    pub mode: IsolationMode,
    /// Container name for Docker mode; empty otherwise.
    pub container_name: String,
}

impl IsolatedWorkspace {
    /// Provision the workspace for `instance` using the given isolation mode.
    ///
    /// For `DryRun`, creates a temp dir and writes synthetic source files so
    /// later stages have a real file-system root to operate against.
    pub async fn setup(instance: &SweInstance, mode: IsolationMode) -> Result<Self> {
        let tag = format!("agent-tui-eval-{}", Uuid::new_v4().simple());
        match mode {
            IsolationMode::DryRun => {
                let root = Utf8PathBuf::from(format!("/tmp/{}", tag));
                tokio::fs::create_dir_all(&root)
                    .await
                    .context("create dry-run workspace dir")?;
                // Write a placeholder source file and test file so paths exist.
                let src_file = root.join("src.py");
                tokio::fs::write(&src_file, b"def placeholder(): pass\n")
                    .await
                    .context("write placeholder source")?;
                let test_dir = root.join("tests");
                tokio::fs::create_dir_all(&test_dir)
                    .await
                    .context("create test dir")?;
                let test_file = test_dir.join("test_placeholder.py");
                tokio::fs::write(&test_file, instance.test_patch.as_bytes())
                    .await
                    .context("write synthetic test file")?;
                Ok(Self {
                    root,
                    mode,
                    container_name: String::new(),
                })
            }

            IsolationMode::GitClone => {
                let root = Utf8PathBuf::from(format!("/tmp/{}", tag));
                let url = format!("https://github.com/{}", instance.repo);
                let status = Command::new("git")
                    .args(["clone", "--depth", "100", &url, root.as_str()])
                    .status()
                    .await
                    .context("git clone")?;
                if !status.success() {
                    anyhow::bail!("git clone failed for {}", instance.repo);
                }
                let status = Command::new("git")
                    .args(["checkout", &instance.base_commit])
                    .current_dir(&root)
                    .status()
                    .await
                    .context("git checkout")?;
                if !status.success() {
                    anyhow::bail!(
                        "git checkout {} failed for {}",
                        instance.base_commit,
                        instance.repo
                    );
                }
                Ok(Self {
                    root,
                    mode,
                    container_name: String::new(),
                })
            }

            IsolationMode::Docker => {
                // Official swebench images follow this naming convention.
                let image = format!(
                    "swebench/sweb.eval.x86_64.{}:latest",
                    instance.instance_id.to_lowercase().replace('/', "_")
                );
                let container = tag.clone();
                let status = Command::new("docker")
                    .args([
                        "run",
                        "-d",
                        "--name",
                        &container,
                        "--workdir",
                        "/testbed",
                        &image,
                        "/bin/bash",
                        "-c",
                        "sleep 7200",
                    ])
                    .status()
                    .await
                    .context("docker run")?;
                if !status.success() {
                    anyhow::bail!("docker run failed for image {}", image);
                }
                // The testbed path inside the container is the standard SWE-bench convention.
                Ok(Self {
                    root: Utf8PathBuf::from("/testbed"),
                    mode,
                    container_name: container,
                })
            }
        }
    }

    /// Apply a unified diff patch to the workspace.
    pub async fn apply_patch(&self, patch: &str) -> Result<()> {
        match self.mode {
            IsolationMode::DryRun => {
                // In dry-run, write the patch file but don't actually apply it.
                // We script pass/fail externally.
                let patch_file = format!("/tmp/agent-tui-patch-{}.diff", Uuid::new_v4().simple());
                tokio::fs::write(&patch_file, patch.as_bytes())
                    .await
                    .context("write dry-run patch file")?;
                Ok(())
            }
            IsolationMode::GitClone => {
                let patch_file = self.root.join(".agent-tui-patch.diff");
                tokio::fs::write(&patch_file, patch.as_bytes())
                    .await
                    .context("write patch file")?;
                let status = Command::new("git")
                    .args(["apply", "--whitespace=fix", patch_file.as_str()])
                    .current_dir(&self.root)
                    .status()
                    .await
                    .context("git apply")?;
                if !status.success() {
                    anyhow::bail!("git apply failed");
                }
                Ok(())
            }
            IsolationMode::Docker => {
                // Write patch to temp file, copy into container, apply.
                let host_patch = format!("/tmp/patch-{}.diff", Uuid::new_v4().simple());
                tokio::fs::write(&host_patch, patch.as_bytes())
                    .await
                    .context("write host patch")?;
                let dest = format!("{}:/testbed/.agent-tui-patch.diff", self.container_name);
                Command::new("docker")
                    .args(["cp", &host_patch, &dest])
                    .status()
                    .await
                    .context("docker cp")?;
                let status = Command::new("docker")
                    .args([
                        "exec",
                        &self.container_name,
                        "git",
                        "apply",
                        "--whitespace=fix",
                        ".agent-tui-patch.diff",
                    ])
                    .status()
                    .await
                    .context("docker exec git apply")?;
                if !status.success() {
                    anyhow::bail!("docker git apply failed");
                }
                Ok(())
            }
        }
    }

    /// Run the fail-to-pass tests and return true if they all pass.
    ///
    /// For `DryRun`, the result is scripted based on the instance_id suffix.
    pub async fn run_tests(&self, instance: &SweInstance) -> bool {
        match self.mode {
            IsolationMode::DryRun => {
                // Scripted: instances with "pass" in their id succeed.
                instance.instance_id.contains("pass")
            }
            IsolationMode::GitClone | IsolationMode::Docker => {
                let test_ids = instance.fail_to_pass.join(" ");
                let cmd_str = format!("python -m pytest {} -x -q 2>&1", test_ids);
                let output = match self.mode {
                    IsolationMode::GitClone => {
                        Command::new("bash")
                            .args(["-c", &cmd_str])
                            .current_dir(&self.root)
                            .output()
                            .await
                    }
                    IsolationMode::Docker => {
                        Command::new("docker")
                            .args(["exec", &self.container_name, "bash", "-c", &cmd_str])
                            .output()
                            .await
                    }
                    IsolationMode::DryRun => unreachable!(),
                };
                match output {
                    Ok(o) => o.status.success(),
                    Err(e) => {
                        warn!("run_tests error: {}", e);
                        false
                    }
                }
            }
        }
    }

    /// Clean up the workspace.
    pub async fn cleanup(self) -> Result<()> {
        match self.mode {
            IsolationMode::DryRun | IsolationMode::GitClone => {
                if self.root.starts_with("/tmp/") {
                    tokio::fs::remove_dir_all(&self.root).await.ok();
                }
            }
            IsolationMode::Docker => {
                Command::new("docker")
                    .args(["rm", "-f", &self.container_name])
                    .status()
                    .await
                    .ok();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::synthetic_instances;

    #[tokio::test]
    async fn dry_run_setup_creates_dir() {
        let instances = synthetic_instances();
        let ws = IsolatedWorkspace::setup(&instances[0], IsolationMode::DryRun)
            .await
            .unwrap();
        assert!(ws.root.exists());
        assert!(ws.root.join("tests/test_placeholder.py").exists());
        ws.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn dry_run_test_result_scripted_by_name() {
        let instances = synthetic_instances();
        let ws_a = IsolatedWorkspace::setup(&instances[0], IsolationMode::DryRun)
            .await
            .unwrap();
        assert!(
            ws_a.run_tests(&instances[0]).await,
            "pass instance should return true"
        );
        ws_a.cleanup().await.unwrap();

        let ws_b = IsolatedWorkspace::setup(&instances[1], IsolationMode::DryRun)
            .await
            .unwrap();
        assert!(
            !ws_b.run_tests(&instances[1]).await,
            "fail instance should return false"
        );
        ws_b.cleanup().await.unwrap();
    }
}
