//! Shared, test-only harness for the overhaul P2 file-ops phase.
//!
//! `host_files.rs` (service + serde) and `agent/mod.rs` (relay-RPC wiring) both need an in-process
//! `AppState` backed by a throwaway temp dir. This is the single reusable copy of the Phase-1
//! `test_state`/`tmp_dir` idiom so the two modules don't each fork their own. `#[cfg(test)]`-gated:
//! it is never compiled into the shipped binary.
//!
//! # Fixtures are confined to ONE directory per process, and reclaimed
//!
//! Every fixture used to be its own top-level directory in the system temp dir, named
//! `j1-<phase>-<tag>-<pid>-<seq>`, and **nothing ever removed any of them**. QA counted **13,557
//! leftovers totalling 15 GB**, going back a week, and they are the real cause of the `EDQUOT`
//! failures three of us misattributed to cargo builds — including the day spent seeking approval
//! for a cleanup that would have treated the symptom.
//!
//! So every fixture now lives under [`fixture_root`], `<temp>/j1-fixtures-<pid>/`, and the first
//! call to it **sweeps the roots of processes that have exited**. That bounds the leftovers to
//! roughly one run's worth instead of a week's, with no change to any of the ~116 call sites.
//!
//! **What this does NOT do, precisely.** It is not an at-exit removal. Rust statics are never
//! dropped, the generated test-harness `main` cannot be hooked, main-thread thread-local
//! destructors are not guaranteed to run, and this crate depends on neither `libc` nor `ctor`, so
//! there is no exit hook available that does not cost a new dependency. The guarantee is
//! therefore "reclaimed by the next run", not "removed at exit"; a run that is the last one for a
//! while leaves its own root behind. [`cleanup_fixture_root`] is the explicit version for a
//! caller that wants it now.
//!
//! **Opt-out for debugging:** set `J1_KEEP_FIXTURES=1` and nothing is swept or removed, so a
//! fixture from a failing test survives for inspection.
//!
//! **The sweep never touches a directory a live process is sitting in.** Two reasons: the suite
//! leaks live `claude` processes whose cwd is a fixture (see the note on
//! `replan_tests::kloo_needs_changes_parks_instead_of_alldone_loop`), and deleting the ground out
//! from under someone else's process is not this harness's business.

use crate::db::Database;
use crate::state::app_state::AppState;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Set this to any value to keep fixtures on disk for inspection.
pub const KEEP_FIXTURES_ENV: &str = "J1_KEEP_FIXTURES";

/// Prefix of a per-process fixture root. Matched when sweeping, so it must stay distinctive.
const ROOT_PREFIX: &str = "j1-fixtures-";

fn keep_fixtures() -> bool {
    std::env::var_os(KEEP_FIXTURES_ENV).is_some()
}

/// This process's single fixture root, `<temp>/j1-fixtures-<pid>/`, created on first use.
///
/// Sweeps dead processes' roots on that first call. Honours `TMPDIR`, because the whole point is
/// that a run can be pointed somewhere with room.
pub fn fixture_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let base = std::env::temp_dir();
        let root = base.join(format!("{}{}", ROOT_PREFIX, std::process::id()));
        std::fs::create_dir_all(&root)
            .unwrap_or_else(|e| panic!("create fixture root {}: {e}", root.display()));
        if !keep_fixtures() {
            sweep_exited_roots(&base, &root);
        }
        root
    })
    .as_path()
}

/// Remove `<temp>/j1-fixtures-<pid>` roots whose pid is gone — never `mine`, and never one some
/// live process is sitting in.
fn sweep_exited_roots(base: &Path, mine: &Path) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == mine || !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.strip_prefix(ROOT_PREFIX)) else {
            continue;
        };
        // Only a pure numeric suffix — never a directory that merely starts with the prefix.
        if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if pid_is_live(pid) {
            continue;
        }
        // Best effort by design: a racing sweep from a sibling run may have won, and losing that
        // race is not a failure worth failing a test suite over.
        if any_live_cwd_inside(&path) {
            // MEASURED, and the reason this branch exists: every stale root on this machine has
            // a dead owner AND exactly one live process inside it — a `claude` leaked by
            // `replan_tests::kloo_needs_changes_parks_instead_of_alldone_loop`, whose cwd is
            // `<root>/state-N/ws`. Skipping the whole root therefore reclaimed nothing at all:
            // three consecutive suite runs left three 120 MB roots standing.
            //
            // So reclaim the root's fixtures INDIVIDUALLY and keep only the occupied ones. One
            // pinned fixture is a few MB; the rest of the root is ~119 of its 120 MB. The
            // occupied directory itself is still never touched, which is the invariant that
            // matters: the orphans are not ours to reap, and pulling the ground out from under
            // someone else's process is not this harness's business.
            reclaim_unoccupied_fixtures(&path);
            continue;
        }
        let _ = std::fs::remove_dir_all(&path);
    }
}

/// Remove the fixtures inside a dead owner's `root` that no live process is sitting in.
///
/// Leaves `root` itself in place: it still holds whatever is occupied, and an empty directory is
/// not the problem. Never recurses into an occupied fixture — the whole fixture is kept or
/// removed, so a partially-deleted tree can never be handed to a live process.
fn reclaim_unoccupied_fixtures(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if any_live_cwd_inside(&path) {
            continue;
        }
        let _ = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
    }
}

/// Is a process with this pid still running? Non-unix has no cheap answer, so it answers "yes",
/// which makes the sweep do nothing rather than guess.
fn pid_is_live(pid: &str) -> bool {
    if cfg!(unix) {
        Path::new("/proc").join(pid).exists()
    } else {
        true
    }
}

/// Is any live process's working directory inside `dir`?
///
/// Reads `/proc/<pid>/cwd` rather than matching command lines: `pgrep -f`-style matching catches
/// the matcher's own shell, which has already cost this project a day.
fn any_live_cwd_inside(dir: &Path) -> bool {
    if !cfg!(unix) {
        return true;
    }
    let Ok(procs) = std::fs::read_dir("/proc") else {
        // Cannot tell -> assume occupied. The sweep is an optimisation; a wrong "no" deletes
        // someone's working directory, a wrong "yes" only keeps a directory one run longer.
        return true;
    };
    for entry in procs.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str() else { continue };
        if !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if let Ok(cwd) = std::fs::read_link(entry.path().join("cwd")) {
            if cwd.starts_with(dir) {
                return true;
            }
        }
    }
    false
}

/// Remove this process's fixture root now. No-op under `J1_KEEP_FIXTURES`.
///
/// For a caller that wants the space back without waiting for the next run's sweep. Not called
/// automatically, because there is no exit hook to call it from (see the module note).
pub fn cleanup_fixture_root() {
    if keep_fixtures() {
        return;
    }
    let _ = std::fs::remove_dir_all(fixture_root());
}

/// Unique, freshly-created fixture dir INSIDE this process's [`fixture_root`].
///
/// `Date::now`/random are unavailable in this environment, so the suffix is made unique with a
/// static counter; the pid now lives in the root's name instead of being repeated in every leaf.
pub fn tmp_dir(tag: &str) -> PathBuf {
    let d = fixture_root().join(format!("{}-{}", tag, SEQ.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("create fixture {}: {e}", d.display()));
    d
}

/// Full in-process harness: `Database::open` runs migrations; `AppState::new` wires the broadcast
/// channels with no live process, relay, or Tauri handle. Returns the state plus its temp root so
/// callers can clean up / point `files_root` at it.
pub fn test_state() -> (AppState, PathBuf) {
    let root = tmp_dir("state");
    let db = Database::open(&root.join("test.db")).expect("open db");
    (AppState::new(db), root)
}

#[cfg(test)]
mod fixture_root_tests {
    use super::*;

    /// A pid that is certainly gone: spawn a child, reap it, keep its number.
    fn an_exited_pid() -> u32 {
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn `true`");
        let pid = child.id();
        child.wait().expect("reap");
        pid
    }

    #[test]
    fn every_fixture_lives_under_one_per_process_root() {
        let root = fixture_root();
        assert!(root.is_dir(), "{} must exist", root.display());
        assert_eq!(
            root.file_name().unwrap().to_string_lossy(),
            format!("{}{}", ROOT_PREFIX, std::process::id()),
            "the root is named after THIS process, so a sweep can tell whose it is"
        );
        // Every factory — including the two that used to be private copies with their own
        // top-level naming — lands inside it.
        for dir in [
            tmp_dir("ft-one"),
            tmp_dir("ft-two"),
            test_state().1,
        ] {
            assert!(
                dir.starts_with(root),
                "{} escaped the fixture root {}",
                dir.display(),
                root.display()
            );
            assert!(dir.is_dir());
            // One level deep, so the root holds fixtures and not a tree of stray parents.
            assert_eq!(dir.parent().unwrap(), root);
        }
        // Distinct names, which is what makes a shared parent safe.
        assert_ne!(tmp_dir("ft-same"), tmp_dir("ft-same"));
    }

    #[test]
    fn the_sweep_reclaims_an_exited_processes_root() {
        let base = tmp_dir("sweep-base");
        let mine = base.join(format!("{}{}", ROOT_PREFIX, std::process::id()));
        std::fs::create_dir_all(&mine).unwrap();
        let dead = base.join(format!("{}{}", ROOT_PREFIX, an_exited_pid()));
        std::fs::create_dir_all(dead.join("state-0/deep")).unwrap();
        std::fs::write(dead.join("state-0/deep/db"), b"x").unwrap();

        sweep_exited_roots(&base, &mine);

        assert!(
            !dead.exists(),
            "an exited process's root must be reclaimed, including its contents"
        );
        assert!(mine.is_dir(), "this process's own root must survive");
    }

    #[test]
    fn the_sweep_spares_live_pids_occupied_roots_and_anything_unrecognised() {
        let base = tmp_dir("sweep-spare");
        let mine = base.join(format!("{}{}", ROOT_PREFIX, std::process::id()));
        std::fs::create_dir_all(&mine).unwrap();

        // A root whose pid is still alive: ours, under a second name, is not available — so use
        // the real live pid of a child we control.
        let mut live = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let live_root = base.join(format!("{}{}", ROOT_PREFIX, live.id()));
        std::fs::create_dir_all(&live_root).unwrap();

        // A root whose pid is DEAD but which a live process is sitting in. This is the real
        // shape on this machine: the suite leaks `claude` processes whose cwd is a fixture, and
        // deleting the ground from under them is not this harness's business.
        let occupied = base.join(format!("{}{}", ROOT_PREFIX, an_exited_pid()));
        std::fs::create_dir_all(&occupied).unwrap();
        let mut squatter = std::process::Command::new("sleep")
            .arg("30")
            .current_dir(&occupied)
            .spawn()
            .expect("spawn squatter");

        // Things that merely look adjacent must be left alone entirely.
        for unrelated in [
            format!("{}notanumber", ROOT_PREFIX),
            format!("{}", ROOT_PREFIX),
            format!("{}12x34", ROOT_PREFIX),
            "j1-p2-state-123-0".to_string(),
            "someone-elses-data".to_string(),
        ] {
            std::fs::create_dir_all(base.join(&unrelated)).unwrap();
        }

        sweep_exited_roots(&base, &mine);

        assert!(live_root.is_dir(), "a live pid's root must survive");
        assert!(
            occupied.is_dir(),
            "a root a live process is sitting in must survive even though its pid is gone"
        );
        for unrelated in [
            format!("{}notanumber", ROOT_PREFIX),
            format!("{}", ROOT_PREFIX),
            format!("{}12x34", ROOT_PREFIX),
            "j1-p2-state-123-0".to_string(),
            "someone-elses-data".to_string(),
        ] {
            assert!(
                base.join(&unrelated).is_dir(),
                "{unrelated} is not a fixture root and must not be touched"
            );
        }

        let _ = squatter.kill();
        let _ = squatter.wait();
        let _ = live.kill();
        let _ = live.wait();
    }

    /// The branch that makes the fix actually reclaim anything on this machine.
    ///
    /// A dead owner's root with ONE occupied fixture in it: the occupied fixture survives
    /// untouched, everything else in the root goes. Before this, the whole root was skipped and
    /// three suite runs left three 120 MB roots standing — measured.
    #[test]
    fn a_stale_root_pinned_by_one_live_process_still_reclaims_the_rest() {
        let base = tmp_dir("sweep-partial");
        let mine = base.join(format!("{}{}", ROOT_PREFIX, std::process::id()));
        std::fs::create_dir_all(&mine).unwrap();

        let stale = base.join(format!("{}{}", ROOT_PREFIX, an_exited_pid()));
        let pinned = stale.join("state-7");
        let pinned_ws = pinned.join("ws");
        std::fs::create_dir_all(&pinned_ws).unwrap();
        // The bulk: fixtures nobody is in.
        for n in 0..3 {
            let big = stale.join(format!("files-{n}"));
            std::fs::create_dir_all(big.join("nested")).unwrap();
            std::fs::write(big.join("nested/blob"), vec![0u8; 1024]).unwrap();
        }
        std::fs::write(stale.join("loose-file"), b"x").unwrap();

        // Exactly the real shape: a live process whose cwd is `<root>/state-N/ws`.
        let mut squatter = std::process::Command::new("sleep")
            .arg("30")
            .current_dir(&pinned_ws)
            .spawn()
            .expect("spawn squatter");

        sweep_exited_roots(&base, &mine);

        assert!(stale.is_dir(), "the root itself stays while something in it is occupied");
        assert!(
            pinned_ws.is_dir(),
            "the occupied fixture must be untouched — the orphan is not ours to reap"
        );
        for n in 0..3 {
            assert!(
                !stale.join(format!("files-{n}")).exists(),
                "files-{n} was unoccupied and must have been reclaimed"
            );
        }
        assert!(!stale.join("loose-file").exists(), "a loose file is reclaimed too");

        let _ = squatter.kill();
        let _ = squatter.wait();
    }

    #[test]
    fn keeping_fixtures_is_opt_out_able() {
        // The predicate is what both the sweep and `cleanup_fixture_root` gate on, so pinning it
        // pins the escape hatch a human debugging a failure relies on.
        assert_eq!(KEEP_FIXTURES_ENV, "J1_KEEP_FIXTURES");
        assert_eq!(keep_fixtures(), std::env::var_os(KEEP_FIXTURES_ENV).is_some());
    }
}
