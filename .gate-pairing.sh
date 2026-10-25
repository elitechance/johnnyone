#!/usr/bin/env bash
# Every resolver must be reachable, and reachable under the RIGHT operation type.
#
# THE DEFECT THIS PINS, three times over.
#
# 1. 2026-10-24: `unblock-agent-plan.ts` was written, the host RPC implemented
#    and dispatched, the Rust side green with 6 tests and 4/4 mutations killed —
#    and the mutation was unreachable, because nothing added `unblockAgentPlan`
#    to the schema.
# 2. The same thing nearly happened to `updateAgentPlanDevProviders`; that one
#    had its line only because its gate checked for it BY NAME.
# 3. 2026-10-25: this gate's own total check reported **89/89 green** and the
#    deploy then refused the worker anyway:
#
#      ✗ Missing resolver: Mutation.unblockAgentPlan
#      ✗ Orphan resolver: resolvers/ai/unblock-agent-plan.ts (no match: Query.unblockAgentPlan)
#
#    Both lines are the same file. The old gate asked "is this field name
#    declared somewhere in the schema?" and the answer was yes — in the Mutation
#    block, while the loader classified the FILE as a Query because `unblock-`
#    was in none of its prefix tables. A total check that is total in the wrong
#    dimension is still a partial check, and it bought a green that cost a
#    failed deploy.
#
# So this gate no longer reimplements the pairing rule. It DELEGATES to the
# loader that actually wires the worker — `lokal cf worker validate` runs the
# same `inferFieldType` + `validateSchemaResolvers` the deploy does, so a green
# here means the deploy agrees, by construction rather than by resemblance.
# Reimplementing an authority's rule in bash is how dimension 3 got missed.
set -uo pipefail
cd "$(dirname "$0")" || exit 1
fail() { echo; echo "GATE FAILED — $*"; exit 1; }

OUT=$(timeout 300 lokal cf worker validate --env prod 2>&1)
rc=$?
echo "$OUT"

[ "$rc" -eq 0 ] || fail "lokal cf worker validate exited $rc. A resolver with no schema field (or one declared under the wrong operation type) is dead code: the desktop RPC behind it is implemented, tested and unreachable."

# Don't trust exit 0 alone — name the success line, so a validator that starts
# short-circuiting (or a flag rename that makes it a no-op) cannot read as a pass.
grep -q "All schema fields have matching resolvers" <<<"$OUT" \
  || fail "validate exited 0 but never printed its success line — it did not actually run the pairing check."

# The control arm: a check that passes because it found NOTHING is worthless.
counts=$(grep -oE "Resolvers: [0-9]+, Schema fields: [0-9]+" <<<"$OUT") \
  || fail "validate printed no counts, so a green here would mean nothing."
r=$(sed -E 's/.*Resolvers: ([0-9]+).*/\1/' <<<"$counts")
s=$(sed -E 's/.*Schema fields: ([0-9]+).*/\1/' <<<"$counts")
[ "$r" -ge 100 ] || fail "only $r resolver(s) were discovered — the worker dir resolved to almost nothing. An absence assertion is satisfied by nothing at all."
[ "$r" -eq "$s" ] || fail "$r resolvers vs $s schema fields: the two sides are not the same size even though validate passed."

echo
echo "GATE PASSED — $r resolvers paired with $s schema fields, under the operation type the loader infers."
