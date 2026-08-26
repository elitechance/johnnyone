import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const html = readFileSync(resolve(here, 'prompt-library.page.html'), 'utf8');
const ts = readFileSync(resolve(here, 'prompt-library.page.ts'), 'utf8');
const scss = readFileSync(resolve(here, 'prompt-library.page.scss'), 'utf8');
const routes = readFileSync(resolve(here, '../../app.routes.ts'), 'utf8');
const nav = readFileSync(resolve(here, '../../nav-items.ts'), 'utf8');

describe('prompt-library wiring', () => {
  it('renders Prompts title, filters, columns, Duplicate/Open, and + New', () => {
    expect(html).toMatch(/ion-title/);
    expect(html).toContain('Prompts');
    expect(html).toContain('Duplicate to edit');
    expect(html).toContain('Open editor');
    expect(html).toContain('+ New');
    expect(html).toContain('Customise a built-in via Duplicate');
    expect(html).toContain('not read by current engine');
    expect(html).toMatch(/ion-searchbar/);
    expect(html).toMatch(/ion-select/);
    expect(html).toContain('NAME');
    expect(html).toContain('ROLE');
    expect(html).toContain('SCOPE');
    expect(html).toContain('VER');
    expect(html).toContain('USED');
    expect(html).toMatch(/ion-header/);
    expect(html).toMatch(/ion-list/);
    expect(html).toMatch(/ion-button/);
    expect(html).toMatch(/ion-spinner/);
    expect(html).not.toMatch(/ion-textarea/);
    expect(html).not.toMatch(/\*ngIf|\*ngFor/);
    expect(html).toMatch(/@if/);
    expect(html).toMatch(/@for/);
  });

  it('loading and error branches are wired', () => {
    expect(html).toContain('Loading prompts');
    expect(html).toMatch(/ion-spinner/);
    expect(html).toContain('Could not load prompts');
    expect(html).toContain('Retry');
    expect(html).toContain('No prompts match');
  });

  it('page calls listPromptLibrary, filterRows, loadView, and usedCount', () => {
    expect(ts).toMatch(/listPromptLibrary/);
    expect(ts).toMatch(/filterRows/);
    expect(ts).toMatch(/loadView/);
    expect(html).toMatch(/usedCount/);
    expect(ts).toMatch(/editorHref|newHref|navigateByUrl|routerLink/);
  });

  it('editor buttons use queryParams, not a ? inside routerLink', () => {
    expect(html).toMatch(/routerLink="\/settings"/);
    expect(html).toMatch(/\[queryParams\]="editorQueryParams\(row\.key\)"/);
    expect(html).not.toMatch(/\[routerLink\]="editorHref/);
    expect(html).not.toMatch(/label="All roles"/);
    expect(html).not.toMatch(/label="All scopes"/);
    expect(html).toContain('placeholder="All roles"');
    expect(html).toContain('placeholder="All scopes"');
    expect(html).toContain('PlannerPromptSettings');
    expect(html).toMatch(/ion-note class="new-hint"/);
    expect(html).not.toMatch(/ion-note class="new-hint" slot="start"/);
    expect(html).toMatch(/<span class="used">[\s\S]*not read by current engine/);
    expect(ts).toMatch(/navigateByUrl\(editorHref/);
  });

  it('route is authenticated settings/prompts', () => {
    expect(routes).toMatch(/path:\s*'settings\/prompts'/);
    expect(routes).toMatch(/PromptLibraryPage/);
    expect(routes).toMatch(/authGuard/);
  });

  it('wraps at 900px not 640px and does not add a nav item', () => {
    expect(scss).toMatch(/900px/);
    expect(scss).not.toMatch(/640px/);
    expect(nav).not.toMatch(/id: 'prompts'/);
    expect(nav).toMatch(/path: '\/settings'/);
  });

  it('desktop filters stay on one row and USED is wider than 88px', () => {
    expect(scss).toMatch(/\.filters \{[\s\S]*flex-wrap:\s*nowrap/);
    expect(scss).toMatch(/\.filters ion-select \{[\s\S]*width:\s*max-content/);
    expect(scss).toMatch(/minmax\(200px,\s*1\.6fr\)/);
    expect(scss).not.toMatch(/88px 100px 48px 88px/);
    expect(html).toMatch(/class="used-count"/);
  });
});
