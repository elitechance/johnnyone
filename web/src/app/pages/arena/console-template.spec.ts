// A static guard over `terminal.page.html`, which is the ONE piece of this work that no mounted
// test can reach: TerminalPage uses `templateUrl` + `styleUrls`, and under this project's vitest
// config (no @analogjs plugin) such a component cannot be compiled — TestBed fails with
// "Component 'TerminalPage' is not resolved". So the wiring is held two ways instead: the
// production build type-checks the template, and this asserts the five tab branches that existed
// before are all still there beside the new one.
//
// It is a text assertion and it says so. It cannot prove the Arena tab renders; it can prove nobody
// deleted the Transcript/Plan/Diff/Checks/Tasks branches while adding it, which is the regression
// the mock warns about ("Arena is an added tab, never a replacement").
import { describe, it, expect } from 'vitest';
import { readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';

const HTML = readFileSync(join(process.cwd(), 'web/src/app/pages/terminal/terminal.page.html'), 'utf8');
const SCSS_PATH = join(process.cwd(), 'web/src/app/pages/terminal/terminal.page.scss');

describe('terminal.page.html — the arena is added, nothing is replaced', () => {
  // CORRECTED mid-round: there is no `activeTab() === 'raw'` branch. Raw is the chain's trailing
  // `@else`, which is also why appending the arena branch inside the chain cannot steal it.
  it('still branches on every tab that existed before, with Raw as the fallback', () => {
    for (const tab of ['plan', 'diff', 'checks', 'tasks']) {
      expect(HTML, `the ${tab} branch is gone`).toContain(`activeTab() === '${tab}'`);
    }
    expect(HTML).not.toContain("activeTab() === 'raw'");
    expect(HTML).toContain('rawAttachNeeded(primarySessionId()');
    expect(HTML).toContain('<johnny-terminal-screen');
  });

  it('adds the arena branch and renders the arena panel in it', () => {
    expect(HTML).toContain("activeTab() === 'arena'");
    expect(HTML).toContain('<app-arena-panel');
    expect(HTML).toContain('[phases]="arenaPhases()"');
  });

  it('puts the arena row on the phase cards inside the PLAN tab, not in a new browser', () => {
    const planStart = HTML.indexOf("activeTab() === 'plan'");
    const checksStart = HTML.indexOf("activeTab() === 'checks'");
    const rowAt = HTML.indexOf('<johnny-arena-row');
    expect(planStart).toBeGreaterThan(-1);
    expect(rowAt).toBeGreaterThan(planStart);
    expect(rowAt).toBeLessThan(checksStart);
  });

  it('puts the gate strip in the CHECKS tab (mock step 5)', () => {
    const checksStart = HTML.indexOf("activeTab() === 'checks'");
    const stripAt = HTML.indexOf('<johnny-gate-strip');
    expect(stripAt).toBeGreaterThan(checksStart);
    expect(HTML).toContain('[gate]="phaseGate()"');
  });

  it('keeps the plan navigator and the artifacts section the Plan tab already had', () => {
    expect(HTML).toContain('class="docnav"');
    expect(HTML).toContain('planArtifacts()');
    expect(HTML).toContain('johnny-markdown-view');
  });

  it('adds NOTHING to terminal.page.scss, which has ~150 bytes of its 48 kB budget left', () => {
    // Measured: the compiled component style is 47.85 kB against a 48 kB error budget, so any new
    // rule here fails the build. Every arena style lives in the new components' own inline sheets.
    // The byte count is the SOURCE file; the budget applies to the compiled output.
    expect(statSync(SCSS_PATH).size).toBe(74503);
    expect(readFileSync(SCSS_PATH, 'utf8')).not.toContain('arena');
  });
});
