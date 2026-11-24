/**
 * Live UI bug 4: plan agents were missing from /shells. `isShellSession` keeps only
 * `provider === 'shell' || attachedTmux`, and a plan agent is a `claude`/`claude_code` session with
 * `attachedTmux:false` (live engine: provider `claude`, planAgent true). #77 made the plan agent
 * watchable in tmux, so /shells lists the ACTIVE ones in their own view-only group.
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import type { AiSession } from '../../../../../ui/src/models/ai-session.model';
import { clearTargetIds, partitionShells, planAgentRowTitle, planAgentSessions } from './shells-page-logic';

function s(p: Partial<AiSession> & { id: string }): AiSession {
  return {
    title: p.id,
    provider: 'claude_code',
    model: '',
    workingDirectory: '',
    status: 'active',
    totalInputTokens: 0,
    totalOutputTokens: 0,
    totalCostCents: 0,
    createdAt: '2026-11-24 10:00:00',
    attachedTmux: false,
    ...p,
  } as AiSession;
}

const SESSIONS: AiSession[] = [
  s({ id: 'kord', provider: 'shell', attachedTmux: true }),
  s({ id: 'pa-old', provider: 'claude', planAgent: true, title: 'Briefing - kart - the visual identity pass', updatedAt: '2026-11-24 09:00:00' }),
  s({ id: 'pa-new', provider: 'claude_code', planAgent: true, title: 'Briefing - UI test', updatedAt: '2026-11-24 13:00:00' }),
  s({ id: 'pa-archived', planAgent: true, status: 'archived' }),
  s({ id: 'worker', provider: 'claude_code' }),
];

describe('plan agents on /shells (bug 4)', () => {
  it('lists active plan agents, newest first; never workers, shells or archived rows', () => {
    expect(planAgentSessions(SESSIONS).map((x) => x.id)).toEqual(['pa-new', 'pa-old']);
  });

  it('keeps them out of the shell list and out of Clear', () => {
    const shells = partitionShells(SESSIONS);
    expect(shells.map((x) => x.id)).toEqual(['kord']);
    expect(clearTargetIds(shells)).toEqual(['kord']);
  });

  it('titles drop the engine "Briefing - " prefix', () => {
    expect(planAgentRowTitle({ title: 'Briefing - kart - the visual identity pass' })).toBe('kart - the visual identity pass');
    expect(planAgentRowTitle({ title: '  ' })).toBe('Plan agent');
  });

  it('the page renders the group: labelled view-only, opens the agent, no rename or clear', () => {
    const html = readFileSync(resolve(__dirname, 'shells.page.html'), 'utf8');
    const ts = readFileSync(resolve(__dirname, 'shells.page.ts'), 'utf8');
    expect(ts).toContain('protected readonly planAgents = computed(() => planAgentSessions(this.sessions()));');
    expect(ts).toMatch(/this\.planAgents\(\)\.length === 0/);
    const start = html.indexOf('<div class="grouphdr">Plan agents</div>');
    const end = html.indexOf('<div class="grouphdr">Attachable tmux sessions</div>');
    expect(start).toBeGreaterThan(-1);
    const group = html.slice(start, end);
    expect(group).toContain('@for (s of planAgents(); track s.id)');
    expect(group).toContain('(click)="open(s)"');
    // Phone width (U-1-10): a short tag, one hint for the group, the sub-line is the time only.
    expect(group).toContain('<span class="tag plan">view-only</span>');
    expect(group).not.toContain('plan agent · view-only');
    expect(group).toContain('<div class="grouphint">Talk to a plan agent in its plan\'s chat.</div>');
    expect(group).toContain('<div class="sub">{{ formatRelTime(toIsoUtc(s.updatedAt || s.createdAt), nowIso()) }}</div>');
    expect(group).not.toContain('renameOne');
    expect(group).not.toContain('clearOne');
  });
});
