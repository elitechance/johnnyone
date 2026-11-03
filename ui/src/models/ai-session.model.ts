export interface AiSession {
  id: string;
  title: string;
  provider: string;
  model: string;
  workingDirectory?: string;
  status: 'active' | 'archived' | 'completed';
  totalInputTokens: number;
  totalOutputTokens: number;
  totalCostCents: number;
  createdAt: string;
  updatedAt: string;
  /** True when this session attaches to an external tmux session (UI shows Detach). */
  attachedTmux?: boolean;
  /**
   * The LIVE terminal state reported by the host: 'none' | 'attached' | 'detached' | 'ended'.
   * `attachedTmux` is only the INTENT (this session is the kind that attaches to a tmux); this is
   * whether the terminal actually exists right now, and 'ended' means the pane is gone.
   */
  terminalStatus?: string;
  /** Watchdog: periodic nudges for a quiet agent. All nullable for deploy-skew safety. */
  watchdogEnabled?: boolean;
  watchdogIntervalSecs?: number;
  watchdogPrompt?: string;
  watchdogLastNudgeAt?: string;
  watchdogNudgeCount?: number;
  /**
   * Nudges the sweep decided WERE due but withheld because the two-capture idle probe saw the pane
   * still changing. Distinct from `watchdogNudgeCount` (nudges actually delivered): a held nudge is
   * not a sent one, and without this the console cannot tell "held back" from "quiet".
   */
  watchdogHeldCount?: number;
  /**
   * The two "Also nudge when" checkboxes (migration 029). Nullable for deploy skew, like the rest of
   * the watchdog block: absent reads as unchecked, which is what a pre-029 row means anyway.
   */
  watchdogOnBlocked?: boolean;
  watchdogOnStaleReport?: boolean;
}

export interface CreateAiSessionInput {
  title?: string;
  provider?: string;
  model?: string;
  systemPrompt?: string;
}
