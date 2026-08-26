// Pure Tasks-tab seam (S4 / S9). Angular/Ionic/DOM-free.

export interface TaskRunLike {
  tasks?: TaskRowLike[];
}

export interface TaskRowLike {
  id: string;
  status: string;
  dependsOn?: string[];
  depends_on?: string[];
  blockedBy?: string | null;
  blocked_by?: string | null;
  route?: string | null;
  succeededTier?: string | null;
  succeeded_tier?: string | null;
  commitSha?: string | null;
  commit_sha?: string | null;
  attempts?: {
    tier?: string;
    endedAt?: string;
    startedAt?: string;
    checks?: { failed?: { check: string }[] };
    failed?: { check: string }[];
    verify?: { passed?: boolean };
    postchecks?: { passed?: boolean }[];
  }[];
}

export interface TaskTableRow {
  index: number;
  id: string;
  files: string;
  state: string;
  tier: string;
  verifyOk: string;
  commit: string;
  took: string;
  blockedBy: string | null;
}

export interface TaskCounts {
  done: number;
  running: number;
  failed: number;
  blocked: number;
  pending: number;
}

export interface SpecLike {
  id: string;
  files?: string[];
  verify?: string;
}

export function taskCounts(run: TaskRunLike | null | undefined): TaskCounts {
  const c: TaskCounts = { done: 0, running: 0, failed: 0, blocked: 0, pending: 0 };
  for (const t of run?.tasks ?? []) {
    const s = (t.status || 'pending').toLowerCase();
    if (s === 'done') c.done += 1;
    else if (s === 'running') c.running += 1;
    else if (s === 'failed') c.failed += 1;
    else if (s === 'blocked') c.blocked += 1;
    else c.pending += 1;
  }
  return c;
}

export function verifyOkOf(
  last:
    | {
        checks?: { failed?: { check: string }[] };
        failed?: { check: string }[];
        verify?: { passed?: boolean };
        postchecks?: { passed?: boolean }[];
      }
    | null
    | undefined,
): string {
  if (!last) return '';
  const failed = last.checks?.failed ?? last.failed;
  if (Array.isArray(failed)) {
    return failed.some((f) => f.check === 'verify') ? 'fail' : 'ok';
  }
  if (typeof last.verify?.passed === 'boolean') {
    return last.verify.passed ? 'ok' : 'fail';
  }
  if (Array.isArray(last.postchecks)) {
    return last.postchecks.every((h) => h.passed) ? 'ok' : 'fail';
  }
  return '';
}

export function failedBlockedCount(
  failedId: string | null | undefined,
  specs: { id?: string; dependsOn?: string[]; depends_on?: string[] }[] | null | undefined,
  rows: { blockedBy: string | null }[],
): number {
  if (!failedId) return 0;
  const fromSpecs = (specs ?? []).filter((s) =>
    (s.dependsOn ?? s.depends_on ?? []).includes(failedId),
  ).length;
  if (fromSpecs) return fromSpecs;
  return rows.filter((r) => r.blockedBy === failedId).length;
}

export function tookOf(start: unknown, end: unknown): string {
  const s = Date.parse(String(start ?? ''));
  const e = Date.parse(String(end ?? ''));
  if (!Number.isFinite(s) || !Number.isFinite(e) || e < s) return '';
  const ms = e - s;
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.round(ms / 60_000)}m`;
}

export function taskTable(
  run: TaskRunLike | null | undefined,
  specs?: SpecLike[] | null,
): TaskTableRow[] {
  const specMap = new Map((specs ?? []).map((s) => [s.id, s]));
  return (run?.tasks ?? []).map((t, i) => {
    const spec = specMap.get(t.id);
    const last = t.attempts?.[t.attempts.length - 1] as
      | {
          tier?: string;
          startedAt?: string;
          started_at?: string;
          endedAt?: string;
          ended_at?: string;
          checks?: { failed?: { check: string }[] };
          failed?: { check: string }[];
          verify?: { passed?: boolean };
          postchecks?: { passed?: boolean }[];
        }
      | undefined;
    const start = last?.startedAt ?? last?.started_at;
    const end = last?.endedAt ?? last?.ended_at;
    const took = tookOf(start, end);
    return {
      index: i + 1,
      id: t.id,
      files: (spec?.files ?? []).join(', '),
      state: t.status || 'pending',
      tier: t.succeededTier ?? t.succeeded_tier ?? last?.tier ?? '',
      verifyOk: verifyOkOf(last),
      commit: (t.commitSha ?? t.commit_sha ?? '').slice(0, 8),
      took,
      blockedBy: t.blockedBy ?? t.blocked_by ?? null,
    };
  });
}

export function initialSelectedId(rows: { id: string; state: string }[]): string | null {
  const failed = rows.find((r) => r.state === 'failed');
  if (failed) return failed.id;
  const running = rows.find((r) => r.state === 'running');
  if (running) return running.id;
  const blocked = rows.find((r) => r.state === 'blocked');
  if (blocked) return blocked.id;
  return rows[0]?.id ?? null;
}

export interface WindowPlaceholder {
  from: number;
  to: number;
  firstId: string;
}

export function windowRows(
  rows: TaskTableRow[],
  selected: string | null | undefined,
  radius = 25,
): { rows: TaskTableRow[]; placeholders: WindowPlaceholder[] } {
  if (rows.length === 0) return { rows: [], placeholders: [] };
  const selId = selected || initialSelectedId(rows);
  const idx = Math.max(0, rows.findIndex((r) => r.id === selId));
  const start = Math.max(0, idx - radius);
  const end = Math.min(rows.length, start + 51);
  const slice = rows.slice(start, end);
  const placeholders: WindowPlaceholder[] = [];
  if (start > 0) {
    placeholders.push({ from: 1, to: start, firstId: rows[0].id });
  }
  if (end < rows.length) {
    placeholders.push({ from: end + 1, to: rows.length, firstId: rows[end].id });
  }
  return { rows: slice, placeholders };
}

export function selectPlaceholder(
  rows: TaskTableRow[],
  range: { from: number; to: number; firstId?: string },
): string | null {
  if (range.firstId && rows.some((r) => r.id === range.firstId)) return range.firstId;
  const at = rows[range.from - 1];
  return at?.id ?? rows[0]?.id ?? null;
}

export function tasksEmptyCopy(
  run: TaskRunLike | null | undefined,
  error?: string | null,
): string | null {
  if (error) return "Could not read this phase's task run";
  if (!run || (run.tasks?.length ?? 0) === 0) return 'No tasks in this phase yet';
  return null;
}

export function replanBanner(
  status: string | null | undefined,
  opts?: { replanExhausted?: boolean; replanParked?: boolean },
): string | null {
  const s = (status ?? '').trim();
  if (s === 'phase_replan_running') return 'Re-planning — planner is amending routed tasks';
  if (s === 'needs_attention' && opts?.replanExhausted) {
    return 'Replan cap reached — still failing';
  }
  if (s === 'needs_attention' && opts?.replanParked) {
    return 'Replan parked — amendment record unreadable';
  }
  return null;
}
