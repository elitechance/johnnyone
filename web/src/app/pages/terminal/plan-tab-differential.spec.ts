import { describe, it, expect } from 'vitest';
import { planDocPath } from './plan-tab-logic';
function old(planPath: string | null, sel: string | null): string | null {
  const b = (planPath ?? '').trim().replace(/\/+$/, '');
  if (!b) return null;
  if (sel === 'brief.md') return `${b}/brief.md`;
  if (sel === 'plan.md') return `${b}/plan.md`;
  if (sel === 'overview.md') return `${b}/overview.md`;
  if (sel === 'status.md') return `${b}/status.md`;
  const id = (sel ?? '').trim();
  if (!id) return null;
  if (id.includes('/') || id.includes('\\') || id.includes('..')) return null;
  return `${b}/phases/${id}/overview.md`;
}
const BASE = '/p/plan';
describe('baseline-vs-new differential for the ORIGINAL navigator selections', () => {
  const sels = [
    'brief.md', 'plan.md', 'overview.md', 'status.md',
    'P1', 'P2-runner', '00-backend-run-from-phase', '06-loop-metering',
    'phase with spaces', 'phase.dot', 'phase_under', 'Ph-123',
  ];
  it('is byte-identical for every realistic selection', () => {
    const diffs = sels.filter((s) => old(BASE, s) !== planDocPath(BASE, s)).map((s) => [s, old(BASE, s), planDocPath(BASE, s)]);
    expect(diffs).toEqual([]);
  });
  it('differs ONLY on these (documented) selections', () => {
    const odd = ['artifacts', ' brief.md ', 'brief.md ', 'phases', '..', 'artifacts/x'];
    expect(odd.map((s) => [s, old(BASE, s), planDocPath(BASE, s)])).toEqual([
      ['artifacts', '/p/plan/phases/artifacts/overview.md', '/p/plan/artifacts'],
      [' brief.md ', '/p/plan/phases/brief.md/overview.md', '/p/plan/brief.md'],
      ['brief.md ', '/p/plan/phases/brief.md/overview.md', '/p/plan/brief.md'],
      ['phases', '/p/plan/phases/phases/overview.md', '/p/plan/phases/phases/overview.md'],
      ['..', null, null],
      ['artifacts/x', null, '/p/plan/artifacts/x'],
    ]);
  });
});
