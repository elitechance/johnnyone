import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const html = readFileSync(resolve(here, 'settings.page.html'), 'utf8');
const ts = readFileSync(resolve(here, 'settings.page.ts'), 'utf8');
const routes = readFileSync(resolve(here, '../../app.routes.ts'), 'utf8');

describe('settings-prompts wiring', () => {
  it('template uses ion-textarea, smallMode, next-run copy, Reset, and seam names', () => {
    expect(html).toMatch(/ion-textarea/);
    expect(html).toMatch(/smallMode/);
    expect(html).toMatch(/next run/i);
    expect(html).toMatch(/Reset/);
    expect(html).toMatch(/savePrompts/);
    expect(html).toMatch(/resetPrompt/);
    expect(html).toMatch(/differsFromDefault/);
    expect(html).toMatch(/smallModeSupported/);
  });

  it('failed-save copy is in the template', () => {
    expect(html).toContain('Could not save planner prompts.');
  });

  it('textareas have no maxlength', () => {
    expect(html).not.toMatch(/maxlength/);
  });

  it('settings.page.ts calls GET/UPDATE and failure helpers', () => {
    expect(ts).toMatch(/getPlannerPromptSettings/);
    expect(ts).toMatch(/updatePlannerPromptSettings/);
    expect(ts).toMatch(/applySaveFailure/);
    expect(ts).toMatch(/applyResetFailure/);
    expect(ts).toMatch(/fromSettings/);
  });

  it('route remains /settings → SettingsPage', () => {
    expect(routes).toMatch(/path:\s*'settings'/);
    expect(routes).toMatch(/SettingsPage/);
  });

  it('Planner prompts card links to the Prompt library', () => {
    expect(html).toContain('Prompt library');
    expect(html).toMatch(/settings\/prompts/);
  });
});
