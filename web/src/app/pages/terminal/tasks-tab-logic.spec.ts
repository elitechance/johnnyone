import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, it, expect } from 'vitest';
import {
  initialSelectedId,
  replanBanner,
  selectPlaceholder,
  taskCounts,
  taskTable,
  tasksEmptyCopy,
  verifyOkOf,
  failedBlockedCount,
  windowRows,
  type TaskRowLike,
} from './tasks-tab-logic';

const here = dirname(fileURLToPath(import.meta.url));

function rows(n: number, over: (i: number) => Partial<TaskRowLike> = () => ({})): TaskRowLike[] {
  return Array.from({ length: n }, (_, i) => ({
    id: `t${String(i).padStart(3, '0')}`,
    status: 'pending',
    ...over(i),
  }));
}

describe('windowRows + default selection', () => {
  it('120 rows → ≤52 entries and includes selected', () => {
    const table = taskTable({ tasks: rows(120) });
    const t0 = performance.now();
    const w = windowRows(table, 't060');
    expect(performance.now() - t0).toBeLessThan(50);
    expect(w.rows.length).toBeLessThanOrEqual(51);
    expect(w.rows.some((r) => r.id === 't060')).toBe(true);
  });

  it('failed at 60 + 3 blocked is the default window', () => {
    const tasks = rows(120, (i) => {
      if (i === 60) return { status: 'failed' };
      if (i === 61 || i === 62 || i === 63) return { status: 'blocked', blockedBy: 't060' };
      return {};
    });
    const table = taskTable({ tasks });
    const sel = initialSelectedId(table);
    expect(sel).toBe('t060');
    const w = windowRows(table, undefined);
    expect(w.rows.some((r) => r.id === 't060')).toBe(true);
    expect(w.rows.filter((r) => r.id === 't061' || r.id === 't062' || r.id === 't063').length).toBe(
      3,
    );
  });

  it('selectPlaceholder jumps into the tail range', () => {
    const table = taskTable({ tasks: rows(120) });
    const w = windowRows(table, 't000');
    expect(w.placeholders.length).toBeGreaterThan(0);
    const id = selectPlaceholder(table, w.placeholders[w.placeholders.length - 1]);
    expect(id).toBeTruthy();
    const w2 = windowRows(table, id);
    expect(w2.rows.some((r) => r.id === id)).toBe(true);
  });

  it('emits head and tail placeholders when the window is mid-list', () => {
    const table = taskTable({ tasks: rows(120, (i) => (i === 60 ? { status: 'failed' } : {})) });
    const w = windowRows(table, undefined);
    expect(w.placeholders.length).toBe(2);
    expect(w.placeholders[0].from).toBe(1);
    expect(w.placeholders[1].to).toBe(120);
  });
});

describe('counts / empty / banner', () => {
  it('120 all-done counts', () => {
    const c = taskCounts({ tasks: rows(120, () => ({ status: 'done' })) });
    expect(c).toEqual({ done: 120, running: 0, failed: 0, blocked: 0, pending: 0 });
  });

  it('empty copy, not a table', () => {
    expect(tasksEmptyCopy({ tasks: [] })).toBe('No tasks in this phase yet');
    expect(tasksEmptyCopy(null, 'invalid tasks.json')).toBe("Could not read this phase's task run");
  });

  it('verify column follows last attempt checks, not row status', () => {
    expect(verifyOkOf({ checks: { failed: [{ check: 'scope' }] } })).toBe('ok');
    expect(verifyOkOf({ checks: { failed: [{ check: 'verify' }] } })).toBe('fail');
    expect(verifyOkOf({ verify: { passed: true } })).toBe('ok');
    expect(verifyOkOf(undefined)).toBe('');
    const table = taskTable({
      tasks: [
        {
          id: 't1',
          status: 'failed',
          attempts: [{ checks: { failed: [{ check: 'must_contain' }] } }],
        },
      ],
    });
    expect(table[0].verifyOk).toBe('ok');
  });

  it('failedBlockedCount prefers dependents, then blockedBy', () => {
    expect(
      failedBlockedCount('t1', [{ id: 't2', dependsOn: ['t1'] }, { id: 't3', depends_on: ['t1'] }], []),
    ).toBe(2);
    expect(
      failedBlockedCount('t1', [], [
        { blockedBy: 't1' },
        { blockedBy: 't1' },
        { blockedBy: 'x' },
      ]),
    ).toBe(2);
    expect(failedBlockedCount(null, [], [])).toBe(0);
  });

  it('replan banners', () => {
    expect(replanBanner('phase_replan_running')).toBe(
      'Re-planning — planner is amending routed tasks',
    );
    expect(replanBanner('needs_attention', { replanExhausted: true })).toBe(
      'Replan cap reached — still failing',
    );
    expect(replanBanner('phase_worker_running')).toBeNull();
  });
});

describe('wiring — tasks tab', () => {
  const html = readFileSync(resolve(here, 'terminal.page.html'), 'utf8');
  const ts = readFileSync(resolve(here, 'terminal.page.ts'), 'utf8');
  it('mentions taskTable / windowRows / tasks tab', () => {
    expect(html + ts).toMatch(/taskTable|windowRows/);
    expect(html).toMatch(/tasks/);
    expect(html).toMatch(/onTaskPlaceholder/);
    expect(html).toMatch(/failedBlockedCount/);
    expect(ts).toMatch(/onTaskPlaceholder[\s\S]*onSelectTask/);
    expect(ts).toMatch(/onJumpFailed[\s\S]*onSelectTask/);
    expect(ts).toMatch(/lastAttemptJson\.set\(null\)/);
  });
});
