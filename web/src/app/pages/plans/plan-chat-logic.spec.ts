import { describe, it, expect } from 'vitest';
import {
  agentState,
  composerState,
  extractChoices,
  parseToolCalls,
  RETIRED_PLACEHOLDER,
  toolLabel,
  toolSummary,
} from './plan-chat-logic';

describe('plan-chat-logic', () => {
  it('maps tool names to short labels', () => {
    const table: [string, string][] = [
      ['mcp__j1__brief_update', 'brief.update'],
      ['mcp__j1__brief_get', 'brief.get'],
      ['mcp__j1__brief_versions', 'brief.versions'],
      ['mcp__j1__outline_add_phase', 'outline.add_phase'],
      ['mcp__j1__plan_check', 'plan.check'],
      ['repo_search', 'repo.search'],
      ['mcp__j1__repo_search', 'repo.search'],
      ['Read', 'Read'],
      ['mcp__other__thing', 'mcp__other__thing'],
      ['', ''],
    ];
    for (const [name, want] of table) expect(toolLabel(name), name).toBe(want);
  });

  it('summarises a brief.update with its version', () => {
    expect(
      toolSummary({ name: 'mcp__j1__brief_update', input: { field: 'outcome', value: 'x' }, result: '{"version":2,"changed_fields":["outcome"]}' }),
    ).toEqual({ label: 'brief.update', detail: 'outcome', mark: 'v2', tone: 'ok' });
    expect(
      toolSummary({ name: 'mcp__j1__brief_update', input: { field: 'out_of_scope', add: 'lens disposal' }, result: '{"version":3}' }).detail,
    ).toBe('out_of_scope += "lens disposal"');
    expect(toolSummary({ name: 'mcp__j1__outline_add_task', input: { phaseId: '00', title: 'swap' }, result: '{"version":4,"id":"00/04"}' })).toMatchObject({
      detail: '00 · swap',
      mark: 'v4',
    });
  });

  it('marks errors with ! and pending calls with …', () => {
    expect(toolSummary({ name: 'mcp__j1__outline_delete', input: { id: '77' }, result: 'unknown outline id `77`' })).toMatchObject({ mark: '!', tone: 'warn' });
    expect(toolSummary({ name: 'mcp__j1__brief_update', input: { field: 'why' }, result: '{"error":"no change"}' }).mark).toBe('!');
    expect(toolSummary({ name: 'mcp__j1__brief_get', input: {} })).toMatchObject({ mark: '…', tone: 'pending', detail: 'latest' });
    expect(toolSummary({ name: 'mcp__j1__brief_get', input: {}, result: '{"version":5,"fields":{}}' }).mark).toBe('✓');
  });

  it('summarises repo.search hits and plan.check counts (and survives a clipped result)', () => {
    expect(
      toolSummary({ name: 'mcp__j1__repo_search', input: { query: 'kill_terminal_session' }, result: '{"matches":[{},{},{}],"truncated":false}' }),
    ).toMatchObject({ detail: '"kill_terminal_session" → 3 hits', mark: '✓' });
    expect(toolSummary({ name: 'mcp__j1__repo_search', input: { query: 'q' }, result: '{"matches":[{"path":"a' }).detail).toBe('"q"');
    expect(
      toolSummary({ name: 'mcp__j1__plan_check', input: {}, result: JSON.stringify({ items: [{ status: 'ok' }, { status: 'ok' }, { status: 'warn' }] }) }),
    ).toEqual({ label: 'plan.check', detail: '2 ✓ · 1 warning', mark: '!', tone: 'warn' });
  });

  it('parses tool_calls JSON tolerantly', () => {
    expect(parseToolCalls('[{"id":"t1","name":"x","input":{}}]')).toHaveLength(1);
    expect(parseToolCalls('not json')).toEqual([]);
    expect(parseToolCalls(null)).toEqual([]);
    expect(parseToolCalls('{"a":1}')).toEqual([]);
  });

  it('extracts CHOICES from the last line only', () => {
    expect(extractChoices('Should I queue it?\nCHOICES: Queue it after the ledger | Start now | Show me the conflict')).toEqual({
      text: 'Should I queue it?',
      choices: ['Queue it after the ledger', 'Start now', 'Show me the conflict'],
    });
    expect(extractChoices('No choices here.')).toEqual({ text: 'No choices here.', choices: [] });
    expect(extractChoices('Hi\nCHOICES:  a | | b |  \n\n')).toEqual({ text: 'Hi', choices: ['a', 'b'] });
    expect(extractChoices('x\nCHOICES: 1 | 2 | 3 | 4 | 5 | 6').choices).toEqual(['1', '2', '3', '4']);
    const notLast = 'CHOICES: a | b\nmore text';
    expect(extractChoices(notLast)).toEqual({ text: notLast, choices: [] });
    expect(extractChoices('')).toEqual({ text: '', choices: [] });
    expect(extractChoices(null)).toEqual({ text: '', choices: [] });
  });

  it('disables the composer when retired or streaming', () => {
    expect(composerState('briefing', false)).toEqual({ disabled: false, placeholder: 'Discuss the plan…' });
    expect(composerState('briefing', true).disabled).toBe(true);
    expect(composerState('planning', false)).toEqual({ disabled: true, placeholder: RETIRED_PLACEHOLDER });
    expect(RETIRED_PLACEHOLDER).toBe('The plan agent is retired — this plan is in planning.');
  });

  it('derives the agent pill state', () => {
    expect(agentState('briefing', false)).toBe('idle');
    expect(agentState('briefing', true)).toBe('working');
    expect(agentState('planning', true)).toBe('retired');
  });
});
