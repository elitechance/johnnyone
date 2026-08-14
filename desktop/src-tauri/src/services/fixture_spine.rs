//! Lib tests for the committed `small-mode-spine` fixture (task 04-01).

use crate::services::task_spec::{load_task_spec, topo_sort, TaskSpec};
use crate::services::verify_policy::check_verify;
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/small-mode-spine")
}

fn phase_dir() -> PathBuf {
    fixture_root().join("plan/phases/00-calc")
}

fn load_all_specs() -> Vec<TaskSpec> {
    let tasks = phase_dir().join("tasks");
    let mut specs = Vec::new();
    let mut dirs: Vec<_> = std::fs::read_dir(&tasks)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    dirs.sort_by_key(|e| e.file_name());
    for d in dirs {
        if d.path().join("task.yml").is_file() {
            specs.push(load_task_spec(&d.path()).expect("parse task.yml"));
        }
    }
    specs
}

fn copy_dir(src: &Path, dst: &Path) {
    crate::services::atomic_fs::copy_dir_skipping_target(src, dst).unwrap();
}

#[test]
fn fixture_spine_has_at_least_ten_task_yml() {
    let specs = load_all_specs();
    assert!(specs.len() >= 10, "got {}", specs.len());
    for s in &specs {
        assert!(
            phase_dir().join("tasks").join(&s.id).join("prompt.md").is_file(),
            "missing prompt.md for {}",
            s.id
        );
    }
}

#[test]
fn fixture_spine_every_task_yml_parses() {
    let _ = load_all_specs();
}

#[test]
fn fixture_spine_topo_sort_succeeds() {
    let specs = load_all_specs();
    let order = topo_sort(&specs).expect("dag");
    assert_eq!(order.len(), specs.len());
    assert!(specs.iter().any(|s| !s.depends_on.is_empty()), "need a depends_on edge");
}

#[test]
fn fixture_spine_verifies_are_narrow() {
    for s in load_all_specs() {
        let v = s.verify.as_str();
        assert!(!v.contains("cargo build"), "{v}");
        assert!(!v.contains("npm test"), "{v}");
        assert!(
            v.contains("--exact"),
            "verify must use --exact: {v}"
        );
        assert!(
            v.contains("cargo test "),
            "{v}"
        );
        assert!(
            !v.trim_end().ends_with("cargo test") && v != "cargo test",
            "bare cargo test forbidden: {v}"
        );
        check_verify(v).unwrap_or_else(|e| panic!("{}: {e}", s.id));
    }
}

#[test]
fn fixture_every_verify_is_red_on_stubs() {
    let tmp = std::env::temp_dir().join(format!(
        "j1-spine-red-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    copy_dir(&fixture_root().join("workspace"), &tmp);
    for s in load_all_specs() {
        let argv = check_verify(&s.verify).expect("fixture verify must be allowlisted");
        let st = Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(&tmp)
            .status()
            .expect("spawn verify");
        assert!(
            !st.success(),
            "verify for {} must be red on stubs (exit {:?}): {}",
            s.id,
            st.code(),
            s.verify
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn fixture_one_verify_turns_green() {
    let tmp = std::env::temp_dir().join(format!(
        "j1-spine-green-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    copy_dir(&fixture_root().join("workspace"), &tmp);
    std::fs::copy(
        fixture_root().join("workspace-expected/src/add.rs"),
        tmp.join("src/add.rs"),
    )
    .unwrap();
    let specs = load_all_specs();
    let add = specs.iter().find(|s| s.id == "01-add").unwrap();
    let add_argv = check_verify(&add.verify).expect("add verify");
    let ok = Command::new(&add_argv[0])
        .args(&add_argv[1..])
        .current_dir(&tmp)
        .status()
        .unwrap();
    assert!(ok.success(), "add should be green after impl");
    let sub = specs.iter().find(|s| s.id == "02-sub").unwrap();
    let sub_argv = check_verify(&sub.verify).expect("sub verify");
    let bad = Command::new(&sub_argv[0])
        .args(&sub_argv[1..])
        .current_dir(&tmp)
        .status()
        .unwrap();
    assert!(!bad.success(), "sub must stay red");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn fixture_spine_gitignore_and_lock() {
    let ws = fixture_root().join("workspace");
    let gi = std::fs::read_to_string(ws.join(".gitignore")).unwrap();
    assert!(gi.contains("/target"), "{gi}");
    assert!(ws.join("Cargo.lock").is_file());
}
