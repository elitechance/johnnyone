import type { AiSession as SharedAiSession } from '@johnnyone/ui';

/**
 * The terminal page's view-model for one session.
 *
 * The API model (`AiSession`) is camelCase; the page's templates and helpers are
 * snake_case for the legacy fields. This is the seam between the two, extracted so
 * the mapping is a pure function a spec can call without compiling the page.
 */
export interface Session {
  id: string;
  title: string;
  provider: string;
  model: string;
  working_directory: string;
  status: 'active' | 'archived' | 'completed';
  total_input_tokens: number;
  total_output_tokens: number;
  total_cost_cents: number;
  created_at: string;
  updated_at: string;
  attached_tmux: boolean;
  /** Watchdog: periodic nudges for a quiet agent. All nullable for deploy-skew safety. */
  watchdogEnabled?: boolean;
  watchdogIntervalSecs?: number;
  watchdogPrompt?: string;
  watchdogLastNudgeAt?: string;
  watchdogNudgeCount?: number;
  /** Nudges the sweep decided were due but WITHHELD (pane still changing). 0/undefined = none. */
  watchdogHeldCount?: number;
}

/**
 * Map one API session onto the page's state shape.
 *
 * Without the last five fields the watchdog UI reads "Off" forever: the wire carries
 * them (see `sessionFields` in the api service), but a mapper that strips them means
 * they never reach `currentSession()`, so the toggle, the countdown and the nudge
 * count all fall back to their defaults no matter what the host has armed.
 */
export function sessionStateFromApi(session: SharedAiSession): Session {
  return {
    id: session.id,
    title: session.title,
    provider: session.provider,
    model: session.model,
    working_directory: session.workingDirectory || '',
    status: session.status,
    total_input_tokens: session.totalInputTokens,
    total_output_tokens: session.totalOutputTokens,
    total_cost_cents: session.totalCostCents,
    created_at: session.createdAt,
    updated_at: session.updatedAt,
    attached_tmux: session.attachedTmux ?? false,
    // Without these five the watchdog UI reads Off forever: the wire carries them, but the
    // mapper stripped them before they ever reached currentSession().
    watchdogEnabled: session.watchdogEnabled,
    watchdogIntervalSecs: session.watchdogIntervalSecs,
    watchdogPrompt: session.watchdogPrompt,
    watchdogLastNudgeAt: session.watchdogLastNudgeAt,
    watchdogNudgeCount: session.watchdogNudgeCount,
    // A held nudge is not a sent nudge; without this the console cannot tell "held back"
    // from "quiet", which is the ambiguity that hid the original watchdog bug.
    watchdogHeldCount: session.watchdogHeldCount,
  };
}