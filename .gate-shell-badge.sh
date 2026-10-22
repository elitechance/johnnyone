#!/usr/bin/env bash
# Gate: the shells list must badge a terminal by its REAL state, and an ended one
# must not be offered as if you could type into it.
#
# Measured: `kloo-cli` is listed as "attached tmux", its tmux does not exist, and
# three characters typed into it vanished with no feedback. The badge reads
# `attachedTmux` -- an INTENT flag -- because `terminalStatus` never left the host.
# The host now reports it (fix/host commit); this is the rest of the wire plus the
# display.
#
# OUT OF SCOPE, named so a failure is not mistaken for one of them: the console's
# loading state, the build budgets, the terminal-screen composer, and the host.
set -uo pipefail
cd "$(dirname "$0")" || exit 2
export NO_COLOR=1
fail() { echo "GATE FAIL: $*"; exit 1; }

LOGIC=web/src/app/pages/shells/shells-page-logic.ts
HTML=web/src/app/pages/shells/shells.page.html
SCHEMA=worker/schema/johnnyone-ai.graphql
QUERY=ui/src/services/johnny-api.service.ts
BAK=$(mktemp); cp "$LOGIC" "$BAK"
trap 'cp "$BAK" "$LOGIC"; rm -f "$BAK"' EXIT

suite() { npx vitest run --config web/vitest.config.ts 2>&1; }

# 1. The field must exist end to end, or the UI is guessing again.
grep -q 'terminalStatus' "$SCHEMA" || fail "$SCHEMA has no terminalStatus on AiSession"
grep -q 'terminalStatus' "$QUERY"  || fail "$QUERY never asks for terminalStatus"
grep -q 'terminalStatus' ui/src/models/ai-session.model.ts || fail "the AiSession model has no terminalStatus"

# 2. The whole web suite must be green.
out=$(suite)
if printf '%s\n' "$out" | grep -q 'Tests .*failed'; then
  echo "--- failing tests on the UNMUTATED tree ---"
  printf '%s\n' "$out" | grep -E '^ *(×|FAIL)' | head -20
  fail "the web suite is not green"
fi

# 3. The template must render the real state, not just hold it.
grep -q 'shellRowState' "$HTML" || fail "$HTML does not mention shellRowState: the state is not wired to anything"
grep -q 'ended' "$HTML" || fail "$HTML never says anything about an ended terminal"

# 4. Mutation: answer 'attached' for every row. The spec MUST fail.
python3 -I - "$LOGIC" <<'PY'
import re, sys, pathlib
p = pathlib.Path(sys.argv[1]); s = p.read_text()
m = re.search(r'export function shellRowState\([^)]*\)[^{]*\{', s, re.S)
if not m:
    sys.stderr.write('MUTATION ANCHOR MISSING: export function shellRowState(...) {\n')
    raise SystemExit(3)
p.write_text(s[:m.end()] + "\n  return 'attached';\n" + s[m.end():])
PY
[ $? -eq 0 ] || fail "could not apply the mutation (see above)"
out2=$(suite)
printf '%s\n' "$out2" | grep -q 'Tests .*failed' || {
  printf '%s\n' "$out2" | tail -6
  fail "MUTATION SURVIVED: shellRowState can answer 'attached' for every row and the suite stays green"
}
echo "killed the mutation (every row reads 'attached'):"
printf '%s\n' "$out2" | grep -E '^ *×' | head -5

# 5. The ANGULAR BUILD must pass. vitest does not typecheck a template: the first
#    attempt shipped a template calling shellRowState() that the component never
#    exposed, and only `nx build web` saw it — after the deploy had started.
cp "$BAK" "$LOGIC"
build=$(npx nx build web --skip-nx-cache 2>&1)
if printf '%s\n' "$build" | grep -qE 'ERROR|error TS|NG[0-9]+:'; then
  echo "--- build errors ---"
  printf '%s\n' "$build" | grep -E 'ERROR|error TS|NG[0-9]+:' | sed 's/\x1b\[[0-9;]*m//g' | sort -u | head -10
  fail "the Angular build does not compile the template"
fi

echo "GATE OK — the badge follows the terminal's real state, an ended shell says so, and it COMPILES"
