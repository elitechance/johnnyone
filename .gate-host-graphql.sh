#!/usr/bin/env bash
# Gate: the host's OWN GraphQL AiSession must expose terminalStatus.
#
# Measured after the host fix shipped: `services::sessions` computes the real status,
# but a query to the running host answers
#   Unknown field "terminalStatus" on type "AiSession"
# because `desktop/src-tauri/src/host/mod.rs` declares a separate SimpleObject for
# the host's own GraphQL, and the console can be routed there
# (`queryPreferLocalHost`). The field died one hop short of the client.
#
# OUT OF SCOPE, named so a failure is not mistaken for one: the worker's schema
# (deployed separately), the shells display, and
# kloo_needs_changes_parks_instead_of_alldone_loop, which fails on the untouched
# tree because it reaches real session state.
set -uo pipefail
cd "$(dirname "$0")/desktop/src-tauri" || exit 2
export NO_COLOR=1
fail() { echo "GATE FAIL: $*"; exit 1; }
KNOWN='kloo_needs_changes_parks_instead_of_alldone_loop'
SRC=src/host/mod.rs
BAK=$(mktemp); cp "$SRC" "$BAK"
trap 'cp "$BAK" "$SRC"; rm -f "$BAK"' EXIT

failures() { printf '%s\n' "$1" | sed -n '/^failures:$/,/^$/p' | grep -E '^ +[a-z]' | sed 's/^ *//' | sort -u; }

out=$(cargo test --lib 2>&1)
unexpected=$(failures "$out" | grep -v "$KNOWN" || true)
if [ -n "$unexpected" ]; then
  echo "--- unexpected failures ---"; printf '%s\n' "$unexpected"
  fail "the crate has failures other than the known out-of-scope one"
fi

out2=$(cargo test --lib ai_session_carries_terminal_status 2>&1)
printf '%s\n' "$out2" | grep -qE 'test result: ok\. [1-9]' \
  || { printf '%s\n' "$out2" | tail -8; fail "no passing test named ai_session_carries_terminal_status*"; }

# Mutation: drop the field on the way across. The test MUST fail.
python3 -I - "$SRC" <<'PY'
import re, sys, pathlib
p = pathlib.Path(sys.argv[1]); s = p.read_text()
pat = re.compile(r'terminal_status:\s*(?:Some\()?value\.terminal_status\)?\s*,')
if not pat.search(s):
    sys.stderr.write('MUTATION ANCHOR MISSING: terminal_status: value.terminal_status... in the From impl\n')
    raise SystemExit(3)
p.write_text(pat.sub('terminal_status: None,', s, count=1))
PY
[ $? -eq 0 ] || fail "could not apply the mutation (see above)"
out3=$(cargo test --lib ai_session_carries_terminal_status 2>&1)
printf '%s\n' "$out3" | grep -qE 'test result: FAILED' || {
  printf '%s\n' "$out3" | tail -8
  fail "MUTATION SURVIVED: the From impl can drop terminal_status and the test stays green"
}
echo "killed the mutation (From<Session> drops terminal_status)"
echo "GATE OK — the host's own GraphQL carries the terminal's real state"
