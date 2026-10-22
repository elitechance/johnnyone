#!/usr/bin/env bash
# Gate: the console must not say "No initiatives yet" when it is still loading, and
# must not say it when the call FAILED either.
#
# Measured on johnnyone.pages.dev: navigating to /initiatives shows
#   "No initiatives yet - Create an initiative to start briefing, planning, and
#    development"  + a New initiative button
# for the first ~2 seconds, after which 33 initiatives appear. `loadInitiatives()`
# starts with an empty list and its `catch` sets an empty list too, so one screen
# stands for three different situations and shows the wrong one most of the time.
#
# OUT OF SCOPE, named so a failure here is not mistaken for one of them: the arena
# pages, the events column, the validation panel, the status bar, terminal.page.scss.
set -uo pipefail
cd "$(dirname "$0")" || exit 2
export NO_COLOR=1
fail() { echo "GATE FAIL: $*"; exit 1; }

LOGIC=web/src/app/pages/terminal/console-logic.ts
HTML=web/src/app/pages/terminal/terminal.page.html
BAK=$(mktemp); cp "$LOGIC" "$BAK"
trap 'cp "$BAK" "$LOGIC"; rm -f "$BAK"' EXIT

suite() { npx vitest run --config web/vitest.config.ts 2>&1; }

# 1. The whole web suite must be green.
out=$(suite)
if printf '%s\n' "$out" | grep -q 'Tests .*failed'; then
  echo "--- failing tests on the UNMUTATED tree ---"
  printf '%s\n' "$out" | grep -E '^ *(×|FAIL)' | head -20
  fail "the web suite is not green"
fi

# 2. The template must actually branch on the state -- a pure function nothing renders
#    changes nothing on screen.
for needle in "consolePaneState" "'loading'" "'failed'"; do
  grep -qF "$needle" "$HTML" || fail "the template does not mention $needle: the state is not wired to anything"
done

# 3. Mutation: make the state function answer 'console' for everything. The spec that
#    pins the states MUST fail -- otherwise it is not testing the decision.
python3 -I - "$LOGIC" <<'PY'
import re, sys, pathlib
p = pathlib.Path(sys.argv[1]); s = p.read_text()
m = re.search(r'export function consolePaneState\([^)]*\)[^{]*\{', s)
if not m:
    sys.stderr.write('MUTATION ANCHOR MISSING: export function consolePaneState(...) {\n')
    raise SystemExit(3)
p.write_text(s[:m.end()] + "\n  return 'console';\n" + s[m.end():])
PY
[ $? -eq 0 ] || fail "could not apply the mutation (see above)"
out=$(suite)
printf '%s\n' "$out" | grep -q 'Tests .*failed' || {
  printf '%s\n' "$out" | tail -6
  fail "MUTATION SURVIVED: consolePaneState can answer 'console' for every input and the suite stays green"
}
echo "killed the mutation (consolePaneState always answers 'console'):"
printf '%s\n' "$out" | grep -E '^ *×' | head -5

echo "GATE OK — loading, failed and empty are distinct states, and the template renders them"
