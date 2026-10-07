import type { UrlMatchResult, UrlSegment } from '@angular/router';

/**
 * URL shape for a single shell surface: `/shells/:sessionId[/transcript|/raw]`, plus the pure
 * route→state resolution the page applies when that URL changes.
 *
 * Kept as a logic module with no component imports so `app.routes.ts` can import the matcher
 * STATICALLY — a `UrlMatcher` runs during URL recognition, before any lazy chunk is fetched, so it
 * cannot itself be lazy. Everything here is total and DOM-free, so it is specced directly
 * (`shells-route.spec.ts`) rather than through the terminal page, which cannot be mounted under
 * this vitest config (no Angular plugin; see `terminal-reconcile-integration.spec.ts`).
 */

/** The two renderings of one shell pane: reflowing prose vs. the fixed-width tmux mirror. */
export type ShellView = 'transcript' | 'raw';

/** Every accepted view segment, in the order the toggle presents them. */
export const SHELL_VIEWS: readonly ShellView[] = ['transcript', 'raw'];

/** Bare `/shells/:id` renders the transcript: it reflows at any width, while the raw pane is a
 *  fixed-width tmux mirror. Raw is now something you ask for by URL, not the default you land on. */
export const DEFAULT_SHELL_VIEW: ShellView = 'transcript';

/** Narrowing guard for an untrusted, already-lowercased value (a hand-typed URL, a stale bookmark). */
export function isShellView(v: string | null | undefined): v is ShellView {
  return v === 'transcript' || v === 'raw';
}

/**
 * Matches `/shells/:sessionId` with an OPTIONAL view segment.
 *
 * One route config serves all three URLs on purpose. Angular destroys and recreates the routed
 * component whenever the MATCHED CONFIG changes, so sibling or child routes per view would rebuild
 * the whole `TerminalPage` on every Transcript/Raw tap: a fresh `ngOnInit`, a fresh `sessions()` /
 * `terminalScreens()` / transcript buffer, re-read `tabOrder` and `paneLayouts`, and a full
 * re-subscribe of the visual + stream lanes. (The xterm view itself is NOT what this saves — the
 * template's `@if (shellView() === 'transcript') … @else` destroys and recreates
 * `johnny-terminal-screen` either way. What survives is the PAGE, which is why toggling back
 * repaints raw instantly from the still-cached `terminalScreens()[id]` instead of waiting on a new
 * subscription.)
 *
 * The view segment is matched CASE-INSENSITIVELY (the folding happens in `resolveShellRoute`, which
 * also replaces the URL with the canonical spelling), because URL segments are case-sensitive while
 * the things that carry URLs are not: iOS
 * autocapitalisation and link rewriters turn a pasted `/shells/<id>/raw` into `/shells/<id>/Raw`,
 * and the shareable link is the entire reason this route exists.
 *
 * An unrecognised third segment still MATCHES rather than returning null. Falling through to the
 * `**` wildcard would silently dump the operator on someone's initiative console with no message
 * and no shell; instead the page renders the default view and replaces the URL with the canonical
 * one (see `resolveShellRoute`), so the address bar self-heals in place.
 */
export function matchShellRoute(segments: UrlSegment[]): UrlMatchResult | null {
  if (segments.length < 2 || segments.length > 3) return null;
  if (segments[0].path !== 'shells') return null;
  // An empty session id would resolve the page with no session to open; `/shells` alone is the
  // list route and must keep matching that config, not this one.
  if (!segments[1].path) return null;
  if (segments.length === 2) {
    return { consumed: segments, posParams: { sessionId: segments[1] } };
  }
  // The segment is passed through VERBATIM, not lower-cased here: canonicalising would mean
  // constructing a `UrlSegment`, i.e. a runtime `@angular/router` import, and this module is
  // imported by `launcher-logic.ts`, which is deliberately Angular-free so it stays loadable under
  // the plugin-less web vitest config. `resolveShellRoute` does the case folding instead — one pure
  // place, and the same place that decides the URL needs healing.
  return { consumed: segments, posParams: { sessionId: segments[1], view: segments[2] } };
}

/**
 * The ONE builder for these URLs — `plainTerminalRoute` (the open-a-shell destination) delegates
 * here, so there is no second place that can drift. Omitting `view` (or passing `null`, or passing
 * the default) yields the bare, shareable `/shells/:id`; passing `raw` yields the explicit form, so
 * a link pasted into chat reopens on the same rendering.
 */
export function shellRoutePath(sessionId: string, view?: ShellView | null): string {
  return view ? `/shells/${sessionId}/${view}` : `/shells/${sessionId}`;
}

/**
 * Where the Transcript/Raw toggle navigates. Selecting the DEFAULT view goes back to the bare
 * `/shells/:id` rather than minting `/shells/:id/transcript`: identical pixels, and because the
 * toggle navigates with `replaceUrl` the explicit form would quietly overwrite the short shareable
 * URL in place. `/shells/:id/transcript` stays a valid URL if someone types it — the toggle just
 * never produces one.
 */
export function shellViewTogglePath(sessionId: string, view: ShellView): string {
  return view === DEFAULT_SHELL_VIEW ? shellRoutePath(sessionId) : shellRoutePath(sessionId, view);
}

/** What the page knows about its own session state when a shell URL arrives. */
export interface ShellRouteState {
  /** The session currently selected and rendered, if one has resolved. */
  currentSessionId: string | null | undefined;
  /** The session a `loadSessions` call is already in flight for, if any. */
  pendingSessionId: string | null | undefined;
  /** Sessions already in `sessions()`, selectable without a fetch. */
  knownSessionIds: readonly string[];
}

export interface ShellRouteResolution {
  /** The view to render. Always concrete — the URL's absence or garbage both resolve to the default. */
  view: ShellView;
  /** A canonical URL to `replaceUrl`-navigate to, or `null` when the URL is already canonical. */
  redirectTo: string | null;
  /** What to do about the session the URL names. */
  session: 'none' | 'select' | 'load';
}

/**
 * Pure resolution of one shell URL against the page's current state. Exists as a seam because the
 * two things it decides are both ordering traps that a refactor silently breaks:
 *
 *  - The VIEW must be resolved unconditionally, even when the session is unchanged. The page's
 *    `paramMap` handler early-returns on an unchanged session id, which is exactly the case for a
 *    Transcript/Raw toggle — deciding the view after that return makes the toggle a no-op.
 *  - The SESSION guard must consider the load already in flight, not just the resolved session.
 *    `currentSessionId` is undefined until the first fetch lands, so on a slow relay a toggle tapped
 *    during boot passed an unguarded `currentSession()`-only check and fired a second concurrent
 *    `loadSessions` for the same id, racing the one `ngOnInit` started.
 */
export function resolveShellRoute(
  params: { sessionId: string | null | undefined; view: string | null | undefined },
  state: ShellRouteState,
): ShellRouteResolution {
  const raw = params.view ?? null;
  const lowered = raw === null ? null : raw.toLowerCase();
  const sessionId = params.sessionId || null;

  let view: ShellView = DEFAULT_SHELL_VIEW;
  let redirectTo: string | null = null;
  if (lowered !== null) {
    if (isShellView(lowered)) {
      view = lowered;
      // Only a non-canonical spelling needs healing. `/shells/x/transcript` stays as typed — it is
      // a valid, meaningful URL; the toggle simply never manufactures it (it uses the bare form).
      if (raw !== lowered && sessionId) redirectTo = shellRoutePath(sessionId, lowered);
    } else if (sessionId) {
      redirectTo = shellRoutePath(sessionId);
    }
  }

  let session: ShellRouteResolution['session'] = 'none';
  if (sessionId && sessionId !== state.currentSessionId && sessionId !== state.pendingSessionId) {
    session = state.knownSessionIds.includes(sessionId) ? 'select' : 'load';
  }

  return { view, redirectTo, session };
}
