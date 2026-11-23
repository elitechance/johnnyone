import { describe, it, expect } from 'vitest';
import {
  changedCounts,
  checkRows,
  detailState,
  diffBase,
  diffView,
  fieldRows,
  isFrozen,
  outlineCards,
  startGate,
  summaryLine,
  undoTarget,
  versionChips,
  whereItems,
} from './plan-brief-logic';

const NOW = '2026-11-23T12:10:00Z';

// The 08 mock: v1 you (seed) … v5 agent.
const versions = [
  { version: 1, author: 'you', changeNote: 'seeded from the ask', changed: ['outcome', 'why'], createdAt: '2026-11-23 12:05:00' },
  { version: 2, author: 'agent', changeNote: '', changed: ['outcome'], createdAt: '2026-11-23 12:06:00' },
  { version: 3, author: 'agent', changeNote: '', changed: ['why', 'where', 'out_of_scope'], createdAt: '2026-11-23 12:06:30' },
  { version: 4, author: 'agent', changeNote: '', changed: ['done_when', 'decided', 'outline:00/04'], createdAt: '2026-11-23 12:10:00' },
  { version: 5, author: 'agent', changeNote: '', changed: ['out_of_scope'], createdAt: '2026-11-23 12:10:00' },
];

const latest = {
  initiativeId: 'P',
  version: 5,
  author: 'agent',
  changeNote: '',
  changed: ['out_of_scope'],
  createdAt: '2026-11-23 12:10:00',
  initiativeStatus: 'briefing',
  acceptedBriefVersion: null,
  fields: {
    outcome: 'Every agent tmux session is killed and archived when its job ends.',
    why: 'Planner sessions stayed alive.\nLens reviewers are fine.',
    where: ['services/agent_plans.rs', 'terminal.rs'],
    doneWhen: 'cargo test -j 8 --lib\nretired session is not resurrected',
    outOfScope: 'Lens disposal\nanything not johnnyone_<uuid>',
    decided: 'Retire = kill + archive.',
  },
  outline: {
    phases: [
      {
        id: '00',
        title: 'Retire on job end',
        author: 'you',
        addedIn: 2,
        tasks: [
          { id: '00/01', title: 'retire helper', author: 'you', addedIn: 2 },
          { id: '00/04', title: 'swap & revive', author: 'agent', addedIn: 4 },
        ],
      },
      { id: '01', title: 'Orphan sweep', author: 'agent', addedIn: 3, tasks: [] },
    ],
  },
};

const item = (rule: string, status: string, detail: string, initiativeIds: string[] = []) => ({
  rule,
  status,
  detail,
  paths: [],
  initiativeIds,
});

describe('plan-brief-logic', () => {
  it('highlights exactly the fields changed in the latest version, with provenance', () => {
    const rows = fieldRows(latest as never, versions as never, NOW);
    expect(rows.map((r) => r.key)).toEqual(['outcome', 'why', 'where', 'done_when', 'out_of_scope', 'decided']);
    expect(rows.filter((r) => r.changed).map((r) => r.key)).toEqual(['out_of_scope']);
    const by = Object.fromEntries(rows.map((r) => [r.key, r]));
    expect([by.outcome.who, by.outcome.version]).toEqual(['agent', 2]);
    expect([by.why.who, by.why.version]).toEqual(['agent', 3]);
    expect([by.done_when.version, by.out_of_scope.version]).toEqual([4, 5]);
    expect(by.out_of_scope.ago).toBe('just now');
    expect(by.where.chips).toEqual(['services/agent_plans.rs', 'terminal.rs']);
    expect(by.why.lines).toEqual(['Planner sessions stayed alive.', 'Lens reviewers are fine.']);
    expect(by.out_of_scope.canUndo).toBe(true);
    expect(by.outcome.canUndo).toBe(false); // not changed in the latest version
  });

  it('computes per-field undo targets (the version before the latest change)', () => {
    expect(undoTarget(versions as never, 'out_of_scope')).toBe(4);
    expect(undoTarget(versions as never, 'outcome')).toBe(1);
    expect(undoTarget(versions as never, 'decided')).toBe(3);
    expect(undoTarget([versions[0]] as never, 'outcome')).toBeNull();
    expect(undoTarget(versions as never, 'never_changed')).toBeNull();
  });

  it('never highlights or offers undo on v1, and freezes after accept', () => {
    const v1 = { ...latest, version: 1, changed: ['outcome', 'why'] };
    expect(fieldRows(v1 as never, [versions[0]] as never, NOW).some((r) => r.changed)).toBe(false);
    const frozen = { ...latest, initiativeStatus: 'planning', acceptedBriefVersion: 5 };
    expect(isFrozen(frozen)).toBe(true);
    expect(isFrozen(latest as never)).toBe(false);
    expect(isFrozen({ initiativeStatus: 'briefing', acceptedBriefVersion: 3 })).toBe(true);
    expect(fieldRows(frozen as never, versions as never, NOW).some((r) => r.canUndo)).toBe(false);
  });

  it('counts the phone badges per pane', () => {
    expect(changedCounts(latest)).toEqual({ brief: 1, outline: 0 });
    expect(changedCounts({ version: 4, changed: versions[3].changed })).toEqual({ brief: 2, outline: 1 });
    expect(changedCounts({ version: 1, changed: ['outcome'] })).toEqual({ brief: 0, outline: 0 });
    expect(changedCounts(null)).toEqual({ brief: 0, outline: 0 });
  });

  it('renders check rows with icons and tones', () => {
    expect(
      checkRows([item('outcome_one_sentence', 'ok', 'Outcome is one sentence'), item('file_conflict', 'warn', 'X touches a.rs'), item('where_exists', 'fail', 'Not found: b')]),
    ).toEqual([
      { rule: 'outcome_one_sentence', icon: '✓', tone: 'ok', text: 'Outcome is one sentence' },
      { rule: 'file_conflict', icon: '!', tone: 'warn', text: 'X touches a.rs' },
      { rule: 'where_exists', icon: '✗', tone: 'fail', text: 'Not found: b' },
    ]);
  });

  it('gates Start: a fail asks to confirm but never blocks (D11); a conflict shows Queue; frozen blocks', () => {
    const fail = startGate([item('outcome_one_sentence', 'fail', 'Outcome is more than one sentence'), item('done_when_runnable', 'ok', 'ok')], 'briefing');
    expect(fail).toEqual({ canStart: true, needsConfirm: true, confirmLines: ['Outcome is more than one sentence'], showQueue: false, queueAfter: [] });
    const conflict = startGate([item('file_conflict', 'warn', 'Ledger touches agent_plans.rs', ['L1', 'L1', 'L2'])], 'briefing');
    expect(conflict.showQueue).toBe(true);
    expect(conflict.queueAfter).toEqual(['L1', 'L2']);
    expect(conflict.needsConfirm).toBe(false);
    expect(startGate([item('file_conflict', 'warn', 'x', ['L1'])], 'planning').canStart).toBe(false);
    expect(startGate([], 'briefing', true)).toMatchObject({ canStart: false, showQueue: false });
    expect(startGate([], 'briefing').canStart).toBe(true);
  });

  it('summarises the sticky bar', () => {
    expect(summaryLine(5, [item('a', 'ok', ''), item('file_conflict', 'warn', '')])).toBe('Brief v5 · 1 warning');
    expect(summaryLine(2, [item('a', 'fail', ''), item('b', 'fail', '')])).toBe('Brief v2 · 2 failed checks');
    expect(summaryLine(3, [item('a', 'ok', '')])).toBe('Brief v3 · all checks pass');
  });

  it('builds outline cards with agent tags and the latest-version highlight', () => {
    const cards = outlineCards(latest.outline as never, versions[3].changed);
    expect(cards.map((c) => c.id)).toEqual(['00', '01']);
    expect(cards[0].agentTag).toBeNull();
    expect(cards[1].agentTag).toBe('agent v3');
    expect(cards[0].tasks[1]).toMatchObject({ id: '00/04', num: '04', agentTag: 'agent v4', added: true, last: true });
    expect(cards[0].tasks[0]).toMatchObject({ added: false, first: true });
    expect(cards[0]).toMatchObject({ first: true, last: false });
    expect(outlineCards(null, [])).toEqual([]);
  });

  it('builds the diff view with del/ins lines and an unchanged note', () => {
    const v = diffView({
      fields: [
        { field: 'done_when', changed: true, removed: [], added: ['retired session is archived'] },
        { field: 'decided', changed: true, removed: ['old'], added: ['Retire = kill + archive'] },
        { field: 'outcome', changed: false, removed: [], added: [] },
        { field: 'why', changed: false, removed: [], added: [] },
      ],
      outline: { added: [{ id: '00/04', title: 'swap & revive' }], removed: [], renamed: [{ id: '01', before: 'Sweep', after: 'Orphan sweep' }], moved: [] },
    });
    expect(v.sections.map((s) => s.title)).toEqual(['Done when', 'Already decided', 'Outline']);
    expect(v.sections[0]).toEqual({ title: 'Done when', del: ['(empty)'], ins: ['retired session is archived'] });
    expect(v.sections[2].ins).toEqual(['00/04 swap & revive', '01: Orphan sweep']); // the +/− marker is styling
    expect(v.sections[2].del).toEqual(['01: Sweep']);
    expect(v.unchangedNote).toBe('2 fields unchanged');
    expect(diffView(null).empty).toBe(true);
  });

  it('labels the version strip and its diff base', () => {
    expect(versionChips(versions as never).map((c) => c.label)).toEqual(['v1 you', 'v2', 'v3', 'v4', 'v5 agent']);
    expect(versionChips(versions as never).filter((c) => c.latest).map((c) => c.version)).toEqual([5]);
    expect(diffBase(5)).toBe(4);
    expect(diffBase(1)).toBeNull();
  });

  it('decides the detail load state (S11)', () => {
    const briefing = { status: 'draft', initiativeStatus: 'briefing' };
    expect(detailState(true, null, null)).toBe('loading');
    expect(detailState(false, 'No online backend app found', null)).toBe('error');
    expect(detailState(false, 'Plan not found: abc', null)).toBe('not-found');
    expect(detailState(false, 'plan abc not found', null)).toBe('not-found');
    expect(detailState(false, null, null)).toBe('not-found');
    expect(detailState(false, null, { status: 'closed', initiativeStatus: 'briefing' })).toBe('not-found');
    expect(detailState(false, null, { status: 'draft', initiativeStatus: 'planning' })).toBe('frozen');
    expect(detailState(false, null, briefing)).toBe('ready');
    expect(detailState(true, null, briefing)).toBe('ready'); // a refresh keeps the page
  });

  it('splits where input into items', () => {
    expect(whereItems(' a.rs \n\n b/ ')).toEqual(['a.rs', 'b/']);
    expect(whereItems(null)).toEqual([]);
  });
});
