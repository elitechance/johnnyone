#!/usr/bin/env bash
# The DEVELOPMENT review fan-out must take the stream transport too — through
# ONE shared dispatch, not a second copy of the branch.
#
# Specification: desktop/src-tauri/tests/dev_lens_fanout.rs. Read its header.
#
# COST NOTE: stage 1 takes ~50 s, almost all of it f2 waiting on a `claude`
# spawn that cannot complete here. That is the control arm and it is worth its
# seconds — the cheapest way to pass f1 is to stop spawning reviewers at all.
set -uo pipefail
cd "$(dirname "$0")/desktop/src-tauri" || exit 1
export CARGO_TARGET_DIR=/home/creepy/.cache/j1-cargo/devlens CARGO_TERM_COLOR=never
fail() { echo; echo "GATE FAILED — $*"; exit 1; }
count_of() { sed -n 's/^test result:.*[^0-9]\([0-9]\+\) passed.*/\1/p' "$1" | tail -1; }
names_of() { sed -n 's/^test \([a-z0-9_]*\) \.\.\. ok$/  \1/p' "$1"; }

echo "== stage 1/3 — the specification (tests/dev_lens_fanout.rs) =="
L=$(mktemp); cargo test --test dev_lens_fanout -- --test-threads=1 >"$L" 2>&1
S1=$(count_of "$L")
grep -q "^test result: ok\." "$L" || { sed -n '/^---- /,$p' "$L" | head -45; fail "the specification is red (above). f1 is the outcome; f4 is what stops the fix being a second copy of the branch."; }
names_of "$L"
[ "${S1:-0}" -ge 4 ] || fail "stage 1 reported ${S1:-0}, expected at least 4 (f1..f4)"
echo "stage 1 OK — $S1 development fan-out tests"

echo
echo "== stage 2/3 — the planning fan-out and the material specs still hold =="
# The whole point is ONE shared dispatch, so the planning path is now downstream
# of the change: s8 drives the real planning fan-out and m6/c5 check its prompt.
for t in stream_lens stream_lens_material stream_lens_conventions lens_material_budget; do
  L2=$(mktemp); cargo test --test "$t" -- --test-threads=1 >"$L2" 2>&1
  C=$(count_of "$L2")
  grep -q "^test result: ok\." "$L2" || { sed -n '/^---- /,$p' "$L2" | head -40; fail "$t went red. The planning fan-out now shares the dispatch you changed — s8 proves it still streams and creates no pane."; }
  case "$t" in
    stream_lens)              [ "${C:-0}" -ge 8 ] || fail "$t reported ${C:-0}, expected >= 8";;
    stream_lens_material)     [ "${C:-0}" -ge 6 ] || fail "$t reported ${C:-0}, expected >= 6";;
    stream_lens_conventions)  [ "${C:-0}" -ge 5 ] || fail "$t reported ${C:-0}, expected >= 5";;
    lens_material_budget)     [ "${C:-0}" -ge 3 ] || fail "$t reported ${C:-0}, expected >= 3";;
  esac
  echo "  $t OK — $C tests"
done

echo
echo "== stage 3/3 — the lib the host is built from =="
cargo check --lib >/dev/null 2>&1 || { cargo check --lib 2>&1 | grep -E "^error" | head -20; fail "cargo check --lib is red."; }
echo "stage 3 OK"
echo
echo "GATE PASSED — $S1 development fan-out tests, every lens spec green, lib compiles."
