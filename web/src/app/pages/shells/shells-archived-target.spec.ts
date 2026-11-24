/**
 * G-1-1 (/shells): a URL naming an ARCHIVED session (a finished worker; the console's "Open
 * transcript" link) opens THAT session read-only — never `activeSessions[0]`, which would be a
 * different, typeable shell.
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { chooseSessionToSelect } from '../terminal/terminal-state-reconcile';
import {
  ARCHIVED_SHELL_NOTE,
  isReadOnlyShell,
  rawPaneView,
  readOnlyNote,
  transcriptRowsFromMessages,
} from '../terminal/pane-notice-logic';
import { matchShellRoute, resolveShellRoute, shellRoutePath } from './shells-route';
import { AGENT_JOB_ENDED_NOTE, PLAN_AGENT_VIEW_ONLY_NOTE } from './shells-page-logic';

const TS = readFileSync(resolve(__dirname, '../terminal/terminal.page.ts'), 'utf8');
const HTML = readFileSync(resolve(__dirname, '../terminal/terminal.page.html'), 'utf8');
const ARCHIVED = 'a1a1a1a1-0000-4000-8000-000000000001';
const LIVE = 'b2b2b2b2-0000-4000-8000-000000000002';

describe('an archived target is selected, never another session', () => {
  it('chooseSessionToSelect returns the archived target, not activeSessions[0]', () => {
    expect(chooseSessionToSelect([{ id: LIVE }], { targetId: ARCHIVED })).toBe(ARCHIVED);
    expect(chooseSessionToSelect([{ id: LIVE }], { targetId: ARCHIVED, currentId: LIVE })).not.toBe(LIVE);
  });

  it('the console "Open transcript" link resolves to that session, opened read-only', () => {
    const link = shellRoutePath(ARCHIVED, 'transcript');
    const segments = link.split('/').filter(Boolean).map((path) => ({ path, parameters: {} })) as never;
    const m = matchShellRoute(segments)!;
    expect(m).not.toBeNull();
    const sid = (m.posParams as Record<string, { path: string }>)['sessionId'].path;
    const view = (m.posParams as Record<string, { path: string }>)['view'].path;
    expect(sid).toBe(ARCHIVED);
    const r = resolveShellRoute({ sessionId: sid, view }, { currentSessionId: LIVE, pendingSessionId: null, knownSessionIds: [LIVE] });
    expect(r).toEqual({ view: 'transcript', redirectTo: null, session: 'load' });
    // The load then picks the target itself (not in the active list):
    expect(chooseSessionToSelect([{ id: LIVE }], { targetId: sid, currentId: LIVE })).toBe(ARCHIVED);
  });

  it('it renders read-only: a view-only banner, the Raw notice, its stored messages as the transcript', () => {
    const archived = { status: 'archived', planAgent: false };
    expect(isReadOnlyShell(archived)).toBe(true);
    expect(isReadOnlyShell({ status: 'active', planAgent: false })).toBe(false);
    expect(isReadOnlyShell({ status: 'active', planAgent: true })).toBe(true);
    expect(readOnlyNote(archived, PLAN_AGENT_VIEW_ONLY_NOTE, null)).toBe(ARCHIVED_SHELL_NOTE);
    expect(readOnlyNote(archived, PLAN_AGENT_VIEW_ONLY_NOTE, AGENT_JOB_ENDED_NOTE)).toBe(AGENT_JOB_ENDED_NOTE);
    expect(readOnlyNote({ planAgent: true }, PLAN_AGENT_VIEW_ONLY_NOTE, null)).toBe(PLAN_AGENT_VIEW_ONLY_NOTE);
    expect(rawPaneView(ARCHIVED, false, {}, null, true)).toEqual({
      kind: 'ended',
      note: ARCHIVED_SHELL_NOTE,
      transcriptLink: `/shells/${ARCHIVED}/transcript`,
    });
    // A recorded job-ended refusal names it more precisely.
    expect(rawPaneView(ARCHIVED, false, { [ARCHIVED]: AGENT_JOB_ENDED_NOTE }, null, true)).toMatchObject({ note: AGENT_JOB_ENDED_NOTE });
    expect(
      transcriptRowsFromMessages([
        { role: 'user', content: 'build it', created_at: 't1' },
        { role: 'assistant', content: 'done', created_at: 't2' },
        { role: 'tool', content: 'x' },
        { role: 'assistant', content: '  ' },
      ]),
    ).toEqual([
      { type: 'you', text: 'build it', at: 't1' },
      { type: 'agent', text: 'done', at: 't2' },
    ]);
  });

  it('page wiring: no fallback, no live-list entry, no input lane, messages as transcript', () => {
    const load = TS.slice(TS.indexOf('async loadSessions('), TS.indexOf('private reconcilePersistedTerminalState('));
    expect(load).toContain('const selected = toSelect ? await this.selectSession(toSelect) : false;');
    expect(load).toContain('if (!selected && (!toSelect || toSelect === targetSessionId)) {');
    const select = TS.slice(TS.indexOf('async selectSession(id: string)'), TS.indexOf('async archiveSession('));
    expect(select).toContain('if (!isReadOnlyShell({ status: session.status })) this.upsertSession(session);');
    const rows = TS.slice(TS.indexOf('protected shellRows()'), TS.indexOf('protected shellActivity()'));
    expect(rows).toContain('return transcriptRowsFromMessages(this.messages());');
    expect(HTML).toContain('@let topViewOnly = isReadOnlyShell(sh);');
    expect(HTML).toContain('@let shViewOnly = isReadOnlyShell(sh);');
    expect(HTML).toContain('[showInput]="!shViewOnly"');
  });
});
