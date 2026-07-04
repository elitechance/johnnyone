import { describe, it, expect } from 'vitest';

// The §06 `+ New` launcher DECISION logic is extracted into an Angular/Ionic-free module (overhaul P6,
// D6) so it can be unit-tested directly under the plugin-less web vitest — the real
// `LauncherMenuComponent` pulls in Ionic + the P2 `JohnnyApiService`. The component delegates its
// entries/intents/create-inputs/terminal-route decisions to exactly these functions. This spec pins
// acceptance criteria A.1–A.5 from the phase overview.
import {
  LAUNCHER_ENTRIES,
  launcherIntent,
  terminalRoute,
  plainTerminalRoute,
  isPlainShellSurface,
  shellCreateInput,
  attachTmuxInput,
} from './launcher-logic';

describe('launcher — pure logic', () => {
  // A.1 — menu entries
  describe('LAUNCHER_ENTRIES', () => {
    it('has exactly the four §06 kinds, in order', () => {
      expect(LAUNCHER_ENTRIES).toHaveLength(4);
      expect(LAUNCHER_ENTRIES.map((e) => e.kind)).toEqual([
        'initiative',
        'shell',
        'attach',
        'files',
      ]);
    });

    it('carries the §06 titles/subtitles verbatim (incl. the · middle dot)', () => {
      expect(LAUNCHER_ENTRIES).toEqual([
        { kind: 'initiative', title: 'New initiative', subtitle: 'describe it → plan & build', icon: 'create-outline' },
        { kind: 'shell', title: 'Raw shell', subtitle: '$SHELL · run commands yourself', icon: 'terminal-outline' },
        { kind: 'attach', title: 'Attach to tmux session', subtitle: 'pick an existing pane', icon: 'git-network-outline' },
        { kind: 'files', title: 'Open file manager', subtitle: 'browse the host FS', icon: 'folder-outline' },
      ]);
    });

    it('uses the exact shell copy and the middle-dot separator', () => {
      const shell = LAUNCHER_ENTRIES.find((e) => e.kind === 'shell');
      expect(shell?.title).toBe('Raw shell');
      expect(shell?.subtitle).toBe('$SHELL · run commands yourself');
      expect(shell?.subtitle).toContain('·');
      expect(LAUNCHER_ENTRIES.find((e) => e.kind === 'files')?.title).toBe('Open file manager');
    });
  });

  // A.2 — navigational intents
  describe('launcherIntent — navigational kinds', () => {
    it('initiative → navigate /briefing/new', () => {
      expect(launcherIntent('initiative')).toEqual({ type: 'navigate', path: '/briefing/new' });
    });

    it('files → navigate /files', () => {
      expect(launcherIntent('files')).toEqual({ type: 'navigate', path: '/files' });
    });
  });

  // A.3 — session intents
  describe('launcherIntent — session kinds', () => {
    it('shell → createShell marker', () => {
      expect(launcherIntent('shell')).toEqual({ type: 'createShell' });
    });

    it('attach → attachPicker marker', () => {
      expect(launcherIntent('attach')).toEqual({ type: 'attachPicker' });
    });
  });

  // A.4 — terminal route
  describe('terminalRoute', () => {
    it('is the shared /initiatives?sessionId= nav shape', () => {
      expect(terminalRoute('abc')).toEqual({ path: '/initiatives', queryParams: { sessionId: 'abc' } });
    });

    it('carries NO surface param (non-shell callers unchanged)', () => {
      expect('surface' in terminalRoute('abc').queryParams).toBe(false);
    });
  });

  // P4 — plain-shell route + surface predicate
  describe('plainTerminalRoute', () => {
    it('opens the session on its own /shells/:sessionId destination (not /terminal)', () => {
      expect(plainTerminalRoute('abc')).toEqual({
        path: '/shells/abc',
        queryParams: {},
      });
    });

    it('carries the id as a path segment and no surface query (route data drives plain mode)', () => {
      const route = plainTerminalRoute('x');
      expect(route.path).toBe('/shells/x');
      expect(route.queryParams).toEqual({});
    });
  });

  describe('isPlainShellSurface', () => {
    it('is true when the surface param is shell (drives first paint)', () => {
      expect(isPlainShellSurface('shell')).toBe(true);
      expect(isPlainShellSurface('shell', null)).toBe(true);
      expect(isPlainShellSurface('shell', { provider: 'claude_code', attachedTmux: false })).toBe(true);
    });

    it('is true for a shell/attached session even with no param (backstop)', () => {
      expect(isPlainShellSurface(null, { provider: 'shell', attachedTmux: false })).toBe(true);
      expect(isPlainShellSurface(undefined, { provider: 'codex', attachedTmux: true })).toBe(true);
    });

    it('is false for an agent session with no shell param', () => {
      expect(isPlainShellSurface(null, { provider: 'claude_code', attachedTmux: false })).toBe(false);
      expect(isPlainShellSurface('', { provider: 'codex', attachedTmux: false })).toBe(false);
    });

    it('is false and null-safe when neither param nor session is a shell', () => {
      expect(isPlainShellSurface(null)).toBe(false);
      expect(isPlainShellSurface(null, null)).toBe(false);
      expect(isPlainShellSurface(undefined, undefined)).toBe(false);
    });
  });

  // A.5 — create inputs (both branches)
  describe('shellCreateInput', () => {
    it('includes workingDirectory when a cwd is given', () => {
      expect(shellCreateInput('/home/x')).toEqual({ provider: 'shell', workingDirectory: '/home/x' });
    });

    it('omits the workingDirectory key entirely when there is no cwd', () => {
      expect(shellCreateInput(undefined)).toEqual({ provider: 'shell' });
      expect('workingDirectory' in shellCreateInput(undefined)).toBe(false);
      expect('workingDirectory' in shellCreateInput('')).toBe(false);
    });
  });

  describe('attachTmuxInput', () => {
    it('attaches to the named external tmux pane AND carries title=name (de-dupe join fix)', () => {
      expect(attachTmuxInput('kloo')).toEqual({ tmuxSessionName: 'kloo', title: 'kloo' });
    });

    it('keeps provider absent (host attaches without one)', () => {
      expect('provider' in attachTmuxInput('kloo')).toBe(false);
    });
  });
});
