import { describe, expect, it } from 'vitest';
import { agentActivityLine, agentIsBusy } from './shell-activity';

// Fixtures are verbatim rows captured from live Claude Code panes.
const WORKING = [
  '  120',
  '',
  '✽ Pontificating… (9m 12s · ↓ 19.8k tokens)',
  '───────────────────────────────',
  '❯ ',
  '  ⏵⏵ auto mode on (shift+tab to cycle) · esc to interrupt · ← for agents',
].join('\n');

const FINISHED = [
  '  ⎿  $ seq 1 120',
  '✽ Cogitated for 6s · done 1:15 PM',
  '❯ ',
  '  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents',
].join('\n');

describe('agentActivityLine', () => {
  it('returns the live status line with its glyph stripped', () => {
    expect(agentActivityLine(WORKING)).toBe('Pontificating… (9m 12s · ↓ 19.8k tokens)');
  });

  it('ignores a finished turn — "done 1:15 PM" is not current activity', () => {
    expect(agentActivityLine(FINISHED)).toBeNull();
  });

  it('takes the NEWEST status line when a pane holds several', () => {
    const pane = ['* Effecting… (1s · ↓ 4 tokens)', 'x', '✽ Julienning… (7s · ↓ 566 tokens)'].join('\n');
    expect(agentActivityLine(pane)).toBe('Julienning… (7s · ↓ 566 tokens)');
  });

  it('accepts ascii ellipsis and hour-scale elapsed', () => {
    expect(agentActivityLine('◐ Thinking... (1h 2m 3s)')).toBe('Thinking... (1h 2m 3s)');
  });

  it('collapses the padding runs a TUI leaves between columns', () => {
    expect(agentActivityLine('✽ Working…      (12s   ·   ↓ 1k)')).toBe('Working… (12s · ↓ 1k)');
  });

  it('requires BOTH an ellipsis and an elapsed timer', () => {
    expect(agentActivityLine('✽ Pontificating…')).toBeNull();
    expect(agentActivityLine('build finished (12s)')).toBeNull();
  });

  it('is safe on empty, null and whitespace panes', () => {
    expect(agentActivityLine(null)).toBeNull();
    expect(agentActivityLine(undefined)).toBeNull();
    expect(agentActivityLine('')).toBeNull();
    expect(agentActivityLine('\n\n   \n')).toBeNull();
  });
});

describe('agentIsBusy', () => {
  it('detects the interrupt affordance shown mid-turn', () => {
    expect(agentIsBusy(WORKING)).toBe(true);
  });

  it('is false at an idle composer', () => {
    expect(agentIsBusy(FINISHED)).toBe(false);
    expect(agentIsBusy(null)).toBe(false);
  });
});
