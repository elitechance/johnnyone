// Pure, Angular/Ionic-free Shells decision logic (overhaul P6, decisions D3/D4/D6) so it can be
// unit-tested under the plugin-less web vitest — the real `ShellsPage` pulls in Ionic + the Phase-01
// launcher, which that config cannot import. The page delegates its filter/partition/dedupe/label/
// rel-time/open decisions to exactly these functions and stays thin wiring.
//
// `AiSession`/`TmuxSession` are imported type-only (erased at build) at the same relative depth the
// shipped pure siblings use (`validation-config-logic.ts` — five `../`), so no Angular/runtime dependency
// leaks in. `openIntent` ALIASES Phase 04's `plainTerminalRoute` (NOT `terminalRoute`, and NOT the
// old `/terminal?sessionId=` shape): a shell opens on `/shells/:id`, built in one place, so the nav
// shape is defined once (D3/D5) and never re-derived here.
import type { AiSession } from '../../../../../ui/src/models/ai-session.model';
import type { TmuxSession } from '../../../../../ui/src/services/johnny-api.service';
import { plainTerminalRoute } from '../../components/launcher-menu/launcher-logic';

/**
 * A session belongs on the Shells list when it is a raw `CliProvider::Shell` OR an attached external
 * tmux pane — that pair is the whole basis of the filter (D4). An agent session
 * (`claude_code`/`codex`/…) with `attachedTmux:false` is excluded.
 */
export function isShellSession(s: Pick<AiSession, 'provider' | 'attachedTmux'>): boolean {
  return s.provider === 'shell' || s.attachedTmux === true;
}

/**
 * The shell/attached subset of `sessions`, sorted **newest-first** by `updatedAt ?? createdAt`
 * (descending ISO string compare — ISO-8601 sorts lexicographically). Agent sessions are dropped.
 * `Array.prototype.sort` is stable, so equal keys keep their input order.
 */
export function partitionShells(sessions: AiSession[]): AiSession[] {
  const key = (s: AiSession): string => s.updatedAt ?? s.createdAt ?? '';
  return sessions.filter(isShellSession).sort((a, b) => key(b).localeCompare(key(a)));
}

/**
 * External tmux panes that are **not already attached** by one of our sessions — so a pane you already
 * attached does not show up in both the "active shells" and "attachable" groups.
 *
 * Join key: an attached-tmux session is created with its `title` defaulted to the tmux `name`
 * (`terminal.page.ts:790` — `title: … || name`), so we drop any `tmux[i]` whose `name` equals the
 * `title` of an attached (`attachedTmux === true`) session. This is a **best-effort UX dedupe only** —
 * the host itself prevents a real double-attach conflict (D10). Case-sensitive exact-name match. With no
 * attached sessions, every external pane is attachable.
 */
export function attachableTmux(tmux: TmuxSession[], sessions: AiSession[]): TmuxSession[] {
  const attachedNames = new Set(
    sessions.filter((s) => s.attachedTmux === true).map((s) => (s.title ?? '').trim()),
  );
  return tmux.filter((t) => !attachedNames.has(t.name));
}

/**
 * Row label for a shell session: the trimmed title (or `'Shell'` when empty) plus a badge tag
 * distinguishing a raw shell from an attached tmux pane. A trivial pure label — NOT a fork of the
 * terminal page's page-private `providerLabel` (D4).
 */
export function shellSessionLabel(
  s: Pick<AiSession, 'title' | 'attachedTmux'>,
): { title: string; tag: 'shell' | 'tmux' } {
  return {
    title: (s.title && s.title.trim()) || 'Shell',
    tag: s.attachedTmux ? 'tmux' : 'shell',
  };
}

/**
 * Coarse relative time from two ISO strings. `nowIso` is a PARAMETER (no `Date.now()` inside) so the
 * unit test is deterministic — the component supplies the real "now". Invalid/empty input → `''`.
 *   < 60s → 'just now' · < 60m → 'Nm ago' · < 24h → 'Nh ago' · else 'Nd ago'
 */
export function formatRelTime(iso: string, nowIso: string): string {
  const then = Date.parse(iso ?? '');
  const now = Date.parse(nowIso ?? '');
  if (Number.isNaN(then) || Number.isNaN(now)) return '';
  const seconds = Math.floor((now - then) / 1000);
  if (seconds < 60) return 'just now';
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

/**
 * Route args to open a shell session on the terminal surface. Shells always open as a PLAIN terminal
 * (no initiative chrome), so this ALIASES Phase 4's `plainTerminalRoute` — the dedicated
 * `/shells/:sessionId` destination (whose route supplies `data.surface = 'shell'`, so there is no
 * `surface=shell` query and no `/terminal?sessionId=` anywhere in this path), defined once (D5).
 * That bare URL also means the transcript view; `/shells/:id/raw` is the explicit alternative.
 * Fix for finding #3(b).
 */
export const openIntent = plainTerminalRoute;

// ── Clear (archive-all) decisions ───────────────────────────────────────────────────────────────
// The /shells "Clear" affordance ARCHIVES; it must never delete. See the comment next to the
// `archiveSession` call in `shells.page.ts` for why (host-side `reportAgentResult` depends on the
// session ROW still existing). Everything decidable about the clear lives here so it is testable
// without an Angular TestBed, per this module's contract.

/** The session ids a clear targets: the shell rows themselves, in list order. Attachable tmux PANES
 *  are not sessions (no id to archive) and can never end up here — callers pass `shells()`, and the
 *  same `isShellSession` filter is re-applied defensively. An empty list yields no work. */
export function clearTargetIds(shells: Pick<AiSession, 'id' | 'provider' | 'attachedTmux'>[]): string[] {
  return shells.filter(isShellSession).map((s) => s.id);
}

/**
 * Confirm-dialog copy for a clear, stating in plain words exactly what happens. It branches on
 * LIVENESS, not merely on `attachedTmux`, because a row can be attached to a tmux session that has
 * since died — and a dead pane cannot come back. `livePaneNames` are the names from
 * `listTmuxSessions()`, which `list_external_tmux_sessions` (`terminal.rs:1355-1386`) builds by
 * shelling out to `tmux list-sessions`, so it is exactly the set of panes that still exist. Passing
 * it in (rather than assuming liveness) is what makes the dead case testable.
 *
 * Three groups, each only claimed when it is true:
 *  - attached + live → keeps running, reappears under "Attachable tmux sessions" (`sessions.rs:198`)
 *  - attached + dead → already gone; archiving just takes the stale row off the list
 *  - plain shell     → no external tmux, so `archive_session` really closes the terminal
 *    (`sessions.rs:221-225`)
 *
 * The transcript survives either way (`list_session_reports` / `get_ai_session` have no status
 * filter) but NO list surface shows archived sessions — `/shells` and the terminal tab bar both ask
 * `listSessions('active')` — so the copy says "reachable by link" rather than implying it stays in
 * front of you. "Nothing is lost" is claimed ONLY when every row is coming back. The word "delete"
 * never appears: nothing is deleted.
 */
export function clearConfirmCopy(
  shells: Pick<AiSession, 'id' | 'title' | 'provider' | 'attachedTmux'>[],
  livePaneNames: Iterable<string>,
): { header: string; message: string; confirmText: string } {
  const rows = shells.filter(isShellSession);
  const n = rows.length;
  if (n === 0) {
    return {
      header: 'Nothing to clear',
      message: 'There are no shells in the list.',
      confirmText: 'Clear',
    };
  }
  // Same join key as `attachableTmux`: an attached session's `title` IS the external pane name.
  const live = new Set(Array.from(livePaneNames, (name) => (name ?? '').trim()));
  const attached = rows.filter((s) => s.attachedTmux === true);
  const returning = attached.filter((s) => live.has((s.title ?? '').trim())).length;
  const gone = attached.length - returning;
  const plain = n - attached.length;

  const header = `Clear ${n} shell${n === 1 ? '' : 's'}?`;
  const lead = `This removes ${n} shell${n === 1 ? '' : 's'} from the list`;
  const attachableGroup = '“Attachable tmux sessions”';
  const kept = `Transcript${n === 1 ? ' is' : 's are'} kept and ${n === 1 ? 'stays' : 'stay'} reachable by link.`;

  // Every row is coming back — the only case that may promise nothing is lost.
  if (returning === n) {
    const subject =
      n === 1 ? 'The tmux session itself keeps running' : 'The tmux sessions themselves keep running';
    return {
      header,
      message: `${lead}. ${subject} and will reappear under ${attachableGroup} after the refresh, so nothing is lost.`,
      confirmText: 'Clear',
    };
  }
  // Every row is a plain shell — each one really closes.
  if (plain === n) {
    const tail = n === 1 ? 'and closes its terminal.' : 'and closes their terminals.';
    return { header, message: `${lead} ${tail} ${kept}`, confirmText: 'Clear' };
  }

  const clauses: string[] = [];
  if (returning > 0) {
    clauses.push(
      `${returning} keep${returning === 1 ? 's' : ''} running and come${returning === 1 ? 's' : ''} back under ${attachableGroup}`,
    );
  }
  if (gone > 0) {
    clauses.push(`${gone} ${gone === 1 ? 'is' : 'are'} already gone and will just leave the list`);
  }
  if (plain > 0) {
    clauses.push(
      `${plain} plain shell${plain === 1 ? '' : 's'} close${plain === 1 ? 's' : ''}`,
    );
  }
  return { header, message: `${lead}. ${clauses.join('; ')}. ${kept}`, confirmText: 'Clear' };
}

/** Honest one-line summary of a partial-failure clear, or `null` when every archive succeeded (so a
 *  clean clear shows no error banner). `total` is how many were attempted, `failed` how many
 *  rejected — `Promise.allSettled` means some can fail without aborting the rest. */
export function clearResultMessage(total: number, failed: number): string | null {
  if (failed <= 0) return null;
  if (total === 1) return 'The shell could not be cleared.';
  if (failed >= total) return `No shells could be cleared — all ${total} failed.`;
  return `Cleared ${total - failed} of ${total} shells — ${failed} could not be cleared.`;
}

/**
 * Must a per-row clear confirm first? Only for a PLAIN shell. An attached row's archive leaves the
 * external tmux running (`sessions.rs:198`) and the pane comes straight back under "Attachable tmux
 * sessions", so it is one tap to undo and a dialog would be friction for nothing. A plain shell has
 * no external tmux, so `archive_session` takes the `else` branch and calls `kill_terminal_session`
 * (`sessions.rs:221-225`) — the terminal really closes, with nothing to re-attach. A missing/unknown
 * `attachedTmux` confirms (fail safe), because the unconfirmed path is the destructive-by-surprise one.
 */
export function needsClearConfirm(s: Pick<AiSession, 'attachedTmux'>): boolean {
  return s.attachedTmux !== true;
}

/**
 * The subset of attempted ids whose archive FULFILLED, from the parallel outcome statuses of
 * `Promise.allSettled`. A rejected archive must not be treated as cleared — the row still needs
 * clearing and has to stay visible. An id with no matching outcome is treated as NOT fulfilled.
 */
export function fulfilledClearIds(
  ids: string[],
  outcomes: ('fulfilled' | 'rejected')[],
): string[] {
  return ids.filter((_id, i) => outcomes[i] === 'fulfilled');
}

/**
 * The session list with `ids` removed. This is a FILTER, never a patch: the surviving rows are the
 * same objects, so no field can be dropped. That matters — patching a local list FROM a mutation
 * result is the exact trap that loses `attachedTmux` and makes `isShellSession` silently drop a row
 * (see the `sessionFields` note in `ui/src/services/johnny-api.service.ts`).
 *
 * Why an optimistic local drop exists at all: `GraphQLClient.mutate` always goes to the worker while
 * `listSessions` only prefers the local host on localhost, and the worker caches `list_sessions` for
 * 5s per isolate with no cross-isolate invalidation. So on a phone the post-clear read can honestly
 * return the rows we just archived. Correcting locally makes the tap right immediately and leaves
 * `refresh()` as reconciliation.
 */
export function sessionsWithoutIds(sessions: AiSession[], ids: Iterable<string>): AiSession[] {
  const drop = new Set(ids);
  if (drop.size === 0) return sessions;
  return sessions.filter((s) => !drop.has(s.id));
}

// ── Suppression window ──────────────────────────────────────────────────────────────────────────
// `sessionsWithoutIds` fixes the tap, but `refresh()` replaces `sessions()` wholesale, so a read
// served from the worker's `list_sessions` cache would hand the cleared rows straight back and the
// user would watch them reappear — "it didn't work", two seconds later, which is the exact
// conclusion the optimistic drop exists to prevent. So a cleared id is also SUPPRESSED for a short
// window, applied to every read (automatic reconcile and manual pull-to-refresh alike).
//
// This cannot hide a legitimately re-created row. Ids are uuids, archiving never mints one, and
// attaching a pane again creates a NEW session id — so a suppressed id can only ever name the exact
// row that was cleared. It is also self-limiting in two independent ways: entries expire, and an id
// is dropped the moment a read comes back without it.

/**
 * How long a cleared id stays suppressed. Deliberately just over the worker's `list_sessions` TTL
 * (`list_sessions: 5_000` in `worker/lib/runtime/desktop-rpc-cache.ts`), which is the whole reason a
 * stale read is possible: one full cache lifetime plus a margin, so the window closes as soon as the
 * worker can no longer be serving the pre-clear list, and not a moment longer.
 */
export const SUPPRESSION_TTL_MS = 6_000;

/** `id -> epoch ms at which suppression lapses`. */
export type SuppressionEntries = Record<string, number>;

/** Suppress `ids` from `now` for one TTL. Returns a new object; existing entries are kept. */
export function addSuppressed(
  entries: SuppressionEntries,
  ids: Iterable<string>,
  now: number,
  ttlMs: number = SUPPRESSION_TTL_MS,
): SuppressionEntries {
  const next: SuppressionEntries = { ...entries };
  for (const id of ids) next[id] = now + ttlMs;
  return next;
}

/** The still-suppressed ids at `now`. The expiry instant itself counts as expired. */
export function suppressed(now: number, entries: SuppressionEntries): string[] {
  return Object.keys(entries).filter((id) => entries[id] > now);
}

/**
 * `sessions` with the still-suppressed ids removed. A filter, never a patch (see
 * `sessionsWithoutIds`), and the same array is returned when nothing is suppressed.
 */
export function applySuppression(
  sessions: AiSession[],
  entries: SuppressionEntries,
  now: number,
): AiSession[] {
  return sessionsWithoutIds(sessions, suppressed(now, entries));
}

/**
 * The entries worth keeping after a read. An entry goes when it has expired, OR when the read came
 * back WITHOUT that id — at that point the host and the UI agree, the suppression has done its job,
 * and keeping it is pure risk. Call this with the RAW read, before `applySuppression`.
 */
export function pruneSuppressed(
  entries: SuppressionEntries,
  read: Pick<AiSession, 'id'>[],
  now: number,
): SuppressionEntries {
  const stillPresent = new Set(read.map((s) => s.id));
  const next: SuppressionEntries = {};
  for (const [id, expiresAt] of Object.entries(entries)) {
    if (expiresAt > now && stillPresent.has(id)) next[id] = expiresAt;
  }
  return next;
}
