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
}

export interface CreateAiSessionInput {
  title?: string;
  provider?: string;
  model?: string;
  systemPrompt?: string;
}
