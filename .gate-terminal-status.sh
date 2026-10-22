#!/usr/bin/env bash
# Gate: the host must report a terminal's REAL state, so a shell whose tmux is gone
# cannot be listed as attached.
#
# Measured: session 00d2bdc0 ("kloo-cli") is status=active, terminal_status=detached,
# tmux_session_name=kloo-cli -- and no `kloo-cli` tmux session exists on this machine.
# The client was handed none of that: `list_sessions` does not even SELECT
# terminal_status, so the UI badges the row from `attached_tmux`, an INTENT flag, and
# three keystrokes typed into it vanished with no feedback.
#
# KNOWN OUT-OF-SCOPE FAILURE, named so it is not mistaken for this work:
#   services::agent_plans::replan_tests::kloo_needs_changes_parks_instead_of_alldone_loop
# It fails on the untouched tree too -- it reaches real session state and trips the
# folder-trust check. Not this change's business.
set -uo pipefail
cd "$(dirname "$0")/desktop/src-tauri" || exit 2
export NO_COLOR=1
fail() { echo "GATE FAIL: $*"; exit 1; }

KNOWN='kloo_needs_changes_parks_instead_of_alldone_loop'
SRC=src/services/sessions.rs
BAK=$(mktemp); cp "$SRC" "$BAK"
trap 'cp "$BAK" "$SRC"; rm -f "$BAK"' EXIT

failures() { # $1 = cargo output -> prints failing test names, one per line
  printf '%s\n' "$1" | sed -n '/^failures:$/,/^$/p' | grep -E '^ +[a-z]' | sed 's/^ *//' | sort -u
}

# 1. The suite, minus the known-bad test, must be green.
out=$(cargo test --lib 2>&1)
unexpected=$(failures "$out" | grep -v "$KNOWN" || true)
if [ -n "$unexpected" ]; then
  echo "--- unexpected failures ---"; printf '%s\n' "$unexpected"
  fail "the crate has failures other than the known out-of-scope one"
fi

# 2. The new decision must be covered by tests that NAME it.
out2=$(cargo test --lib reported_terminal_status 2>&1)
printf '%s\n' "$out2" | grep -qE 'test result: ok\. [1-9]' \
  || { printf '%s\n' "$out2" | tail -8; fail "no passing tests matching reported_terminal_status"; }

# 3. The list query must actually carry the column, or the client still cannot know.
grep -q 'terminal_status' "$SRC" || fail "$SRC never mentions terminal_status"

# 4. Mutation: answer "attached" for every input. The new tests MUST fail.
python3 -I - "$SRC" <<'PY'
import re, sys, pathlib
p = pathlib.Path(sys.argv[1]); s = p.read_text()
m = re.search(r'pub fn reported_terminal_status\([^)]*\)[^{]*\{', s, re.S)
if not m:
    sys.stderr.write('MUTATION ANCHOR MISSING: pub fn reported_terminal_status(...) {\n')
    raise SystemExit(3)
p.write_text(s[:m.end()] + '\n    return "attached";\n' + s[m.end():])
PY
[ $? -eq 0 ] || fail "could not apply the mutation (see above)"
out3=$(cargo test --lib reported_terminal_status 2>&1)
printf '%s\n' "$out3" | grep -qE 'test result: FAILED' || {
  printf '%s\n' "$out3" | tail -8
  fail "MUTATION SURVIVED: reported_terminal_status can answer \"attached\" for everything and the tests stay green"
}
echo "killed the mutation (always \"attached\"):"
failures "$out3" | head -5

echo "GATE OK — the host reports a real terminal status and the tests notice when it lies"
