#!/usr/bin/env bash
# The lens material budget must cover the material the host really assembles.
# Specification: desktop/src-tauri/tests/lens_material_budget.rs — read its
# header; it carries the measurement.
#
# Stage 2 is the whole point of this gate's shape: the budget constant is read
# by three other spec files, and raising it must not silently relax any of them.
set -uo pipefail
cd "$(dirname "$0")/desktop/src-tauri" || exit 1
export CARGO_TARGET_DIR=/home/creepy/.cache/j1-cargo/budget CARGO_TERM_COLOR=never
fail() { echo; echo "GATE FAILED — $*"; exit 1; }
count_of() { sed -n 's/^test result:.*[^0-9]\([0-9]\+\) passed.*/\1/p' "$1" | tail -1; }
names_of() { sed -n 's/^test \([a-z0-9_]*\) \.\.\. ok$/  \1/p' "$1"; }

echo "== stage 1/3 — the specification (tests/lens_material_budget.rs) =="
L=$(mktemp); cargo test --test lens_material_budget -- --test-threads=1 >"$L" 2>&1
S1=$(count_of "$L")
grep -q "^test result: ok\." "$L" || { sed -n '/^---- /,$p' "$L" | head -40; fail "the specification is red (above)."; }
names_of "$L"
[ "${S1:-0}" -ge 3 ] || fail "stage 1 reported ${S1:-0}, expected at least 3 (b1..b3)"
echo "stage 1 OK — $S1 budget tests"

echo
echo "== stage 2/3 — every other spec that reads this budget still holds =="
for t in stream_lens_conventions stream_lens_material stream_lens; do
  L2=$(mktemp); cargo test --test "$t" -- --test-threads=1 >"$L2" 2>&1
  C=$(count_of "$L2")
  grep -q "^test result: ok\." "$L2" || { sed -n '/^---- /,$p' "$L2" | head -40; fail "$t went red. m3/m4 and c3 all reason about where the cut falls, so a budget change can relax a truncation rail without touching its code."; }
  case "$t" in
    stream_lens_conventions) [ "${C:-0}" -ge 5 ] || fail "$t reported ${C:-0}, expected >= 5";;
    stream_lens_material)    [ "${C:-0}" -ge 6 ] || fail "$t reported ${C:-0}, expected >= 6";;
    stream_lens)             [ "${C:-0}" -ge 8 ] || fail "$t reported ${C:-0}, expected >= 8";;
  esac
  echo "  $t OK — $C tests"
done

echo
echo "== stage 3/3 — the lib the host is built from =="
cargo check --lib >/dev/null 2>&1 || { cargo check --lib 2>&1 | grep -E "^error" | head -20; fail "cargo check --lib is red."; }
echo "stage 3 OK"
echo
echo "GATE PASSED — $S1 budget tests, conventions + material + transport specs green, lib compiles."
