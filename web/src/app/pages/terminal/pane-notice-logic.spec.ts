/**
 * G-1-1: the "job has ended, Transcript only" notice must REACH the user on both Raw surfaces.
 * The engine refusal arrives as (a) a rejected `terminal_command_ack` on the relay socket or (b) a
 * failed `captureTerminal`. This spec drives both through the REAL relay service and the page's
 * pure state rules, and checks what each surface renders. (`TerminalPage` cannot be
 * TestBed-compiled under the plugin-less vitest, so the template wiring is pinned as text.)
 */
import '@angular/compiler';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { Subject } from 'rxjs';
import { describe, expect, it } from 'vitest';
import { RelayTerminalService } from '../../services/relay-terminal.service';
import { AGENT_JOB_ENDED_NOTE } from '../shells/shells-page-logic';
import {
  errorText,
  paneRefusalNotice,
  rawPaneView,
  rejectionForView,
  shouldPollPane,
  surfacesAsTerminalError,
  withoutPaneNotice,
  withPaneNotice,
} from './pane-notice-logic';

/** `terminal::AGENT_JOB_ENDED`, verbatim. */
const ENGINE_REFUSAL = "This agent's job has ended; its pane is not restarted. Transcript only.";
const HTML = readFileSync(resolve(__dirname, 'terminal.page.html'), 'utf8');
const TS = readFileSync(resolve(__dirname, 'terminal.page.ts'), 'utf8');

function relay(): { svc: any; rejections: Array<{ sessionId: string; error: string }> } {
  const svc: any = Object.create(RelayTerminalService.prototype);
  svc.zone = { run: (fn: () => void) => fn() };
  svc.commandRejectionSubject = new Subject();
  svc.screenSubject = new Subject();
  svc.streamEventSubject = new Subject();
  svc.agentPlanRunSubject = new Subject();
  svc.sessionUpdatedSubject = new Subject();
  svc.sessionDeletedSubject = new Subject();
  svc.terminalScreenCache = { remember: () => undefined };
  const rejections: Array<{ sessionId: string; error: string }> = [];
  svc.commandRejections().subscribe((r: { sessionId: string; error: string }) => rejections.push(r));
  return { svc, rejections };
}

/** The page's state transition for a rejection (mirrors `TerminalPage.onCommandRejected`). */
function pageOnRejected(notices: Record<string, string>, ack: { sessionId: string; error: string }, viewed: string[]) {
  const r = rejectionForView({ ...ack, accepted: false }, viewed);
  return r ? withPaneNotice(notices, r.sessionId, paneRefusalNotice(r.error)) : notices;
}

describe('a rejected ack reaches the Raw surfaces', () => {
  it('the real relay service emits the rejection (it used to only console.error it)', () => {
    const { svc, rejections } = relay();
    const quiet = console.error;
    console.error = () => undefined;
    try {
      svc.handleMessage(JSON.stringify({
        type: 'terminal_command_ack',
        data: { requestId: 'r1', sessionId: 'S1', accepted: false, error: ENGINE_REFUSAL },
      }));
      svc.handleMessage(JSON.stringify({ type: 'terminal_command_ack', data: { requestId: 'r2', sessionId: 'S1', accepted: true } }));
    } finally {
      console.error = quiet;
    }
    expect(rejections).toEqual([{ sessionId: 'S1', error: ENGINE_REFUSAL, kind: 'unknown' }]);
  });

  it('→ page state → the /shells Raw and the console Raw tab render the notice, not a blank screen', () => {
    const { svc, rejections } = relay();
    const quiet = console.error;
    console.error = () => undefined;
    svc.handleMessage(JSON.stringify({ type: 'terminal_command_ack', data: { requestId: 'r', sessionId: 'S1', accepted: false, error: ENGINE_REFUSAL } }));
    console.error = quiet;
    let notices: Record<string, string> = {};
    for (const r of rejections) notices = pageOnRejected(notices, r, ['S1']);
    // /shells Raw (no screen yet) and the console Raw tab (primary session S1, no screen):
    for (const view of [rawPaneView('S1', false, notices, null), rawPaneView('S1', false, notices)]) {
      expect(view).toEqual({ kind: 'ended', note: AGENT_JOB_ENDED_NOTE, transcriptLink: '/shells/S1/transcript' });
    }
    // A recorded refusal wins even over a (cached) screen (G-2-24); an unrelated session is unaffected.
    expect(rawPaneView('S1', true, notices).kind).toBe('ended');
    expect(rawPaneView('S2', false, notices).kind).toBe('attach');
  });

  it('only for a session the page shows; other refusals are not a notice', () => {
    expect(rejectionForView({ sessionId: 'BG', accepted: false, error: 'x' }, ['S1', null])).toBeNull();
    expect(rejectionForView({ sessionId: 'S1', accepted: true }, ['S1'])).toBeNull();
    expect(rejectionForView({ sessionId: 'S1', accepted: false, error: '' }, ['S1'])?.error).toBe('The terminal command was refused.');
    expect(pageOnRejected({}, { sessionId: 'S1', error: 'session x is archived; refusing to resurrect it' }, ['S1'])).toEqual({});
  });
});

describe('a capture error reaches the console Raw tab', () => {
  it('the engine refusal from captureTerminal (Error or GraphQL error) becomes the notice', () => {
    for (const err of [new Error(ENGINE_REFUSAL), { message: `Error: ${ENGINE_REFUSAL}` }, ENGINE_REFUSAL]) {
      const notices = withPaneNotice({}, 'P1', paneRefusalNotice(err));
      expect(rawPaneView('P1', false, notices)).toEqual({
        kind: 'ended',
        note: AGENT_JOB_ENDED_NOTE,
        transcriptLink: '/shells/P1/transcript',
      });
    }
    expect(paneRefusalNotice(new Error('relay timeout'))).toBeNull(); // transient: skip the tick
    expect(errorText({ message: 'm' })).toBe('m');
  });

  it('a terminal error alone (no recorded notice) also renders it on /shells', () => {
    expect(rawPaneView('S1', false, {}, ENGINE_REFUSAL).kind).toBe('ended');
    expect(rawPaneView('S1', false, {}, 'something else').kind).toBe('attach');
  });
});

describe('page wiring (template/ts as text)', () => {
  it('the page subscribes to rejections and records the notice from captures', () => {
    expect(TS).toContain('this.relayTerminal.commandRejections().subscribe({');
    expect(TS).toContain('next: (ack) => this.onCommandRejected(ack),');
    expect(TS).toContain('this.paneNotices.update((m) => withPaneNotice(m, rejection.sessionId, paneRefusalNotice(rejection.error)));');
    const capture = TS.slice(TS.indexOf('private async capturePrimaryScreen('), TS.indexOf('private stopPrimaryScreenPoll('));
    expect(capture).toContain('const notice = paneRefusalNotice(err);');
    expect(capture).toContain('this.paneNotices.update((m) => withPaneNotice(m, sessionId, notice));');
  });
  it('both Raw surfaces render from rawPaneView, with an Open transcript action', () => {
    const consoleAt = HTML.indexOf("@let consoleRaw = rawPaneView(primarySessionId(), !!terminalScreens()[primarySessionId() ?? ''], paneNotices());");
    expect(consoleAt).toBeGreaterThan(-1);
    const consoleBlock = HTML.slice(consoleAt, HTML.indexOf("} @else if (consoleRaw.kind === 'attach') {", consoleAt));
    expect(consoleBlock).toContain("@if (consoleRaw.kind === 'ended') {");
    expect(consoleBlock).toContain('{{ consoleRaw.note }}');
    expect(consoleBlock).toContain('[routerLink]="consoleRaw.transcriptLink">Open transcript</a>');
    expect(HTML).toContain("} @else if (sh && rawPaneView(sh.id, !!shScreen, paneNotices(), terminalError(), endedNote(sh)).kind === 'ended') {");
  });
});

describe('G-2-17: only open/type refusals become the terminal error', () => {
  it('the real relay service tags each rejection with what was asked', () => {
    const { svc, rejections } = relay();
    const quiet = console.error;
    console.error = () => undefined;
    const resizeId = svc.newRequestId('resize');
    const subId = svc.newRequestId('visual_subscribe');
    svc.handleMessage(JSON.stringify({ type: 'terminal_command_ack', data: { requestId: resizeId, sessionId: 'S1', accepted: false, error: 'no pane' } }));
    svc.handleMessage(JSON.stringify({ type: 'terminal_command_ack', data: { requestId: subId, sessionId: 'S1', accepted: false, error: ENGINE_REFUSAL } }));
    console.error = quiet;
    expect(rejections.map((r) => r.kind)).toEqual(['resize', 'visual_subscribe']);
    expect(svc.requestKinds.size).toBe(0); // forgotten once answered
  });
  it('classifies kinds', () => {
    for (const k of ['input', 'visual_subscribe', 'visual_refresh', 'visual_history', 'terminal_visual_subscribe']) {
      expect(surfacesAsTerminalError(k)).toBe(true);
    }
    for (const k of ['resize', 'terminal_kill', 'unknown', undefined]) expect(surfacesAsTerminalError(k)).toBe(false);
    const handler = TS.slice(TS.indexOf('protected onCommandRejected('), TS.indexOf('private subscribeToRelayErrorEvents('));
    expect(handler).toContain('surfacesAsTerminalError(ack.kind)');
  });
});

describe('G-2-22: a refused pane is not polled again', () => {
  it('shouldPollPane stops once a notice is recorded', () => {
    expect(shouldPollPane('P1', 'P1', {})).toBe(true);
    expect(shouldPollPane('P1', 'P1', { P1: 'x' })).toBe(false);
    expect(shouldPollPane('P1', 'P2', {})).toBe(false);
  });
  it('the tick checks it before capturing AND before re-arming', () => {
    const tick = TS.slice(TS.indexOf('const tick = async () => {'), TS.indexOf('this.primaryScreenPollInterval = setTimeout(', TS.indexOf('const tick = async () => {')));
    expect(tick.match(/if \(!shouldPollPane\(sessionId, this\.primaryScreenSessionId, this\.paneNotices\(\)\)\) return;/g)?.length).toBe(2);
  });
});

describe('G-2-24: a recorded notice wins over a cached screen', () => {
  it('rawPaneView prefers the notice; a live frame clears it', () => {
    const notices = withPaneNotice({}, 'P1', AGENT_JOB_ENDED_NOTE);
    expect(rawPaneView('P1', true, notices).kind).toBe('ended');
    const cleared = withoutPaneNotice(notices, 'P1');
    expect(rawPaneView('P1', true, cleared).kind).toBe('screen');
    // A stale terminal error alone does not hide a screen.
    expect(rawPaneView('P1', true, {}, ENGINE_REFUSAL).kind).toBe('screen');
    expect(TS).toContain('this.paneNotices.update((m) => withoutPaneNotice(m, screen.sessionId));');
  });
});
