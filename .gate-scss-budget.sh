#!/usr/bin/env bash
# Gate: terminal.page.scss must be held to a BUDGET, not pinned to a byte count.
#
# `console-template.spec.ts` asserts `statSync(SCSS_PATH).size === 74450`. A later
# commit deleted 34 dead rule blocks and the file is now 65776 bytes, so the test
# is RED on main and says "expected 65776 to be 74450" -- reporting a legitimate
# cleanup as a failure while telling nobody that the real constraint (a 48 kB
# compiled-style budget) is still satisfied.
#
# The requirement is the budget. This gate proves the spec enforces it in both
# directions, by mutating the stylesheet:
#   M-GROW    +3 kB of real rules  -> the spec MUST fail
#   M-SHRINK  a legitimate cleanup -> the spec MUST pass
#
# OUT OF SCOPE, named so a failure is not mistaken for one of them: the console's
# loading state, the arena pages, anything in terminal.page.html.
set -uo pipefail
cd "$(dirname "$0")" || exit 2
export NO_COLOR=1
fail() { echo "GATE FAIL: $*"; exit 1; }

SCSS=web/src/app/pages/terminal/terminal.page.scss
SPEC=web/src/app/pages/arena/console-template.spec.ts
BAK=$(mktemp); cp "$SCSS" "$BAK"
trap 'cp "$BAK" "$SCSS"; rm -f "$BAK"' EXIT

run_spec() { npx vitest run --config web/vitest.config.ts "$SPEC" 2>&1; }
spec_failed() { printf '%s\n' "$1" | grep -qc 'Tests .*failed' 2>/dev/null; }

# 1. The whole web suite must be green.
out=$(npx vitest run --config web/vitest.config.ts 2>&1)
if printf '%s\n' "$out" | grep -q 'Tests .*failed'; then
  echo "--- failing tests on the UNMUTATED tree ---"
  printf '%s\n' "$out" | grep -E '^ *(×|FAIL)' | head -20
  fail "the web suite is not green"
fi

# 2. M-GROW: 3 kB of plausible rules. The budget exists because the compiled sheet
#    sits at ~47.5 kB of 48 kB, so growth of this size is exactly what must be caught.
{
  echo ""
  for i in $(seq 1 60); do
    echo ".gate-probe-$i { margin: 0; padding: 0; color: inherit; background: none; }"
  done
} >> "$SCSS"
out=$(run_spec)
printf '%s\n' "$out" | grep -q 'Tests .*failed' || {
  printf '%s\n' "$out" | tail -6
  fail "M-GROW SURVIVED: 3 kB of new rules did not fail the budget spec"
}
echo "killed M-GROW (3 kB of new rules)"
cp "$BAK" "$SCSS"

# 3. M-SHRINK: a legitimate cleanup must NOT be a failure. Drop a trailing block.
python3 -I - "$SCSS" <<'PY'
import sys, pathlib
p = pathlib.Path(sys.argv[1]); s = p.read_text()
cut = s.rstrip()[:-400]          # remove ~400 bytes from the end
p.write_text(cut + "\n")
PY
out=$(run_spec)
if printf '%s\n' "$out" | grep -q 'Tests .*failed'; then
  printf '%s\n' "$out" | grep -E '^ *×' | head -5
  fail "M-SHRINK was reported as a failure: a 400-byte cleanup must not break the budget spec"
fi
echo "survived M-SHRINK (a 400-byte cleanup is not a failure)"

echo "GATE OK — the stylesheet is held to a budget in both directions"
