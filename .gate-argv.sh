#!/usr/bin/env bash
# The lens request body must travel on stdin, not in argv.
#
# Specification: desktop/src-tauri/tests/lens_body_not_in_argv.rs. Read its
# header — it carries the measured MAX_ARG_STRLEN table and says why a smaller
# budget is NOT the fix.
#
# COST NOTE, and it matters — this gate killed a run once.
#
# kloo runs its `--verify` through a tool call with a 5-minute ceiling and no
# flag to raise it. The first attempt at this task spent 13m16s and 940,515
# tokens, got the fix essentially right across 9 files, and was ROLLED BACK
# because the final verify — the first one that reached a compiling tree — had
# to build seven test binaries in a CARGO_TARGET_DIR that had never built one.
# That exceeded 5 minutes and the work was discarded.
#
# Measured on a warm target dir: cargo check --lib 1s after touching the shared
# struct, and all six family suites 27s. Warm, this whole gate is ~70s. The
# cost is entirely first-build, not test execution.
#
# So GATE_FAST=1 runs stage 1 + stage 3 only — the four argv tests, which ARE
# the work, plus the library compiling. That is what a worker's inner loop
# should carry. Stage 2 (collateral damage across the lens family) is MINE to
# run before any merge, and this gate says so out loud rather than letting a
# fast green look like a full one.
#
# Stage 1 pushes 480,000 bytes through a local socket in a4, so budget a few
# seconds for the capture rather than assuming a hang.
#
# OUT OF SCOPE — do not try to fix these, they are red before you start:
#   * the `select_planning_planner` unused-import warning in the lib.
#   * anything under tests/kloo_loop.rs, tests/small_mode_acceptance.rs or
#     tests/verify_wave_isolation.rs — untouched by this change.
set -uo pipefail
cd "$(dirname "$0")/desktop/src-tauri" || exit 1
export CARGO_TARGET_DIR=/home/creepy/.cache/j1-cargo/argv CARGO_TERM_COLOR=never
fail() { echo; echo "GATE FAILED — $*"; exit 1; }
count_of() { sed -n 's/^test result:.*[^0-9]\([0-9]\+\) passed.*/\1/p' "$1" | tail -1; }
names_of() { sed -n 's/^test \([a-z0-9_]*\) \.\.\. ok$/  \1/p' "$1"; }

echo "== stage 1/3 — the specification (tests/lens_body_not_in_argv.rs) =="
L=$(mktemp); cargo test --test lens_body_not_in_argv -- --test-threads=1 >"$L" 2>&1
S1=$(count_of "$L")
grep -q "^test result: ok\." "$L" || {
  # Name the failure, whichever kind it is. A gate that greps only for libtest's
  # `---- name stdout ----` blocks prints NOTHING when the suite did not compile,
  # and a red with no reason is indistinguishable from a broken gate. Measured:
  # the first run of this gate did exactly that and said only "the specification
  # is red (above)" with nothing above it.
  if grep -q "^error" "$L"; then
    echo "--- the suite did not COMPILE ---"
    grep -E "^error(\[E[0-9]+\])?" -A 6 "$L" | head -40
  else
    echo "--- failing tests ---"
    sed -n '/^---- /,$p' "$L" | head -60
  fi
  grep -qiE "Argument list too long|os error 7" "$L" && echo ">>> That IS the defect: the body is still in argv."
  fail "the specification is red (above). a2 is the outcome; a4 is what stops a smaller budget counting as the fix."
}
names_of "$L"
[ "${S1:-0}" -ge 4 ] || fail "stage 1 reported ${S1:-0}, expected at least 4 (a1..a4)"
echo "stage 1 OK — $S1 argv/stdin tests"

if [ "${GATE_FAST:-0}" = "1" ]; then
  echo
  echo "== stage 2/3 — SKIPPED (GATE_FAST=1) =="
  echo "   The lens family regression did NOT run. This green covers the four"
  echo "   argv tests and the library compiling, nothing else. A full run is"
  echo "   required before merge: GATE_FAST=0 ./.gate-argv.sh"
else
echo
echo "== stage 2/3 — the whole lens family still holds =="
# build_config and the spawn path are shared by every lens, planning and
# development alike, so all five suites are downstream of this change.
for t in stream_lens stream_lens_material stream_lens_conventions lens_material_budget dev_lens_fanout lokalai_provider; do
  L2=$(mktemp); cargo test --test "$t" -- --test-threads=1 >"$L2" 2>&1
  C=$(count_of "$L2")
  grep -q "^test result: ok\." "$L2" || { sed -n '/^---- /,$p' "$L2" | head -40; fail "$t went red. Every lens shares build_config and the spawn path you changed."; }
  case "$t" in
    stream_lens)             [ "${C:-0}" -ge 8 ] || fail "$t reported ${C:-0}, expected >= 8";;
    stream_lens_material)    [ "${C:-0}" -ge 6 ] || fail "$t reported ${C:-0}, expected >= 6";;
    stream_lens_conventions) [ "${C:-0}" -ge 5 ] || fail "$t reported ${C:-0}, expected >= 5";;
    lens_material_budget)    [ "${C:-0}" -ge 3 ] || fail "$t reported ${C:-0}, expected >= 3";;
    dev_lens_fanout)         [ "${C:-0}" -ge 4 ] || fail "$t reported ${C:-0}, expected >= 4";;
    lokalai_provider)        [ "${C:-0}" -ge 1 ] || fail "$t reported ${C:-0}, expected >= 1";;
  esac
  echo "  $t OK — $C tests"
done
fi

echo
echo "== stage 3/3 — the library still compiles =="
cargo check --lib >/dev/null 2>&1 || { cargo check --lib 2>&1 | grep -E "^error" | head -20; fail "cargo check --lib is red"; }
echo "stage 3 OK — cargo check --lib clean"

echo
if [ "${GATE_FAST:-0}" = "1" ]; then
  echo "GATE PASSED (FAST) — the body is on stdin and 3x the budget reaches the wire."
  echo "Stage 2 was skipped, so this says nothing about the rest of the lens family."
else
  echo "GATE PASSED — the body is on stdin and 3x the budget reaches the wire."
fi
echo "WHAT THIS GATE CANNOT SEE: whether a real glm review of a real phase"
echo "returns a usable verdict. It proves the bytes arrive, not that the review"
echo "is good. That is the live G6 box and it is mine to check by eye."
