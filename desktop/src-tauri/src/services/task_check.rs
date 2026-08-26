//! Deterministic five-check, failure classifier, and escalation cursor.
//! Pure: no `std::fs`, no process, no network.

use crate::providers::kloo_cli::KlooResult;
use crate::services::task_spec::TaskSpec;
use std::collections::HashMap;

pub const CHECK_EXIT: &str = "exit";
pub const CHECK_SCOPE: &str = "scope";
pub const CHECK_CHANGED: &str = "changed";
pub const CHECK_MUST_CONTAIN: &str = "must_contain";
pub const CHECK_VERIFY: &str = "verify";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckFail {
    pub check: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    pub passed: bool,
    pub failed: Vec<CheckFail>,
}

impl CheckReport {
    pub fn has_fail(&self, check: &str) -> bool {
        self.failed.iter().any(|f| f.check == check)
    }
}

/// Stable content fingerprint (FNV-1a 64 + length). Equality is all the
/// five-check needs; no extra crate.
pub fn digest_bytes(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}:{}", hash, bytes.len())
}

/// Caller-supplied disk snapshot for [`run_checks`]. Bundled so pre/post hash
/// maps cannot be transposed at a call site. Keys are exact `files[]` strings
/// (no slash-folding: this desktop is Linux-only and `\` is a legal filename).
#[derive(Debug, Clone, Default)]
pub struct CheckInputs {
    pub pre_spawn_dirty: Vec<String>,
    pub pre_hashes: HashMap<String, Option<String>>,
    pub post_hashes: HashMap<String, Option<String>>,
    pub post_contents: HashMap<String, String>,
}

/// Five checks. Caller supplies hashes/contents; this function does no I/O.
pub fn run_checks(
    spec: &TaskSpec,
    result: &KlooResult,
    exit_code: i32,
    inputs: &CheckInputs,
) -> CheckReport {
    let mut failed = Vec::new();

    if exit_code != 0 || !result.success {
        failed.push(CheckFail {
            check: CHECK_EXIT.into(),
            reason: format!(
                "exit_code={} success={}",
                exit_code, result.success
            ),
        });
    }

    let dirty: std::collections::HashSet<&str> = inputs
        .pre_spawn_dirty
        .iter()
        .map(String::as_str)
        .collect();
    let allowed: std::collections::HashSet<&str> =
        spec.files.iter().map(String::as_str).collect();
    let delta: Vec<&str> = result
        .files_changed
        .paths
        .iter()
        .map(String::as_str)
        .filter(|p| !dirty.contains(p))
        .collect();
    for path in &delta {
        if !allowed.contains(path) {
            failed.push(CheckFail {
                check: CHECK_SCOPE.into(),
                reason: format!("delta path '{}' is not in files[]", path),
            });
        }
    }
    if result.off_scope_edits.unwrap_or(0) != 0 {
        failed.push(CheckFail {
            check: CHECK_SCOPE.into(),
            reason: format!(
                "off_scope_edits={}",
                result.off_scope_edits.unwrap_or(0)
            ),
        });
    }

    let mut content_changed = false;
    for file in &spec.files {
        let pre = inputs.pre_hashes.get(file).cloned().unwrap_or(None);
        let post = inputs.post_hashes.get(file).cloned().unwrap_or(None);
        if pre != post {
            content_changed = true;
            break;
        }
    }
    if !content_changed {
        if let Some(new_paths) = &spec.new {
            for file in new_paths {
                let pre = inputs.pre_hashes.get(file).cloned().unwrap_or(None);
                let post = inputs.post_hashes.get(file).cloned().unwrap_or(None);
                if pre.is_none() && post.is_some() {
                    content_changed = true;
                    break;
                }
            }
        }
    }
    if !content_changed {
        failed.push(CheckFail {
            check: CHECK_CHANGED.into(),
            reason: "no files[] content hash changed and no new: path was created".into(),
        });
    }

    for needle in &spec.must_contain {
        let found = spec.files.iter().any(|file| {
            inputs
                .post_contents
                .get(file)
                .map(|c| c.contains(needle.as_str()))
                .unwrap_or(false)
        });
        if !found {
            failed.push(CheckFail {
                check: CHECK_MUST_CONTAIN.into(),
                reason: format!("must_contain {:?} not found in files[]", needle),
            });
        }
    }

    let verify_ok = result.verify.as_ref().map(|v| v.passed) == Some(true);
    let postchecks_ok = result
        .postchecks
        .as_ref()
        .map(|hooks| hooks.iter().all(|h| h.passed))
        .unwrap_or(true);
    if !verify_ok || !postchecks_ok {
        failed.push(CheckFail {
            check: CHECK_VERIFY.into(),
            reason: format!(
                "verify.passed={:?} postchecks_ok={}",
                result.verify.as_ref().map(|v| v.passed),
                postchecks_ok
            ),
        });
    }

    CheckReport {
        passed: failed.is_empty(),
        failed,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    Shape,
    Model,
    Infra,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnFailure {
    MissingBinary,
    /// `check_verify` / cwd jail rejected the request. Shape, not infra.
    Policy(String),
    Other(String),
}

const SHAPE_CODES: &[&str] = &["off_scope_edit", "context_too_small", "unverified"];
const MODEL_CODES: &[&str] = &[
    "verify_failed",
    "postcheck_failed",
    "repetition_halt",
    "edit_failed",
    "budget_exceeded",
    "tool_call_invalid",
    "tool_error",
    "answered",
    "json_invalid",
];
const INFRA_CODES: &[&str] = &[
    "model_error",
    "interrupted",
    "internal_error",
    "config_error",
    "precheck_failed",
];

fn hook_exit_127(result: &KlooResult) -> bool {
    if result.verify.as_ref().and_then(|v| v.exit_code) == Some(127) {
        return true;
    }
    result
        .postchecks
        .as_ref()
        .map(|hooks| hooks.iter().any(|h| h.exit_code == Some(127)))
        .unwrap_or(false)
}

/// Classify a failed attempt. Caller short-circuits when `report.passed`.
pub fn classify(
    report: Option<&CheckReport>,
    result: Option<&KlooResult>,
    spawn: Option<&SpawnFailure>,
    process_exit: Option<i32>,
    preflight: Option<&str>,
) -> FailureClass {
    if preflight.is_some() {
        return FailureClass::Shape;
    }
    if matches!(spawn, Some(SpawnFailure::MissingBinary)) {
        return FailureClass::Infra;
    }
    if matches!(spawn, Some(SpawnFailure::Policy(_))) {
        return FailureClass::Shape;
    }

    let code = result.and_then(|r| r.failure_code.as_deref());
    let verify_127 = result.map(hook_exit_127).unwrap_or(false);

    // Process-level 127 (missing kloo / shell 127) is Infra, not Shape.
    if process_exit == Some(127) && !verify_127 {
        return FailureClass::Infra;
    }

    if verify_127 {
        return FailureClass::Shape;
    }
    if let Some(c) = code {
        if SHAPE_CODES.contains(&c) {
            return FailureClass::Shape;
        }
    }
    if report.map(|r| r.has_fail(CHECK_SCOPE)).unwrap_or(false) {
        return FailureClass::Shape;
    }

    if let Some(c) = code {
        if MODEL_CODES.contains(&c) {
            return FailureClass::Model;
        }
        if INFRA_CODES.contains(&c) {
            return FailureClass::Infra;
        }
        // Unrecognised failure_code wins over J1 check 3/4. A mystery must
        // not climb the ladder (would burn opus).
        return FailureClass::Infra;
    }
    if report
        .map(|r| r.has_fail(CHECK_CHANGED) || r.has_fail(CHECK_MUST_CONTAIN))
        .unwrap_or(false)
    {
        return FailureClass::Model;
    }
    if report.map(|r| r.has_fail(CHECK_VERIFY)).unwrap_or(false) {
        return FailureClass::Model;
    }
    if matches!(spawn, Some(SpawnFailure::Other(_))) {
        return FailureClass::Infra;
    }
    FailureClass::Infra
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub label: String,
    pub model: String,
    pub class: Option<FailureClass>,
    pub infra_retry: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Run {
        model: String,
        label: String,
        attempt_index: usize,
    },
    Exhausted,
    StopShape,
    RetryInfra {
        model: String,
        label: String,
    },
}

#[derive(Debug, Clone)]
pub struct LadderSlot {
    pub model: &'static str,
    pub label: &'static str,
    pub max_model_attempts: u32,
}

#[derive(Debug, Clone)]
pub struct Ladder {
    pub slots: Vec<LadderSlot>,
}

impl Default for Ladder {
    fn default() -> Self {
        Self {
            slots: vec![
                LadderSlot {
                    model: "qwen/qwen3-coder",
                    label: "qwen3-coder",
                    max_model_attempts: 2,
                },
                LadderSlot {
                    model: "anthropic/claude-sonnet-5",
                    label: "claude",
                    max_model_attempts: 1,
                },
                LadderSlot {
                    model: "anthropic/claude-opus-5",
                    label: "opus",
                    max_model_attempts: 1,
                },
            ],
        }
    }
}

impl Ladder {
    pub fn next_attempt(&self, history: &[Attempt], class: Option<FailureClass>) -> Step {
        if class == Some(FailureClass::Shape) {
            return Step::StopShape;
        }
        if history.is_empty() {
            let Some(slot) = self.slots.first() else {
                return Step::Exhausted;
            };
            return Step::Run {
                model: slot.model.to_string(),
                label: slot.label.to_string(),
                attempt_index: 1,
            };
        }

        if class == Some(FailureClass::Infra) {
            let last = history.last().unwrap();
            let already_retried = history
                .iter()
                .any(|a| a.label == last.label && a.infra_retry);
            if already_retried {
                return Step::Exhausted;
            }
            return Step::RetryInfra {
                model: last.model.clone(),
                label: last.label.clone(),
            };
        }

        // Model (or empty class after some history): next unused model slot.
        for slot in &self.slots {
            let used = history
                .iter()
                .filter(|a| a.label == slot.label && !a.infra_retry)
                .count() as u32;
            if used < slot.max_model_attempts {
                return Step::Run {
                    model: slot.model.to_string(),
                    label: slot.label.to_string(),
                    attempt_index: (used + 1) as usize,
                };
            }
        }
        Step::Exhausted
    }
}

pub fn next_attempt(history: &[Attempt], class: Option<FailureClass>) -> Step {
    Ladder::default().next_attempt(history, class)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::kloo_cli::{FilesChanged, HookResult, VerifyResult};

    fn spec_add() -> TaskSpec {
        TaskSpec {
            id: "01-add".into(),
            files: vec!["src/add.rs".into()],
            verify: "cargo test add -- --exact".into(),
            must_contain: vec!["pub fn add".into()],
            depends_on: vec![],
            ctx: Some(32_768),
            mock: None,
            new: None,
            cwd: None,
        }
    }

    fn ok_result(paths: &[&str]) -> KlooResult {
        KlooResult {
            success: true,
            failure_code: None,
            files_changed: FilesChanged {
                count: paths.len() as u64,
                paths: paths.iter().map(|p| (*p).to_string()).collect(),
            },
            off_scope_edits: Some(0),
            rail_fires: None,
            postchecks: None,
            verify: Some(VerifyResult {
                command: "cargo test add -- --exact".into(),
                passed: true,
                exit_code: Some(0),
            }),
            failure_detail: None,
            transcript_tail: None,
        }
    }

    fn hashes(pairs: &[(&str, Option<&str>)]) -> HashMap<String, Option<String>> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.map(|s| s.to_string())))
            .collect()
    }

    fn contents(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn ci(
        dirty: &[&str],
        pre: HashMap<String, Option<String>>,
        post: HashMap<String, Option<String>>,
        body: HashMap<String, String>,
    ) -> CheckInputs {
        CheckInputs {
            pre_spawn_dirty: dirty.iter().map(|s| (*s).to_string()).collect(),
            pre_hashes: pre,
            post_hashes: post,
            post_contents: body,
        }
    }

    fn green_inputs() -> (
        KlooResult,
        HashMap<String, Option<String>>,
        HashMap<String, Option<String>>,
        HashMap<String, String>,
    ) {
        (
            ok_result(&["src/add.rs"]),
            hashes(&[("src/add.rs", Some("pre"))]),
            hashes(&[("src/add.rs", Some("post"))]),
            contents(&[("src/add.rs", "pub fn add(a: i32, b: i32) -> i32 { a + b }")]),
        )
    }

    #[test]
    fn all_green_passes() {
        let spec = spec_add();
        let (result, pre, post, body) = green_inputs();
        let report = run_checks(&spec, &result, 0, &ci(&[], pre, post, body));
        assert!(report.passed, "{:?}", report.failed);
    }

    #[test]
    fn exit_zero_but_success_false_fails_exit() {
        let spec = spec_add();
        let (mut result, pre, post, body) = green_inputs();
        result.success = false;
        let report = run_checks(&spec, &result, 0, &ci(&[], pre, post, body));
        assert!(report.has_fail(CHECK_EXIT));
        assert!(!report.passed);
    }

    #[test]
    fn delta_outside_files_fails_scope() {
        let spec = spec_add();
        let (mut result, pre, post, body) = green_inputs();
        result.files_changed.paths = vec!["src/add.rs".into(), "src/secret.rs".into()];
        result.files_changed.count = 2;
        let report = run_checks(&spec, &result, 0, &ci(&[], pre, post, body));
        assert!(report.has_fail(CHECK_SCOPE));
    }

    #[test]
    fn dirty_pre_spawn_path_not_in_delta_does_not_fail_scope() {
        let spec = spec_add();
        let (mut result, pre, post, body) = green_inputs();
        result.files_changed.paths = vec!["src/add.rs".into(), "target/foo".into()];
        result.files_changed.count = 2;
        let report = run_checks(&spec, &result, 0, &ci(&["target/foo"], pre, post, body));
        assert!(!report.has_fail(CHECK_SCOPE), "{:?}", report.failed);
        assert!(report.passed);
    }

    #[test]
    fn off_scope_edits_fails_scope_even_if_delta_clean() {
        let spec = spec_add();
        let (mut result, pre, post, body) = green_inputs();
        result.off_scope_edits = Some(1);
        let report = run_checks(&spec, &result, 0, &ci(&[], pre, post, body));
        assert!(report.has_fail(CHECK_SCOPE));
    }

    #[test]
    fn dirty_and_further_modified_passes_changed() {
        let spec = spec_add();
        let (result, pre, post, body) = green_inputs();
        let report = run_checks(&spec, &result, 0, &ci(&["src/add.rs"], pre, post, body));
        assert!(!report.has_fail(CHECK_CHANGED));
        assert!(report.passed);
    }

    #[test]
    fn dirty_and_untouched_fails_changed() {
        let spec = spec_add();
        let (mut result, _pre, _, body) = green_inputs();
        result.files_changed.paths = vec!["src/add.rs".into()];
        let same = hashes(&[("src/add.rs", Some("same"))]);
        let report = run_checks(&spec, &result, 0, &ci(&["src/add.rs"], same.clone(), same, body));
        assert!(report.has_fail(CHECK_CHANGED));
    }

    #[test]
    fn new_path_none_before_exists_after_passes_changed() {
        let mut spec = spec_add();
        spec.new = Some(vec!["src/add.rs".into()]);
        let (result, _, post, body) = green_inputs();
        let pre = hashes(&[("src/add.rs", None)]);
        let report = run_checks(&spec, &result, 0, &ci(&[], pre, post, body));
        assert!(!report.has_fail(CHECK_CHANGED));
        assert!(report.passed);
    }

    #[test]
    fn empty_path_delta_is_not_by_itself_a_check3_fail() {
        let spec = spec_add();
        let (mut result, pre, post, body) = green_inputs();
        result.files_changed.paths.clear();
        result.files_changed.count = 0;
        let report = run_checks(&spec, &result, 0, &ci(&[], pre, post, body));
        assert!(!report.has_fail(CHECK_CHANGED));
        assert!(report.passed, "{:?}", report.failed);
    }

    #[test]
    fn missing_needle_fails_must_contain() {
        let spec = spec_add();
        let (result, pre, post, _) = green_inputs();
        let body = contents(&[("src/add.rs", "fn nope() {}")]);
        let report = run_checks(&spec, &result, 0, &ci(&[], pre, post, body));
        assert!(report.has_fail(CHECK_MUST_CONTAIN));
    }

    #[test]
    fn verify_passed_false_fails_verify() {
        let spec = spec_add();
        let (mut result, pre, post, body) = green_inputs();
        result.verify.as_mut().unwrap().passed = false;
        let report = run_checks(&spec, &result, 0, &ci(&[], pre, post, body));
        assert!(report.has_fail(CHECK_VERIFY));
    }

    #[test]
    fn postcheck_failed_fails_verify() {
        let spec = spec_add();
        let (mut result, pre, post, body) = green_inputs();
        result.postchecks = Some(vec![HookResult {
            command: "lint".into(),
            passed: false,
            exit_code: Some(1),
        }]);
        let report = run_checks(&spec, &result, 0, &ci(&[], pre, post, body));
        assert!(report.has_fail(CHECK_VERIFY));
    }

    #[test]
    fn postchecks_all_passed_keeps_verify_green() {
        let spec = spec_add();
        let (mut result, pre, post, body) = green_inputs();
        result.postchecks = Some(vec![
            HookResult {
                command: "a".into(),
                passed: true,
                exit_code: Some(0),
            },
            HookResult {
                command: "b".into(),
                passed: true,
                exit_code: Some(0),
            },
        ]);
        let report = run_checks(&spec, &result, 0, &ci(&[], pre, post, body));
        assert!(!report.has_fail(CHECK_VERIFY));
        assert!(report.passed);
    }

    #[test]
    fn empty_must_contain_vacuous_pass() {
        let mut spec = spec_add();
        spec.must_contain.clear();
        let (result, pre, post, body) = green_inputs();
        let report = run_checks(&spec, &result, 0, &ci(&[], pre, post, body));
        assert!(report.passed);
    }

    fn model_attempt(label: &str, model: &str) -> Attempt {
        Attempt {
            label: label.into(),
            model: model.into(),
            class: Some(FailureClass::Model),
            infra_retry: false,
        }
    }

    #[test]
    fn empty_ladder_is_exhausted() {
        let ladder = Ladder { slots: vec![] };
        assert_eq!(ladder.next_attempt(&[], None), Step::Exhausted);
    }

    #[test]
    fn first_call_is_qwen_attempt_1() {
        match next_attempt(&[], None) {
            Step::Run {
                model,
                label,
                attempt_index,
            } => {
                assert_eq!(model, "qwen/qwen3-coder");
                assert_eq!(label, "qwen3-coder");
                assert_eq!(attempt_index, 1);
            }
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn one_model_fail_is_qwen_attempt_2() {
        let hist = vec![model_attempt("qwen3-coder", "qwen/qwen3-coder")];
        match next_attempt(&hist, Some(FailureClass::Model)) {
            Step::Run { label, attempt_index, .. } => {
                assert_eq!(label, "qwen3-coder");
                assert_eq!(attempt_index, 2);
            }
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn two_model_fails_is_claude() {
        let hist = vec![
            model_attempt("qwen3-coder", "qwen/qwen3-coder"),
            model_attempt("qwen3-coder", "qwen/qwen3-coder"),
        ];
        match next_attempt(&hist, Some(FailureClass::Model)) {
            Step::Run { model, label, .. } => {
                assert_eq!(model, "anthropic/claude-sonnet-5");
                assert_eq!(label, "claude");
            }
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn three_model_fails_is_opus() {
        let hist = vec![
            model_attempt("qwen3-coder", "qwen/qwen3-coder"),
            model_attempt("qwen3-coder", "qwen/qwen3-coder"),
            model_attempt("claude", "anthropic/claude-sonnet-5"),
        ];
        match next_attempt(&hist, Some(FailureClass::Model)) {
            Step::Run { model, label, .. } => {
                assert_eq!(model, "anthropic/claude-opus-5");
                assert_eq!(label, "opus");
            }
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn four_model_fails_is_exhausted() {
        let hist = vec![
            model_attempt("qwen3-coder", "qwen/qwen3-coder"),
            model_attempt("qwen3-coder", "qwen/qwen3-coder"),
            model_attempt("claude", "anthropic/claude-sonnet-5"),
            model_attempt("opus", "anthropic/claude-opus-5"),
        ];
        assert_eq!(
            next_attempt(&hist, Some(FailureClass::Model)),
            Step::Exhausted
        );
    }

    #[test]
    fn shape_on_attempt_1_is_stop_shape() {
        let hist = vec![model_attempt("qwen3-coder", "qwen/qwen3-coder")];
        assert_eq!(
            next_attempt(&hist, Some(FailureClass::Shape)),
            Step::StopShape
        );
    }

    #[test]
    fn infra_retries_same_slot_then_exhausts() {
        let hist = vec![Attempt {
            label: "qwen3-coder".into(),
            model: "qwen/qwen3-coder".into(),
            class: Some(FailureClass::Infra),
            infra_retry: false,
        }];
        match next_attempt(&hist, Some(FailureClass::Infra)) {
            Step::RetryInfra { model, label } => {
                assert_eq!(model, "qwen/qwen3-coder");
                assert_eq!(label, "qwen3-coder");
            }
            other => panic!("{:?}", other),
        }
        let mut hist2 = hist;
        hist2.push(Attempt {
            label: "qwen3-coder".into(),
            model: "qwen/qwen3-coder".into(),
            class: Some(FailureClass::Infra),
            infra_retry: true,
        });
        assert_eq!(
            next_attempt(&hist2, Some(FailureClass::Infra)),
            Step::Exhausted
        );
    }

    fn class_for(
        report: Option<&CheckReport>,
        result: Option<&KlooResult>,
        spawn: Option<&SpawnFailure>,
        process_exit: Option<i32>,
        preflight: Option<&str>,
    ) -> FailureClass {
        classify(report, result, spawn, process_exit, preflight)
    }

    #[test]
    fn classify_missing_file_preflight_is_shape() {
        assert_eq!(
            class_for(None, None, None, None, Some("shape: missing task.yml")),
            FailureClass::Shape
        );
    }

    #[test]
    fn classify_off_scope_edit_is_shape() {
        let mut r = ok_result(&[]);
        r.success = false;
        r.failure_code = Some("off_scope_edit".into());
        r.off_scope_edits = Some(1);
        assert_eq!(
            class_for(None, Some(&r), None, Some(10), None),
            FailureClass::Shape
        );
    }

    #[test]
    fn classify_context_too_small_is_shape() {
        let mut r = ok_result(&[]);
        r.success = false;
        r.failure_code = Some("context_too_small".into());
        assert_eq!(
            class_for(None, Some(&r), None, Some(10), None),
            FailureClass::Shape
        );
    }

    #[test]
    fn classify_verify_exit_127_is_shape() {
        let mut r = ok_result(&[]);
        r.success = false;
        r.failure_code = Some("verify_failed".into());
        r.verify = Some(VerifyResult {
            command: "missing-bin".into(),
            passed: false,
            exit_code: Some(127),
        });
        assert_eq!(
            class_for(None, Some(&r), None, Some(10), None),
            FailureClass::Shape
        );
    }

    #[test]
    fn process_exit_127_is_infra_not_shape() {
        assert_eq!(
            class_for(None, None, None, Some(127), None),
            FailureClass::Infra
        );
    }

    #[test]
    fn classify_verify_failed_exit_1_is_model() {
        let mut r = ok_result(&[]);
        r.success = false;
        r.failure_code = Some("verify_failed".into());
        r.verify = Some(VerifyResult {
            command: "cargo test".into(),
            passed: false,
            exit_code: Some(1),
        });
        assert_eq!(
            class_for(None, Some(&r), None, Some(10), None),
            FailureClass::Model
        );
    }

    #[test]
    fn classify_postcheck_failed_exit_1_is_model() {
        let mut r = ok_result(&[]);
        r.success = false;
        r.failure_code = Some("postcheck_failed".into());
        r.postchecks = Some(vec![HookResult {
            command: "lint".into(),
            passed: false,
            exit_code: Some(1),
        }]);
        assert_eq!(
            class_for(None, Some(&r), None, Some(10), None),
            FailureClass::Model
        );
    }

    #[test]
    fn classify_json_invalid_is_model() {
        let mut r = ok_result(&[]);
        r.success = false;
        r.failure_code = Some("json_invalid".into());
        assert_eq!(
            class_for(None, Some(&r), None, Some(10), None),
            FailureClass::Model
        );
    }

    #[test]
    fn classify_must_contain_fail_is_model() {
        let spec = spec_add();
        let (result, pre, post, _) = green_inputs();
        let body = contents(&[("src/add.rs", "nope")]);
        let report = run_checks(&spec, &result, 0, &ci(&[], pre, post, body));
        assert!(report.has_fail(CHECK_MUST_CONTAIN));
        assert_eq!(
            class_for(Some(&report), Some(&result), None, Some(0), None),
            FailureClass::Model
        );
    }

    #[test]
    fn classify_model_error_is_infra() {
        let mut r = ok_result(&[]);
        r.success = false;
        r.failure_code = Some("model_error".into());
        assert_eq!(
            class_for(None, Some(&r), None, Some(10), None),
            FailureClass::Infra
        );
    }

    #[test]
    fn classify_config_error_is_infra() {
        let mut r = ok_result(&[]);
        r.success = false;
        r.failure_code = Some("config_error".into());
        assert_eq!(
            class_for(None, Some(&r), None, Some(10), None),
            FailureClass::Infra
        );
    }

    #[test]
    fn classify_unverified_is_shape() {
        let mut r = ok_result(&[]);
        r.success = false;
        r.failure_code = Some("unverified".into());
        assert_eq!(
            class_for(None, Some(&r), None, Some(10), None),
            FailureClass::Shape
        );
    }

    #[test]
    fn classify_precheck_failed_is_infra() {
        let mut r = ok_result(&[]);
        r.success = false;
        r.failure_code = Some("precheck_failed".into());
        assert_eq!(
            class_for(None, Some(&r), None, Some(10), None),
            FailureClass::Infra
        );
    }

    #[test]
    fn classify_unknown_code_is_infra() {
        let mut r = ok_result(&[]);
        r.success = false;
        r.failure_code = Some("no_such_code".into());
        assert_eq!(
            class_for(None, Some(&r), None, Some(10), None),
            FailureClass::Infra
        );
    }

    #[test]
    fn classify_unknown_code_wins_over_j1_check_fail() {
        let mut r = ok_result(&[]);
        r.success = false;
        r.failure_code = Some("no_such_code".into());
        let report = CheckReport {
            passed: false,
            failed: vec![CheckFail {
                check: CHECK_MUST_CONTAIN.into(),
                reason: "missing needle".into(),
            }],
        };
        assert_eq!(
            class_for(Some(&report), Some(&r), None, Some(10), None),
            FailureClass::Infra,
            "unrecognised failure_code must not escalate even when check 3/4 failed"
        );
    }

    #[test]
    fn classify_remaining_table_rows() {
        let rows = [
            ("repetition_halt", FailureClass::Model),
            ("edit_failed", FailureClass::Model),
            ("budget_exceeded", FailureClass::Model),
            ("tool_call_invalid", FailureClass::Model),
            ("tool_error", FailureClass::Model),
            ("answered", FailureClass::Model),
            ("interrupted", FailureClass::Infra),
            ("internal_error", FailureClass::Infra),
        ];
        for (code, want) in rows {
            let mut r = ok_result(&[]);
            r.success = false;
            r.failure_code = Some(code.into());
            assert_eq!(
                class_for(None, Some(&r), None, Some(10), None),
                want,
                "{code}"
            );
        }
    }

    #[test]
    fn classify_kloo_binary_missing_is_infra() {
        assert_eq!(
            class_for(
                None,
                None,
                Some(&SpawnFailure::MissingBinary),
                None,
                None
            ),
            FailureClass::Infra
        );
    }

    #[test]
    fn classify_policy_spawn_is_shape() {
        assert_eq!(
            class_for(
                None,
                None,
                Some(&SpawnFailure::Policy(
                    "shape: verify_not_allowlisted: denied runner".into()
                )),
                Some(1),
                None
            ),
            FailureClass::Shape
        );
    }

    #[test]
    fn classify_scope_fail_is_shape() {
        let spec = spec_add();
        let (mut result, pre, post, body) = green_inputs();
        result.files_changed.paths = vec!["src/secret.rs".into()];
        let report = run_checks(&spec, &result, 0, &ci(&[], pre, post, body));
        assert!(report.has_fail(CHECK_SCOPE));
        assert_eq!(
            class_for(Some(&report), Some(&result), None, Some(0), None),
            FailureClass::Shape
        );
    }
}

#[cfg(test)]
mod acceptance2 {
    use super::*;
    use crate::providers::kloo_cli::{
        build_config, default_profile_path, parse_result_from_stdout, parse_result_json,
        KlooRunRequest,
    };
    use crate::services::task_spec::parse_task_yaml;

    const HAPPY_YML: &str = r#"
id: 01-add
files: [src/add.rs]
verify: "cargo test add -- --exact"
must_contain: ["pub fn add"]
depends_on: []
ctx: 32768
"#;

    fn happy_spec() -> TaskSpec {
        parse_task_yaml(HAPPY_YML).unwrap()
    }

    fn hashes(pre: &str, post: &str) -> (HashMap<String, Option<String>>, HashMap<String, Option<String>>) {
        let mut a = HashMap::new();
        a.insert("src/add.rs".into(), Some(pre.into()));
        let mut b = HashMap::new();
        b.insert("src/add.rs".into(), Some(post.into()));
        (a, b)
    }

    fn ci(
        dirty: &[&str],
        pre: HashMap<String, Option<String>>,
        post: HashMap<String, Option<String>>,
        body: HashMap<String, String>,
    ) -> CheckInputs {
        CheckInputs {
            pre_spawn_dirty: dirty.iter().map(|s| (*s).to_string()).collect(),
            pre_hashes: pre,
            post_hashes: post,
            post_contents: body,
        }
    }

    fn body() -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert(
            "src/add.rs".into(),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }".into(),
        );
        m
    }

    #[test]
    fn acceptance2_happy() {
        let spec = happy_spec();
        // kloo --benchmark emits one prefix line (not a pretty-printed object).
        let stdout = concat!(
            "noise\n",
            r#"KLOO_RESULT_JSON {"success":true,"files_changed":{"count":1,"paths":["src/add.rs"]},"off_scope_edits":0,"verify":{"command":"cargo test add -- --exact","passed":true,"exit_code":0}}"#,
            "\n",
        );
        let result = parse_result_from_stdout(stdout).unwrap();
        let (pre, post) = hashes("aaa", "bbb");
        let report = run_checks(&spec, &result, 0, &ci(&[], pre, post, body()));
        assert!(report.passed, "{:?}", report.failed);
    }

    #[test]
    fn acceptance2_scope_fail() {
        let spec = happy_spec();
        let json = r#"{
            "success": true,
            "files_changed": {"count": 2, "paths": ["src/add.rs", "src/secret.rs"]},
            "off_scope_edits": 0,
            "verify": {"command": "cargo test add -- --exact", "passed": true, "exit_code": 0}
        }"#;
        let result = parse_result_json(json.as_bytes()).unwrap();
        let (pre, post) = hashes("aaa", "bbb");
        let report = run_checks(&spec, &result, 0, &ci(&[], pre, post, body()));
        assert!(!report.passed);
        assert!(report.has_fail(CHECK_SCOPE));
        assert_eq!(
            classify(Some(&report), Some(&result), None, Some(0), None),
            FailureClass::Shape
        );
    }

    #[test]
    fn acceptance2_verify_fail_advances_cursor() {
        let spec = happy_spec();
        let json = r#"{
            "success": false,
            "failure_code": "verify_failed",
            "files_changed": {"count": 1, "paths": ["src/add.rs"]},
            "verify": {"command": "cargo test add -- --exact", "passed": false, "exit_code": 1}
        }"#;
        let result = parse_result_json(json.as_bytes()).unwrap();
        let (pre, post) = hashes("aaa", "bbb");
        let report = run_checks(&spec, &result, 10, &ci(&[], pre, post, body()));
        assert!(!report.passed);
        let class = classify(Some(&report), Some(&result), None, Some(10), None);
        assert_eq!(class, FailureClass::Model);
        let hist = [Attempt {
            label: "qwen3-coder".into(),
            model: "qwen/qwen3-coder".into(),
            class: Some(class),
            infra_retry: false,
        }];
        match next_attempt(&hist, Some(class)) {
            Step::Run { label, attempt_index, .. } => {
                assert_eq!(label, "qwen3-coder");
                assert_eq!(attempt_index, 2);
            }
            other => panic!("expected next qwen slot, got {:?}", other),
        }
    }

    #[test]
    fn acceptance2_unrunnable_verify_is_stop_shape() {
        let spec = happy_spec();
        let json = r#"{
            "success": false,
            "failure_code": "verify_failed",
            "files_changed": {"count": 0, "paths": []},
            "verify": {"command": "cargo test add -- --exact", "passed": false, "exit_code": 127}
        }"#;
        let result = parse_result_json(json.as_bytes()).unwrap();
        let (pre, post) = hashes("aaa", "aaa");
        let report = run_checks(&spec, &result, 10, &ci(&[], pre, post, body()));
        assert!(!report.passed);
        let class = classify(Some(&report), Some(&result), None, Some(10), None);
        assert_eq!(class, FailureClass::Shape);
        assert_eq!(
            next_attempt(&[], Some(class)),
            Step::StopShape
        );
    }

    #[test]
    fn acceptance2_invocation_matches_d1() {
        let spec = happy_spec();
        let req = KlooRunRequest {
            workspace: "/tmp/ws".into(),
            prompt: "implement add()".into(),
            files: spec.files.clone(),
            verify: spec.verify.clone(),
            cwd: spec.cwd.clone(),
            model: "qwen/qwen3-coder".into(),
            ctx: spec.ctx,
            profile: None,
            provider: None,
            cli_path: None,
            status_file: None,
        };
        let cfg = build_config(&req).unwrap();
        assert!(cfg.args.iter().any(|a| a == "--benchmark"));
        assert!(cfg.args.windows(2).any(|w| {
            w[0] == "--verify"
                && w[1]
                    == crate::services::verify_policy::quote_argv_for_shell(
                        &crate::services::verify_policy::check_verify(&spec.verify).unwrap(),
                    )
        }));
        assert!(cfg.args.windows(2).any(|w| w[0] == "--allow" && w[1] == "src/add.rs"));
        assert!(cfg.args.windows(2).any(|w| w[0] == "--profile" && w[1] == default_profile_path()));
        assert!(cfg.args.windows(2).any(|w| w[0] == "--provider" && w[1] == "openrouter"));
        assert!(cfg.args.windows(2).any(|w| w[0] == "--model" && w[1] == "qwen/qwen3-coder"));
        assert!(cfg.args.windows(2).any(|w| w[0] == "--ctx" && w[1] == "32768"));
        assert!(cfg.args.windows(2).any(|w| w[0] == "--stop-on" && w[1] == "off-scope-edit"));
        assert!(cfg.args.iter().any(|a| a == "--no-mcp"));
        assert!(!cfg.args.iter().any(|a| a == "--status-file"));
        assert!(!cfg.args.iter().any(|a| a == "--postcheck"));
        assert_eq!(
            cfg.args[cfg.args.len() - 2..],
            ["--".to_string(), "implement add()".to_string()]
        );
    }
}
