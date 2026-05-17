//! Auto-test validation loop (extension 3.3).
//!
//! After a tool batch that modifies files, detect the project's test runner
//! and execute it. On failure, the engine injects the captured output as the
//! next turn's user message; the agent retries up to `MAX_RETRIES` times.

use agent_tui_execpolicy::SandboxedCommand;
use camino::Utf8Path;
use serde::{Deserialize, Serialize};

pub const MAX_RETRIES: u32 = 3;
const TEST_TIMEOUT_SECS: u64 = 180;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestRunner {
    Cargo,
    Pytest,
    Npm,
    Go,
}

impl TestRunner {
    pub fn label(&self) -> &'static str {
        match self {
            TestRunner::Cargo => "cargo test",
            TestRunner::Pytest => "pytest",
            TestRunner::Npm => "npm test",
            TestRunner::Go => "go test ./...",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TestOutcome {
    pub runner: TestRunner,
    pub passed: bool,
    pub output: String,
    pub timed_out: bool,
}

/// Detect the project's test runner by looking at marker files in the root.
///
/// Detection order matches the build plan:
///   `Cargo.toml` → cargo; `pyproject.toml` or `pytest.ini` → pytest;
///   `package.json` with a `test` script → npm; `go.mod` → go.
pub fn detect_runner(root: &Utf8Path) -> Option<TestRunner> {
    if root.join("Cargo.toml").exists() {
        return Some(TestRunner::Cargo);
    }
    if root.join("pyproject.toml").exists() || root.join("pytest.ini").exists() {
        return Some(TestRunner::Pytest);
    }
    if root.join("package.json").exists()
        && package_json_has_test_script(&root.join("package.json"))
    {
        return Some(TestRunner::Npm);
    }
    if root.join("go.mod").exists() {
        return Some(TestRunner::Go);
    }
    None
}

fn package_json_has_test_script(path: &Utf8Path) -> bool {
    let Ok(body) = std::fs::read_to_string(path.as_std_path()) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
        return false;
    };
    v.get("scripts")
        .and_then(|s| s.get("test"))
        .and_then(|t| t.as_str())
        .is_some()
}

/// Run the detected test runner.
///
/// Tests run through the sandbox layer so they inherit the standard egress
/// and signal-handling policies. Output is truncated by `SandboxedCommand`'s
/// own buffer cap; if the call times out, the `timed_out` flag is set.
pub async fn run_tests(root: &Utf8Path, runner: TestRunner) -> TestOutcome {
    let (prog, args): (&str, Vec<String>) = match runner {
        TestRunner::Cargo => ("cargo", vec!["test".into(), "--quiet".into()]),
        TestRunner::Pytest => ("pytest", vec!["-q".into()]),
        TestRunner::Npm => ("npm", vec!["test".into(), "--silent".into()]),
        TestRunner::Go => ("go", vec!["test".into(), "./...".into()]),
    };
    let mut cmd = SandboxedCommand::new(prog);
    cmd.args = args;
    cmd.cwd = Some(root.to_owned());
    cmd.timeout_secs = TEST_TIMEOUT_SECS;
    match cmd.run().await {
        Ok(out) => {
            let mut body = String::new();
            if !out.stdout.is_empty() {
                body.push_str(&out.stdout);
            }
            if !out.stderr.is_empty() {
                if !body.is_empty() {
                    body.push('\n');
                }
                body.push_str(&out.stderr);
            }
            TestOutcome {
                runner,
                passed: out.exit_code == Some(0) && !out.timed_out,
                output: body,
                timed_out: out.timed_out,
            }
        }
        Err(e) => TestOutcome {
            runner,
            passed: false,
            output: format!("auto-test failed to launch: {e}"),
            timed_out: false,
        },
    }
}

/// Build the structured user message body that gets fed back to the model
/// after a failing test run.
pub fn failure_message(outcome: &TestOutcome, attempts: u32) -> String {
    let truncated = truncate_for_context(&outcome.output, 4_000);
    let timeout_note = if outcome.timed_out {
        "\n(note: tests timed out)"
    } else {
        ""
    };
    format!(
        "Auto-test ran `{}` and failed after your last edit (attempt {attempts}/{MAX_RETRIES}). \
         Fix the failure without reverting correct changes.\n\n```\n{truncated}\n```{timeout_note}",
        outcome.runner.label()
    )
}

fn truncate_for_context(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let head = &s[..max / 2];
    let tail = &s[s.len() - max / 2..];
    format!(
        "{head}\n... [truncated {} bytes] ...\n{tail}",
        s.len() - max
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;

    fn td() -> (tempfile::TempDir, Utf8PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let p = Utf8PathBuf::from_path_buf(t.path().to_path_buf()).unwrap();
        (t, p)
    }

    #[test]
    fn detect_cargo() {
        let (_t, p) = td();
        std::fs::write(p.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        assert_eq!(detect_runner(&p), Some(TestRunner::Cargo));
    }

    #[test]
    fn detect_pytest_via_pyproject() {
        let (_t, p) = td();
        std::fs::write(p.join("pyproject.toml"), "").unwrap();
        assert_eq!(detect_runner(&p), Some(TestRunner::Pytest));
    }

    #[test]
    fn detect_npm_requires_test_script() {
        let (_t, p) = td();
        std::fs::write(p.join("package.json"), r#"{"name":"x"}"#).unwrap();
        assert_eq!(detect_runner(&p), None, "no test script -> no npm");
        std::fs::write(
            p.join("package.json"),
            r#"{"name":"x","scripts":{"test":"jest"}}"#,
        )
        .unwrap();
        assert_eq!(detect_runner(&p), Some(TestRunner::Npm));
    }

    #[test]
    fn detect_go() {
        let (_t, p) = td();
        std::fs::write(p.join("go.mod"), "module x\n").unwrap();
        assert_eq!(detect_runner(&p), Some(TestRunner::Go));
    }

    #[test]
    fn detect_none() {
        let (_t, p) = td();
        assert_eq!(detect_runner(&p), None);
    }

    #[test]
    fn failure_message_truncates_long_output() {
        let outcome = TestOutcome {
            runner: TestRunner::Cargo,
            passed: false,
            output: "x".repeat(20_000),
            timed_out: false,
        };
        let m = failure_message(&outcome, 1);
        assert!(m.contains("[truncated"));
        assert!(m.len() < 6_000);
    }
}
