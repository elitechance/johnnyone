#!/usr/bin/env bash
# Gate for the lokalai chat provider. Run from the worktree root.
set -uo pipefail
cd "$(dirname "$0")/desktop/src-tauri" || exit 1
export CARGO_TARGET_DIR=/home/creepy/Documents/Workspace/personal/apps/johnnyone/desktop/src-tauri/target

fail() { echo "GATE FAILED: $1"; exit 1; }

echo "--- (1) the crate still compiles ---"
cargo check --lib --tests 2>&1 | tail -30
[ "${PIPESTATUS[0]}" -eq 0 ] || fail "cargo check is red (the dispatch sites are not exhaustive, or the module does not build)"

echo "--- (2) the specification ---"
log=$(mktemp)
cargo test --test lokalai_provider 2>&1 | tee "$log" | tail -40
[ "${PIPESTATUS[0]}" -eq 0 ] || fail "the lokalai specification is red"

ran=$(grep -cE '^test t[0-9]_.* \.\.\. ok$' "$log")
[ "$ran" -ge 9 ] || fail "only $ran of the 9 specified tests ran — a test that does not run proves nothing"

echo "--- test names seen by the gate ---"
grep -E '^test t[0-9]_' "$log"
echo "GATE PASSED"
