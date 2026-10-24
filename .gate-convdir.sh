#!/usr/bin/env bash
# The conventions are a DIRECTORY. Inline its files, not its error.
#
# The specification is desktop/src-tauri/tests/stream_lens_conventions.rs — read
# its header first; it carries the live measurement this closes.
#
# DESIGN NOTES, carried from the four gates before this one:
#   * counts come from libtest's own `test result:` line with --color never, and
#     each stage prints the test NAMES it ran.
#   * CARGO_TARGET_DIR is on the real disk. /tmp here is a 30 GB tmpfs and a
#     sibling gate died seven times on `Disk quota exceeded (os error 122)` —
#     a red no amount of code could fix.
#   * stage 2 re-runs the EXISTING material and transport specs. The change is
#     inside the same material assembly they cover, and a fix that inlines the
#     conventions while dropping the brief is a worse bug than the one it fixes.
set -uo pipefail
cd "$(dirname "$0")/desktop/src-tauri" || exit 1
export CARGO_TARGET_DIR=/home/creepy/.cache/j1-cargo/convdir
export CARGO_TERM_COLOR=never
fail() { echo; echo "GATE FAILED — $*"; exit 1; }
count_of() { sed -n 's/^test result:.*[^0-9]\([0-9]\+\) passed.*/\1/p' "$1" | tail -1; }
names_of() { sed -n 's/^test \([a-z0-9_]*\) \.\.\. ok$/  \1/p' "$1"; }

echo "== stage 1/3 — the specification (tests/stream_lens_conventions.rs) =="
L=$(mktemp); cargo test --test stream_lens_conventions -- --test-threads=1 >"$L" 2>&1
S1=$(count_of "$L")
grep -q "^test result: ok\." "$L" || { sed -n '/^---- /,$p' "$L" | head -50; fail "the specification is red (above). c5 is the one that matters: it drives the real fan-out and requires a file from the conventions DIRECTORY on the wire."; }
names_of "$L"
[ "${S1:-0}" -ge 5 ] || fail "stage 1 reported ${S1:-0} passing tests, expected at least 5 (c1..c5). A deleted test is not a pass."
echo "stage 1 OK — $S1 conventions tests"

echo
echo "== stage 2/3 — the material and transport specs still hold =="
for t in stream_lens_material stream_lens; do
  L2=$(mktemp); cargo test --test "$t" -- --test-threads=1 >"$L2" 2>&1
  C=$(count_of "$L2")
  grep -q "^test result: ok\." "$L2" || { sed -n '/^---- /,$p' "$L2" | head -40; fail "$t went red. The conventions expansion lives inside the same material assembly these cover — a fix that inlines the conventions and drops the brief is worse than the bug."; }
  names_of "$L2"
  case "$t" in
    stream_lens_material) [ "${C:-0}" -ge 6 ] || fail "$t reported ${C:-0}, expected >= 6";;
    stream_lens)          [ "${C:-0}" -ge 8 ] || fail "$t reported ${C:-0}, expected >= 8";;
  esac
  echo "  $t OK — $C tests"
done

echo
echo "== stage 3/3 — the lib the host is built from =="
cargo check --lib >/dev/null 2>&1 || { cargo check --lib 2>&1 | grep -E "^error" | head -20; fail "cargo check --lib is red."; }
echo "stage 3 OK"
echo
echo "GATE PASSED — $S1 conventions tests, material + transport specs green, lib compiles."
