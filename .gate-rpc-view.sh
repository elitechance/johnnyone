#!/usr/bin/env bash
# Gate: the WHOLE wire must carry terminalStatus, not four fifths of it.
#
# Measured after deploying: the worker accepts the field and answers
#   terminalStatus=None   for every session
# because `RpcSessionView` — the DTO the host serialises for the relay — never
# had the field, so `list_sessions` RPC JSON does not contain it. That is the
# FIFTH hop in this chain, and I found the previous four one at a time. This gate
# checks all of them at once so there is no sixth surprise.
#
# KNOWN OUT-OF-SCOPE FAILURE: kloo_needs_changes_parks_instead_of_alldone_loop
# (fails on the untouched tree; it reaches real session state).
set -uo pipefail
cd "$(dirname "$0")" || exit 2
export NO_COLOR=1
fail() { echo "GATE FAIL: $*"; exit 1; }
KNOWN='kloo_needs_changes_parks_instead_of_alldone_loop'
SRC=desktop/src-tauri/src/agent/mod.rs
BAK=$(mktemp); cp "$SRC" "$BAK"
trap 'cp "$BAK" "$SRC"; rm -f "$BAK"' EXIT

# 1. Every hop must mention it. Named in order so a failure says WHICH one broke.
check() { grep -q "$2" "$1" || fail "hop missing: $1 does not carry $2"; }
check desktop/src-tauri/src/db/models.rs                 terminal_status
check desktop/src-tauri/src/services/sessions.rs         terminal_status
check desktop/src-tauri/src/agent/message_types.rs       terminal_status
check desktop/src-tauri/src/agent/mod.rs                 terminal_status
check desktop/src-tauri/src/host/mod.rs                  terminal_status
check worker/schema/johnnyone-ai.graphql                 terminalStatus
check ui/src/models/ai-session.model.ts                  terminalStatus
check ui/src/services/johnny-api.service.ts              terminalStatus
check web/src/app/pages/shells/shells-page-logic.ts      terminalStatus

# 2. The crate must be green apart from the known failure.
failures() { printf '%s\n' "$1" | sed -n '/^failures:$/,/^$/p' | grep -E '^ +[a-z]' | sed 's/^ *//' | sort -u; }
out=$(cd desktop/src-tauri && cargo test --lib 2>&1)
unexpected=$(failures "$out" | grep -v "$KNOWN" || true)
[ -z "$unexpected" ] || { echo "--- unexpected failures ---"; printf '%s\n' "$unexpected"; fail "the crate has other failures"; }

# 3. A test must name the RPC view's carrying of the field.
out2=$(cd desktop/src-tauri && cargo test --lib session_view_carries_terminal_status 2>&1)
printf '%s\n' "$out2" | grep -qE 'test result: ok\. [1-9]' \
  || { printf '%s\n' "$out2" | tail -8; fail "no passing test named session_view_carries_terminal_status*"; }

# 4. Mutation: drop it in session_view. That test MUST fail.
python3 -I - "$SRC" <<'PY'
import re, sys, pathlib
p = pathlib.Path(sys.argv[1]); s = p.read_text()
pat = re.compile(r'terminal_status:\s*session\.terminal_status\s*,')
if not pat.search(s):
    sys.stderr.write('MUTATION ANCHOR MISSING: terminal_status: session.terminal_status, in session_view\n')
    raise SystemExit(3)
p.write_text(pat.sub('terminal_status: String::new(),', s, count=1))
PY
[ $? -eq 0 ] || fail "could not apply the mutation (see above)"
out3=$(cd desktop/src-tauri && cargo test --lib session_view_carries_terminal_status 2>&1)
printf '%s\n' "$out3" | grep -qE 'test result: FAILED' || {
  printf '%s\n' "$out3" | tail -8
  fail "MUTATION SURVIVED: session_view can blank terminal_status and the test stays green"
}
echo "killed the mutation (session_view blanks terminal_status)"
echo "GATE OK — every hop from the DB row to the shells list carries the terminal's real state"
