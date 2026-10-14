// The arena's client-side rules. ONE home per rule: every derived string or enable/disable decision
// the arena screens show comes from a function here, so no template re-derives it and no second copy
// can drift (the `effective_test_commands` / `effectiveTestCommands` precedent).
//
// What this module deliberately does NOT do:
//   * it never recomputes comparability from `seatsHeld` — `WorkflowPref.comparable` is computed by
//     the host from the two runs and is not an argument a client can assert;
//   * it never recounts the rejection streak — `WorkflowSearchState.halted` is the host's answer and
//     the host is what refuses `startWorkflowRun`. This module only explains that refusal;
//   * it never produces a composite score, a rating or a percentage. The gate's exit code is the
//     only score (schema.md, "No `score` column").
//
// Framework-free on purpose (the `*-logic.ts` pure-seam pattern), so it runs under the plugin-less
// web vitest.

/** The exact copy the mock's step 1 shows on the disabled arena row. A hidden control teaches
 *  nobody why a phase cannot compete — it just looks like the feature is broken. */
export const NO_GATE_REASON =
  'No test command on this phase. The winner is decided by a command, not by an opinion, so there is nothing to compete for yet.';

export interface ArenaEntryRow {
  enabled: boolean;
  /** Non-null exactly when `enabled` is false: the reason, shown beside the disabled control. */
  reason: string | null;
}

/** Step 1. Whether a phase can enter the arena, and why not when it cannot. Reads the host's
 *  `effectiveTestCommands` (plan fallback already applied there) — never the raw per-level value. */
export function arenaEntryRow(phase: {
  effectiveTestCommands?: readonly string[] | null;
}): ArenaEntryRow {
  const cmds = (phase.effectiveTestCommands ?? []).filter((c) => c.trim().length > 0);
  return cmds.length > 0 ? { enabled: true, reason: null } : { enabled: false, reason: NO_GATE_REASON };
}

/**
 * `gateCollected` / `TestGateRun.collected` is ADVISORY and NULLABLE, and null is not an error: a
 * lint, a build or a bespoke script reports no count and is still a perfectly good gate. Rendering
 * null as `0` would read as "this suite collects nothing", which is the one thing the count exists
 * to make visible — so null renders as "not reported" and 0 renders as 0.
 */
export function gateCollectedLabel(collected?: number | null): string {
  return collected === null || collected === undefined ? 'not reported' : `${collected} collected`;
}

/** The exit code is the whole score. NULL means the gate never ran, which is NOT a pass. */
export function gateExitLabel(exitCode?: number | null): string {
  if (exitCode === null || exitCode === undefined) return 'never ran';
  return exitCode === 0 ? 'exit 0 · pass' : `exit ${exitCode} · fail`;
}

export interface GateAcceptance {
  accepted: boolean;
  headline: string;
  reason: string | null;
}

/**
 * Step 2. The one screen a human has to think about: the gate is run once, now, on untouched code,
 * and the result is shown before the setup may continue.
 *
 * Refused: no gate declared at all; a command already red before anyone has done any work (a setup
 * failure and a real failure are not the same signal); and a suite that collects ZERO tests, which
 * exits 0 and scores every workflow identically. A null count is NOT refused.
 */
export function gateAcceptance(result: {
  gateDeclared: boolean;
  exitCode: number;
  collected?: number | null;
}): GateAcceptance {
  if (!result.gateDeclared) {
    return { accepted: false, headline: 'refused · no gate', reason: NO_GATE_REASON };
  }
  if (result.exitCode !== 0) {
    return {
      accepted: false,
      headline: `refused · ${gateExitLabel(result.exitCode)}`,
      reason:
        'This command is already red on untouched code. A setup failure and a real failure are not the same signal, so nothing can be scored against it yet.',
    };
  }
  if (result.collected === 0) {
    return {
      accepted: false,
      headline: 'refused · 0 collected',
      reason:
        'This suite collects nothing. It exits 0 and scores every workflow identically, so the search would follow pure noise.',
    };
  }
  return {
    accepted: true,
    headline: `accepted · ${gateExitLabel(0)} · ${gateCollectedLabel(result.collected)}`,
    reason: null,
  };
}

/** A reason longer than this is refused. Nothing capped it at either layer; a megabyte of text in
 *  a halt banner is its own denial of service, on a phone most of all. */
export const MAX_REJECTION_REASON = 2000;

/**
 * Characters that are NOT Unicode `White_Space` — so `trim()` keeps them — and yet render as
 * nothing or as nothing-on-their-own. A reason made only of these passed a `trim().length > 0`
 * check and then appeared in the halt banner as an empty line.
 *
 * The classes, and why each is here:
 *   * C0/C1 controls (NUL among them), the BOM, zero-width space/joiners, bidi controls, the word
 *     joiner, the soft hyphen, the Hangul fillers, Mongolian vowel separator;
 *   * U+2800 BRAILLE PATTERN BLANK — a printing character whose glyph is empty, which is exactly
 *     why it is the one people reach for;
 *   * COMBINING MARKS (`\p{M}`) — they are stripped only when nothing precedes them to combine
 *     with, which `visibleText` handles by removing marks that follow nothing visible, so
 *     "pathe\u0301" keeps its accent while a bare "\u0301" does not survive;
 *   * the astral VARIATION SELECTORS U+E0100–E01EF and TAG characters U+E0000–E007F, which need the
 *     `u` flag to be matched at all — without it these ranges are unmatchable as written, which is
 *     how they got through the first version.
 */
const INVISIBLE =
  /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f\u00ad\u034f\u061c\u115f\u1160\u17b4\u17b5\u180b-\u180e\u200b-\u200f\u202a-\u202e\u2060-\u2064\u2066-\u206f\u2800\u3164\ufe00-\ufe0f\ufeff\uffa0\ufff9-\ufffb]|[\u{E0000}-\u{E007F}]|[\u{E0100}-\u{E01EF}]/gu;

/** A combining mark that follows NOTHING — nothing to combine with, so nothing is drawn. */
const ORPHAN_MARK = /(?<![\p{L}\p{N}\p{S}\p{P}])\p{M}+/gu;

/**
 * What a reason actually SHOWS: every invisible character removed, then combining marks that have
 * nothing to combine with, then whitespace trimmed. Run twice, because removing an invisible can
 * leave a mark orphaned ("\u200b\u0301" is two characters that draw nothing between them).
 */
export function visibleText(value: string): string {
  let out = value;
  for (let pass = 0; pass < 2; pass += 1) {
    out = out.replace(INVISIBLE, '').replace(ORPHAN_MARK, '');
  }
  return out.trim();
}

/**
 * Saying no always carries a reason — there is no bare no (mock step 8), and "a reason that renders
 * as nothing" is a bare no with extra steps. Checked here rather than left to the host: the host is
 * being fixed in parallel, and a client that submits something it knows is empty is wrong either
 * way.
 */
export function rejectionReasonValid(reason: string): { ok: boolean; message: string | null } {
  const visible = visibleText(reason);
  if (visible.length === 0) {
    return {
      ok: false,
      message:
        'Write a reason that is actually visible. A challenger that passed every check and was rejected anyway means the gate is missing something, and the reason is the only record of what — an empty line in the halt banner tells the next reader nothing.',
    };
  }
  // The cap counts what a READER sees, for the same reason emptiness does. Checking the raw string
  // here while checking the visible text above meant 1,990 real characters padded with zero-width
  // ones were refused as "2010 against a limit of 2000" — one end of the same rule measured one way
  // and the other end the other.
  if (visible.length > MAX_REJECTION_REASON) {
    return {
      ok: false,
      message: `That reason is too long: ${visible.length} characters against a limit of ${MAX_REJECTION_REASON}. It has to be readable in a halt banner on a phone.`,
    };
  }
  return { ok: true, message: null };
}

export interface SearchStateLike {
  repo: string;
  phaseType: string;
  family: string;
  consecutiveRejections: number;
  halted: boolean;
  strikeLimit: number;
  recentRejections: readonly string[];
  championWorkflowId?: string | null;
}

/**
 * Why a run cannot be started, or null when it can. The halt is the HOST's: `startWorkflowRun`
 * refuses while a family is halted, and this reads `halted` rather than recounting the streak, so a
 * host that changes its strike limit needs no client change. A state that failed to load blocks too
 * — starting blind is how a halted search gets a run anyway and reports success.
 */
export function startRunBlock(state: SearchStateLike | null | undefined): string | null {
  if (!state) {
    return 'The search state for this family could not be read, so a run cannot be started. It is what says whether the search is halted.';
  }
  if (!state.halted) return null;
  return `The search is paused: ${state.consecutiveRejections} challengers in a row passed the gate and were rejected (limit ${state.strikeLimit}). The gate is not describing what you want, so change it — a promotion is what resets the streak.`;
}

export interface RunLike {
  id: string;
  gateExit?: number | null;
}
export interface PrefLike {
  id: string;
  winnerRunId: string;
  loserRunId: string;
  comparable: boolean;
}

/**
 * Why this run's workflow cannot be promoted, or null when it can. Mirrors the host's
 * `promotion_block` for DISPLAY only — the host refuses the mutation regardless, and every input
 * here is a host-computed field (`gateExit`, `pref.comparable`, `pref.winnerRunId`). Promotion is
 * blocked, never warned: two sides run under different seat budgets measure scheduling luck.
 */
export function promotionBlock(args: {
  run: RunLike;
  pref: PrefLike | null | undefined;
  incumbentExists: boolean;
}): string | null {
  const { run, pref, incumbentExists } = args;
  if (run.gateExit === null || run.gateExit === undefined) {
    return 'This run’s gate never ran. "Never ran" is not a pass, so there is nothing to promote.';
  }
  if (run.gateExit !== 0) {
    return `This run did not pass its gate (${gateExitLabel(run.gateExit)}).`;
  }
  if (!pref) {
    return incumbentExists
      ? 'There is no comparable head-to-head against the current champion yet. A matched replay has to exist before a champion can be displaced.'
      : null;
  }
  if (pref.winnerRunId !== run.id) {
    return 'The recorded preference names a different run as the winner.';
  }
  if (!pref.comparable) {
    return 'The two sides held different seat budgets, so this comparison is not comparable and cannot promote. Run a matched replay first.';
  }
  return null;
}

/** The confirm's copy. It states the blast radius plainly, because that is the only irreversible
 *  step in the feature. */
export function promotionConfirmCopy(args: {
  repo: string;
  phaseType: string;
  family: string;
  version: number;
}): string {
  return `Promoting makes v${args.version} the starting prompt for every future ${args.phaseType} in ${args.repo}, on ${args.family}. It does not touch any other repo, phase type or model family.`;
}

// ── Derived values the screens need ───────────────────────────────────────────────────────────
// Each lives here rather than inline in a template, so there is one home per rule and a spec can
// hold it. None of them is a score.

/** A champion belongs to ONE repo. J1 has no repo field, so the repo is the workspace directory's
 *  name — the same string a human would call the repo. Empty, never a guess, when there is none. */
export function repoNameFromWorkspacePath(path: string | null | undefined): string {
  if (!path) return '';
  const parts = path.split('/').filter((p) => p.length > 0);
  return parts.length > 0 ? parts[parts.length - 1] : '';
}

/**
 * A champion is keyed by phase TYPE, and J1 has no phase-type column anywhere today: a phase has an
 * id ('04-leases') and a title, nothing that says what KIND of phase it is. So this is a suggested
 * default the operator can correct, derived from the run type — not a lookup, and the setup screen
 * says so in those words.
 */
export function defaultPhaseType(runType: string | null | undefined): string {
  const t = (runType ?? '').toLowerCase();
  if (t === 'planning') return 'planning-phase';
  return 'dev-phase';
}

export interface LineageNode {
  id: string;
  parentId?: string | null;
  version: number;
}

/** The parent chain, oldest first, ending at `id`. Bounded, so a cyclic `parentId` (which the host
 *  refuses to write, but a corrupted row could still hold) cannot hang a phone. */
export function lineageOf<T extends LineageNode>(all: readonly T[], id: string): T[] {
  const byId = new Map(all.map((w) => [w.id, w]));
  const chain: T[] = [];
  const seen = new Set<string>();
  let cur = byId.get(id);
  while (cur && !seen.has(cur.id)) {
    seen.add(cur.id);
    chain.unshift(cur);
    cur = cur.parentId ? byId.get(cur.parentId) : undefined;
  }
  return chain;
}

export interface SlotChange {
  slot: 'role' | 'instruction' | 'contract' | 'hop' | 'width';
  from: string;
  to: string;
}

export interface SlotsLike {
  role: string;
  instruction: string;
  contract: string;
  hop: string;
  fanOut: number;
}

/**
 * WHICH of the four parts changed, and their two values — deliberately not a text diff of one big
 * prompt. The comparison a human needs is "the contract and the hop moved", because that is where
 * detail migrates to as the search progresses.
 */
export function changedSlots(champion: SlotsLike, challenger: SlotsLike): SlotChange[] {
  const out: SlotChange[] = [];
  for (const slot of ['role', 'instruction', 'contract', 'hop'] as const) {
    if (champion[slot] !== challenger[slot]) {
      out.push({ slot, from: champion[slot], to: challenger[slot] });
    }
  }
  if (champion.fanOut !== challenger.fanOut) {
    out.push({ slot: 'width', from: String(champion.fanOut), to: String(challenger.fanOut) });
  }
  return out;
}

/** "Four parts, never blank" (mock step 4). Names every blank part so the screen can say which. */
export function slotsComplete(slots: {
  role: string;
  instruction: string;
  contract: string;
  hop: string;
}): { ok: boolean; missing: string[] } {
  const missing = (['role', 'instruction', 'contract', 'hop'] as const).filter(
    (k) => slots[k].trim().length === 0,
  );
  return { ok: missing.length === 0, missing: [...missing] };
}

/** `hop` is stored as JSON and the host validates that it IS JSON (deliberately not its shape).
 *  Checking here means a malformed hop is a sentence on the screen, not a failed mutation. */
export function hopIsJson(hop: string): boolean {
  const t = hop.trim();
  if (!t.startsWith('{') && !t.startsWith('[')) return false;
  try {
    const parsed: unknown = JSON.parse(t);
    return typeof parsed === 'object' && parsed !== null;
  } catch {
    return false;
  }
}

/** The run a side is currently represented by: the most recently STARTED one. Null for none —
 *  "no runs yet" is an answer, and rendering it as a zero would read as a passing gate. */
export function latestRunOf<T extends { startedAt: string }>(runs: readonly T[]): T | null {
  if (runs.length === 0) return null;
  return [...runs].sort((a, b) => (a.startedAt < b.startedAt ? 1 : a.startedAt > b.startedAt ? -1 : 0))[0];
}
