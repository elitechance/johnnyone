// Pure, Angular/Ionic-free Shells decision logic (overhaul P6, decisions D3/D4/D6) so it can be
// unit-tested under the plugin-less web vitest — the real `ShellsPage` pulls in Ionic + the Phase-01
// launcher, which that config cannot import. The page delegates its filter/partition/dedupe/label/
// rel-time/open decisions to exactly these functions and stays thin wiring.
//
// `AiSession`/`TmuxSession` are imported type-only (erased at build) at the same relative depth the
// shipped pure siblings use (`briefing-page-logic.ts:6` — five `../`), so no Angular/runtime dependency
// leaks in. `openIntent` REUSES Phase 01's `terminalRoute` — the `/terminal?sessionId=` nav shape is
// defined once (D3), not re-derived here.
import type { AiSession } from '../../../../../ui/src/models/ai-session.model';
import type { TmuxSession } from '../../../../../ui/src/services/johnny-api.service';
import { terminalRoute } from '../../components/launcher-menu/launcher-logic';

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
 * Route args to open a shell session on the existing terminal surface. Reuses Phase 01's
 * `terminalRoute` verbatim — the `/terminal?sessionId=` shape is defined once (D3).
 */
export const openIntent = terminalRoute;
