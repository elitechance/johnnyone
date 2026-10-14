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

/// Run a fixture's verify command with its output CAPTURED, never inherited.
///
/// These fixtures exist to be RED: `fixture_every_verify_is_red_on_stubs` asserts that every one of
/// them fails against the stubs. With `.status()` the child cargo inherited this process's stdout, so
/// each of those expected failures printed its own `test add_works ... FAILED`, its own `failures:`
/// block and its own `error: test failed, to rerun pass \`--lib\`` into the PARENT's test stream —
/// roughly 1,700 lines of them. Reading that output, the parent's one real failure is invisible among
/// a dozen deliberate ones that look identical, and on 2026-10-14 it cost a wrong diagnosis: the
/// genuine failure (`kloo_needs_changes_parks_instead_of_alldone_loop`, environment-dependent) was
/// reported as "just the fixture failing on purpose", because the `failures: add_works` block nearest
/// the summary belonged to a child.
///
/// Captured output is attached to the assertion message instead, so it is there when a failure is
/// real and absent when it is not.
fn run_verify(argv: &[String], dir: &std::path::Path) -> (bool, String) {
    let out = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(dir)
        .output()
        .expect("spawn verify");
    let detail = format!(
        "exit {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    (out.status.success(), detail)
}

/// NOTE (integration, 2026-10-14): an earlier comment here measured the nested `FAILED` noise
/// this test used to print into the parent stream and taught readers to grep `0 filtered out`
/// for the real verdict. `run_verify` above now CAPTURES that output, so the noise is gone at
/// the source and the counts in that comment no longer describe this tree. The final
/// `test result` line is the verdict; it was removed rather than left asserting stale numbers.
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
        let (ok, detail) = run_verify(&argv, &tmp);
        assert!(
            !ok,
            "verify for {} must be red on stubs: {}\n{}",
            s.id, s.verify, detail
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
    let (ok, detail) = run_verify(&add_argv, &tmp);
    assert!(ok, "add should be green after impl\n{detail}");
    let sub = specs.iter().find(|s| s.id == "02-sub").unwrap();
    let sub_argv = check_verify(&sub.verify).expect("sub verify");
    let (sub_ok, detail) = run_verify(&sub_argv, &tmp);
    assert!(!sub_ok, "sub must stay red\n{detail}");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn fixture_spine_gitignore_and_lock() {
    let ws = fixture_root().join("workspace");
    let gi = std::fs::read_to_string(ws.join(".gitignore")).unwrap();
    assert!(gi.contains("/target"), "{gi}");
    assert!(ws.join("Cargo.lock").is_file());
}
