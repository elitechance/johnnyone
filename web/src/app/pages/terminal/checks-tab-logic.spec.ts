import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, it, expect } from 'vitest';
import type { AgentPlan } from '../../../../../ui/src/services/johnny-api.service';
import { visibleConsoleTabs } from './console-tabs-logic';
import {
  checksView,
  ruleLabel,
  RULE_LABELS,
  selectRuleChip,
  windowCheckRows,
  type CheckItem,
} from './checks-tab-logic';

const here = dirname(fileURLToPath(import.meta.url));

function plan(over: Partial<AgentPlan>): AgentPlan {
  return { id: 'p', ...over } as AgentPlan;
}

const SEVEN: CheckItem[] = [
  { rule: 'file_missing', blocking: true, taskId: 'a' },
  { rule: 'file_collision', blocking: true, taskId: 'b' },
  { rule: 'verify_not_scoped', blocking: true, taskId: 'c' },
  { rule: 'verify_not_runnable', blocking: true, taskId: 'd' },
  { rule: 'depends_forward', blocking: true, taskId: 'e' },
  { rule: 'prompt_exceeds_context', blocking: true, taskId: 'f' },
  { rule: 'ui_task_forbidden', blocking: true, taskId: 'g' },
];

describe('visibleConsoleTabs', () => {
  it('commercial planning hides checks and tasks', () => {
    expect(
      visibleConsoleTabs(plan({ initiativeStatus: 'planning' }), false, false, false),
    ).toEqual(['raw', 'plan', 'diff']);
  });

  it('local-small planning shows checks', () => {
    const tabs = visibleConsoleTabs(
      plan({ initiativeStatus: 'planning', executorConfig: '{"mode":"local-small"}' }),
      false,
      false,
      false,
    );
    expect(tabs).toContain('checks');
    expect(tabs).not.toContain('tasks');
  });

  it('local-small development + preflight and no task-run shows checks, hides tasks', () => {
    const tabs = visibleConsoleTabs(
      plan({ initiativeStatus: 'development', executorConfig: '{"mode":"local-small"}' }),
      false,
      false,
      true,
    );
    expect(tabs).toContain('checks');
    expect(tabs).not.toContain('tasks');
  });
});

describe('checksView + RULE_LABELS', () => {
  it('maps 7 blocking items to 7 rows', () => {
    const v = checksView({ passed: false, items: SEVEN });
    expect(v.state).toBe('rejected');
    expect(v.stats.violations).toBe(7);
    expect(v.rows).toHaveLength(7);
  });

  it('renders labels not raw ids for the seven mock-visible rules', () => {
    const v = checksView({ passed: false, items: SEVEN });
    expect(v.rows.map((r) => r.label)).toEqual([
      'file does not exist',
      'two tasks claim one file',
      'verify not scoped',
      'verify not runnable',
      'dependency points forward',
      'prompt exceeds context',
      'UI task forbidden',
    ]);
    expect(v.rows.every((r) => r.label !== r.rule)).toBe(true);
    expect(ruleLabel('file_missing')).toBe('file does not exist');
    expect(RULE_LABELS.ui_task_forbidden).toBe('UI task forbidden');
    expect(RULE_LABELS.replan_amendment).toBe('replan amendment unreadable');
  });

  it('passed empty items is passed with green copy', () => {
    const v = checksView({ passed: true, items: [] });
    expect(v.state).toBe('passed');
    expect(v.rows).toHaveLength(0);
    expect(v.card).toContain('Plan check passed');
    expect(v.card).toContain('Dispatching 3 lenses');
  });

  it('advisory timeout does not flip passed', () => {
    const v = checksView({
      passed: true,
      items: [{ rule: 'verify_timeout', blocking: false }],
    });
    expect(v.state).toBe('passed');
    expect(v.advisories[0].label).toMatch(/timeout/i);
  });

  it('not-executed advisory + executed/skipped stats', () => {
    const v = checksView({
      passed: true,
      verify_executed: 96,
      verify_skipped: 24,
      items: [{ rule: 'verify_not_executed', blocking: false }],
    });
    expect(v.state).toBe('passed');
    expect(v.advisories[0].label).toMatch(/not-executed/i);
    expect(v.stats.executed).toBe(96);
    expect(v.stats.skipped).toBe(24);
  });

  it('null report is empty', () => {
    expect(checksView(null).state).toBe('empty');
  });

  it('preflight first-start / replan / cap / unreadable copy', () => {
    const fail = { passed: false, items: SEVEN };
    expect(checksView(fail, { source: 'preflight' }).card).toContain('Loop not started');
    expect(
      checksView(fail, { source: 'preflight', replanRound: 2, replanCap: 3 }).card,
    ).toContain('replan round 2 of 3');
    expect(checksView(fail, { source: 'preflight', exhausted: true }).card).toContain(
      'Replan cap reached',
    );
    expect(
      checksView(fail, { source: 'preflight', checkKind: 'replan_park', exhausted: false })
        .card,
    ).toContain('amendment record unreadable');
  });
});

describe('windowCheckRows + ruleCounts', () => {
  const many: CheckItem[] = Array.from({ length: 300 }, (_, i) => ({
    rule: i % 3 === 0 ? 'file_missing' : i % 3 === 1 ? 'file_collision' : 'verify_not_scoped',
    blocking: true,
    taskId: `t${i}`,
  }));

  it('300 items → ≤52 data rows + placeholder', () => {
    const w = windowCheckRows(many, 't150');
    expect(w.rows.length).toBeLessThanOrEqual(52);
    expect(w.moreCount).toBeGreaterThanOrEqual(248);
    expect(w.rows.some((r) => r.taskId === 't150')).toBe(true);
  });

  it('ruleCounts sum to 300 and chip click filters', () => {
    const v = checksView({ passed: false, items: many });
    expect(v.ruleCounts.length).toBeGreaterThanOrEqual(3);
    expect(v.ruleCounts.reduce((s, c) => s + c.count, 0)).toBe(300);
    expect(v.ruleCounts.every((c) => c.label !== c.rule || RULE_LABELS[c.rule] === c.rule)).toBe(
      true,
    );
    expect(v.ruleCounts.every((c) => c.label === ruleLabel(c.rule))).toBe(true);
    const id = selectRuleChip(many, 'file_collision', null);
    expect(id).toBeTruthy();
    const w = windowCheckRows(many, id);
    expect(w.rows.some((r) => r.rule === 'file_collision')).toBe(true);
  });
});

describe('wiring — checks tab', () => {
  const html = readFileSync(resolve(here, 'terminal.page.html'), 'utf8');
  const ts = readFileSync(resolve(here, 'terminal.page.ts'), 'utf8');
  it('template mentions checksView and checks tab', () => {
    expect(html + ts).toMatch(/checksView/);
    expect(html).toMatch(/checks/);
  });
});
