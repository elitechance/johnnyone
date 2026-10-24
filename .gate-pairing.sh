#!/usr/bin/env bash
# Every AI resolver must be reachable: a resolver file with no schema field is
# dead code the API cannot call.
#
# THE DEFECT THIS PINS, twice over. On 2026-10-24 `unblock-agent-plan.ts` was
# written, the host RPC was implemented and dispatched, the Rust side went green
# with 6 tests and 4/4 mutations killed — and the mutation was unreachable,
# because nothing ever added `unblockAgentPlan` to
# worker/schema/johnnyone-ai.graphql. I noticed only when I went to use it. The
# same thing nearly happened again with `updateAgentPlanDevProviders`; that one
# has its line because its gate checked for it by name.
#
# Checking for ONE name by name is not the fix — the next resolver will be the
# third instance. Resolvers here are discovered by convention (worker.yaml points
# at `resolvers/`, and the default export's name is the field), so the pairing is
# exactly a filename-to-schema-field check, and it can be made total.
set -uo pipefail
cd "$(dirname "$0")" || exit 1
SCHEMA=worker/schema/johnnyone-ai.graphql
DIR=worker/resolvers/ai
fail() { echo; echo "GATE FAILED — $*"; exit 1; }

[ -f "$SCHEMA" ] || fail "$SCHEMA not found"
[ -d "$DIR" ]    || fail "$DIR not found"

missing=()
checked=0
for f in "$DIR"/*.ts; do
  base=$(basename "$f" .ts)
  # kebab-case filename -> camelCase field, the convention the loader uses.
  field=$(printf '%s' "$base" | awk -F- '{printf "%s", $1; for (i=2; i<=NF; i++) printf "%s%s", toupper(substr($i,1,1)), substr($i,2)}')
  # The field must be DECLARED in the schema: start of a line (inside Query or
  # Mutation), followed by `(` or `:`. A mere mention in a comment is not a
  # declaration, which is the whole point.
  if grep -qE "^[[:space:]]+${field}[(:]" "$SCHEMA"; then
    checked=$((checked + 1))
  else
    missing+=("$base.ts -> $field")
  fi
done

echo "AI resolvers paired with a schema field: $checked"
if [ ${#missing[@]} -gt 0 ]; then
  echo
  echo "resolvers the API cannot reach:"
  for m in "${missing[@]}"; do echo "  $m"; done
  fail "${#missing[@]} resolver(s) have no field in $SCHEMA. A resolver with no schema line is dead code: the desktop RPC behind it is implemented, tested and unreachable."
fi

# The control arm: a check that passes because it found NOTHING is worthless.
# This directory has resolvers; if the loop ran zero times the glob is wrong.
[ "$checked" -ge 10 ] || fail "only $checked resolver(s) were checked — the glob matched almost nothing, so a green here would mean nothing. An absence assertion is satisfied by nothing at all."

echo
echo "GATE PASSED — all $checked AI resolvers are declared in the schema."
