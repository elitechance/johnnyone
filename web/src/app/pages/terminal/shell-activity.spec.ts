import { describe, expect, it } from 'vitest';
import {
  agentActivityLine,
  agentIsBusy,
  ansiLineToHtml,
  paneTail,
  paneTailHtml,
  pollDelayMs,
  reportFilePaths,
  screenIdleKey,
} from './shell-activity';

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

describe('agentActivityLine with real wire data', () => {
  // Captured verbatim off the relay. Every word carries its own colour change — the hand-written
  // fixtures above are far cleaner than anything that actually arrives, which is exactly how the
  // escape codes shipped unnoticed the first time.
  const WIRE =
    '[38;5;174m✽[39m [38;5;180mMustering…[38;5;174m ' +
    '[38;5;246m(36s · ↓[39m [38;5;246m2.0k tokens)[39m';

  it('returns clean text with no escape codes left in it', () => {
    const out = agentActivityLine(WIRE);
    expect(out).toBe('Mustering… (36s · ↓ 2.0k tokens)');
    expect(out).not.toContain('');
    expect(out).not.toMatch(/\[\d+;/);
  });

  it('strips the colour code sitting in front of the spinner glyph', () => {
    // The glyph is wrapped, so a strip that ran after matching would leave "✽" attached.
    expect(agentActivityLine(WIRE)?.startsWith('Mustering')).toBe(true);
  });

  it('finds the line inside a full coloured pane', () => {
    const pane = ['[2m  120[0m', '', WIRE, '[90m────[0m', '❯ '].join('\n');
    expect(agentActivityLine(pane)).toBe('Mustering… (36s · ↓ 2.0k tokens)');
  });

  it('detects busy through colour codes split across the phrase', () => {
    expect(agentIsBusy('[2m · [36mesc to [0minterrupt[0m')).toBe(true);
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


describe('screenIdleKey (provider-agnostic churn)', () => {
  it('ignores colour changes - a repaint in new colours is not progress', () => {
    const a = '\u001b[38;5;174mBuilding\u001b[39m';
    const b = '\u001b[38;5;180mBuilding\u001b[39m';
    expect(screenIdleKey(a)).toBe(screenIdleKey(b));
  });

  it('ignores the blinking cursor block', () => {
    expect(screenIdleKey('ready \u2588')).toBe(screenIdleKey('ready'));
  });

  it('ignores blank-line churn and trailing padding', () => {
    expect(screenIdleKey('a\n\n\nb')).toBe(screenIdleKey('a\nb'));
    expect(screenIdleKey('a   \nb')).toBe(screenIdleKey('a\nb'));
  });

  it('DOES change when real content changes', () => {
    expect(screenIdleKey('Worked for 13s')).not.toBe(screenIdleKey('Worked for 14s'));
  });

  it('works on a Grok pane, which has none of the Claude markers', () => {
    const grok = [
      '     Worked for 13s',
      '  | \u276f                  |',
      '  Shift+Tab:mode  |  Ctrl+x:shortcuts',
    ].join('\n');
    expect(agentIsBusy(grok)).toBe(false);
    expect(agentActivityLine(grok)).toBeNull();
    expect(screenIdleKey(grok).length).toBeGreaterThan(0);
    expect(screenIdleKey(grok)).not.toBe(screenIdleKey(grok.replace('13s', '19s')));
  });

  it('is empty for nothing at all', () => {
    expect(screenIdleKey(null)).toBe('');
    expect(screenIdleKey('\n  \n')).toBe('');
  });
});


describe('paneTail', () => {
  it('returns the last N meaningful lines', () => {
    const pane = Array.from({ length: 40 }, (_, i) => `line ${i}`).join('\n');
    const tail = paneTail(pane, 5);
    expect(tail).toEqual(['line 35', 'line 36', 'line 37', 'line 38', 'line 39']);
  });

  it('strips escapes so the preview is plain text', () => {
    const tail = paneTail('\u001b[38;5;180mBuilding\u001b[39m', 5);
    expect(tail).toEqual(['Building']);
    expect(tail[0]).not.toContain('\u001b');
  });

  it('drops separator rules and blank padding but keeps real content', () => {
    const pane = [
      'Ran 1 shell command',
      '\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500',
      '',
      '------',
      '2 + 2 = 4',
    ].join('\n');
    expect(paneTail(pane, 10)).toEqual(['Ran 1 shell command', '2 + 2 = 4']);
  });

  it('keeps a prompt line - it is content, not chrome', () => {
    expect(paneTail('\u276f what is 2+2?', 5)).toEqual(['\u276f what is 2+2?']);
  });

  it('works on a Grok pane', () => {
    const grok = ['     Worked for 13s', '  Shift+Tab:mode  |  Ctrl+x:shortcuts'].join('\n');
    expect(paneTail(grok, 10)).toEqual(['     Worked for 13s', '  Shift+Tab:mode  |  Ctrl+x:shortcuts']);
  });

  it('is empty for nothing, and never returns zero lines for a bad depth', () => {
    expect(paneTail(null)).toEqual([]);
    expect(paneTail('')).toEqual([]);
    expect(paneTail('a\nb', 0)).toEqual(['b']);
  });
});


describe('ansiLineToHtml / paneTailHtml (colour preserved)', () => {
  const E = '';

  it('wraps 256-colour runs in styled spans', () => {
    const html = ansiLineToHtml(`${E}[38;5;180mMustering${E}[39m`);
    expect(html).toContain('<span style="color:');
    expect(html).toContain('Mustering');
  });

  it('maps the basic 30-37 range', () => {
    expect(ansiLineToHtml(`${E}[31mred${E}[0m`)).toContain('#cd3131');
    expect(ansiLineToHtml(`${E}[32mgreen${E}[0m`)).toContain('#0dbc79');
  });

  it('resets colour on 0 and 39 so a run does not bleed', () => {
    const html = ansiLineToHtml(`${E}[31mred${E}[0mplain`);
    expect(html.endsWith('plain')).toBe(true);
  });

  it('carries bold and dim as weight and opacity', () => {
    expect(ansiLineToHtml(`${E}[1mbold${E}[22m`)).toContain('font-weight:600');
    expect(ansiLineToHtml(`${E}[2mdim${E}[22m`)).toContain('opacity:.65');
  });

  it('ESCAPES pane content - markup in the terminal can never become markup here', () => {
    const html = ansiLineToHtml('<img src=x onerror=alert(1)>');
    expect(html).not.toContain('<img');
    expect(html).toContain('&lt;img');
  });

  it('escapes inside a coloured run too', () => {
    const html = ansiLineToHtml(`${E}[31m<b>hi</b>${E}[0m`);
    expect(html).not.toContain('<b>');
    expect(html).toContain('&lt;b&gt;');
  });

  it('drops non-SGR escapes, which mean nothing in a static block', () => {
    expect(ansiLineToHtml(`${E}[2Jcleared`)).toBe('cleared');
  });

  it('plain text passes through untouched', () => {
    expect(ansiLineToHtml('just text')).toBe('just text');
  });

  it('paneTailHtml keeps the same lines as paneTail, but coloured', () => {
    const pane = [`${E}[31mone${E}[0m`, '───', `${E}[32mtwo${E}[0m`].join('\n');
    const html = paneTailHtml(pane, 10);
    expect(html.length).toBe(2);
    expect(html[0]).toContain('one');
    expect(html[1]).toContain('#0dbc79');
  });

  it('paneTailHtml is empty for nothing', () => {
    expect(paneTailHtml(null)).toEqual([]);
    expect(paneTailHtml('')).toEqual([]);
  });
});

describe('pollDelayMs (adaptive sampling)', () => {
  it('polls fast only while the pane is changing', () => {
    expect(pollDelayMs(true, 2000, 10000)).toBe(2000);
    expect(pollDelayMs(false, 2000, 10000)).toBe(10000);
  });

  it('idles far slower than it runs — that is the whole point', () => {
    expect(pollDelayMs(false, 500, 5000)).toBeGreaterThan(pollDelayMs(true, 500, 5000) * 4);
  });
});

describe('reportFilePaths', () => {
  it('finds a viewable path inside prose and backticks', () => {
    const md = 'Saved to `personal/plans/x/transcript-sample.html` for review.';
    expect(reportFilePaths(md)).toEqual(['personal/plans/x/transcript-sample.html']);
  });

  it('takes images and docs, ignores prose that merely contains dots', () => {
    expect(reportFilePaths('see a/b/shot.png and e.g. this')).toEqual(['a/b/shot.png']);
  });

  it('ignores paths with no viewable extension', () => {
    expect(reportFilePaths('run src/app/main.ts and node_modules/x.mjs')).toEqual([]);
  });

  it('requires a slash — a bare filename is probably prose', () => {
    expect(reportFilePaths('the README.md file')).toEqual([]);
  });

  it('de-duplicates and caps the list', () => {
    const md = Array.from({ length: 12 }, (_, i) => `d/f${i}.png`).join(' ') + ' d/f0.png';
    const out = reportFilePaths(md);
    expect(out.length).toBe(6);
    expect(new Set(out).size).toBe(6);
  });

  it('is empty for nothing', () => {
    expect(reportFilePaths(null)).toEqual([]);
    expect(reportFilePaths('')).toEqual([]);
  });
});
