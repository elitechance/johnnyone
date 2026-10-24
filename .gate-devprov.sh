#!/usr/bin/env bash
# G4 — creation and execution must agree about the development-stage providers.
#
# WHAT THIS GATE IS FOR. On 2026-10-24 at 14:02:40 the kart plan passed its glm
# planning review and then could not hand off, because `dev_worker_provider =
# 'kloo'` — a value creation accepted without checking and the run path refuses.
# The specification is desktop/src-tauri/tests/dev_stage_providers.rs; read its
# header before changing anything.
#
# DESIGN NOTES, from the gates on the four branches before this one:
#   * counts come from libtest's own `test result:` line with --color never, and
#     every stage prints the test NAMES it ran. A gate that greps a summary
#     reports a parse miss as a code failure.
#   * CARGO_TARGET_DIR points at the real disk. /tmp here is a 30 GB tmpfs and
#     seven verify attempts died on `Disk quota exceeded (os error 122)` on
#     2026-10-23 — a red the worker could not fix by writing code.
#   * stage 2 EXCLUDES one test by name, and says why. It is not a licence to
#     exclude more.
#   * stage 3 checks the wire, not just the module. Four defects on these
#     branches were a correct helper nothing called.
set -uo pipefail

cd "$(dirname "$0")/desktop/src-tauri" || exit 1
export CARGO_TARGET_DIR=/home/creepy/.cache/j1-cargo/devprov
export CARGO_TERM_COLOR=never
ROOT="$(cd ../.. && pwd)"
fail() { echo; echo "GATE FAILED — $*"; exit 1; }

# The one test stage 2 does not run, and the reason.
#
#   services::agent_plans::replan_tests::kloo_needs_changes_parks_instead_of_alldone_loop
#
# It spawns a real agent CLI into a fresh temp workspace and dies on
# "agent CLI is waiting on a folder-trust prompt and cannot receive its task"
# (agent_plans.rs:18010). That is this machine's ~/.codex trust state, not the
# code under change — no edit to the provider columns can turn it green. It is
# red on this branch's base commit too. OUT OF SCOPE: do not try to fix it, and
# do not add anything else to this list.
SKIP=kloo_needs_changes_parks_instead_of_alldone_loop

count_of() { sed -n 's/^test result:.*[^0-9]\([0-9]\+\) passed.*/\1/p' "$1" | tail -1; }
names_of() { sed -n 's/^test \([a-z0-9_:]*\) \.\.\. ok$/  \1/p' "$1"; }

# ---------------------------------------------------------------- stage 1
echo "== stage 1/4 — the specification (tests/dev_stage_providers.rs) =="
L=$(mktemp) ; cargo test --test dev_stage_providers -- --test-threads=1 >"$L" 2>&1
S1=$(count_of "$L")
grep -q "^test result: ok\." "$L" || { tail -40 "$L"; fail "the specification is red (see above). This is the gate; it is not optional."; }
names_of "$L"
[ "${S1:-0}" -ge 6 ] || fail "stage 1 reported ${S1:-0} passing tests, expected at least 6 (d1..d6). A deleted or renamed test is not a pass."
echo "stage 1 OK — $S1 spec tests"

# ---------------------------------------------------------------- stage 2
echo
echo "== stage 2/4 — the existing agent_plans unit tests still pass =="
L2=$(mktemp) ; cargo test --lib -- services::agent_plans --skip "$SKIP" >"$L2" 2>&1
S2=$(count_of "$L2")
grep -q "^test result: ok\." "$L2" || { grep -A 6 "^---- " "$L2" | head -60; fail "a pre-existing agent_plans test broke. The provider rule is a write-path check; nothing downstream of it should move."; }
[ "${S2:-0}" -ge 254 ] || fail "stage 2 reported ${S2:-0} passing tests, expected at least 254. Tests were removed or silenced."
echo "stage 2 OK — $S2 agent_plans tests (1 skipped, named above)"

# ---------------------------------------------------------------- stage 3
echo
echo "== stage 3/4 — the repair reaches the wire =="
# A service function nothing calls repairs nothing. The live kart row is only
# reachable through the worker's GraphQL, so the mutation must exist in the
# schema, have a resolver, and be dispatched by the host's RPC table.
SCHEMA="$ROOT/worker/schema/johnnyone-ai.graphql"
grep -q "updateAgentPlanDevProviders" "$SCHEMA" \
  || fail "worker/schema/johnnyone-ai.graphql declares no updateAgentPlanDevProviders mutation. G3 shipped a resolver with no schema line on 2026-10-24 and the mutation was unreachable; do not repeat it."
ls "$ROOT"/worker/resolvers/ai/update-agent-plan-dev-providers.ts >/dev/null 2>&1 \
  || fail "worker/resolvers/ai/update-agent-plan-dev-providers.ts is missing."
grep -q "rebind_development_providers" "$ROOT/desktop/src-tauri/src/agent/mod.rs" \
  || fail "desktop/src-tauri/src/agent/mod.rs does not dispatch the rebind over the relay, so the worker's mutation has nothing to reach."
grep -q "update_agent_plan_dev_providers" "$ROOT/desktop/src-tauri/src/agent/mod.rs" \
  || fail "the RPC method name update_agent_plan_dev_providers is not registered in agent/mod.rs's dispatch table."
echo "stage 3 OK — schema line, resolver file, and RPC dispatch all present"

# ---------------------------------------------------------------- stage 4
echo
echo "== stage 4/4 — the crate still builds =="
cargo check --lib >/dev/null 2>&1 || { cargo check --lib 2>&1 | grep -E "^error" | head -20; fail "cargo check --lib is red."; }
echo "stage 4 OK"

echo
echo "GATE PASSED — $S1 spec tests, $S2 agent_plans tests, wire present, crate builds."
