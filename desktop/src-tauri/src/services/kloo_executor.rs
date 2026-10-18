//! The one real [`PhaseExecutor`]: spawn `kloo` as a child process and let it run the phase.
//!
//! [`crate::services::workflows::PhaseExecutor`] is the arena's seam between "we have a prompt" and
//! "an agent ran it". Everything else that implements it is a recording double in a test. This is the
//! implementation that actually starts a process, and it exists so that the seam has exactly one
//! production-side answer.
//!
//! # The prompt is an argument, never a shell string
//!
//! A composed workflow prompt carries newlines, double quotes, backslashes, `$`-signs and backticks.
//! Handing that to `sh -c` would let the shell eat all of it, and worse, would let a prompt run
//! commands. So the prompt goes into [`Command::arg`] as ONE element of `argv` and never through a
//! formatter into a command line. [`KlooExecutor::argv`] is the pure half of that — it builds the
//! argument vector and is what the tests assert against — and [`Command::spawn`] is the effecting
//! half that consumes it. Nothing between the two can split or re-quote an element, because there is
//! no shell in between at all.
//!
//! # Exit status is not this trait's business
//!
//! The trait's doc comment is explicit: `Ok(())` means an execution *ran*; whether it *passed* its
//! gate is [`crate::services::workflows::record_workflow_run_gate`]'s job. So a child that ran to
//! completion and exited 1 (or 7, or was killed by a signal) is still `Ok(())` here — that is an
//! execution that failed its gate, which is a scoreable result. Only a child that never started is
//! `Err`, because an execution that did not happen must not look like one that failed.

use crate::services::workflows::PhaseExecutor;
use std::process::Command;

/// Drives one phase hop by spawning the `kloo` binary in a workspace.
///
/// Three fields, all of them things the host knows and this module must not guess: which binary to
/// run, which directory to run it in, and which verify command the gate should be told about.
pub struct KlooExecutor {
    /// The executable to spawn — a bare name found on `PATH`, or a full path.
    pub binary: String,
    /// The child's working directory: the workspace the phase edits.
    pub workdir: std::path::PathBuf,
    /// The verify command handed to the child with `--verify`.
    pub verify: String,
}

impl KlooExecutor {
    /// Build the argument vector for one execution: `prompt`, then the flags.
    ///
    /// Pure and public because it is the whole contract with the binary, and a test can check it
    /// without starting anything. The prompt is element 0, whole; `--model` is immediately followed
    /// by `family`.
    pub fn argv(&self, family: &str, prompt: &str) -> Vec<String> {
        vec![
            prompt.to_string(),
            "--verify".to_string(),
            self.verify.clone(),
            "--json".to_string(),
            "--effort".to_string(),
            "medium".to_string(),
            "--model".to_string(),
            family.to_string(),
        ]
    }

    /// The prepared, un-spawned child — so that the spawn and the wait can each fail on their own.
    fn command(&self, family: &str, prompt: &str) -> Command {
        let mut cmd = Command::new(&self.binary);
        cmd.args(self.argv(family, prompt))
            .current_dir(&self.workdir)
            // The child's stdout is `--json` output the caller does not consume here; letting it
            // inherit would dump a whole run's transcript into the desktop app's terminal.
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        cmd
    }
}

impl PhaseExecutor for KlooExecutor {
    /// Spawn the binary, wait for it, and report only whether it RAN.
    ///
    /// `Ok(())` for any completed child, whatever its exit status. `Err` only when the process could
    /// not be started or could not be waited on, naming the binary and the OS error.
    fn execute(&self, family: &str, prompt: &str) -> Result<(), String> {
        let mut cmd = self.command(family, prompt);
        let mut child = cmd.spawn().map_err(|e| {
            format!(
                "could not start the phase executor binary {:?}: {}",
                self.binary, e
            )
        })?;
        match child.wait() {
            // Ran to completion: exit 0, exit 1, signal — all of them are "it ran". The gate result
            // is read from the run's own scoring path, not from here.
            Ok(_status) => Ok(()),
            // The child started but could not be reaped. It still ran, so this is reported as an
            // error rather than silently as success: the caller must not record a hop whose end it
            // never learned.
            Err(e) => Err(format!(
                "could not wait for the phase executor binary {:?}: {}",
                self.binary, e
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::workflows::PhaseExecutor;
    use std::path::PathBuf;

    /// An executor pointed at `binary`, running in the test harness's own directory.
    ///
    /// `cwd()` rather than `/tmp`: the tests below never write anything, they only run a process, so
    /// the workspace directory is as safe a working directory as any and it exists on every platform
    /// this suite runs on.
    fn executor(binary: &str) -> KlooExecutor {
        KlooExecutor {
            binary: binary.to_string(),
            workdir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            verify: "cargo test".to_string(),
        }
    }

    /// A prompt containing a newline and a double quote arrives as EXACTLY ONE argument, unmangled.
    ///
    /// This is the whole reason the executor does not build a command line: the composed prompt is
    /// arbitrary text, and the binary must receive it byte for byte. The length assertion is the
    /// load-bearing one — a shell-quoted or whitespace-splitting implementation would produce two
    /// elements or a mangled one, and either way this fails.
    #[test]
    fn kloo_executor_passes_the_prompt_as_one_argument() {
        let exec = executor("kloo");
        let prompt = "role: you are a dev\nsay \"hello\" then run: ls\n";

        let argv = exec.argv("glm-4.6", prompt);

        assert_eq!(
            argv.len(),
            8,
            "prompt + --verify <v> + --json + --effort medium + --model <family> = 8 elements, \
             so the prompt was neither split nor padded: {argv:?}"
        );
        assert_eq!(
            argv[0], prompt,
            "the prompt must be argv[0], byte for byte, newlines and quotes intact"
        );
        assert_eq!(
            argv[0].matches('\n').count(),
            prompt.matches('\n').count(),
            "no newline may be dropped or converted to a space"
        );
        assert_eq!(
            argv[0].matches('"').count(),
            2,
            "neither double quote may be stripped, doubled or escaped into something else"
        );
        assert!(
            !argv.iter().any(|a| a == "sh" || a == "-c"),
            "the prompt must never be routed through a shell: {argv:?}"
        );
    }

    /// `--model` is immediately followed by the family, and nothing else carries it.
    ///
    /// The family is a third of the champion key, so an executor that put it anywhere other than
    /// directly after `--model` — or that dropped the flag and passed the family bare — would run
    /// the hop on a model the run's family does not name.
    #[test]
    fn kloo_executor_puts_the_family_after_the_model_flag() {
        let exec = executor("kloo");
        let family = "glm-4.6";

        let argv = exec.argv(family, "some prompt");

        let at = argv
            .iter()
            .position(|a| a == "--model")
            .expect("--model must be passed");
        assert_eq!(
            argv.get(at + 1).map(String::as_str),
            Some(family),
            "--model must be immediately followed by the family: {argv:?}"
        );
        assert_eq!(
            argv.iter().filter(|a| a.as_str() == family).count(),
            1,
            "the family appears once, as --model's value, nowhere else: {argv:?}"
        );
        // The rest of the fixed flag contract, checked here because this is the only argv test that
        // pins positions: --verify takes the executor's own verify string, and the two valueless
        // flags sit where the binary expects them.
        let verify_at = argv
            .iter()
            .position(|a| a == "--verify")
            .expect("--verify must be passed");
        assert_eq!(
            argv.get(verify_at + 1).map(String::as_str),
            Some("cargo test"),
            "--verify must be immediately followed by the executor's verify command: {argv:?}"
        );
        let effort_at = argv
            .iter()
            .position(|a| a == "--effort")
            .expect("--effort must be passed");
        assert_eq!(
            argv.get(effort_at + 1).map(String::as_str),
            Some("medium"),
            "--effort must be immediately followed by `medium`: {argv:?}"
        );
        assert!(
            argv.iter().any(|a| a == "--json"),
            "--json must be passed: {argv:?}"
        );
    }

    /// A child that ran and exited NONZERO is `Ok(())` — "ran and failed its gate" is not an error.
    ///
    /// `/bin/false` exits 1. The trait says an execution that ran and failed its gate is `Ok(())`,
    /// and scoring is `record_workflow_run_gate`'s job; if this returned `Err`, every red-phase hop
    /// would be recorded as never having happened and its run would be unscoreable.
    ///
    /// Skipped rather than failed where `/bin/false` does not exist (Windows), because the assertion
    /// is about exit status, not about the existence of a POSIX binary.
    #[test]
    fn kloo_executor_a_nonzero_exit_is_not_an_error() {
        let exec = executor("/bin/false");
        if !PathBuf::from("/bin/false").exists() {
            eprintln!("/bin/false is absent on this platform; nothing to exit nonzero");
            return;
        }

        let result = exec.execute("glm-4.6", "a prompt that will never be read");

        assert!(
            result.is_ok(),
            "a completed child with a nonzero exit must be Ok(()), got {result:?}"
        );
    }

    /// A binary that does not exist is `Err`, and the message names the path that was missing.
    ///
    /// The message assertion is the point: an executor that reported a bare "spawn failed" would
    /// leave an operator staring at an arena row with no way to find out which binary was wrong.
    /// Nothing is spawned here, so no kloo and no network are involved.
    #[test]
    fn kloo_executor_a_missing_binary_is_an_error() {
        let missing = "/nonexistent/kloo-executor-does-not-exist-9b1c";
        let exec = executor(missing);

        let result = exec.execute("glm-4.6", "any prompt");

        let err = result.expect_err("spawning a missing binary must be an error");
        assert!(
            err.contains(missing),
            "the error must name the binary that could not be started, got {err:?}"
        );
    }

    /// End-to-end: a real phase run through the whole arena wire, driven by a real `kloo`.
    ///
    /// `Ok(())` alone only means the child RAN — a model the endpoint does not serve is refused in
    /// under two seconds and still returns `Ok(())`. So the family is one the endpoint does serve,
    /// and the run is proved by evidence: `kloo` writes `.kloo/token-calibration.json` under its
    /// working directory once it has talked to the endpoint, so that file existing in the temp
    /// workdir is what shows the prompt actually reached a model.
    ///
    /// Drive one real phase run through the whole arena wire and report the evidence.
    ///
    /// The family is the ONLY thing the two tests below differ on, so it is the only input here.
    ///
    /// Shared by the two `#[ignore]`d tests below, which differ ONLY in the family they ask for.
    /// Everything here is the setup both of them need and nothing else: a real `AppState`, a
    /// workflow version carrying a real role, instruction, contract and a hop that parses, a run
    /// started for it, and a FRESH temporary workdir — never the repo — with a unique name.
    ///
    /// Returns the executor's result together with whether the evidence file
    /// `.kloo/token-calibration.json` existed in the workdir. The existence check is taken BEFORE
    /// the workdir is removed, because removing it first destroys the only evidence of what the
    /// child actually did — a test that deleted the directory and then asserted nothing about its
    /// contents would pass whether or not a model was ever reached.
    fn run_real_phase(family: &str) -> (Result<(), String>, bool) {
        use crate::services::workflows::{
            create_workflow_version, execute_workflow_run, start_workflow_run, NewWorkflowRun,
            NewWorkflowVersion,
        };
        use crate::test_support::test_state;

        // 1. A real AppState, exactly as workflows.rs builds its own.
        let (state, _root) = test_state();

        // 2. A workflow version with a real role, instruction, contract and a hop that parses.
        let workflow = create_workflow_version(
            &state,
            NewWorkflowVersion {
                workspace_path: std::env::temp_dir()
                    .join("kloo-e2e-workspace")
                    .display()
                    .to_string(),
                phase_type: "dev-phase".to_string(),
                family: family.to_string(),
                parent_id: None,
                role: "you are a developer".to_string(),
                instruction: "implement the phase".to_string(),
                contract: "the gate must pass".to_string(),
                hop: r#"{"steps":["plan","edit","verify"]}"#.to_string(),
                fan_out: 1,
            },
        )
        .expect("create workflow version");

        // 3. Start a run for it.
        let run = start_workflow_run(
            &state,
            NewWorkflowRun {
                workflow_id: workflow.id.clone(),
                plan_id: None,
                seed: 0,
                seats: None,
            },
        )
        .expect("start run");

        // 4. A FRESH temporary workdir — never the repo — with a unique name.
        let workdir = std::env::temp_dir().join(format!(
            "kloo-executor-e2e-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workdir).expect("create the test's temporary workdir");

        let exec = KlooExecutor {
            binary: "kloo".to_string(),
            workdir: workdir.clone(),
            verify: "true".to_string(),
        };

        // 5. Drive the phase through the whole wire...
        let result = execute_workflow_run(&state, &run.id, &exec);

        // 6. ...and CAPTURE THE EVIDENCE before the workdir goes, then remove the temp dir.
        let evidence_existed = workdir.join(".kloo/token-calibration.json").exists();
        let _ = std::fs::remove_dir_all(&workdir);

        (result, evidence_existed)
    }

    /// Ignored by default; run it by name with `cargo test --lib -- --ignored`.
    #[ignore = "spawns a real kloo against lokalai; run with --ignored"]
    #[test]
    fn kloo_executor_runs_a_real_phase_end_to_end() {
        let (result, evidence_existed) = run_real_phase("qwen3.8-flash-next");

        assert!(
            result.is_ok(),
            "a real phase run must return Ok(()), got {result:?}"
        );
        assert!(
            evidence_existed,
            "the evidence file .kloo/token-calibration.json was never written in the run's \
             workdir, so no model was reached: the prompt did not get an answer from the \
             endpoint, whatever the child's exit status was"
        );
    }

    /// The negative case: a family the endpoint does NOT serve still runs, but leaves no evidence.
    ///
    /// This is what makes the test above mean anything. `Ok(())` is returned for any completed
    /// child, so a refused model is `Ok(())` too — the trait promises only that the execution RAN.
    /// If this test ever reported the evidence file as PRESENT, the evidence file would be proving
    /// nothing and the positive test's assertion would be vacuous.
    ///
    /// Ignored by default; run it by name with `cargo test --lib -- --ignored`.
    #[ignore = "spawns a real kloo against lokalai; run with --ignored"]
    #[test]
    fn kloo_executor_e2e_catches_a_model_the_endpoint_does_not_serve() {
        let (result, evidence_existed) = run_real_phase("glm-4.6");

        assert!(
            result.is_ok(),
            "a child that ran against a model the endpoint does not serve is still Ok(()): the \
             trait reports whether the execution ran, not whether it passed, got {result:?}"
        );
        assert!(
            !evidence_existed,
            "no evidence file may be written for a model the endpoint does not serve — if \
             .kloo/token-calibration.json appeared anyway, its presence would no longer prove \
             the prompt reached a model"
        );
    }
}
