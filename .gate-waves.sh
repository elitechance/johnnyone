#!/usr/bin/env bash
# Gate for the verify wave-isolation invariant. Run from the worktree root.
set -uo pipefail
cd "$(dirname "$0")/desktop/src-tauri" || exit 1
export CARGO_TARGET_DIR=/home/creepy/Documents/Workspace/personal/apps/johnnyone/desktop/src-tauri/target
fail() { echo "GATE FAILED: $1"; exit 1; }

echo "--- (1) the crate still compiles ---"
cargo check --lib --tests 2>&1 | tail -25
[ "${PIPESTATUS[0]}" -eq 0 ] || fail "cargo check is red"

echo "--- (2) plan_check's own suite must not regress ---"
cargo test --lib services::plan_check 2>&1 | tail -15
[ "${PIPESTATUS[0]}" -eq 0 ] || fail "plan_check's existing tests are red — the packing change broke them"

echo "--- (3) the specification ---"
log=$(mktemp)
cargo test --test verify_wave_isolation 2>&1 | tee "$log" | tail -25
[ "${PIPESTATUS[0]}" -eq 0 ] || fail "the wave-isolation specification is red"
ran=$(grep -cE '^test w[0-9]_.* \.\.\. ok$' "$log")
[ "$ran" -ge 4 ] || fail "only $ran of the 4 specified tests ran"
grep -E '^test w[0-9]_' "$log"
echo "GATE PASSED"
