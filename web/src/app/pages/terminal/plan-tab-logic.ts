// Pure, framework-free model behind the Plan tab (Overhaul P9 / phase P2, mock §03). Kept
// Angular/Ionic/DOM-free so the spec runs under the plugin-less web vitest, mirroring the shipped
// `console-tabs-logic.ts` / `console-logic.ts` pure-seam pattern (D6). Every transform is a projection
// over the EXISTING `getAgentPlan` result — no new data call, no markdown/mermaid re-implementation
// (rendering is `johnny-markdown-view`). Types are `import type` only (erased at build), so no
// `@johnnyone/ui` / Angular runtime dependency leaks into the plugin-less vitest.
import type { AgentPlanRun, AgentPlanTask } from '../../../../../ui/src/services/johnny-api.service';

/** Normalize a task/phase status the same way the host does (`normalize_task_status`): trim, lower,
 *  `_`→`-`. Kept local so a raw/un-normalized value (deploy skew, direct markdown) still classifies. */
function normStatus(status: string | null | undefined): string {
  return (status ?? '').trim().toLowerCase().replace(/_/g, '-');
}

/** The host collapses `passed`/`complete`/`completed` → `done`; accept those aliases defensively. */
const DONE_STATUSES = new Set(['done', 'completed', 'complete', 'passed']);

function isDone(status: string | null | undefined): boolean {
  return DONE_STATUSES.has(normStatus(status));
}

export interface PlanCounts {
  phases: number;
  tasks: number;
  done: number;
}

/** Header counts "N phases · M tasks · K done" — over `AgentPlanRun.phases`/`.tasks`. Null-safe. */
export function planCounts(run: AgentPlanRun | null | undefined): PlanCounts {
  const phases = run?.phases ?? [];
  const tasks = run?.tasks ?? [];
  return {
    phases: phases.length,
    tasks: tasks.length,
    done: tasks.filter((t) => isDone(t.status)).length,
  };
}

/** One task row in the docnav / phase card (§03 `.taskrow`). */
export interface PlanTaskRow {
  id: string;
  title: string;
  status: string;
}

/** Tasks belonging to a phase, preserving the host-provided order. */
function taskRowsFor(run: AgentPlanRun | null | undefined, phaseId: string): PlanTaskRow[] {
  return (run?.tasks ?? [])
    .filter((t: AgentPlanTask) => t.phaseId === phaseId)
    .map((t) => ({ id: t.taskId, title: t.taskTitle, status: t.status }));
}

/** A doc-navigator entry: either a fixed markdown file or an expandable phase (§03 `.docnav`). */
export type DocNavEntry =
  | { kind: 'file'; id: string; label: string }
  | { kind: 'phase'; id: string; label: string; status: string; tasks: PlanTaskRow[] };

/**
 * Doc navigator model: fixed `overview.md`, `status.md`, then one entry per phase (in host order),
 * each carrying its task rows so the nav can expand them (§03 `:728-737`).
 */
export function docNavModel(run: AgentPlanRun | null | undefined): DocNavEntry[] {
  const entries: DocNavEntry[] = [
    { kind: 'file', id: 'overview.md', label: 'overview.md' },
    { kind: 'file', id: 'status.md', label: 'status.md' },
  ];
  for (const ph of run?.phases ?? []) {
    entries.push({
      kind: 'phase',
      id: ph.phaseId,
      label: ph.phaseTitle,
      status: ph.status,
      tasks: taskRowsFor(run, ph.phaseId),
    });
  }
  return entries;
}

/** A §03 phase card: `phaseId`, `phaseTitle`, `done/total`, and its task rows. */
export interface PhaseCard {
  phaseId: string;
  phaseTitle: string;
  done: number;
  total: number;
  tasks: PlanTaskRow[];
}

/** Phase-card model — per phase, in host order (§03 `:757-782`). Null-safe → `[]`. */
export function phaseCards(run: AgentPlanRun | null | undefined): PhaseCard[] {
  return (run?.phases ?? []).map((ph) => {
    const tasks = taskRowsFor(run, ph.phaseId);
    return {
      phaseId: ph.phaseId,
      phaseTitle: ph.phaseTitle,
      total: tasks.length,
      done: tasks.filter((t) => isDone(t.status)).length,
      tasks,
    };
  });
}

/**
 * Build the file path for a navigator selection from FIXED suffixes only, joined to the
 * server-provided `planPath` (never a user-typed path). Returns `null` for an empty `planPath` or any
 * selection that is not one of the two fixed files or a safe phase id. A phase id containing `/`,
 * `\`, or `..` is rejected (defense-in-depth against traversal even though the id is server data).
 */
export function planDocPath(
  planPath: string | null | undefined,
  sel: string | null | undefined,
): string | null {
  const base = (planPath ?? '').trim().replace(/\/+$/, '');
  if (!base) return null;
  if (sel === 'overview.md') return `${base}/overview.md`;
  if (sel === 'status.md') return `${base}/status.md`;
  const id = (sel ?? '').trim();
  if (!id) return null;
  if (id.includes('/') || id.includes('\\') || id.includes('..')) return null;
  return `${base}/phases/${id}/overview.md`;
}

/** Display label + class token for a task-row status chip (§03 taskrow states). */
export interface TaskStatusView {
  label: string;
  token: string;
}

export function taskStatusLabel(status: string | null | undefined): TaskStatusView {
  const n = normStatus(status);
  // Done aliases (completed/complete/passed) render as the single DONE chip, matching `isDone`.
  if (isDone(status)) return { label: 'DONE', token: 'done' };
  if (n === 'planned') return { label: 'PLANNED', token: 'planned' };
  if (n === 'in-progress') return { label: 'IN-PROGRESS', token: 'in-progress' };
  return { label: (status ?? '').trim().toUpperCase(), token: n || 'unknown' };
}
