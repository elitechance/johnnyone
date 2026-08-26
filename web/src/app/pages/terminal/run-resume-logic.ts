// Pure decision layer behind the console Run/Resume panel (initiative overhaul — console-features,
// phase 01). All "what to show / what to send" logic lives here so `terminal.page` only renders the
// model and calls `johnny-api.runInitiativeFromPhase`. Kept Angular/Ionic/DOM-free so the spec runs
// under the plugin-less web vitest, mirroring the shipped `plan-tab-logic.ts` / `console-logic.ts` /
// `initiative-events-logic.ts` pure-seam pattern. Types are `import type` only (erased at build), so
// no `@johnnyone/ui` / Angular runtime dependency leaks into the plugin-less vitest.
import type { AgentPlanRun } from '../../../../../ui/src/services/johnny-api.service';
import { taskStatusLabel, type TaskStatusView } from './plan-tab-logic';

/** Normalize a status the same way the host does (`normalize_*`): trim, lower, `_`→`-`. Kept local so
 *  a raw value (deploy skew) still classifies. */
function normStatus(status: string | null | undefined): string {
  return (status ?? '').trim().toLowerCase().replace(/_/g, '-');
}

/** The two "paused" conditions a run can be in — the backend `run.plan.status`/`health` a Resume clears
 *  (`needs_attention`/`blocked`). Development statuses only; a healthy/running/idle run is not paused. */
const PAUSED_STATUSES = new Set(['needs-attention', 'blocked']);

/** In-flight development statuses. These win over a `needs-attention` *health* chip
 *  (S9 maps `phase_replan_running` → that chip; the run is busy, not Resume). */
const BUSY_STATUSES = new Set([
  'phase-replan-running',
  'phase-worker-running',
  'phase-review-running',
]);

export function isBusyStatus(status: string | null | undefined): boolean {
  return BUSY_STATUSES.has(normStatus(status));
}

/** True when the run is paused (needs_attention/blocked) — drives Resume vs Run, the banner, and the
 *  pre-selected paused phase. Busy status wins over health so a replan in flight is not Resume. */
export function isPausedRun(run: AgentPlanRun | null | undefined): boolean {
  const plan = run?.plan;
  if (!plan) return false;
  const status = normStatus(plan.status);
  if (isBusyStatus(plan.status)) return false;
  return PAUSED_STATUSES.has(status) || PAUSED_STATUSES.has(normStatus(plan.health));
}

/** Panel is shown ONLY for a DEVELOPMENT run that has ≥1 phase — hidden while planning/briefing or when
 *  a run has no phases yet. Total/null-safe. */
export function panelVisible(run: AgentPlanRun | null | undefined): boolean {
  const plan = run?.plan;
  if (!plan) return false;
  return normStatus(plan.runType) === 'development' && (run?.phases?.length ?? 0) > 0;
}

/** Error banner above the panel: shown with the run's `error` reason when paused, else hidden. */
export function bannerModel(run: AgentPlanRun | null | undefined): { show: boolean; reason: string } {
  if (!isPausedRun(run)) return { show: false, reason: '' };
  return { show: true, reason: (run?.plan?.error ?? '').trim() };
}

/** Primary button label: `Resume` for a paused run, else `Run`. */
export function runButtonLabel(run: AgentPlanRun | null | undefined): 'Run' | 'Resume' {
  return isPausedRun(run) ? 'Resume' : 'Run';
}

/** Phase whose status marks it as the paused/current one — `plan.currentPhaseId` first, else the first
 *  phase carrying a paused-ish status, else null. */
export function pausedPhaseId(run: AgentPlanRun | null | undefined): string | null {
  const plan = run?.plan;
  if (!plan) return null;
  const current = (plan.currentPhaseId ?? '').trim();
  if (current) return current;
  const paused = (run?.phases ?? []).find((ph) =>
    PAUSED_STATUSES.has(normStatus(ph.status)) || normStatus(ph.status) === 'needs-changes',
  );
  return paused ? paused.phaseId : null;
}

/** The phase the picker pre-selects: the paused phase when paused; else the current phase, else the
 *  first phase, else null. Total/null-safe. */
export function defaultSelectedPhaseId(run: AgentPlanRun | null | undefined): string | null {
  if (isPausedRun(run)) {
    const paused = pausedPhaseId(run);
    if (paused) return paused;
  }
  const current = (run?.plan?.currentPhaseId ?? '').trim();
  if (current) return current;
  const first = run?.phases?.[0];
  return first ? first.phaseId : null;
}

/** One radio row in the Start-at-phase picker. `statusChip` reuses `taskStatusLabel` (no fork); `paused`
 *  marks the paused row (gets the "paused here" tag); `selected` reflects the current radio value. */
export interface PhasePickerRow {
  phaseId: string;
  phaseTitle: string;
  statusChip: TaskStatusView;
  paused: boolean;
  selected: boolean;
}

/** Radio rows for every phase, in host order. Total/null-safe → `[]`. */
export function phasePickerRows(
  run: AgentPlanRun | null | undefined,
  selectedId: string | null | undefined,
): PhasePickerRow[] {
  const paused = pausedPhaseId(run);
  return (run?.phases ?? []).map((ph) => ({
    phaseId: ph.phaseId,
    phaseTitle: ph.phaseTitle,
    statusChip: taskStatusLabel(ph.status),
    paused: ph.phaseId === paused,
    selected: ph.phaseId === selectedId,
  }));
}

/** Default run mode — "from here · continue". */
export function defaultRunMode(): 'continue' | 'single' {
  return 'continue';
}

/** Map the mode toggle: "from here" → `continue`, "only this phase" → `single`. */
export function modeFromToggle(fromHere: boolean): 'continue' | 'single' {
  return fromHere ? 'continue' : 'single';
}

/** Validate the optional comment. Empty/missing → ok with `null` (pure re-run/resume). Whitespace-only
 *  (a used-but-blank field) → not ok. Non-blank → ok with the VERBATIM text (trim only detects blank). */
export function validateComment(
  text: string | null | undefined,
): { ok: true; value: string | null } | { ok: false; error: string } {
  if (text === null || text === undefined || text === '') return { ok: true, value: null };
  if (text.trim() === '') return { ok: false, error: 'Comment must not be only whitespace.' };
  return { ok: true, value: text };
}

/** Arguments for `johnny-api.runInitiativeFromPhase`. Blanks normalized to null. Throws when the comment
 *  is whitespace-only — the caller guards the button via `validateComment`, so this is a safety net. */
export function buildRunFromPhaseArgs(
  id: string,
  selectedPhaseId: string | null | undefined,
  comment: string | null | undefined,
  mode: 'continue' | 'single',
): { id: string; phaseId: string | null; phaseRunMode: 'continue' | 'single'; comment: string | null } {
  const validated = validateComment(comment);
  if (!validated.ok) throw new Error(validated.error);
  const phaseId = (selectedPhaseId ?? '').trim();
  return {
    id,
    phaseId: phaseId ? phaseId : null,
    phaseRunMode: mode === 'single' ? 'single' : 'continue',
    comment: validated.value,
  };
}
