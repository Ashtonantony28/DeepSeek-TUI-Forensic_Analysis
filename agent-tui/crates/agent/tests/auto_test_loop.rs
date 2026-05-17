//! Integration tests for extension 3.3 — auto-test validation loop.
//!
//! Detection only: an end-to-end run that actually invokes `cargo test`
//! would be slow and brittle inside the test harness, so we exercise the
//! detection helpers directly and rely on the unit tests in
//! `auto_test.rs` for behaviour.

use agent_tui_agent::auto_test::{detect_runner, TestRunner};
use camino::Utf8PathBuf;

fn temp_root() -> (tempfile::TempDir, Utf8PathBuf) {
    let td = tempfile::tempdir().unwrap();
    let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
    (td, root)
}

#[test]
fn cargo_project_detected() {
    let (_td, root) = temp_root();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
    assert_eq!(detect_runner(&root), Some(TestRunner::Cargo));
}

#[test]
fn pytest_via_pytest_ini() {
    let (_td, root) = temp_root();
    std::fs::write(root.join("pytest.ini"), "").unwrap();
    assert_eq!(detect_runner(&root), Some(TestRunner::Pytest));
}

#[test]
fn npm_without_test_script_is_skipped() {
    let (_td, root) = temp_root();
    std::fs::write(root.join("package.json"), r#"{"name":"x"}"#).unwrap();
    assert_eq!(detect_runner(&root), None);
}

#[test]
fn no_markers_returns_none() {
    let (_td, root) = temp_root();
    assert_eq!(detect_runner(&root), None);
}
