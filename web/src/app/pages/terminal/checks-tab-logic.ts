// Pure Checks-tab seam (S3 + S11). Angular/Ionic/DOM-free.

export const RULE_LABELS: Record<string, string> = {
  file_missing: 'file does not exist',
  missing_files: 'files missing from the task',
  empty_verify: 'empty verify',
  empty_prompt: 'empty prompt',
  file_collision: 'two tasks claim one file',
  depends_unresolved: 'dependency unresolved',
  depends_cycle: 'dependency cycle',
  depends_forward: 'dependency points forward',
  depends_self: 'task depends on itself',
  verify_not_allowlisted: 'verify not allowlisted',
  verify_not_scoped: 'verify not scoped',
  verify_target_missing: 'verify target missing',
  verify_cwd_missing: 'verify cwd missing',
  files_outside_cwd: 'files outside cwd',
  verify_not_runnable: 'verify not runnable',
  must_contain_trivial: 'must_contain is trivial',
  verify_timeout: 'verify timed out',
  verify_not_executed: 'verify not executed',
  prompt_exceeds_context: 'prompt exceeds context',
  tokens_unavailable: 'tokens unavailable',
  task_count_phase: 'too many tasks in phase',
  task_count_total: 'too many tasks in plan',
  empty_plan: 'plan has no tasks',
  ui_task_forbidden: 'UI task forbidden',
  replan_amendment: 'replan amendment unreadable',
};

export function ruleLabel(rule: string | null | undefined): string {
  const id = (rule ?? '').trim();
  if (!id) return '';
  return RULE_LABELS[id] ?? id.replaceAll('_', ' ');
}

export interface CheckItem {
  taskId?: string | null;
  task_id?: string | null;
  rule: string;
  detail?: string;
  blocking?: boolean;
}

export interface PlanCheckReportLike {
  passed?: boolean;
  items?: CheckItem[];
  tasksChecked?: number;
  tasks_checked?: number;
  verifyExecuted?: number;
  verify_executed?: number;
  verifySkipped?: number;
  verify_skipped?: number;
  phases?: {
    phaseId?: string;
    phase_id?: string;
    executed?: number;
    skipped?: number;
    verifyExecuted?: number;
    verify_executed?: number;
    verifySkipped?: number;
    verify_skipped?: number;
  }[];
  checkKind?: string;
  check_kind?: string;
  replanRound?: number;
  replan_round?: number;
  exhausted?: boolean;
}

export interface ChecksCtx {
  source?: 'planning' | 'preflight';
  replanRound?: number;
  replanCap?: number;
  exhausted?: boolean;
  checkKind?: string;
  selectedId?: string | null;
  selectedRule?: string | null;
}

export interface RuleCount {
  rule: string;
  label: string;
  count: number;
}

export interface CheckRow {
  id: string;
  taskId: string;
  rule: string;
  label: string;
  detail: string;
  blocking: boolean;
}

export interface ChecksView {
  state: 'empty' | 'rejected' | 'passed';
  card: string;
  stats: {
    tasks: number;
    violations: number;
    affected: number;
    executed: number;
    skipped: number;
  };
  rows: CheckRow[];
  advisories: { rule: string; label: string; detail: string }[];
  moreCount: number;
  ruleCounts: RuleCount[];
  phaseStats: { phaseId: string; executed: number; skipped: number }[];
}

export function windowCheckRows(
  items: CheckItem[],
  selected: string | null | undefined,
  radius = 25,
): { rows: CheckRow[]; moreCount: number } {
  const mapped = items.map((it, i) => toRow(it, i));
  if (mapped.length <= 52) return { rows: mapped, moreCount: 0 };
  const sel = selected
    ? mapped.findIndex((r) => r.id === selected || r.taskId === selected || r.rule === selected)
    : 0;
  const idx = sel < 0 ? 0 : sel;
  const start = Math.max(0, idx - radius);
  const end = Math.min(mapped.length, start + 51);
  const slice = mapped.slice(start, end);
  return { rows: slice, moreCount: mapped.length - slice.length };
}

function toRow(it: CheckItem, i: number): CheckRow {
  const taskId = (it.taskId ?? it.task_id ?? '') || `item-${i}`;
  return {
    id: `${taskId}:${it.rule}:${i}`,
    taskId: String(it.taskId ?? it.task_id ?? ''),
    rule: it.rule,
    label: ruleLabel(it.rule),
    detail: it.detail ?? '',
    blocking: it.blocking !== false,
  };
}

export function ruleCountsOf(items: CheckItem[]): RuleCount[] {
  const blocking = items.filter((i) => i.blocking !== false);
  const map = new Map<string, number>();
  for (const it of blocking) {
    map.set(it.rule, (map.get(it.rule) ?? 0) + 1);
  }
  return [...map.entries()]
    .map(([rule, count]) => ({ rule, label: ruleLabel(rule), count }))
    .sort((a, b) => b.count - a.count || a.label.localeCompare(b.label));
}

export function selectRuleChip(
  items: CheckItem[],
  rule: string | null,
  active: string | null,
): string | null {
  if (!rule || rule === active) return null;
  const first = items.findIndex((i) => i.rule === rule && i.blocking !== false);
  if (first < 0) return null;
  return toRow(items[first], first).id;
}

function num(...vals: (number | undefined)[]): number {
  for (const v of vals) {
    if (typeof v === 'number' && Number.isFinite(v)) return v;
  }
  return 0;
}

export function checksView(
  report: PlanCheckReportLike | null | undefined,
  ctx?: ChecksCtx,
): ChecksView {
  if (!report) {
    return {
      state: 'empty',
      card: 'No plan-check yet',
      stats: { tasks: 0, violations: 0, affected: 0, executed: 0, skipped: 0 },
      rows: [],
      advisories: [],
      moreCount: 0,
      ruleCounts: [],
      phaseStats: [],
    };
  }
  const items = report.items ?? [];
  const blocking = items.filter((i) => i.blocking !== false);
  const advisories = items
    .filter((i) => i.blocking === false)
    .map((i) => ({
      rule: i.rule,
      label:
        i.rule === 'verify_timeout'
          ? 'timeout'
          : i.rule === 'verify_not_executed'
            ? 'not-executed'
            : ruleLabel(i.rule),
      detail: i.detail ?? '',
    }));
  const executed = num(report.verifyExecuted, report.verify_executed);
  const skipped = num(report.verifySkipped, report.verify_skipped);
  const tasks = num(report.tasksChecked, report.tasks_checked, items.length);
  const affected = new Set(blocking.map((i) => String(i.taskId ?? i.task_id ?? '')).filter(Boolean)).size;
  const selectedRule = ctx?.selectedRule ?? null;
  const visible = selectedRule ? blocking.filter((i) => i.rule === selectedRule) : blocking;
  const win = windowCheckRows(visible, ctx?.selectedId);
  const phaseStats = (report.phases ?? []).map((p) => ({
    phaseId: String(p.phaseId ?? p.phase_id ?? ''),
    executed: num(p.verifyExecuted, p.verify_executed, p.executed),
    skipped: num(p.verifySkipped, p.verify_skipped, p.skipped),
  }));
  const counts = ruleCountsOf(blocking);
  const passed = report.passed === true || blocking.length === 0;
  const source = ctx?.source ?? 'planning';
  const exhausted = ctx?.exhausted === true || report.exhausted === true;
  const checkKind = ctx?.checkKind ?? report.checkKind ?? report.check_kind ?? '';
  const round = ctx?.replanRound ?? report.replanRound ?? report.replan_round;
  const cap = ctx?.replanCap ?? 3;

  let card: string;
  let state: ChecksView['state'];
  if (passed) {
    state = 'passed';
    if (source === 'preflight') {
      card =
        round || checkKind === 'post_replan'
          ? 'Preflight passed — 0 violations. Resuming the loop.'
          : 'Preflight passed — 0 violations. Starting the loop.';
    } else {
      card = 'Plan check passed — 0 violations. Dispatching 3 lenses.';
    }
  } else {
    state = 'rejected';
    const n = blocking.length;
    if (source === 'preflight') {
      if (checkKind === 'replan_park' && !exhausted) {
        card = 'Replan parked — amendment record unreadable. Initiative needs attention.';
      } else if (exhausted) {
        card = 'Replan cap reached — still failing. Initiative needs attention.';
      } else if (typeof round === 'number' && round > 0) {
        card = `Preflight rejected — ${n} violations. Returned to the planner (replan round ${round} of ${cap}).`;
      } else {
        card = `Preflight rejected — ${n} violations. Loop not started.`;
      }
    } else {
      card = `Returned to the planner as ${n} structured items — lenses were not dispatched.`;
    }
  }

  return {
    state,
    card,
    stats: { tasks, violations: blocking.length, affected, executed, skipped },
    rows: win.rows,
    advisories,
    moreCount: win.moreCount,
    ruleCounts: passed ? [] : counts,
    phaseStats,
  };
}
