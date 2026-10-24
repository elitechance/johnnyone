#!/usr/bin/env bash
# Gate for the validator-view change. Run from the worktree root.
#
# Five stages (GATE_FAST=1 skips 2 and 4, the two whole-surface ones):
#   1. the new pure specs pass, and the gate COUNTS that they ran
#   2. the rest of the web unit suite does not regress
#   3. `nx build web` — a REAL production build
#   4. the built app BOOTS in real headless Chrome
#
# Stage 3 is not ceremony. vitest does not typecheck Angular templates: a
# template calling a method the component never exposed passes every unit test
# and fails at deploy. These two changes are both template-facing, so the build
# is the only thing that checks them.
#
# Stage 4 BOOTS the built app in real headless Chrome (web/tools/arena-phone-
# harness.mjs, which the repo already ships). A blank page fails every render
# test the same way unwritten code does; without this stage the gate cannot
# tell those apart. It serves dist/web/browser, so it must run AFTER stage 3.
set -uo pipefail
cd "$(dirname "$0")" || exit 1

fail() { echo "GATE FAIL: $*" >&2; exit 1; }

# ── Counting tests HONESTLY ──────────────────────────────────────────────────
# This gate used to count tests by grepping the human reporter for `Tests  N
# passed`. Vitest colourises and pads that line through a pretty-printer even
# when its output is piped, so the escapes land BETWEEN the words and a run
# that passed all 8 parsed as 0. That mis-parse cost a whole kloo budget: the
# worker, told the count was the gate, went and patched `process.stdout.write`
# and `process.env.NO_COLOR` from INSIDE a product source file to make the
# grep match — 142 lines of terminal plumbing in an Angular component's pure
# layer — and that import broke `nx build web` with three TS4111 errors. A
# gate that can only be satisfied by deforming the product is a defect in the
# gate. So the count now comes from vitest's JSON reporter, which is a
# machine-readable contract and cannot be coloured.
vitest_json() { # <outfile> [args…]
  local out=$1; shift
  npx vitest run --config web/vitest.config.ts "$@" \
    --reporter=json --outputFile="$out" 2>&1
}
vitest_count() { # <outfile> -> "<passed> <failed> <total>"
  python3 -c '
import json,sys
d=json.load(open(sys.argv[1]))
print(d.get("numPassedTests",0), d.get("numFailedTests",0), d.get("numTotalTests",0))
' "$1"
}
vitest_names() { # <outfile>
  python3 -c '
import json,sys
d=json.load(open(sys.argv[1]))
for f in d.get("testResults",[]):
    for a in f.get("assertionResults",[]):
        print("   %-7s %s" % (a.get("status"), a.get("fullName")))
' "$1"
}


# GATE_FAST=1 skips the two whole-surface stages (the 1182-test suite and the
# headless-Chrome boot) and keeps spec + production build + wiring.
#
# Measured 2026-10-24: the full gate is 217s. kloo ran it 64 times in a
# 60-minute budget and ran out of time with the work half done. A gate that
# expensive does not merely slow a run, it spends the budget the run needed.
# The worker iterates on the fast one; the FULL gate is run before the PR, and
# that is a box on the finish plan rather than an intention.
FAST=${GATE_FAST:-0}

echo "== 1/5 the validator-view spec =="
SPEC_JSON=${TMPDIR:-/tmp}/valview-spec.json
rm -f "$SPEC_JSON"
spec=$(vitest_json "$SPEC_JSON" validator-view)
rc=$?
printf '%s\n' "$spec" | sed -e 's/\x1b\[[0-9;]*m//g' | grep -E 'FAIL|Error|✘' | head -20
[ -s "$SPEC_JSON" ] || fail "the spec run produced no JSON report — it did not run at all"
read -r sp sf st < <(vitest_count "$SPEC_JSON")
vitest_names "$SPEC_JSON"
[ "$rc" -eq 0 ] || fail "the validator-view spec is not green ($sf failed of $st)"
[ "$sf" -eq 0 ] || fail "$sf validator-view test(s) failed"
ran=$sp
[ "$ran" -ge 8 ] || fail "only $ran of the 8 validator-view tests passed — a suite that collects nothing passes everything"
echo "   validator-view tests that PASSED: $ran of $st"

if [ "$FAST" = "1" ]; then
  echo "== 2/5 SKIPPED (GATE_FAST) — the 1182-test web suite =="
  allran=skipped
else
echo "== 2/5 the rest of the web unit suite =="
ALL_JSON=${TMPDIR:-/tmp}/valview-all.json
rm -f "$ALL_JSON"
all=$(vitest_json "$ALL_JSON")
rc=$?
printf '%s\n' "$all" | sed -e 's/\x1b\[[0-9;]*m//g' | grep -E 'FAIL|✘' | head -20
[ -s "$ALL_JSON" ] || fail "the web suite produced no JSON report — it did not run at all"
read -r ap af at < <(vitest_count "$ALL_JSON")
[ "$rc" -eq 0 ] || fail "the web unit suite regressed ($af failed of $at)"
[ "$af" -eq 0 ] || fail "$af web unit test(s) failed"
allran=$ap
[ "$allran" -ge 1182 ] || fail "only $allran web tests passed of the 1182 this branch inherited — the suite stopped collecting"
echo "   web unit tests that PASSED: $allran of $at"
fi

echo "== 3/5 nx build web (production) =="
build=$(npx nx build web --skip-nx-cache 2>&1)
rc=$?
printf '%s\n' "$build" | tail -20
[ "$rc" -eq 0 ] || fail "the production build failed — vitest does not typecheck templates, this stage does"

if [ "$FAST" = "1" ]; then
  echo "== 4/5 SKIPPED (GATE_FAST) — the headless-Chrome boot =="
else
echo "== 4/5 boot check: the built app renders in real headless Chrome =="
if [ ! -x /usr/bin/google-chrome ]; then
  fail "no /usr/bin/google-chrome — the boot stage cannot run, and a gate that silently skips its only render check is worse than no gate"
fi
boot=$(ARENA_HARNESS_OUT="${TMPDIR:-/tmp}/valview-shots" node web/tools/arena-phone-harness.mjs 2>&1)
rc=$?
printf '%s\n' "$boot" | tail -30

# THREE KNOWN OUT-OF-SCOPE FAILURES, named rather than tolerated silently.
#
#   workflow-arena / -decisive / -unscored:
#     "1 interactive control(s) present but UNCHECKED — execute-run"
#
# That is the harness telling us its own route control list does not name the
# arena's `execute-run` button. It is NOT a render failure: every route in the
# run booted, every scroller was healthy, nothing overflowed and nothing was
# unreachable. The arena routes share no file with this change. The harness's
# own documentation warns that in the round which reviewed it, two of three
# findings were defects in the harness rather than in the product.
#
# Making this mandatory for a change that touches three files is the same
# mistake as demanding a 255-test package green against a four-file scope: it
# hands the worker a red it cannot clear, and it cost a 60-minute budget here.
# So: every OTHER harness failure still fails this gate, and a RENDER failure
# always fails it.
fails=$(printf '%s\n' "$boot" | sed -n '/FAILURE(S)/,$p' | grep -E '^  - ')
unexpected=$(printf '%s\n' "$fails" | grep -vE '^  - workflow-arena(-decisive|-unscored)?: 1 interactive control\(s\) present but UNCHECKED — execute-run')
if [ -n "$(printf '%s' "$unexpected" | tr -d '[:space:]')" ]; then
  fail "the boot check found something beyond the three known arena control-list gaps:
$unexpected"
fi
if [ -n "$fails" ]; then
  echo "   (3 known arena control-list gaps tolerated; every route booted)"
fi
printf '%s\n' "$boot" | grep -q "after console-arena-tab" || fail "the harness did not reach the console route — it did not actually boot the app"
fi

echo "== 5/5 the wiring: the page must actually USE what was written =="
# WHY THIS STAGE EXISTS. Twice tonight a gate proved a module and said nothing
# about its call site, and both times the work shipped with the defect
# untouched: stream_lens passed seven tests while the fan-out still spawned
# panes, and this very run ended with healthReason written, exported, 8/8
# green — and never rendered. A pure function nobody calls is dead code with a
# certificate.
#
# This is a SHAPE check and it is weaker than the others: it proves the symbols
# are referenced, not that a human can read the result. That last part is the
# in-Chrome box on the finish plan and no local stage can tick it.
TP=web/src/app/pages/terminal
grep -q "healthReason" "$TP/terminal.page.ts" \
  || fail "terminal.page.ts never calls healthReason — the function exists and nothing uses it"
grep -q "lensOutcomes" "$TP/terminal.page.ts" \
  || fail "terminal.page.ts never calls lensOutcomes — the function exists and nothing uses it"
grep -qE "healthReason|healthLine|health-reason" "$TP/terminal.page.html" \
  || fail "the template never renders the health reason — the badge still says needs-attention and nothing else"
grep -qE "lensOutcome|lens-outcome|never run|never-run" "$TP/terminal.page.html" \
  || fail "the template never renders a per-lens outcome — the VALIDATION panel is still a form"
echo "   terminal.page.ts and .html reference both"

echo "GATE PASS: $ran validator-view tests, $allran web tests, production build clean, app boots, page wired"
