// The arena's client-side rules, each with ONE home (D: "one home per rule"). Everything a human
// sees derived from arena data goes through a function here, so no template re-derives it and no
// second copy can drift. The HOST is the enforcer of promotion and of the halt; these functions
// read host-provided fields only and never recompute them (notably: comparability is never
// recomputed from `seatsHeld`, and the halt is never recounted from a decision log).
import { describe, it, expect } from 'vitest';
import {
  arenaEntryRow,
  gateCollectedLabel,
  gateExitLabel,
  gateAcceptance,
  rejectionReasonValid,
  startRunBlock,
  promotionBlock,
  promotionConfirmCopy,
  NO_GATE_REASON,
} from './arena-logic';

describe('arenaEntryRow — step 1, the entry point', () => {
  it('enables the row when the phase has an effective gate', () => {
    const row = arenaEntryRow({ effectiveTestCommands: ['cargo test --lib leases::'] });
    expect(row.enabled).toBe(true);
    expect(row.reason).toBeNull();
  });

  it('keeps the row on screen but DISABLED, with its reason, when there is no gate', () => {
    for (const phase of [
      { effectiveTestCommands: [] },
      { effectiveTestCommands: null },
      {},
    ]) {
      const row = arenaEntryRow(phase);
      expect(row.enabled).toBe(false);
      expect(row.reason).toBe(NO_GATE_REASON);
    }
  });

  it('does not treat a blank-string command as a gate', () => {
    expect(arenaEntryRow({ effectiveTestCommands: ['   '] }).enabled).toBe(false);
  });
});

describe('gateCollectedLabel — advisory and nullable, NEVER rendered as 0', () => {
  it('says "not reported" for null/undefined, not 0', () => {
    expect(gateCollectedLabel(null)).toBe('not reported');
    expect(gateCollectedLabel(undefined)).toBe('not reported');
    expect(gateCollectedLabel(null)).not.toContain('0');
  });

  it('reports a real 0 as 0 collected — that is the discriminates-nothing case', () => {
    expect(gateCollectedLabel(0)).toBe('0 collected');
    expect(gateCollectedLabel(23)).toBe('23 collected');
  });
});

describe('gateExitLabel — the exit code is the only score', () => {
  it('a NULL exit is "never ran", which is not a pass', () => {
    expect(gateExitLabel(null)).toBe('never ran');
    expect(gateExitLabel(undefined)).toBe('never ran');
  });
  it('0 passes, anything else carries its code', () => {
    expect(gateExitLabel(0)).toBe('exit 0 · pass');
    expect(gateExitLabel(101)).toBe('exit 101 · fail');
    expect(gateExitLabel(-3)).toBe('exit -3 · fail');
  });
  it('never invents a rating, a score or a percentage', () => {
    for (const v of [0, 1, 101, -1]) {
      expect(gateExitLabel(v)).not.toMatch(/%|score|rating/i);
    }
  });
});

describe('gateAcceptance — step 2, the test that decides', () => {
  it('accepts exit 0 with a positive collected count', () => {
    const a = gateAcceptance({ gateDeclared: true, exitCode: 0, collected: 23 });
    expect(a.accepted).toBe(true);
    expect(a.reason).toBeNull();
  });

  it('ACCEPTS exit 0 with a null collected count — a lint or a build is a legitimate gate', () => {
    expect(gateAcceptance({ gateDeclared: true, exitCode: 0, collected: null }).accepted).toBe(true);
    expect(gateAcceptance({ gateDeclared: true, exitCode: 0 }).accepted).toBe(true);
  });

  it('refuses 0 collected — it scores every workflow identically', () => {
    const a = gateAcceptance({ gateDeclared: true, exitCode: 0, collected: 0 });
    expect(a.accepted).toBe(false);
    expect(a.reason).toMatch(/collects nothing/i);
  });

  it('refuses a command already red on untouched code', () => {
    const a = gateAcceptance({ gateDeclared: true, exitCode: 101, collected: 23 });
    expect(a.accepted).toBe(false);
    expect(a.reason).toMatch(/already red/i);
  });

  it('refuses when no gate is declared at all, even though the exit code is 0', () => {
    const a = gateAcceptance({ gateDeclared: false, exitCode: 0, collected: null });
    expect(a.accepted).toBe(false);
    expect(a.reason).toBe(NO_GATE_REASON);
  });
});

describe('rejectionReasonValid — there is no bare no', () => {
  it('refuses an empty or whitespace reason', () => {
    for (const r of ['', '   ', '\n\t']) {
      const v = rejectionReasonValid(r);
      expect(v.ok).toBe(false);
      expect(v.message).toMatch(/reason/i);
    }
  });
  it('accepts a real reason', () => {
    expect(rejectionReasonValid('the gate never touches the cashier path').ok).toBe(true);
  });
});

describe('startRunBlock — a halted search refuses a run, as the HOST decided', () => {
  const state = {
    repo: 'johnnyone',
    phaseType: 'dev-phase',
    family: 'glm-5.3-flash',
    consecutiveRejections: 3,
    halted: true,
    strikeLimit: 3,
    recentRejections: ['c', 'b', 'a'],
    championWorkflowId: 'w1',
  };

  it('blocks with a message naming the streak and the limit', () => {
    const b = startRunBlock(state);
    expect(b).not.toBeNull();
    expect(b).toContain('3');
    expect(b).toMatch(/halt|pause/i);
  });

  it('does not block when the host says not halted, even on a non-zero streak', () => {
    expect(startRunBlock({ ...state, halted: false, consecutiveRejections: 2 })).toBeNull();
  });

  it('reads `halted` and NEVER recounts it from the streak — the host owns the rule', () => {
    // A host that raised strikeLimit to 5 must stop blocking immediately, with no client change.
    expect(startRunBlock({ ...state, halted: false, strikeLimit: 5 })).toBeNull();
    // ...and a host halting at 1 must block, even though 1 < this client's idea of three.
    const early = startRunBlock({ ...state, halted: true, consecutiveRejections: 1, strikeLimit: 1 });
    expect(early).not.toBeNull();
  });

  it('blocks when the state could not be loaded at all, rather than starting blind', () => {
    expect(startRunBlock(null)).not.toBeNull();
  });
});

describe('promotionBlock — the one human-confirmed step', () => {
  const run = { id: 'r2', gateExit: 0 };
  const pref = { id: 'p1', winnerRunId: 'r2', loserRunId: 'r1', comparable: true };

  it('allows a green run with a comparable preference naming it the winner', () => {
    expect(promotionBlock({ run, pref, incumbentExists: true })).toBeNull();
  });

  it('blocks a NULL gate — never ran is not a pass', () => {
    expect(promotionBlock({ run: { id: 'r2', gateExit: null }, pref, incumbentExists: true }))
      .toMatch(/never ran/i);
  });

  it('blocks a non-zero gate', () => {
    expect(promotionBlock({ run: { id: 'r2', gateExit: 1 }, pref, incumbentExists: true }))
      .toMatch(/did not pass|exit 1/i);
  });

  it('BLOCKS a non-comparable preference — mismatched seat budgets, blocked not warned', () => {
    const msg = promotionBlock({ run, pref: { ...pref, comparable: false }, incumbentExists: true });
    expect(msg).toMatch(/seat/i);
    expect(msg).not.toMatch(/warning/i);
  });

  it('blocks a preference that names a different winner', () => {
    expect(promotionBlock({ run, pref: { ...pref, winnerRunId: 'rX' }, incumbentExists: true }))
      .toMatch(/different run/i);
  });

  it('blocks displacing an incumbent with no head-to-head at all', () => {
    expect(promotionBlock({ run, pref: null, incumbentExists: true })).toMatch(/comparable/i);
  });

  it('allows seeding the FIRST champion of a family with no preference', () => {
    expect(promotionBlock({ run, pref: null, incumbentExists: false })).toBeNull();
  });
});

describe('promotionConfirmCopy — the confirm states the blast radius plainly', () => {
  it('names the repo, the phase type and the family, and nothing wider', () => {
    const copy = promotionConfirmCopy({
      repo: 'johnnyone',
      phaseType: 'dev-phase',
      family: 'glm-5.3-flash',
      version: 2,
    });
    expect(copy).toContain('johnnyone');
    expect(copy).toContain('dev-phase');
    expect(copy).toContain('glm-5.3-flash');
    expect(copy).toContain('v2');
    expect(copy).toMatch(/does not|not touch/i);
  });
});

// ── Derived values the screens need, each with ONE home here rather than inline in a template ──
import { repoNameFromWorkspacePath, defaultPhaseType, lineageOf, changedSlots } from './arena-logic';

describe('repoNameFromWorkspacePath', () => {
  it('names the repo from the plan’s workspace path', () => {
    expect(repoNameFromWorkspacePath('/home/creepy/Documents/Workspace/personal/apps/johnnyone')).toBe('johnnyone');
    expect(repoNameFromWorkspacePath('/a/b/kord/')).toBe('kord');
  });
  it('is empty rather than wrong when there is no path', () => {
    expect(repoNameFromWorkspacePath('')).toBe('');
    expect(repoNameFromWorkspacePath(null)).toBe('');
  });
});

describe('defaultPhaseType', () => {
  it('suggests dev-phase for a development run and planning for a planning run', () => {
    expect(defaultPhaseType('development')).toBe('dev-phase');
    expect(defaultPhaseType('planning')).toBe('planning-phase');
  });
  it('falls back to a plain value, never a blank string', () => {
    expect(defaultPhaseType(undefined)).toBe('dev-phase');
  });
});

describe('lineageOf — a rejected challenger stays in the lineage', () => {
  const ws = [
    { id: 'a', parentId: null, version: 1 },
    { id: 'b', parentId: 'a', version: 2 },
    { id: 'c', parentId: 'b', version: 3 },
  ];
  it('walks parents, oldest first, ending at the asked-for version', () => {
    expect(lineageOf(ws, 'c').map((w) => w.id)).toEqual(['a', 'b', 'c']);
    expect(lineageOf(ws, 'a').map((w) => w.id)).toEqual(['a']);
  });
  it('returns nothing for an unknown id instead of throwing', () => {
    expect(lineageOf(ws, 'zz')).toEqual([]);
  });
  it('does not loop forever on a cyclic parent chain', () => {
    const cyclic = [
      { id: 'x', parentId: 'y', version: 1 },
      { id: 'y', parentId: 'x', version: 2 },
    ];
    expect(lineageOf(cyclic, 'x').length).toBeLessThanOrEqual(2);
  });
});

describe('changedSlots — which of the four parts changed, never a text diff', () => {
  const champ = { role: 'r', instruction: 'i', contract: 'c', hop: 'h', fanOut: 1 };
  it('names only the parts that differ', () => {
    const changed = changedSlots(champ, { ...champ, contract: 'c2', hop: 'h2' });
    expect(changed.map((c) => c.slot)).toEqual(['contract', 'hop']);
    expect(changed[0].from).toBe('c');
    expect(changed[0].to).toBe('c2');
  });
  it('reports width separately when the seat cost changed', () => {
    const changed = changedSlots(champ, { ...champ, fanOut: 2 });
    expect(changed.map((c) => c.slot)).toEqual(['width']);
  });
  it('is empty when nothing changed', () => {
    expect(changedSlots(champ, { ...champ })).toEqual([]);
  });
});

import { slotsComplete, hopIsJson, latestRunOf } from './arena-logic';

describe('slotsComplete — four parts, never blank', () => {
  const full = { role: 'r', instruction: 'i', contract: 'c', hop: '{}' };
  it('is satisfied only when all four carry text', () => {
    expect(slotsComplete(full)).toEqual({ ok: true, missing: [] });
  });
  it('names every blank part, and treats whitespace as blank', () => {
    expect(slotsComplete({ ...full, role: '', contract: '  ' }).missing).toEqual(['role', 'contract']);
    expect(slotsComplete({ ...full, role: '' }).ok).toBe(false);
  });
});

describe('hopIsJson — the host validates it IS json, so the screen should not send garbage', () => {
  it('accepts an object or an array', () => {
    expect(hopIsJson('{"steps":[]}')).toBe(true);
    expect(hopIsJson('[1,2]')).toBe(true);
  });
  it('refuses prose and a bare number', () => {
    expect(hopIsJson('1 implement, then report')).toBe(false);
    expect(hopIsJson('')).toBe(false);
    expect(hopIsJson('7')).toBe(false);
  });
});

describe('latestRunOf', () => {
  it('picks the most recently started run, not the first in the array', () => {
    const runs = [
      { id: 'a', startedAt: '2026-10-14T01:00:00Z' },
      { id: 'c', startedAt: '2026-10-14T03:00:00Z' },
      { id: 'b', startedAt: '2026-10-14T02:00:00Z' },
    ];
    expect(latestRunOf(runs)?.id).toBe('c');
  });
  it('is null for no runs — "no runs yet" is an answer, not a zero', () => {
    expect(latestRunOf([])).toBeNull();
  });
});
