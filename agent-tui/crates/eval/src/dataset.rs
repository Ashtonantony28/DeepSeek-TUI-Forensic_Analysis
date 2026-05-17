//! SWE-bench Verified dataset fetching.
//!
//! Two paths:
//! - `fetch_dataset`: hits the HuggingFace datasets-server API via HTTP.
//! - `synthetic_instances`: returns two hardcoded instances for `--dry-run` mode.

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct SweInstance {
    pub instance_id: String,
    pub repo: String,
    pub base_commit: String,
    pub issue_title: String,
    pub issue_body: String,
    /// Unified diff of the reference fix (used for comparison, not given to agent).
    pub reference_patch: String,
    /// Test file content (used for mutation strengthening).
    pub test_patch: String,
    /// Tests that should go from failing to passing.
    pub fail_to_pass: Vec<String>,
    /// Tests that must remain passing.
    pub pass_to_pass: Vec<String>,
}

// ── HuggingFace API response shapes ─────────────────────────────────────────

#[derive(Deserialize)]
struct HfResponse {
    rows: Vec<HfRow>,
}

#[derive(Deserialize)]
struct HfRow {
    row: HfInstance,
}

#[derive(Deserialize)]
struct HfInstance {
    instance_id: String,
    repo: String,
    base_commit: String,
    problem_statement: String,
    patch: String,
    test_patch: String,
    #[serde(rename = "FAIL_TO_PASS", default)]
    fail_to_pass: Vec<String>,
    #[serde(rename = "PASS_TO_PASS", default)]
    pass_to_pass: Vec<String>,
}

impl From<HfInstance> for SweInstance {
    fn from(h: HfInstance) -> Self {
        // First line of problem_statement used as title; rest as body.
        let mut lines = h.problem_statement.splitn(2, '\n');
        let title = lines.next().unwrap_or("").trim().to_string();
        let body = lines.next().unwrap_or("").trim().to_string();
        SweInstance {
            instance_id: h.instance_id,
            repo: h.repo,
            base_commit: h.base_commit,
            issue_title: title,
            issue_body: body,
            reference_patch: h.patch,
            test_patch: h.test_patch,
            fail_to_pass: h.fail_to_pass,
            pass_to_pass: h.pass_to_pass,
        }
    }
}

/// Fetch up to `n` instances from HuggingFace datasets-server.
/// Optionally filter to only `ids` (empty = no filter).
pub async fn fetch_dataset(n: usize, ids: Vec<String>) -> Result<Vec<SweInstance>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("build reqwest client")?;

    let url = format!(
        "https://datasets-server.huggingface.co/rows\
         ?dataset=princeton-nlp/SWE-bench_Verified\
         &config=default&split=test&offset=0&length={}",
        n.min(500)
    );

    let resp: HfResponse = client
        .get(&url)
        .send()
        .await
        .context("HuggingFace datasets-server request")?
        .error_for_status()
        .context("HuggingFace datasets-server error response")?
        .json()
        .await
        .context("parse HuggingFace response")?;

    let mut out: Vec<SweInstance> = resp.rows.into_iter().map(|r| r.row.into()).collect();

    if !ids.is_empty() {
        out.retain(|i| ids.contains(&i.instance_id));
    }

    Ok(out)
}

/// Return two synthetic instances for `--dry-run` mode.
///
/// Instance A is designed to "pass" (the mock pipeline will emit a valid diff
/// that passes synthetic tests).
/// Instance B is designed to "fail" (the mock pipeline emits no diff).
pub fn synthetic_instances() -> Vec<SweInstance> {
    vec![
        SweInstance {
            instance_id: "synthetic__pass__001".into(),
            repo: "example/repo-a".into(),
            base_commit: "aaaaaaa".into(),
            issue_title: "Off-by-one in range check".into(),
            issue_body: "The function `clamp` returns wrong value when input equals upper bound."
                .into(),
            reference_patch: SYNTHETIC_PATCH_A.into(),
            test_patch: SYNTHETIC_TEST_A.into(),
            fail_to_pass: vec!["tests/test_clamp.py::test_clamp_upper".into()],
            pass_to_pass: vec!["tests/test_clamp.py::test_clamp_lower".into()],
        },
        SweInstance {
            instance_id: "synthetic__fail__002".into(),
            repo: "example/repo-b".into(),
            base_commit: "bbbbbbb".into(),
            issue_title: "Comparison inverted in validator".into(),
            issue_body: "The `is_valid` function incorrectly rejects valid input."
                .into(),
            reference_patch: SYNTHETIC_PATCH_B.into(),
            test_patch: SYNTHETIC_TEST_B.into(),
            fail_to_pass: vec!["tests/test_validator.py::test_valid_input".into()],
            pass_to_pass: vec!["tests/test_validator.py::test_invalid_input".into()],
        },
    ]
}

// ── Synthetic data ───────────────────────────────────────────────────────────

const SYNTHETIC_PATCH_A: &str = "\
diff --git a/clamp.py b/clamp.py
--- a/clamp.py
+++ b/clamp.py
@@ -3,4 +3,4 @@
 def clamp(x, lo, hi):
-    if x > hi:
+    if x >= hi:
         return hi
     return x
";

const SYNTHETIC_TEST_A: &str = "\
def test_clamp_upper():
    assert clamp(10, 0, 10) == 10

def test_clamp_lower():
    assert clamp(-1, 0, 10) == 0

def test_clamp_middle():
    result = clamp(5, 0, 10)
    assert result == 5
    assert result >= 0
    assert result <= 10
";

const SYNTHETIC_PATCH_B: &str = "\
diff --git a/validator.py b/validator.py
--- a/validator.py
+++ b/validator.py
@@ -1,4 +1,4 @@
 def is_valid(value):
-    return value != 0
+    return value == 0
";

const SYNTHETIC_TEST_B: &str = "\
def test_valid_input():
    assert is_valid(1) == True

def test_invalid_input():
    assert is_valid(0) == False
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_instances_shape() {
        let instances = synthetic_instances();
        assert_eq!(instances.len(), 2);
        assert_eq!(instances[0].instance_id, "synthetic__pass__001");
        assert_eq!(instances[1].instance_id, "synthetic__fail__002");
        assert!(!instances[0].test_patch.is_empty());
        assert!(!instances[1].test_patch.is_empty());
        assert!(!instances[0].fail_to_pass.is_empty());
    }

    #[test]
    fn synthetic_test_a_contains_assertions() {
        let t = SYNTHETIC_TEST_A;
        assert!(t.contains("assert"));
        assert!(t.contains("=="));
    }
}
