/**
 * Live UI bugs 5 and 6 on the plan detail page.
 *  5. A CLOSED plan (the engine soft-deletes by closing: `delete_plan` sets `status='closed'`, so the
 *     row and title remain) said "Plan not found. It was deleted, or the link is wrong." under its
 *     real title. It now says "This plan is closed", read-only. A truly missing plan keeps not-found.
 *  6. An approved pre-#76 plan has no brief versions, only the markdown `brief` (live DB: e.g.
 *     8ad3ae71, approved/planning, 8 KB brief, 0 versions; `check_latest` grades it as an empty
 *     brief). It showed empty fields and "4 failed checks"; it now renders that markdown read-only.
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { CLOSED_PLAN_COPY, detailState, legacyBriefMarkdown } from './plan-brief-logic';

const HTML = readFileSync(resolve(__dirname, 'plan-detail.page.html'), 'utf8');
const TS = readFileSync(resolve(__dirname, 'plan-detail.page.ts'), 'utf8');

/** The legacy plan shape, as the engine returns it (getBriefVersion errors → null, no versions). */
const LEGACY = { status: 'approved', initiativeStatus: 'planning', brief: '# Outcome\nShip the thing.\n' };

describe('a closed plan is "closed", not "not found" (bug 5)', () => {
  it('a closed row → closed; a missing one → not-found', () => {
    expect(detailState(false, null, { status: 'closed', initiativeStatus: 'planning' })).toBe('closed');
    expect(detailState(false, null, { status: 'closed', initiativeStatus: 'briefing' })).toBe('closed');
    expect(detailState(false, 'Plan not found: abc', null)).toBe('not-found');
    expect(detailState(false, null, null)).toBe('not-found');
  });

  it('the page renders a styled, human closed card with a way back (L7, L10)', () => {
    expect(CLOSED_PLAN_COPY.title).toBe('This plan is closed');
    expect(CLOSED_PLAN_COPY.body).not.toMatch(/link is wrong|not found/i);
    const at = HTML.indexOf("@case ('closed') {");
    expect(at).toBeGreaterThan(-1);
    const block = HTML.slice(at, HTML.indexOf('@default {', at));
    expect(block).toContain('<ion-card class="state-card empty">');
    expect(block).toContain('{{ closedCopy.title }}');
    expect(block).toContain('routerLink="/plans"');
    expect(block).toContain("[routerLink]=\"['/initiatives']\"");
    expect(HTML).toContain('<h2>Plan not found</h2>'); // still there for a truly missing plan
  });
});

describe('a legacy plan shows its markdown brief, not empty fields + failed checks (bug 6)', () => {
  it('legacyBriefMarkdown: only for an unversioned, past-briefing plan with a brief', () => {
    expect(legacyBriefMarkdown(LEGACY, null, [])).toBe('# Outcome\nShip the thing.');
    expect(legacyBriefMarkdown(LEGACY, null, null)).toBe('# Outcome\nShip the thing.');
    // A versioned (structured) plan is never legacy.
    expect(legacyBriefMarkdown(LEGACY, null, [{ version: 1, author: 'you', changeNote: '', changed: [], createdAt: '' } as any])).toBeNull();
    expect(legacyBriefMarkdown(LEGACY, { version: 1 } as any, [])).toBeNull();
    // A draft still in briefing with a load hiccup is not legacy.
    expect(legacyBriefMarkdown({ ...LEGACY, initiativeStatus: 'briefing' }, null, [])).toBeNull();
    // No brief text → nothing to show.
    expect(legacyBriefMarkdown({ ...LEGACY, brief: '  ' }, null, [])).toBeNull();
    expect(legacyBriefMarkdown(null, null, [])).toBeNull();
  });

  it('the page renders it read-only and hides the fields, the checks and the "failed checks" footer', () => {
    expect(TS).toContain('legacyBriefMarkdown(this.plan(), this.latest(), this.versions())');
    const legacyAt = HTML.indexOf('@if (legacyBrief(); as md) {');
    const elseAt = HTML.indexOf('} @else {', legacyAt);
    const fieldsAt = HTML.indexOf('<div class="fields">');
    const checksAt = HTML.indexOf('<div class="checks">');
    expect(legacyAt).toBeGreaterThan(-1);
    expect(HTML.slice(legacyAt, elseAt)).toContain('<johnny-markdown-view [markdown]="md"></johnny-markdown-view>');
    expect(fieldsAt).toBeGreaterThan(elseAt);
    expect(checksAt).toBeGreaterThan(elseAt);
    expect(HTML).toContain("@if ((state() === 'ready' || state() === 'frozen') && !legacyBrief()) {");
  });
});
