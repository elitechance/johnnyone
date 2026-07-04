// Pure decision layer behind the console Events pane (replaces the old "Host files" column). Maps the
// initiative-level `AgentPlanEvent[]` (planning-run + development-run events merged oldest-first by the
// host `list_initiative_events`) into compact timeline rows carrying the SDLC stage, the lens/phase
// under review, and a NORMALIZED ISO timestamp the template formats with Angular's DatePipe (date +
// time). Kept Angular/Ionic/DOM-free so the spec runs under the plugin-less web vitest — mirroring the
// `console-logic.ts` / `lifecycle-status.ts` pure-seam pattern. Types are imported type-only (erased at
// build) at the depth the shipped pure siblings use, so no Angular/runtime dependency leaks in.
import type { AgentPlanEvent } from '../../../../../ui/src/services/johnny-api.service';

/** SDLC stage a timeline row belongs to — drives the left rail marker color (reuses `--jo-st-*`). */
export type TimelineStage = 'planning' | 'development' | 'review' | 'done' | 'other';

/** One rendered timeline row (mock §02 right column, now Events). */
export interface TimelineEvent {
  id: string;
  stage: TimelineStage;
  /** Who acted — t1 / t2 / coordinator / user / human (from the enriched `actor`). */
  actor: string;
  /** Display label for the actor — `human` renders as "you"; everything else passes through. */
  actorLabel: string;
  /** Human one-liner (the backend-composed `summary`, already lens/verdict-aware). */
  title: string;
  /** "Phase 2 · Cashier atomicity" when the event is phase-scoped, else null. */
  phaseLabel: string | null;
  /** The configured lens that produced this row (`product`/`qa`/`lead`/custom), else null. */
  lens: string | null;
  /** PASS / NEEDS_CHANGES / BLOCKED when the event carries a verdict, else null. */
  verdict: string | null;
  /** Extra reason/detail line (feedback source, block reason…), else null. */
  reason: string | null;
  /** Normalized ISO-8601 UTC instant (`…Z`) — format in the template via `| date`. */
  iso: string;
  /** Headline SDLC milestone (planning started, plan approved, phase passed, done…) — emphasized. */
  milestone: boolean;
}

/** Event types that are headline SDLC milestones (bigger marker, bold title). */
const MILESTONE_TYPES = new Set<string>([
  'planning_started',
  'planning_planner_ready',
  'agent_plan_created',
  'development_autostarted',
  'agent_phase_review_started',
  'agent_plan_completed',
  'agent_single_phase_completed',
]);

/** Map an event_type → the SDLC stage its row belongs under. */
export function stageOf(eventType: string, category: string): TimelineStage {
  switch (eventType) {
    case 'agent_phase_review_started':
    case 'agent_lens_review_started':
    case 'agent_lens_verdict':
    case 'agent_phase_gate_result':
      return 'review';
    case 'agent_plan_completed':
    case 'agent_single_phase_completed':
      return 'done';
    // Initiative genesis (created / brief accepted) opens the planning stage — group it there rather
    // than the neutral "other" bucket so the timeline reads as planning from the first row.
    case 'briefing_run_created':
    case 'brief_accepted':
    case 'planning_run_created':
      return 'planning';
    // The development plan-run's creation / automatic handoff opens the development stage.
    case 'agent_plan_created':
    case 'development_autostarted':
    case 'development_autostart_failed':
      return 'development';
    // A human's run/resume comment. Grouped under the run's SDLC stage — development by default
    // (its backend category is `phase`), or planning when it rode a planning-run resume. Never falls
    // through to the neutral "other" bucket.
    case 'human_comment':
      return category === 'planning' ? 'planning' : 'development';
  }
  if (category === 'planning' || eventType.startsWith('planning_')) return 'planning';
  if (category === 'phase' || eventType.startsWith('agent_phase') || eventType === 'agent_feedback_sent_to_worker') {
    return 'development';
  }
  return 'other';
}

/** `planning_gate_result`/`agent_phase_gate_result` with a PASS verdict are the "approved"/"passed"
 *  milestones — surface them even though the type isn't in MILESTONE_TYPES. */
function isGatePass(ev: AgentPlanEvent): boolean {
  return (
    (ev.eventType === 'planning_gate_result' || ev.eventType === 'agent_phase_gate_result') &&
    (ev.verdict ?? '').toUpperCase() === 'PASS'
  );
}

/** Normalize the host `created_at` to an ISO-8601 UTC instant the browser parses unambiguously.
 *  SQLite `datetime('now')` yields a NAIVE-UTC `YYYY-MM-DD HH:MM:SS` (no `T`, no `Z`) which V8 would
 *  otherwise read as LOCAL time — so add both. Already-ISO inputs (with `T` + offset/`Z`) pass through. */
export function normalizeEventIso(createdAt: string | null | undefined): string {
  const raw = (createdAt ?? '').trim();
  if (!raw) return '';
  const hasTime = raw.includes('T') || raw.includes(' ');
  const hasZone = /[zZ]$/.test(raw) || /[+-]\d{2}:?\d{2}$/.test(raw);
  if (raw.includes('T') && hasZone) return raw;
  const isoish = raw.replace(' ', 'T');
  return hasZone ? isoish : `${isoish}Z`;
}

/** Pull the lens name from `payloadJson` (lens events carry `{ lens }`); tolerant of bad JSON. */
export function lensOf(payloadJson: string | null | undefined): string | null {
  if (!payloadJson) return null;
  try {
    const parsed = JSON.parse(payloadJson) as { lens?: unknown };
    return typeof parsed.lens === 'string' && parsed.lens.trim() ? parsed.lens.trim() : null;
  } catch {
    return null;
  }
}

/** Display label for a row's actor. A `human_comment`'s actor is the backend `human`; surface it as
 *  "you" so the timeline reads as first-person guidance. All other actors (t1/t2/coordinator/user)
 *  pass through unchanged. Kept here (pure) so the component has no inline label logic. */
export function actorLabel(actor: string | null | undefined): string {
  const a = (actor ?? '').trim();
  return a === 'human' ? 'you' : a;
}

/** Compose the phase label ("Phase 2 · Cashier atomicity") from the enriched index/title. */
export function phaseLabelOf(ev: AgentPlanEvent): string | null {
  const hasIndex = typeof ev.phaseIndex === 'number';
  const title = (ev.phaseTitle ?? '').trim();
  if (!hasIndex && !title) return null;
  const num = hasIndex ? `Phase ${(ev.phaseIndex as number) + 1}` : 'Phase';
  return title ? `${num} · ${title}` : num;
}

/**
 * Project the initiative's merged events to timeline rows. Preserves the incoming (oldest-first) order
 * — the host already sorts `created_at ASC` across both runs. Pure/total: `null`/empty → `[]`.
 */
export function initiativeTimeline(events: AgentPlanEvent[] | null | undefined): TimelineEvent[] {
  return (events ?? []).map((ev) => {
    const verdict = (ev.verdict ?? '').trim() || null;
    return {
      id: ev.id,
      stage: stageOf(ev.eventType, ev.category),
      actor: (ev.actor ?? '').trim(),
      actorLabel: actorLabel(ev.actor),
      title: (ev.summary ?? '').trim() || ev.eventType,
      phaseLabel: phaseLabelOf(ev),
      lens: lensOf(ev.payloadJson),
      verdict,
      reason: (ev.reason ?? '').trim() || null,
      iso: normalizeEventIso(ev.createdAt),
      milestone: MILESTONE_TYPES.has(ev.eventType) || isGatePass(ev),
    };
  });
}
