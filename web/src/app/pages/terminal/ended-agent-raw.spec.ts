/**
 * Live UI bug 1 (#78 regression): `/shells/<id>/raw` for a finished or archived PLAN agent was a
 * black pane, although the engine's job-ended refusal reached `relay-terminal.service.ts`.
 *
 * Root causes:
 *  (a) the archived-means-ended rule was `shViewOnly && !isPlanAgentShell(sh)`, so an ARCHIVED plan
 *      agent never counted as ended. Its pane is never requested by this page (an archived session
 *      is not in the active list, so no visual subscribe or resize is sent), so no refusal arrives
 *      to fill the gap: the raw screen rendered empty. The banner also kept saying "talk to the plan
 *      agent in its plan's chat".
 *  (b) ordering: a refusal ack that lands before `getSession` resolves the session was dropped by
 *      `rejectionForView` (nothing viewed yet) and never applied later.
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { AGENT_JOB_ENDED_NOTE, PLAN_AGENT_VIEW_ONLY_NOTE } from '../shells/shells-page-logic';
import {
  ARCHIVED_SHELL_NOTE,
  ENDED_CARD_SUBCOPY,
  HELD_REFUSAL_TTL_MS,
  applyHeldRefusals,
  rawEndedCardShown,
  endedNote,
  heldNotice,
  holdRefusal,
  paneRefusalNotice,
  rawPaneView,
  readOnlyNote,
  rejectionForView,
  withPaneNotice,
} from './pane-notice-logic';

const ENGINE_REFUSAL = "This agent's job has ended; its pane is not restarted. Transcript only.";
const HTML = readFileSync(resolve(__dirname, 'terminal.page.html'), 'utf8');
const TS = readFileSync(resolve(__dirname, 'terminal.page.ts'), 'utf8');

/** What the /shells Raw surface renders for `sh` (mirrors the template expression). */
function shellRaw(sh: { id: string; planAgent?: boolean; status?: string }, notices: Record<string, string> = {}, hasScreen = false) {
  return rawPaneView(sh.id, hasScreen, notices, null, endedNote(sh));
}

describe('an ended plan agent never shows a blank Raw pane (bug 1)', () => {
  it('an ARCHIVED plan agent shows the job-ended notice with no refusal at all', () => {
    const sh = { id: 'PA', planAgent: true, status: 'archived' };
    expect(shellRaw(sh)).toEqual({ kind: 'ended', note: AGENT_JOB_ENDED_NOTE, transcriptLink: '/shells/PA/transcript' });
    // Even over a cached (non-live) frame.
    expect(shellRaw(sh, {}, true).kind).toBe('ended');
    expect(readOnlyNote(sh, PLAN_AGENT_VIEW_ONLY_NOTE, null)).toBe(AGENT_JOB_ENDED_NOTE);
  });

  it('a LIVE plan agent is view-only, not ended', () => {
    const sh = { id: 'PA', planAgent: true, status: 'active' };
    expect(endedNote(sh)).toBeNull();
    expect(shellRaw(sh, {}, true).kind).toBe('screen');
    expect(readOnlyNote(sh, PLAN_AGENT_VIEW_ONLY_NOTE, null)).toBe(PLAN_AGENT_VIEW_ONLY_NOTE);
    // ...until its job-ended refusal is recorded.
    expect(readOnlyNote(sh, PLAN_AGENT_VIEW_ONLY_NOTE, AGENT_JOB_ENDED_NOTE)).toBe(AGENT_JOB_ENDED_NOTE);
  });

  it('an archived ordinary shell keeps the generic ended note', () => {
    expect(endedNote({ status: 'archived' })).toBe(ARCHIVED_SHELL_NOTE);
    expect(endedNote({ status: 'active' })).toBeNull();
    expect(endedNote(null)).toBeNull();
  });

  it('ordering: a refusal that arrives BEFORE the session is shown is held, then applied', () => {
    const now = 1_000;
    const ack = { sessionId: 'PA', accepted: false, error: ENGINE_REFUSAL };
    // The page has not resolved the session yet: nothing is viewed, so the ack is not for the view.
    expect(rejectionForView(ack, [undefined, null])).toBeNull();
    const held = holdRefusal({}, ack, now);
    // The session resolves (an ACTIVE plan agent, so no status-based note): the held notice applies.
    const notice = heldNotice(held, 'PA', now + 500);
    expect(notice).toBe(AGENT_JOB_ENDED_NOTE);
    const notices = withPaneNotice({}, 'PA', notice);
    expect(shellRaw({ id: 'PA', planAgent: true, status: 'active' }, notices)).toMatchObject({ kind: 'ended', note: AGENT_JOB_ENDED_NOTE });
    // Same notice whichever way it arrives.
    expect(notice).toBe(paneRefusalNotice(ENGINE_REFUSAL));
  });

  it('only job-ended refusals are held, briefly, and never for an accepted ack', () => {
    expect(holdRefusal({}, { sessionId: 'X', accepted: false, error: 'resize failed' }, 0)).toEqual({});
    expect(holdRefusal({}, { sessionId: 'X', accepted: true, error: ENGINE_REFUSAL }, 0)).toEqual({});
    expect(holdRefusal({}, { sessionId: null, accepted: false, error: ENGINE_REFUSAL }, 0)).toEqual({});
    const held = holdRefusal({}, { sessionId: 'X', accepted: false, error: ENGINE_REFUSAL }, 0);
    expect(heldNotice(held, 'X', HELD_REFUSAL_TTL_MS)).toBe(AGENT_JOB_ENDED_NOTE);
    expect(heldNotice(held, 'X', HELD_REFUSAL_TTL_MS + 1)).toBeNull();
    expect(heldNotice(held, 'Y', 0)).toBeNull();
    // Bounded: old entries are dropped as new ones arrive.
    let h = {};
    for (let i = 0; i < 80; i++) h = holdRefusal(h, { sessionId: `S${i}`, accepted: false, error: ENGINE_REFUSAL }, i);
    expect(Object.keys(h).length).toBeLessThanOrEqual(50);
    expect(heldNotice(h, 'S79', 79)).toBe(AGENT_JOB_ENDED_NOTE);
  });

  it('applyHeldRefusals: applies a shown session\'s held notice once, ignores the rest', () => {
    const held = holdRefusal(holdRefusal({}, { sessionId: 'PA', accepted: false, error: ENGINE_REFUSAL }, 0), { sessionId: 'BG', accepted: false, error: ENGINE_REFUSAL }, 0);
    expect(applyHeldRefusals({}, held, [undefined, null], 10)).toBeNull();
    const r = applyHeldRefusals({ X: 'kept' }, held, ['PA', null], 10)!;
    expect(r.notices).toEqual({ X: 'kept', PA: AGENT_JOB_ENDED_NOTE });
    expect(Object.keys(r.held)).toEqual(['BG']);
    // Applied once: a live frame clears the notice, and the held entry is gone, so it stays cleared.
    expect(applyHeldRefusals({}, r.held, ['PA'], 20)).toBeNull();
    // Expired entries never apply.
    expect(applyHeldRefusals({}, held, ['PA'], HELD_REFUSAL_TTL_MS + 1)).toBeNull();
  });

  it('the page wires it (anchored to live code lines, not comments)', () => {
    expect(HTML).toContain("rawPaneView(sh.id, !!shScreen, paneNotices(), terminalError(), endedNote(sh)).kind === 'ended'");
    expect(HTML).not.toContain('shViewOnly && !isPlanAgentShell(sh)');
    const code = TS.split('\n').filter((l) => !/^\s*(\/\/|\*|\/\*)/.test(l)).join('\n');
    expect(code).toMatch(/^\s+this\.heldRefusals\.update\(\(h\) => holdRefusal\(h, \{ \.\.\.ack, accepted: false \}, Date\.now\(\)\)\);$/m);
    expect(code).toMatch(/^\s+const ids = \[this\.currentSession\(\)\?\.id, this\.primarySessionId\(\)\];$/m);
    expect(code).toMatch(/^\s+const held = this\.heldRefusals\(\);$/m);
    expect(code).toMatch(/^\s+const next = applyHeldRefusals\(this\.paneNotices\(\), held, ids, Date\.now\(\)\);$/m);
    expect(code).toMatch(/^\s+this\.paneNotices\.set\(next\.notices\);\n\s+this\.heldRefusals\.set\(next\.held\);$/m);
  });

  it('U-1-11: the ended notice shows once (card on Raw, bar elsewhere), with the new sub-copy', () => {
    expect(rawEndedCardShown('raw', true, 'ended')).toBe(true);
    expect(rawEndedCardShown('watchdog', true, 'ended')).toBe(true); // view-only /watchdog renders Raw
    expect(rawEndedCardShown('watchdog', false, 'ended')).toBe(false);
    expect(rawEndedCardShown('transcript', true, 'ended')).toBe(false);
    expect(rawEndedCardShown('raw', false, 'screen')).toBe(false);
    expect(ENDED_CARD_SUBCOPY).toBe("It won't restart. The Transcript keeps what it did.");
    expect(HTML).toContain('<div class="empty-copy">{{ endedCardSubcopy }}</div>');
    expect(HTML).toContain('@if (topViewOnly && !endedCard) {');
    expect(HTML).toContain('@if (topJobEnded && !topViewOnly && !endedCard) {');
    expect(HTML).toContain("@let endedCard = rawEndedCardShown(shellView(), topViewOnly, sh ? rawPaneView(sh.id, !!terminalScreens()[sh.id], paneNotices(), terminalError(), endedNote(sh)).kind : null);");
  });
});
