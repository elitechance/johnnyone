#!/usr/bin/env bash
# The lens request body must travel on stdin, not in argv.
#
# Specification: desktop/src-tauri/tests/lens_body_not_in_argv.rs. Read its
# header — it carries the measured MAX_ARG_STRLEN table and says why a smaller
# budget is NOT the fix.
#
# COST NOTE: stage 1 is four tests against a local socket; a4 pushes 480,000
# bytes through it, so budget a few seconds for the capture rather than
# assuming a hang.
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

echo
echo "== stage 3/3 — the library still compiles =="
cargo check --lib >/dev/null 2>&1 || { cargo check --lib 2>&1 | grep -E "^error" | head -20; fail "cargo check --lib is red"; }
echo "stage 3 OK — cargo check --lib clean"

echo
echo "GATE PASSED — the body is on stdin and 3x the budget reaches the wire."
echo "WHAT THIS GATE CANNOT SEE: whether a real glm review of a real phase"
echo "returns a usable verdict. It proves the bytes arrive, not that the review"
echo "is good. That is the live G6 box and it is mine to check by eye."
