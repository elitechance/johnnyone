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

/// QA B round 3 — attacking the fixture-root sweep.
///
/// Every test here builds its own FAKE base directory inside this process's real fixture root and
/// points the sweep at that, so no test can touch a sibling run's root or anything outside this
/// process's sandbox. Child processes used to occupy a fixture are spawned and reaped by the test
/// that made them.
#[cfg(test)]
mod qa_b_r3_tests {
    use super::*;

    /// A pid that is certainly gone.
    fn dead_pid() -> u32 {
        let mut child = std::process::Command::new("true").spawn().expect("spawn true");
        let pid = child.id();
        child.wait().expect("reap");
        pid
    }

    /// A fake `<temp>` to sweep, inside this process's own root.
    fn fake_base(tag: &str) -> PathBuf {
        tmp_dir(tag)
    }

    fn root_in(base: &Path, pid: u32) -> PathBuf {
        let r = base.join(format!("{}{}", ROOT_PREFIX, pid));
        std::fs::create_dir_all(&r).unwrap();
        r
    }

    fn fixture_with_file(root: &Path, name: &str) -> PathBuf {
        let d = root.join(name);
        std::fs::create_dir_all(d.join("ws")).unwrap();
        std::fs::write(d.join("payload.bin"), vec![0u8; 4096]).unwrap();
        d
    }

    /// A child process parked in `dir`, which the caller must reap.
    fn occupy(dir: &Path) -> std::process::Child {
        let child = std::process::Command::new("sleep")
            .arg("120")
            .current_dir(dir)
            .spawn()
            .expect("spawn sleep");
        // Wait until the kernel really shows the cwd, so the test is not racing the spawn.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Ok(cwd) = std::fs::read_link(format!("/proc/{}/cwd", child.id())) {
                if cwd.starts_with(dir) {
                    break;
                }
            }
            assert!(std::time::Instant::now() < deadline, "child never entered {}", dir.display());
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        child
    }

    fn reap(mut child: std::process::Child) {
        let _ = child.kill();
        let _ = child.wait();
    }

    /// THE invariant: a fixture a live process is sitting in is never removed, while the rest of
    /// a dead owner's root is reclaimed.
    ///
    /// This is also the re-measurement of Dev B's own correction. Their first version skipped the
    /// whole root when anything in it was occupied, which reclaimed nothing, because every stale
    /// root has a dead owner AND exactly one leaked `claude` inside it.
    #[test]
    fn qa_an_occupied_fixture_survives_while_its_siblings_are_reclaimed() {
        let base = fake_base("qa-r3-sweep");
        let mine = root_in(&base, std::process::id());
        let stale = root_in(&base, dead_pid());

        let occupied = fixture_with_file(&stale, "state-0");
        let free_a = fixture_with_file(&stale, "state-1");
        let free_b = fixture_with_file(&stale, "state-2");
        let child = occupy(&occupied.join("ws"));

        sweep_exited_roots(&base, &mine);

        assert!(stale.is_dir(), "the root itself must stay while it holds something occupied");
        assert!(occupied.is_dir(), "the OCCUPIED fixture must never be removed");
        assert!(
            occupied.join("payload.bin").is_file(),
            "the occupied fixture must not be partially emptied either"
        );
        assert!(!free_a.exists(), "an unoccupied sibling must be reclaimed");
        assert!(!free_b.exists(), "an unoccupied sibling must be reclaimed");
        reap(child);
    }

    /// A crashed run — dead owner, nothing occupied — is reclaimed whole by the next run.
    #[test]
    fn qa_a_crashed_runs_root_is_reclaimed_entirely() {
        let base = fake_base("qa-r3-crashed");
        let mine = root_in(&base, std::process::id());
        let stale = root_in(&base, dead_pid());
        fixture_with_file(&stale, "state-0");
        fixture_with_file(&stale, "state-1");

        sweep_exited_roots(&base, &mine);
        assert!(!stale.exists(), "a crashed run's root must be gone entirely");
        assert!(mine.is_dir(), "and this process's own root must survive");
    }

    /// A root whose pid is LIVE is left alone — which is what makes two concurrent runs sharing
    /// one `TMPDIR` safe, and what makes pid reuse conservative rather than destructive.
    #[test]
    fn qa_a_root_owned_by_a_live_pid_is_left_alone() {
        let base = fake_base("qa-r3-livepid");
        let mine = root_in(&base, std::process::id());
        let child = std::process::Command::new("sleep").arg("120").spawn().unwrap();
        let sibling = root_in(&base, child.id());
        let f = fixture_with_file(&sibling, "state-0");

        sweep_exited_roots(&base, &mine);

        assert!(sibling.is_dir(), "a live pid's root must not be swept");
        assert!(f.join("payload.bin").is_file(), "nor anything inside it");
        assert!(pid_is_live(&child.id().to_string()));
        reap(child);
    }

    /// The only guard against deleting something the harness did not create is the NAME, so the
    /// exact shape of that guard matters. Near misses must all survive.
    #[test]
    fn qa_only_an_exact_prefix_plus_digits_can_be_swept() {
        let base = fake_base("qa-r3-names");
        let mine = root_in(&base, std::process::id());
        let gone = dead_pid();

        // Every one of these must SURVIVE the sweep.
        let survivors: Vec<PathBuf> = [
            format!("{ROOT_PREFIX}abc"),          // not numeric
            format!("{ROOT_PREFIX}"),             // empty suffix
            format!("{ROOT_PREFIX}12a"),          // partly numeric
            format!("{ROOT_PREFIX}1_2"),          // underscore
            format!("{ROOT_PREFIX}-1"),           // sign
            format!("{ROOT_PREFIX}{gone} "),      // trailing space
            format!("{ROOT_PREFIX}{gone}.bak"),   // suffixed
            format!("not-{ROOT_PREFIX}{gone}"),   // prefixed
            format!("j1-fixture-{gone}"),         // one char short of the prefix
            "j1-p2-state-123-4".to_string(),      // the OLD naming, which this sweep must ignore
        ]
        .iter()
        .map(|n| {
            let d = base.join(n);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("keep.txt"), b"keep").unwrap();
            d
        })
        .collect();
        // A FILE whose name matches exactly must also survive — the sweep only removes dirs.
        let file_named_like_a_root = base.join(format!("{ROOT_PREFIX}{}", dead_pid()));
        std::fs::write(&file_named_like_a_root, b"not a root").unwrap();

        sweep_exited_roots(&base, &mine);

        for s in &survivors {
            assert!(
                s.join("keep.txt").is_file(),
                "{} was swept but does not match the harness's own naming",
                s.display()
            );
        }
        assert!(file_named_like_a_root.is_file(), "a regular file must not be removed");
        assert!(mine.is_dir());
    }

    /// DOCUMENTED BLAST RADIUS. The guard is naming-only, so ANY directory in the temp dir that
    /// happens to be called `j1-fixtures-<digits>` for a pid that is not running is removed —
    /// whoever made it. Pinned so the radius is a stated property rather than a surprise. A
    /// numeric name far outside the pid range is the realistic accident.
    #[test]
    fn qa_documents_a_coincidentally_named_directory_is_swept() {
        let base = fake_base("qa-r3-radius");
        let mine = root_in(&base, std::process::id());
        // No such pid can exist: Linux pids are bounded well below this.
        let impostor = base.join(format!("{ROOT_PREFIX}999999999999"));
        std::fs::create_dir_all(&impostor).unwrap();
        std::fs::write(impostor.join("someone-elses.txt"), b"not the harness's").unwrap();
        assert!(!pid_is_live("999999999999"));

        sweep_exited_roots(&base, &mine);

        assert!(
            !impostor.exists(),
            "the sweep removes anything matching its naming, not only what it created"
        );
    }

    /// FINDING, measured. A SYMLINK named like a stale root is followed by `is_dir()` and the
    /// occupancy check cannot see through it, because `/proc/<pid>/cwd` resolves to the real path
    /// while the check compares against the link path.
    ///
    /// Whether that destroys the target depends on `remove_dir_all`'s symlink behaviour, which is
    /// the thing this test measures rather than assumes.
    #[test]
    fn qa_a_symlinked_root_is_measured_not_assumed() {
        let base = fake_base("qa-r3-symroot");
        let mine = root_in(&base, std::process::id());
        let target = tmp_dir("qa-r3-symtarget");
        std::fs::write(target.join("precious.txt"), b"must survive").unwrap();
        std::fs::create_dir_all(target.join("state-0")).unwrap();

        let linked_root = base.join(format!("{}{}", ROOT_PREFIX, dead_pid()));
        std::os::unix::fs::symlink(&target, &linked_root).unwrap();
        // The sweep's own predicate follows the link, so the entry looks like a stale root.
        assert!(linked_root.is_dir(), "is_dir() follows the link");

        sweep_exited_roots(&base, &mine);

        // The assertion that matters: the link's TARGET and its contents are outside the base the
        // sweep was told to clean, and must not be destroyed through the link.
        assert!(
            target.is_dir(),
            "the symlink target directory was destroyed through the link"
        );
        assert!(
            target.join("precious.txt").is_file(),
            "a file outside the swept base was deleted through a symlinked root"
        );
        assert!(
            target.join("state-0").is_dir(),
            "a subdirectory outside the swept base was deleted through a symlinked root"
        );
    }

    /// The occupancy probe reads `/proc/<pid>/cwd`, never command lines — the `pgrep -f` trap
    /// that has already cost this project a day. Pinned at the predicate.
    #[test]
    fn qa_occupancy_is_decided_by_cwd_not_by_command_line() {
        let base = fake_base("qa-r3-cwd");
        let dir = base.join("occupied");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!any_live_cwd_inside(&dir), "nothing is in it yet");

        let child = occupy(&dir);
        assert!(any_live_cwd_inside(&dir), "a live cwd inside must be seen");
        // A sibling path that merely shares a prefix string is NOT occupied — `starts_with` is
        // path-component-wise, so `occupied-sibling` must not match `occupied`.
        let sibling = base.join("occupied-sibling");
        std::fs::create_dir_all(&sibling).unwrap();
        assert!(
            !any_live_cwd_inside(&sibling),
            "a name sharing a prefix must not be read as occupied"
        );
        reap(child);

        // Once the holder is gone the directory is free again. The child is reaped above, so the
        // pid is gone rather than a zombie.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while any_live_cwd_inside(&dir) {
            assert!(std::time::Instant::now() < deadline, "cwd still held after reap");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// `J1_KEEP_FIXTURES` gates BOTH paths, and both read one predicate.
    ///
    /// QA deliberately does NOT call `cleanup_fixture_root()` here. Its whole job is
    /// `remove_dir_all(fixture_root())`, and `fixture_root()` is shared by every test in this
    /// process — so if the gate were broken, this test's failure mode would be deleting the
    /// fixtures out from under whatever else is running, in a suite that runs tests in parallel
    /// by default. A test that destroys the suite to prove a point is not worth the point.
    ///
    /// What is asserted instead is the predicate both paths branch on, both ways, plus the fact
    /// that there are exactly two readers of it: the sweep's gate inside `fixture_root()` and the
    /// early return in `cleanup_fixture_root()`. The sweep half is additionally unexercisable
    /// in-process regardless: it sits behind a `OnceLock` that has already fired by the time any
    /// test body runs.
    #[test]
    fn qa_keep_fixtures_is_one_predicate_that_gates_both_paths() {
        let had = std::env::var_os(KEEP_FIXTURES_ENV);

        std::env::remove_var(KEEP_FIXTURES_ENV);
        assert!(!keep_fixtures(), "unset must mean sweep-and-clean");
        // Any value at all opts out — it is an `is_some` check, not a parse.
        for v in ["1", "0", "", "no", "false"] {
            std::env::set_var(KEEP_FIXTURES_ENV, v);
            assert!(
                keep_fixtures(),
                "{KEEP_FIXTURES_ENV}={v:?} must opt out; the check is presence, not value"
            );
        }

        match had {
            Some(v) => std::env::set_var(KEEP_FIXTURES_ENV, v),
            None => std::env::remove_var(KEEP_FIXTURES_ENV),
        }
        assert!(!keep_fixtures(), "the var must be restored for every other test");
    }
}
