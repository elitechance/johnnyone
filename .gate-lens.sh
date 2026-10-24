#!/usr/bin/env bash
# Gate for the stream-lens transport. Run from the worktree root.
#
# Three stages, each of which must name its own failure:
#   1. the crate compiles, lib + tests
#   2. the existing agent_plans suite does not regress
#   3. the 7 spec tests in tests/stream_lens.rs pass, and there are 7 of them
#
# Stage 3 COUNTS the tests that ran. A suite that collects nothing passes
# everything, and `cargo test` exits 0 on "0 passed".
set -uo pipefail
cd "$(dirname "$0")/desktop/src-tauri" || exit 1

fail() { echo "GATE FAIL: $*" >&2; exit 1; }

echo "== 1/3 cargo check --lib --tests =="
cargo check --lib --tests 2>&1 | tail -40
[ "${PIPESTATUS[0]}" -eq 0 ] || fail "the crate does not compile"

echo "== 2/3 regression: cargo test --lib services::agent_plans =="
reg=$(cargo test --lib services::agent_plans 2>&1)
rc=$?
echo "$reg" | tail -25
[ "$rc" -eq 0 ] || fail "the existing agent_plans suite regressed"
regran=$(printf '%s\n' "$reg" | grep -oE '^test result: ok\. [0-9]+ passed' | grep -oE '[0-9]+' | head -1)
regran=${regran:-0}
[ "$regran" -ge 30 ] || fail "only $regran agent_plans tests ran — the filter stopped matching"
echo "   agent_plans tests passed: $regran"

echo "== 3/3 spec: cargo test --test stream_lens =="
out=$(cargo test --test stream_lens -- --nocapture 2>&1)
rc=$?
echo "$out" | tail -60
[ "$rc" -eq 0 ] || fail "the stream_lens spec is not green"

# Name every test that ran, then count them.
printf '%s\n' "$out" | grep -E '^test s[0-9]' | sort
ran=$(printf '%s\n' "$out" | grep -cE '^test s[0-9].* \.\.\. ok$')
echo "   stream_lens tests that ran: $ran"
[ "$ran" -ge 7 ] || fail "only $ran of the 7 spec tests ran — a suite that collects nothing passes everything"

echo "GATE PASS: compiles, $regran agent_plans tests green, $ran/7 stream_lens spec tests green"
