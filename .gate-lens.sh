#!/usr/bin/env bash
# Gate for the stream-lens transport. Run from the worktree root.
#
# Four stages, each of which must name its own failure:
#   1. the crate compiles, lib + tests
#   2. the existing agent_plans suite does not regress
#   3. tests/lokalai_provider.rs: 9 transport tests + 3 model-alias tests
#   4. the 8 spec tests in tests/stream_lens.rs pass, and there are 8 of them
#
# Stages 3 and 4 COUNT the tests that ran. A suite that collects nothing passes
# everything, and `cargo test` exits 0 on "0 passed".
set -uo pipefail
cd "$(dirname "$0")/desktop/src-tauri" || exit 1

fail() { echo "GATE FAIL: $*" >&2; exit 1; }

echo "== 1/4 cargo check --lib --tests =="
cargo check --lib --tests 2>&1 | tail -40
[ "${PIPESTATUS[0]}" -eq 0 ] || fail "the crate does not compile"

echo "== 2/4 regression: cargo test --lib services::agent_plans =="
# ONE known failure is OUT OF SCOPE for this change and must not fail the gate.
#
#   services::agent_plans::replan_tests::kloo_needs_changes_parks_instead_of_alldone_loop
#
# It spawns a REAL agent CLI into a randomly-named temp workspace, so the CLI
# asks "do you trust the files in this folder?" and `pane_awaiting_trust`
# correctly catches it — the test's `.unwrap()` then panics. Measured on the
# untouched base (merge 1f39f2b, which does not touch agent_plans.rs at all) and
# it fails there identically, so it is environmental, not a regression. A gate
# that demands a whole 255-test package green against a four-file scope hands
# the worker a red it cannot fix; that is what stopped the first run here.
KNOWN_OOS='kloo_needs_changes_parks_instead_of_alldone_loop'

reg=$(cargo test --lib services::agent_plans 2>&1)
echo "$reg" | tail -8

# Name every failure, then subtract the one known to be out of scope.
failed=$(printf '%s\n' "$reg" | sed -n '/^failures:$/,$p' | grep -E '^    services::' | sed 's/^ *//' | sort -u)
if [ -n "$failed" ]; then
  echo "   failures reported:"; printf '%s\n' "$failed" | sed 's/^/     /'
fi
unexpected=$(printf '%s\n' "$failed" | grep -v "$KNOWN_OOS" | grep -E '^services::')
[ -z "$unexpected" ] || fail "agent_plans regressed, beyond the known out-of-scope failure:
$unexpected"

regpass=$(printf '%s\n' "$reg" | grep -oE '[0-9]+ passed' | grep -oE '[0-9]+' | head -1)
regpass=${regpass:-0}
[ "$regpass" -ge 250 ] || fail "only $regpass agent_plans tests passed — the filter stopped matching, and a suite that collects nothing passes everything"
echo "   agent_plans tests passed: $regpass (plus 1 known out-of-scope failure: $KNOWN_OOS)"

echo "== 3/4 spec: cargo test --test lokalai_provider =="
lok=$(cargo test --test lokalai_provider 2>&1)
rc=$?
echo "$lok" | tail -20
[ "$rc" -eq 0 ] || fail "the lokalai provider spec is not green (t1-t9 plus a1-a3, the model-alias tests)"
lokran=$(printf '%s\n' "$lok" | grep -cE '^test (t|a)[0-9].* \.\.\. ok$')
echo "   lokalai_provider tests that ran: $lokran"
[ "$lokran" -ge 12 ] || fail "only $lokran of the 12 lokalai_provider tests ran (9 transport + 3 model-alias)"

echo "== 4/4 spec: cargo test --test stream_lens =="
out=$(cargo test --test stream_lens -- --nocapture 2>&1)
rc=$?
echo "$out" | tail -60
[ "$rc" -eq 0 ] || fail "the stream_lens spec is not green"

# Name every test that ran, then count them.
printf '%s\n' "$out" | grep -E '^test s[0-9]' | sort
ran=$(printf '%s\n' "$out" | grep -cE '^test s[0-9].* \.\.\. ok$')
echo "   stream_lens tests that ran: $ran"
[ "$ran" -ge 8 ] || fail "only $ran of the 8 spec tests ran — a suite that collects nothing passes everything"

echo "GATE PASS: compiles, $regpass agent_plans green, $lokran lokalai_provider green, $ran/8 stream_lens green"
