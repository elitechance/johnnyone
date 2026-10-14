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
