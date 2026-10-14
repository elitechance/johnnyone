//! Plan-store artifact paths — the one place the artifact layout is written down.
//!
//! **The defect this exists to close.** Until now the convention was ONE artifact directory per
//! phase, `phases/<NN-slug>/artifacts/`, and that directory is addressed by phase id alone. Two
//! runs of the same phase therefore write to the same directory and silently overwrite each
//! other's evidence — the same failure class as two development runs sharing one checkout, and
//! just as invisible: the loser's screenshots are simply the ones that are not there any more.
//! The arena makes that normal rather than exceptional, because comparing two workflows means
//! running the same phase twice on purpose.
//!
//! So the per-run path is
//!
//! ```text
//! phases/<NN-slug>/artifacts/runs/<run_id>/
//! ```
//!
//! and the BARE `phases/<NN-slug>/artifacts/` path is kept as the champion's. Nothing is moved:
//! every plan already on disk keeps exactly the layout it has, and [`resolve_phase_artifacts_dir`]
//! is the reader that accepts both.
//!
//! **`<run_id>` is derived, not stored.** It is the identity the caller already has — today
//! [`crate::services::agent_plans`]' `phase_template_values` passes `run.plan.id`, and the arena
//! will pass `workflow_runs.id`. No column is added anywhere for it.
//!
//! **A `run_id` is not a free-form path segment.** It arrives from a caller and ends up joined
//! onto the plan store, so every builder here validates it with [`is_safe_segment`] and returns
//! `None` rather than a path. That predicate is deliberately the same rule as `isSafeSegment` in
//! `web/src/app/pages/terminal/plan-tab-logic.ts` — rejecting the empty segment, `.`, anything
//! *containing* `..`, and anything containing `/` or `\` — so the host and the navigator agree on
//! which paths exist instead of the UI offering a row the host then refuses.

use std::path::{Path, PathBuf};

/// The artifact directory's name, at plan level and inside a phase.
pub const ARTIFACTS_DIR: &str = "artifacts";

/// The directory that holds the per-run subdirectories, inside an `artifacts/` directory.
///
/// Nested under `artifacts/` rather than beside it so that everything which already treats
/// "under an `artifacts/` directory" as worker output — the spec-weakening classifier, the
/// evidence-provenance walk, the Plan tab's `DOC_DIRS` allowlist — keeps working with no change.
pub const RUNS_DIR: &str = "runs";

/// Is `seg` usable as a single path segment?
///
/// Rejects the empty segment (what a leading `/`, a trailing `/`, a `//` and a UNC `//host` all
/// decompose into), `.`, anything CONTAINING `..` (not merely equal to it, so `..` cannot hide
/// inside a longer name), and anything containing a forward or back slash. The backslash matters
/// on its own terms: on this host `\` is a legal character *in a filename*, so a Windows-shaped
/// `01-shell\artifacts\x.png` would otherwise be accepted as one long, odd name.
pub fn is_safe_segment(seg: &str) -> bool {
    !seg.is_empty() && seg != "." && !seg.contains("..") && !seg.contains('/') && !seg.contains('\\')
}

/// Is every `/`-separated segment of `path` safe? Empty input is not a path.
pub fn is_safe_relative_path(path: &str) -> bool {
    !path.is_empty() && path.split('/').all(is_safe_segment)
}

/// Does `path` name a file *inside* an `artifacts/` directory, at plan or phase level?
///
/// Segment-exact, and requires at least one segment after the `artifacts` segment — the directory
/// itself is not a file in it. Any unsafe segment anywhere makes the answer `false`, because a
/// path containing `..` names a different file from the one it reads as, and a classifier that
/// answers about the wrong file is worse than one that answers "no".
pub fn is_artifact_path(path: &str) -> bool {
    if !is_safe_relative_path(path) {
        return false;
    }
    let segs: Vec<&str> = path.split('/').collect();
    // `len() - 1` so the LAST segment cannot be the `artifacts` one.
    segs[..segs.len() - 1].iter().any(|s| *s == ARTIFACTS_DIR)
}

/// `phases/<phase_id>` — or `None` when `phase_id` is not one safe segment.
pub fn phase_rel(phase_id: &str) -> Option<String> {
    is_safe_segment(phase_id).then(|| format!("phases/{}", phase_id))
}

/// `phases/<phase_id>/artifacts` — the CHAMPION's directory, and the only one that exists in
/// every plan written before this change.
pub fn phase_artifacts_rel(phase_id: &str) -> Option<String> {
    Some(format!("{}/{}", phase_rel(phase_id)?, ARTIFACTS_DIR))
}

/// `phases/<phase_id>/artifacts/runs/<run_id>` — one competing run's own directory.
///
/// `None` if either `phase_id` or `run_id` is not a single safe segment; in particular a `run_id`
/// of `..`, `../x`, `a/b` or `a\b` yields no path at all rather than one that escapes the phase.
pub fn phase_run_artifacts_rel(phase_id: &str, run_id: &str) -> Option<String> {
    if !is_safe_segment(run_id) {
        return None;
    }
    Some(format!(
        "{}/{}/{}",
        phase_artifacts_rel(phase_id)?,
        RUNS_DIR,
        run_id
    ))
}

/// Absolute champion artifact directory for a phase (not created).
pub fn phase_artifacts_dir(plan_path: &Path, phase_id: &str) -> Option<PathBuf> {
    Some(plan_path.join(phase_artifacts_rel(phase_id)?))
}

/// Absolute per-run artifact directory for a phase (not created).
pub fn phase_run_artifacts_dir(plan_path: &Path, phase_id: &str, run_id: &str) -> Option<PathBuf> {
    Some(plan_path.join(phase_run_artifacts_rel(phase_id, run_id)?))
}

/// Create and return a phase's per-run artifact directory.
///
/// The directory is created so the worker is handed a path that exists; `create_dir_all` is
/// idempotent, so a re-entered phase lands in the same place rather than a second one.
pub fn ensure_phase_run_artifacts_dir(
    plan_path: &Path,
    phase_id: &str,
    run_id: &str,
) -> Result<PathBuf, String> {
    let dir = phase_run_artifacts_dir(plan_path, phase_id, run_id).ok_or_else(|| {
        format!(
            "Refusing to build an artifact path from phase {:?} / run {:?}: both must be one safe \
             path segment (no empty name, no `.`, no `..`, no `/` or `\\`).",
            phase_id, run_id
        )
    })?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {}", dir.display(), e))?;
    Ok(dir)
}

/// The artifact directory to READ for a phase, across both layouts.
///
/// Order: the run's own directory when a `run_id` is given and that directory exists, then the
/// bare champion directory, then `None`. The middle step is the back-compat case and it is the
/// common one — every plan in the store today has only the bare directory, and this reader must
/// keep finding it without any migration of files on disk.
pub fn resolve_phase_artifacts_dir(
    plan_path: &Path,
    phase_id: &str,
    run_id: Option<&str>,
) -> Option<PathBuf> {
    if let Some(run_id) = run_id {
        if let Some(dir) = phase_run_artifacts_dir(plan_path, phase_id, run_id) {
            if dir.is_dir() {
                return Some(dir);
            }
        }
    }
    let bare = phase_artifacts_dir(plan_path, phase_id)?;
    bare.is_dir().then_some(bare)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, bytes: &[u8]) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    /// THE defect. Two runs of the same phase each deliver a file called `seat-meter.html`;
    /// under the old one-directory-per-phase convention the second write destroyed the first and
    /// nothing anywhere reported it.
    #[test]
    fn two_runs_of_one_phase_do_not_clobber_each_other() {
        let plan = crate::test_support::tmp_dir("artifacts-clobber");
        let a = ensure_phase_run_artifacts_dir(&plan, "04-leases", "9c21ae").unwrap();
        let b = ensure_phase_run_artifacts_dir(&plan, "04-leases", "4f0821").unwrap();

        // Both runs deliver a file of the SAME name, which is the realistic case: the two sides
        // of an arena are running one phase against one contract.
        std::fs::write(a.join("seat-meter.html"), b"v1 pixels").unwrap();
        std::fs::write(b.join("seat-meter.html"), b"v2 pixels").unwrap();

        // The assertion that matters: run A's evidence survived run B writing.
        assert_eq!(
            std::fs::read(a.join("seat-meter.html")).unwrap(),
            b"v1 pixels",
            "run 9c21ae's artifact was clobbered by run 4f0821"
        );
        assert_eq!(std::fs::read(b.join("seat-meter.html")).unwrap(), b"v2 pixels");
        assert_ne!(a, b, "two runs must not be handed the same directory");
        assert_eq!(
            phase_run_artifacts_rel("04-leases", "9c21ae").unwrap(),
            "phases/04-leases/artifacts/runs/9c21ae"
        );
    }

    /// Every plan in the store today has only `phases/<id>/artifacts/`. The reader must find it
    /// even when it is asked for a run that never wrote anything.
    #[test]
    fn the_old_flat_layout_is_still_readable() {
        let plan = crate::test_support::tmp_dir("artifacts-oldlayout");
        // Laid out exactly the OLD way: no `runs/` directory anywhere.
        write(&plan, "phases/01-shell/artifacts/aux-open.png", b"open pixels");
        write(&plan, "phases/01-shell/tasks/t1/status.yml", b"status: done\n");
        assert!(!plan.join("phases/01-shell/artifacts/runs").exists());

        let resolved = resolve_phase_artifacts_dir(&plan, "01-shell", Some("9c21ae"))
            .expect("the bare artifacts dir must still resolve");
        assert_eq!(resolved, plan.join("phases/01-shell/artifacts"));
        assert!(resolved.join("aux-open.png").is_file());

        // And with no run id at all — the champion's own read path.
        assert_eq!(
            resolve_phase_artifacts_dir(&plan, "01-shell", None).unwrap(),
            plan.join("phases/01-shell/artifacts")
        );
        // A phase with no artifacts at all is None, not a path that does not exist.
        assert_eq!(resolve_phase_artifacts_dir(&plan, "02-detail", None), None);
    }

    /// Once a run HAS written, that run reads its own directory, not the champion's.
    #[test]
    fn a_run_that_wrote_reads_its_own_directory() {
        let plan = crate::test_support::tmp_dir("artifacts-prefer-run");
        write(&plan, "phases/01-shell/artifacts/aux-open.png", b"champion pixels");
        write(
            &plan,
            "phases/01-shell/artifacts/runs/4f0821/aux-open.png",
            b"challenger pixels",
        );
        let resolved = resolve_phase_artifacts_dir(&plan, "01-shell", Some("4f0821")).unwrap();
        assert_eq!(resolved, plan.join("phases/01-shell/artifacts/runs/4f0821"));
        assert_eq!(
            std::fs::read(resolved.join("aux-open.png")).unwrap(),
            b"challenger pixels"
        );
    }

    /// A `run_id` reaches these builders from a caller, so it is input, not a constant.
    #[test]
    fn a_hostile_run_id_yields_no_path_at_all() {
        for hostile in [
            "..",
            "../..",
            "../../../etc",
            "a/b",
            "a\\b",
            "",
            ".",
            "runs/../../..",
            "x..y",
        ] {
            assert_eq!(
                phase_run_artifacts_rel("04-leases", hostile),
                None,
                "run_id {:?} must not build a path",
                hostile
            );
            assert_eq!(
                phase_run_artifacts_dir(Path::new("/plan"), "04-leases", hostile),
                None,
                "run_id {:?} must not build a path",
                hostile
            );
        }
        // And the refusal is loud, not a silently-skipped directory.
        let err = ensure_phase_run_artifacts_dir(Path::new("/plan"), "04-leases", "../../etc")
            .expect_err("a traversing run id must be an error");
        assert!(err.contains("Refusing to build an artifact path"), "{err}");
        assert!(!Path::new("/plan").exists(), "nothing may be created on refusal");
    }

    /// The phase id goes through the same gate — it is a host-generated slug today, but it is
    /// also what an arena API would accept from a client tomorrow.
    #[test]
    fn a_hostile_phase_id_yields_no_path_at_all() {
        for hostile in ["..", "../x", "a/b", "a\\b", "", "."] {
            assert_eq!(phase_rel(hostile), None, "phase {:?}", hostile);
            assert_eq!(phase_artifacts_rel(hostile), None, "phase {:?}", hostile);
            assert_eq!(
                phase_run_artifacts_rel(hostile, "4f0821"),
                None,
                "phase {:?}",
                hostile
            );
        }
        assert_eq!(
            phase_artifacts_rel("01-shell").unwrap(),
            "phases/01-shell/artifacts"
        );
    }

    #[test]
    fn artifact_paths_are_recognised_by_segment_not_by_substring() {
        // Plan level, phase level, and run-scoped.
        assert!(is_artifact_path("artifacts/summary.md"));
        assert!(is_artifact_path("phases/02-acceptance/artifacts/cargo-test-lib.txt"));
        assert!(is_artifact_path(
            "phases/04-leases/artifacts/runs/4f0821/seat-meter.html"
        ));
        // The directory itself is not a file in it.
        assert!(!is_artifact_path("artifacts"));
        assert!(!is_artifact_path("phases/01-shell/artifacts"));
        // A substring is not a segment.
        assert!(!is_artifact_path("artifactsx/summary.md"));
        assert!(!is_artifact_path("my-artifacts/summary.md"));
        assert!(!is_artifact_path("phases/01-shell/notartifacts/x.png"));
        // Traversal is never an artifact path.
        assert!(!is_artifact_path("phases/01-shell/artifacts/../../overview.md"));
        assert!(!is_artifact_path("artifacts/../overview.md"));
        assert!(!is_artifact_path("/artifacts/x.png"));
        assert!(!is_artifact_path("artifacts//x.png"));
        assert!(!is_artifact_path(""));
    }

    /// The host's segment rule must stay the same rule the navigator applies, or the UI offers
    /// rows the host refuses. Pinned against the cases `isSafeSegment` names in
    /// `web/src/app/pages/terminal/plan-tab-logic.ts`.
    #[test]
    fn the_segment_rule_matches_the_navigators() {
        for bad in ["", ".", "..", "x..y", "a/b", "a\\b", "C:\\x"] {
            assert!(!is_safe_segment(bad), "{bad:?} must be refused");
        }
        for good in ["01-shell", "4f0821", "P2-plan-tab", "v1.2.png", "runs"] {
            assert!(is_safe_segment(good), "{good:?} must be accepted");
        }
    }
}

/// QA B — adversarial tests against the claims in this module's doc comment.
///
/// Tests named `qa_documents_` pin behaviour QA believes is a GAP rather than a guarantee; their
/// doc comments say what a reviewer should conclude. They are tripwires in both directions: the
/// gap cannot close silently, and it cannot widen silently either.
#[cfg(test)]
mod qa_b_tests {
    use super::*;

    fn write(dir: &Path, rel: &str, bytes: &[u8]) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    // ── Claim 4: the traversal guard, and that the old rule really was broken ──────────────────

    /// The OLD rule, replicated verbatim from `agent_plans.rs` at the parent commit `6637776`:
    ///
    /// ```ignore
    /// if path.starts_with("artifacts/") || path.contains("/artifacts/") { return true; }
    /// ```
    ///
    /// Replicated rather than cited, so "the defect was real" is a thing this suite MEASURES and
    /// keeps measuring, instead of a claim about a commit nobody re-reads. Both halves matter:
    /// the old rule said yes to a path that names the spec, and the new one says no to it while
    /// still saying yes to every real artifact.
    fn the_old_substring_rule(path: &str) -> bool {
        path.starts_with("artifacts/") || path.contains("/artifacts/")
    }

    #[test]
    fn qa_the_old_substring_rule_really_did_unlock_the_store() {
        // The exact path Dev B named. `overview.md` is the phase SPEC — the file the classifier
        // exists to protect — and the old rule classified it as the worker's own output.
        let escape = "phases/01-shell/artifacts/../../overview.md";
        assert!(
            the_old_substring_rule(escape),
            "the parent commit's rule must be shown to let this through, or there was no defect"
        );
        assert!(
            !is_artifact_path(escape),
            "the new segment rule must refuse it"
        );

        // More ways out through the same hole, all accepted by the old rule.
        for out in [
            "artifacts/../overview.md",
            "artifacts/../../../../etc/passwd",
            "phases/01-shell/artifacts/../../../decisions.md",
            "phases/01-shell/artifacts/../artifacts/../overview.md",
        ] {
            assert!(the_old_substring_rule(out), "old rule should accept {out:?}");
            assert!(!is_artifact_path(out), "new rule must refuse {out:?}");
        }

        // And the rules AGREE on every honest artifact path, so the fix is not a regression that
        // stopped classifying real worker output.
        for real in [
            "artifacts/summary.md",
            "artifacts/mocks/seat-meter.html",
            "phases/02-acceptance/artifacts/cargo-test-lib.txt",
            "phases/04-leases/artifacts/runs/4f0821/seat-meter.html",
            "phases/01-shell/artifacts/screenshots/aux-open.png",
        ] {
            assert!(the_old_substring_rule(real), "old rule on {real:?}");
            assert!(is_artifact_path(real), "new rule must still accept {real:?}");
        }
    }

    /// Ways past a segment guard that are NOT `../`. None of these may be classified as an
    /// artifact unless they are genuinely inside an `artifacts/` directory.
    #[test]
    fn qa_the_new_guard_holds_against_the_non_dotdot_bypasses() {
        for refused in [
            // Absolute and UNC: the leading empty segment is the rejection.
            "/artifacts/x.png",
            "//host/artifacts/x.png",
            "/phases/01-shell/artifacts/x.png",
            // Windows separators, including a drive-qualified name.
            "phases\\01-shell\\artifacts\\x.png",
            "phases/01-shell\\artifacts\\x.png",
            "C:\\artifacts\\x.png",
            "artifacts\\..\\overview.md",
            // Empty and current-dir segments.
            "artifacts//x.png",
            "artifacts/./x.png",
            "./artifacts/x.png",
            "artifacts/",
            "",
            // `artifacts` is the LAST segment, i.e. it is the file, not a directory above one.
            "artifacts",
            "phases/01-shell/artifacts",
            // Substrings are not segments.
            "artifactsx/summary.md",
            "my-artifacts/summary.md",
            "artifacts.bak/x.png",
            "ARTIFACTS/x.png",
            "Artifacts/x.png",
            // `..` hidden inside a longer name is still refused (deliberately strict).
            "artifacts/v1..2.png",
            "phases/01-shell/artifacts/runs/4f..21/x.png",
        ] {
            assert!(
                !is_artifact_path(refused),
                "{refused:?} must not be classified as an artifact path"
            );
        }
    }

    /// GAP (benign today, load-bearing tomorrow). The guard is a BYTE rule, so a percent-encoded
    /// or unicode-look-alike traversal is accepted as an ordinary, literal segment name.
    ///
    /// What a reviewer should conclude: this is safe only because nothing between the guard and
    /// the filesystem decodes the path — the host joins these bytes straight onto the plan dir,
    /// where `%2e%2e` is a directory literally called `%2e%2e` and `‥` (U+2025) is a file name.
    /// It stops being safe the moment an HTTP surface with URL-decoding is put in front of these
    /// paths, which is exactly what "the arena API" is. Pinned with the containment assertion, so
    /// the test says why it is safe rather than only that it is accepted.
    #[test]
    fn qa_documents_encoded_and_lookalike_traversals_stay_literal_segments() {
        // Accepted as names...
        assert!(is_safe_segment("%2e%2e"));
        assert!(is_safe_segment("%2E%2E%2F"));
        assert!(is_safe_segment("\u{2025}")); // ‥ TWO DOT LEADER
        assert!(is_safe_segment("\u{ff0e}\u{ff0e}")); // ．． FULLWIDTH FULL STOP x2
        assert!(is_artifact_path("phases/01-shell/artifacts/%2e%2e/%2e%2e/overview.md"));

        // ...and staying names is what makes that harmless: the built path is still INSIDE the
        // phase's artifacts directory, one literally-named subdirectory deeper.
        let built = phase_run_artifacts_rel("01-shell", "%2e%2e").unwrap();
        assert_eq!(built, "phases/01-shell/artifacts/runs/%2e%2e");
        assert!(built.starts_with("phases/01-shell/artifacts/runs/"));

        // Proof on a real filesystem: the directory created is a child, not an escape.
        let plan = crate::test_support::tmp_dir("qa-encoded-seg");
        let dir = ensure_phase_run_artifacts_dir(&plan, "01-shell", "%2e%2e").unwrap();
        assert!(dir.starts_with(plan.join("phases/01-shell/artifacts/runs")));
        assert!(dir.is_dir());
        assert!(
            !plan.parent().unwrap().join("overview.md").exists(),
            "nothing may have been created outside the plan dir"
        );
    }

    /// A `run_id` long enough to blow the filesystem's name limit must be an error, not a panic
    /// and not a half-made tree. (`is_safe_segment` imposes no length bound, so the refusal comes
    /// from the OS — which is fine, as long as it is reported.)
    #[test]
    fn qa_an_overlong_run_id_is_a_reported_error_not_a_panic() {
        let plan = crate::test_support::tmp_dir("qa-longname");
        let long = "r".repeat(5000);
        // The predicate accepts it: it is one segment with no `..`, no slash.
        assert!(is_safe_segment(&long));
        assert_eq!(
            phase_run_artifacts_rel("01-shell", &long),
            Some(format!("phases/01-shell/artifacts/runs/{long}"))
        );
        // The filesystem does not, and that is surfaced rather than panicked or swallowed.
        let err = ensure_phase_run_artifacts_dir(&plan, "01-shell", &long)
            .expect_err("a 5000-byte directory name cannot be created on this host");
        assert!(err.starts_with("create "), "{err}");
        // Trailing dots and spaces ARE legal names here, and must really work rather than being
        // accepted by the guard and then failing at the syscall.
        for odd in ["4f0821.", "4f0821 ", " ", "-", "_"] {
            assert!(is_safe_segment(odd), "{odd:?}");
            let dir = ensure_phase_run_artifacts_dir(&plan, "01-shell", odd).unwrap();
            assert!(dir.is_dir(), "{odd:?} was accepted but not created");
        }
    }

    /// GAP, and the sharpest one in this module. Nothing here canonicalises, so a SYMLINK planted
    /// inside the plan store redirects both the writer and the reader outside it.
    ///
    /// `ensure_phase_run_artifacts_dir` calls `create_dir_all`, which follows a symlinked parent,
    /// and `resolve_phase_artifacts_dir` tests `is_dir()`, which follows too. The worker is the
    /// process that writes the plan store, so the worker is the process that can plant the link —
    /// and `phase_template_values` calls the writer on every live prompt.
    ///
    /// What a reviewer should conclude: the segment guard confines the path STRING, not the path.
    /// Closing it needs a `canonicalize` + prefix check (or `O_NOFOLLOW`-equivalent) that this
    /// module does not have, and that is a decision for the author, not a QA edit. Pinned so it
    /// cannot be reported as closed without this test changing.
    #[test]
    fn qa_documents_a_symlinked_run_dir_escapes_the_plan_store() {
        let plan = crate::test_support::tmp_dir("qa-symlink-plan");
        let outside = crate::test_support::tmp_dir("qa-symlink-outside");
        std::fs::write(outside.join("stolen.txt"), b"not in the plan store").unwrap();

        // A worker-plantable link: `…/artifacts/runs` points out of the plan dir entirely.
        let runs = plan.join("phases/01-shell/artifacts/runs");
        std::fs::create_dir_all(runs.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, &runs).unwrap();

        // The WRITER follows it and creates a directory outside the plan store.
        let made = ensure_phase_run_artifacts_dir(&plan, "01-shell", "4f0821")
            .expect("create_dir_all follows the link rather than refusing");
        assert!(
            outside.join("4f0821").is_dir(),
            "a directory was created outside the plan store at {}",
            outside.join("4f0821").display()
        );
        // The returned path still LOOKS confined, which is what makes this invisible.
        assert!(made.starts_with(&plan));
        assert_eq!(made, plan.join("phases/01-shell/artifacts/runs/4f0821"));

        // And the READER resolves to it, so the review gate would read evidence from there.
        std::fs::write(outside.join("4f0821/evidence.png").as_path(), b"x").unwrap();
        let resolved = resolve_phase_artifacts_dir(&plan, "01-shell", Some("4f0821"))
            .expect("the symlinked run dir resolves");
        assert!(resolved.join("evidence.png").is_file());
        assert_eq!(
            resolved.canonicalize().unwrap(),
            outside.canonicalize().unwrap().join("4f0821"),
            "the resolved directory is physically outside the plan store"
        );
    }

    // ── Claim 5: back-compat, attacked at the mixed cases ─────────────────────────────────────

    /// A phase with BOTH layouts at once, and the same basename in three places.
    #[test]
    fn qa_a_phase_with_both_layouts_keeps_every_copy_addressable() {
        let plan = crate::test_support::tmp_dir("qa-mixed-layout");
        write(&plan, "phases/01-shell/artifacts/seat-meter.html", b"champion");
        write(
            &plan,
            "phases/01-shell/artifacts/runs/aaaa/seat-meter.html",
            b"run aaaa",
        );
        write(
            &plan,
            "phases/01-shell/artifacts/runs/bbbb/seat-meter.html",
            b"run bbbb",
        );

        // Three distinct directories, three distinct files, same basename.
        let champion = resolve_phase_artifacts_dir(&plan, "01-shell", None).unwrap();
        let a = resolve_phase_artifacts_dir(&plan, "01-shell", Some("aaaa")).unwrap();
        let b = resolve_phase_artifacts_dir(&plan, "01-shell", Some("bbbb")).unwrap();
        assert_ne!(a, b);
        assert_ne!(a, champion);
        assert_eq!(std::fs::read(champion.join("seat-meter.html")).unwrap(), b"champion");
        assert_eq!(std::fs::read(a.join("seat-meter.html")).unwrap(), b"run aaaa");
        assert_eq!(std::fs::read(b.join("seat-meter.html")).unwrap(), b"run bbbb");
        // The champion's directory is the PARENT of `runs/`, so listing it sees `runs` as an
        // entry. Pinned because the Plan tab descends `artifacts/` recursively: a reader of the
        // champion's directory must expect a `runs` child, not only files.
        assert!(champion.join("runs").is_dir());
    }

    /// A run id that is a perfectly valid segment but names no run falls back to the champion —
    /// including one that collides with the layout's own words.
    #[test]
    fn qa_an_unknown_but_valid_run_id_falls_back_to_the_champion() {
        let plan = crate::test_support::tmp_dir("qa-unknown-run");
        write(&plan, "phases/01-shell/artifacts/aux-open.png", b"champion");
        for unknown in ["never-ran", "runs", "artifacts", "phases", "01-shell", "4f0821"] {
            assert_eq!(
                resolve_phase_artifacts_dir(&plan, "01-shell", Some(unknown)),
                Some(plan.join("phases/01-shell/artifacts")),
                "run id {unknown:?} must fall back to the champion's directory"
            );
        }
        // A run id of `runs` would build `artifacts/runs/runs` — a child, never the `runs/`
        // directory itself, so it cannot shadow every other run at once.
        assert_eq!(
            phase_run_artifacts_rel("01-shell", "runs").unwrap(),
            "phases/01-shell/artifacts/runs/runs"
        );
        // And a phase with NEITHER layout is still None rather than a path that does not exist.
        assert_eq!(resolve_phase_artifacts_dir(&plan, "99-absent", Some("4f0821")), None);
    }

    /// GAP. An EMPTY run directory wins over the champion's, so the champion's evidence becomes
    /// invisible to any reader that passes a run id.
    ///
    /// `resolve_phase_artifacts_dir` prefers the run directory on `is_dir()` — existence, not
    /// content — and `ensure_phase_run_artifacts_dir` creates that directory unconditionally.
    /// Since `phase_template_values` now calls the creator for EVERY phase of EVERY run, with
    /// `run.plan.id` as the run id, every plan in the store acquires an empty
    /// `artifacts/runs/<plan_id>/` as soon as a phase prompt is rendered. A reader that then asks
    /// for that run id sees an artifact directory with nothing in it, while the phase's real
    /// artifacts sit one level up.
    ///
    /// What a reviewer should conclude: harmless TODAY and only today, because
    /// `resolve_phase_artifacts_dir` has no production caller (QA grepped: the only references
    /// outside this file are in this module's own tests). It becomes a live regression on every
    /// pre-existing plan the moment the arena wires the reader up. The fix is one condition —
    /// prefer the run directory only when it is non-empty — not a migration.
    #[test]
    fn qa_documents_an_empty_run_dir_hides_the_champions_artifacts() {
        let plan = crate::test_support::tmp_dir("qa-empty-run");
        write(&plan, "phases/01-shell/artifacts/aux-open.png", b"champion pixels");

        // Exactly what a live phase prompt does today, with the plan id as the run id.
        let created = ensure_phase_run_artifacts_dir(&plan, "01-shell", "plan-7fe2").unwrap();
        assert_eq!(std::fs::read_dir(&created).unwrap().count(), 0, "created empty");

        let resolved = resolve_phase_artifacts_dir(&plan, "01-shell", Some("plan-7fe2")).unwrap();
        assert_eq!(resolved, created, "the EMPTY run dir wins over the champion's");
        assert!(
            !resolved.join("aux-open.png").exists(),
            "the champion's only artifact is now unreachable through this reader"
        );
        // The file itself is fine — it is the reader that cannot see it.
        assert!(plan.join("phases/01-shell/artifacts/aux-open.png").is_file());
    }

    /// Creating a run directory twice is idempotent and destroys nothing already in it — the
    /// re-entered-phase case, which happens on every replan.
    #[test]
    fn qa_re_entering_a_phase_keeps_the_runs_own_evidence() {
        let plan = crate::test_support::tmp_dir("qa-reentrant");
        let first = ensure_phase_run_artifacts_dir(&plan, "01-shell", "4f0821").unwrap();
        std::fs::write(first.join("cargo-test-lib.txt"), b"708 passed").unwrap();
        let again = ensure_phase_run_artifacts_dir(&plan, "01-shell", "4f0821").unwrap();
        assert_eq!(first, again);
        assert_eq!(
            std::fs::read(again.join("cargo-test-lib.txt")).unwrap(),
            b"708 passed",
            "re-entering the phase wiped the run's own evidence"
        );
    }

    /// The builders are pure string arithmetic, so a plan path that is itself odd must not be
    /// able to turn a safe segment into an escape.
    #[test]
    fn qa_a_relative_or_trailing_slash_plan_path_still_confines_the_run_dir() {
        for root in ["/plan", "/plan/", "plan", "./plan", "/a/b/../b/plan"] {
            let dir = phase_run_artifacts_dir(Path::new(root), "01-shell", "4f0821").unwrap();
            let shown = dir.to_string_lossy().to_string();
            assert!(
                shown.ends_with("phases/01-shell/artifacts/runs/4f0821"),
                "{root:?} produced {shown:?}"
            );
            // Whatever the root was, the SUFFIX this module appended contributes no `..`.
            let suffix = &shown[shown.len() - "phases/01-shell/artifacts/runs/4f0821".len()..];
            assert!(!suffix.contains(".."), "{suffix:?}");
        }
    }
}
