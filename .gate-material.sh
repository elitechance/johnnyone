#!/usr/bin/env bash
# Gate for G1b — a stream lens must be GIVEN the material, because it has no
# tools. Run from the worktree root.
#
# Three stages, kept deliberately narrow:
#   1. the new material spec — 6 tests, named and COUNTED
#   2. the existing stream_lens spec — 8 tests, so the transport does not regress
#   3. `cargo check` on the lib, which is what the host is built from
#
# WHY IT IS THIS SMALL. The valview gate measured 217s and kloo ran it 64 times
# inside a 60-minute budget, running out with the work half done. And a gate
# demanding a whole 255-test package green against a four-file scope handed an
# earlier run a red it could not clear. Gate cost and gate scope are both part
# of gate design. This one covers the change and nothing else.
#
# `--color never` everywhere, and the counts come from libtest's own
# `test result:` line. On 2026-10-24 a gate that grepped vitest's COLOURED
# summary parsed 8 passing tests as 0, and the worker responded by patching
# `process.stdout.write` from inside a product source file to make the grep
# match — which then broke the production build. A gate that can only be
# satisfied by deforming the product is a defect in the gate.
set -uo pipefail
cd "$(dirname "$0")" || exit 1

MF=desktop/src-tauri/Cargo.toml

# Build OFF the tmpfs: `/tmp` is a 30 GB tmpfs and this worktree's target reached
# 9.5 GB. On 2026-10-24 cargo failed with `Disk quota exceeded (os error 122)`
# in a sibling worktree and the gate reported a red the worker could not fix.
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-/home/creepy/.cache/j1-cargo/lens}
mkdir -p "$CARGO_TARGET_DIR" || fail "cannot create $CARGO_TARGET_DIR"
echo "cargo target: $CARGO_TARGET_DIR"
fail() { echo "GATE FAIL: $*" >&2; exit 1; }

# "test result: ok. 6 passed; 0 failed; …" -> passed count
passed_of() { printf '%s\n' "$1" | sed -n 's/^test result: ok\. \([0-9]*\) passed.*/\1/p' | head -1; }
failed_of() { printf '%s\n' "$1" | sed -n 's/^test result: .*\. [0-9]* passed; \([0-9]*\) failed.*/\1/p' | head -1; }

run_spec() { # <test target> <expected count> <label>
  local target=$1 want=$2 label=$3
  echo "== $label — $target =="
  local out rc
  out=$(cargo test --manifest-path "$MF" --test "$target" --color never -- --nocapture 2>&1)
  rc=$?
  # Every test NAME with its result. A count with no names cannot tell a
  # renamed test from a deleted one.
  printf '%s\n' "$out" | grep -E '^test [a-z0-9_]+ \.\.\.' | sed 's/^/   /'
  printf '%s\n' "$out" | grep -E '^(error|error\[|thread .* panicked|assertion)' | head -20
  local p f
  p=$(passed_of "$out"); f=$(failed_of "$out")
  p=${p:-0}; f=${f:-0}
  [ "$rc" -eq 0 ] || fail "$target is not green (rc=$rc, $p passed, $f failed)"
  [ "$f" -eq 0 ] || fail "$f test(s) failed in $target"
  [ "$p" -ge "$want" ] || fail "only $p of the $want $target tests ran — a target that collects nothing passes everything"
  echo "   $target tests that PASSED: $p"
}

t0=$(date +%s)
run_spec stream_lens_material 6 "1/3 the material spec (G1b)"
run_spec stream_lens           8 "2/3 the transport spec (G1, must not regress)"

echo "== 3/3 cargo check — the lib the host is built from =="
# `cargo check`, never `--release`: a release build of this crate has killed a
# session before, and the host is rebuilt separately and deliberately.
chk=$(cargo check --manifest-path "$MF" --lib --color never 2>&1)
rc=$?
printf '%s\n' "$chk" | grep -E '^(error|error\[)' | head -20
[ "$rc" -eq 0 ] || fail "the lib does not compile"
echo "   lib compiles"

echo "GATE PASS: 6 material tests, 8 transport tests, lib compiles  [$(( $(date +%s) - t0 ))s]"
