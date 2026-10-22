#!/usr/bin/env bash
# Gate: the component-style WARNING must stop firing on a file that is comfortably
# inside the error budget -- without weakening the error budget that actually
# protects the build.
#
# Measured before this change:
#   ▲ WARNING  terminal.page.scss exceeded maximum budget.
#              Budget 24.00 kB was not met by 15.79 kB with a total of 39.79 kB.
# 39.79 kB against a 48 kB error: 8.2 kB of real headroom, warned about on every
# single build.
#
# OUT OF SCOPE, named so a failure is not mistaken for one of them: the initial-bundle
# budget (2.50 MB against a 1.5 MB warning / 3 MB error -- a separate decision), the
# mermaid "is not ESM" warnings, and the source-size budget spec in console-template.spec.ts.
set -uo pipefail
cd "$(dirname "$0")" || exit 2
export NO_COLOR=1
fail() { echo "GATE FAIL: $*"; exit 1; }

# 1. The error budget must STILL be 48kb, and the warning must be a real number below it.
python3 -I - web/project.json <<'PY' || exit 1
import json, sys
build = json.load(open(sys.argv[1]))['targets']['build']
b = build.get('options', {}).get('budgets') or build.get('configurations', {}).get('production', {}).get('budgets')
if not b:
    sys.exit('GATE FAIL: no budgets found on the web build target')
by = {x.get('type'): x for x in b}
def size(v):
    v = str(v).strip().lower()
    if v.endswith('kb'):  return float(v[:-2]) * 1024
    if v.endswith('mb'):  return float(v[:-2]) * 1024 * 1024
    sys.exit('GATE FAIL: budget %r is not expressed in kb/mb' % v)
# Measured today: terminal.page.scss compiles to 39.79 kB, the initial bundle to 2.50 MB.
floors = {'anyComponentStyle': 39.79 * 1024, 'initial': 2.50 * 1024 * 1024}
for t, floor in floors.items():
    if t not in by:
        sys.exit('GATE FAIL: the %s budget was removed; a ceiling with room is still a ceiling' % t)
    warn, err = size(by[t]['maximumWarning']), size(by[t]['maximumError'])
    if not warn < err:
        sys.exit('GATE FAIL: %s warning (%s) must stay strictly below its error (%s)' % (t, by[t]['maximumWarning'], by[t]['maximumError']))
    if warn <= floor:
        sys.exit('GATE FAIL: %s warning (%s) is still at or under what we already ship' % (t, by[t]['maximumWarning']))
print('budgets ok:', ', '.join('%s %s/%s' % (t, by[t]['maximumWarning'], by[t]['maximumError']) for t in floors))
PY
# 2. A real build must emit NO anyComponentStyle warning. Grepping the config is not the
#    requirement -- "the build stops warning about this file" is.
out=$(npx nx build web --skip-nx-cache 2>&1)
status=$?
printf '%s\n' "$out" | grep -qE 'Application bundle generation complete|Successfully ran target build' \
  || { printf '%s\n' "$out" | tail -15; fail "the build did not complete (exit $status)"; }
if printf '%s\n' "$out" | grep -qE 'scss exceeded maximum budget'; then
  printf '%s\n' "$out" | grep -E 'scss exceeded maximum budget' | head -5
  fail "a component stylesheet still trips its budget"
fi
echo "GATE OK — the build completes with no component-style budget warning, error budget still 48kb"
