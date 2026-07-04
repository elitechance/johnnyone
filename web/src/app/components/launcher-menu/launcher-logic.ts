// Pure, Angular/Ionic-free launcher decision logic (overhaul P6, decisions D2/D3/D5/D6) so it can be
// unit-tested under the plugin-less web vitest — the real `LauncherMenuComponent` pulls in Ionic + the
// P2 `JohnnyApiService`, which that config cannot import. The component delegates every launcher
// decision (which entries, what each does, the create inputs, the terminal nav shape) to exactly these
// functions and stays thin wiring; Phase 02's `/shells` page reuses `terminalRoute` verbatim.
//
// `CreateAiSessionInput` is imported type-only (erased at build time) at the same relative depth the
// shipped pure siblings use to reach `ui/` types (`shells-page-logic.ts` — five `../`), so no
// Angular/runtime dependency leaks in. NOTE: the service-file interface (with `workingDirectory` +
// `tmuxSessionName`) is the one `createSession` actually accepts — not the narrower barrel-exported
// `models/ai-session.model` shape.
import type { CreateAiSessionInput } from '../../../../../ui/src/services/johnny-api.service';
import type { AiSession } from '../../../../../ui/src/models/ai-session.model';

/** The four §06 launcher rows. Order is the mock's order and is asserted by the spec. */
export type LauncherKind = 'initiative' | 'shell' | 'attach' | 'files';

/** A single launcher row: what it is, plus the icon/title/subtitle rendered in the popover (mock §06). */
export interface LauncherEntry {
  kind: LauncherKind;
  title: string;
  subtitle: string;
  icon: string;
}

/**
 * What choosing a launcher row means, resolved in the pure layer. The two navigational kinds resolve
 * fully to a path; the two session kinds return a marker the thin component acts on with IO
 * (create-a-shell / open-the-attach-picker).
 */
export type LauncherIntent =
  | { type: 'navigate'; path: string }
  | { type: 'createShell' }
  | { type: 'attachPicker' };

/**
 * The §06 `+ New` launcher entries, in the mock's order. Subtitles are the §06 copy verbatim (including
 * the `·` middle dot in the shell subtitle). Icons are ionicons names the component registers.
 */
export const LAUNCHER_ENTRIES: readonly LauncherEntry[] = [
  { kind: 'initiative', title: 'New initiative', subtitle: 'describe it → plan & build', icon: 'create-outline' },
  { kind: 'shell', title: 'Raw shell', subtitle: '$SHELL · run commands yourself', icon: 'terminal-outline' },
  { kind: 'attach', title: 'Attach to tmux session', subtitle: 'pick an existing pane', icon: 'git-network-outline' },
  { kind: 'files', title: 'Open file manager', subtitle: 'browse the host FS', icon: 'folder-outline' },
];

/**
 * Map a launcher row to its intent (D2/D3/D5):
 *   initiative → navigate /briefing/new (P4)   ·   files → navigate /files (P5)
 *   shell      → createShell marker            ·   attach → attachPicker marker
 */
export function launcherIntent(kind: LauncherKind): LauncherIntent {
  switch (kind) {
    case 'initiative':
      return { type: 'navigate', path: '/briefing/new' };
    case 'files':
      return { type: 'navigate', path: '/files' };
    case 'shell':
      return { type: 'createShell' };
    case 'attach':
      return { type: 'attachPicker' };
  }
}

/**
 * The shared "open this session in the existing terminal surface" nav shape (P3 `?sessionId=`
 * deep-link, D3). Reused by Phase 02's `/shells` page. The component spreads it into
 * `router.navigate([route.path], { queryParams: route.queryParams })`.
 */
export function terminalRoute(sessionId: string): { path: string; queryParams: { sessionId: string } } {
  return { path: '/initiatives', queryParams: { sessionId } };
}

/**
 * Nav shape for opening a **shell / attached-tmux** session as a PLAIN terminal (overhaul P9 / phase P4,
 * D5). A shell is NOT an initiative, so it gets its OWN destination — `/shells/:sessionId` — instead of
 * the initiative console at `/terminal` (fix for the shells-route finding). The route carries
 * `data: { surface: 'shell' }`, so the terminal page renders the plain surface from first paint with no
 * `surface=shell` query param. `queryParams` stays empty (the session id is a path segment) so the
 * `router.navigate([route.path], { queryParams })` call shape at both callers is unchanged.
 * `terminalRoute` is left unchanged for any non-shell caller.
 */
export function plainTerminalRoute(
  sessionId: string,
): { path: string; queryParams: Record<string, never> } {
  return { path: `/shells/${sessionId}`, queryParams: {} };
}

/**
 * Whether the terminal page should render the PLAIN shell surface (no initiative chrome): true when the
 * `surface` query param is `'shell'` (drives first paint) OR the resolved session is a shell / attached
 * -tmux pane (`provider === 'shell' || attachedTmux === true`, the backstop once the session loads).
 * Pure/total and null-safe.
 */
export function isPlainShellSurface(
  surfaceParam: string | null | undefined,
  session?: Pick<AiSession, 'provider' | 'attachedTmux'> | null,
): boolean {
  if (surfaceParam === 'shell') return true;
  return !!session && (session.provider === 'shell' || session.attachedTmux === true);
}

/**
 * Create-input for a raw shell (`CliProvider::Shell`, D5). The `workingDirectory` key is ABSENT when
 * there is no cwd (not set to `undefined`) so the host falls back to its own default — the launcher does
 * not carry a path-picker (that stays on the terminal page's New-session flow).
 */
export function shellCreateInput(cwd?: string): CreateAiSessionInput {
  return cwd ? { provider: 'shell', workingDirectory: cwd } : { provider: 'shell' };
}

/**
 * Create-input for attaching to an existing external tmux pane (D1) — attach instead of spawn. Carries
 * `title: name` so the attached pane matches the `attachableTmux` de-dupe join (`title === tmux.name`)
 * and leaves the "attachable" list — mirroring the working terminal-page attach (`terminal.page.ts`,
 * `title: … || name`). `provider` stays absent (the host attaches without one). Fix for finding #3(a).
 */
export function attachTmuxInput(name: string): CreateAiSessionInput {
  return { tmuxSessionName: name, title: name };
}
