import { describe, it, expect } from 'vitest';
import type {
  AgentPlan,
  AgentPlanPhase,
  AgentPlanRun,
} from '../../../../../ui/src/services/johnny-api.service';
import {
  panelVisible,
  bannerModel,
  runButtonLabel,
  isPausedRun,
  pausedPhaseId,
  defaultSelectedPhaseId,
  phasePickerRows,
  defaultRunMode,
  modeFromToggle,
  validateComment,
  buildRunFromPhaseArgs,
} from './run-resume-logic';

/** Minimal AgentPlanPhase factory — only the fields the seam reads. */
function phase(over: Partial<AgentPlanPhase>): AgentPlanPhase {
  return {
    id: 'ph',
    planId: 'p1',
    phaseId: 'P01',
    phaseTitle: 'Scaffold',
    phaseIndex: 0,
    status: 'done',
    gateVerdict: '',
    clarificationAttempts: 0,
    findingsJson: '{}',
    createdAt: '',
    updatedAt: '',
    ...over,
  } as AgentPlanPhase;
}

/** Build an AgentPlanRun with a plan of the given status/type + phases. */
function run(
  plan: Partial<AgentPlan>,
  phases: AgentPlanPhase[] = [],
): AgentPlanRun {
  return {
    plan: {
      id: 'p1',
      runType: 'development',
      title: 't',
      workspacePath: '/w',
      planPath: '/p',
      status: 'phase_worker_running',
      workerProvider: 'claude_code',
      reviewerProvider: 'claude_code',
      currentPhaseIndex: 0,
      phaseRunMode: 'continue',
      initiativeId: 'i1',
      initiativeStatus: 'development',
      health: 'in-progress',
      createdAt: '',
      updatedAt: '',
      ...plan,
    } as AgentPlan,
    phases,
    tasks: [],
    events: [],
  };
}

const THREE_PHASES = [
  phase({ phaseId: 'P01', phaseTitle: 'Scaffold', status: 'done' }),
  phase({ phaseId: 'P02', phaseTitle: 'Cart atomicity', status: 'needs_changes' }),
  phase({ phaseId: 'P03', phaseTitle: 'Receipt flow', status: 'planned' }),
];

describe('panelVisible', () => {
  it('is true for a development run with ≥1 phase', () => {
    expect(panelVisible(run({ runType: 'development' }, THREE_PHASES))).toBe(true);
  });
  it('is false for a planning run, a phase-less run, or null', () => {
    expect(panelVisible(run({ runType: 'planning' }, THREE_PHASES))).toBe(false);
    expect(panelVisible(run({ runType: 'development' }, []))).toBe(false);
    expect(panelVisible(null)).toBe(false);
  });
});

describe('isPausedRun / bannerModel / runButtonLabel', () => {
  it('treats needs_attention and blocked as paused (status or health)', () => {
    expect(isPausedRun(run({ status: 'needs_attention' }, THREE_PHASES))).toBe(true);
    expect(isPausedRun(run({ status: 'blocked' }, THREE_PHASES))).toBe(true);
    expect(isPausedRun(run({ status: 'phase_worker_running', health: 'needs-attention' }, THREE_PHASES))).toBe(true);
    expect(isPausedRun(run({ status: 'approved' }, THREE_PHASES))).toBe(false);
  });

  it('bannerModel shows the error reason when paused, hidden otherwise', () => {
    expect(bannerModel(run({ status: 'needs_attention', error: 'P02 gate FAILED — qa lens' }, THREE_PHASES))).toEqual({
      show: true,
      reason: 'P02 gate FAILED — qa lens',
    });
    expect(bannerModel(run({ status: 'approved' }, THREE_PHASES))).toEqual({ show: false, reason: '' });
  });

  it('runButtonLabel is Resume when paused, Run otherwise', () => {
    expect(runButtonLabel(run({ status: 'needs_attention' }, THREE_PHASES))).toBe('Resume');
    expect(runButtonLabel(run({ status: 'blocked' }, THREE_PHASES))).toBe('Resume');
    expect(runButtonLabel(run({ status: 'approved' }, THREE_PHASES))).toBe('Run');
  });
});

describe('pausedPhaseId / defaultSelectedPhaseId', () => {
  it('paused run pre-selects the current/paused phase', () => {
    const paused = run({ status: 'needs_attention', currentPhaseId: 'P02' }, THREE_PHASES);
    expect(pausedPhaseId(paused)).toBe('P02');
    expect(defaultSelectedPhaseId(paused)).toBe('P02');
  });
  it('falls back to a phase carrying a paused-ish status when no currentPhaseId', () => {
    const paused = run({ status: 'blocked' }, THREE_PHASES); // P02 is needs_changes
    expect(defaultSelectedPhaseId(paused)).toBe('P02');
  });
  it('completed/idle run selects current phase, else first', () => {
    expect(defaultSelectedPhaseId(run({ status: 'approved', currentPhaseId: 'P03' }, THREE_PHASES))).toBe('P03');
    expect(defaultSelectedPhaseId(run({ status: 'approved' }, THREE_PHASES))).toBe('P01');
    expect(defaultSelectedPhaseId(run({ status: 'approved' }, []))).toBeNull();
  });
});

describe('phasePickerRows', () => {
  it('produces a row per phase in order, flags the paused row, chips via taskStatusLabel', () => {
    const paused = run({ status: 'needs_attention', currentPhaseId: 'P02' }, THREE_PHASES);
    const rows = phasePickerRows(paused, 'P02');
    expect(rows.map((r) => r.phaseId)).toEqual(['P01', 'P02', 'P03']);
    expect(rows[0].statusChip).toEqual({ label: 'DONE', token: 'done' });
    expect(rows[1].paused).toBe(true);
    expect(rows[1].selected).toBe(true);
    expect(rows[0].paused).toBe(false);
    expect(rows[2].selected).toBe(false);
  });
  it('is total for null', () => {
    expect(phasePickerRows(null, null)).toEqual([]);
  });
});

describe('mode helpers', () => {
  it('defaults to continue and maps the toggle', () => {
    expect(defaultRunMode()).toBe('continue');
    expect(modeFromToggle(true)).toBe('continue');
    expect(modeFromToggle(false)).toBe('single');
  });
});

describe('validateComment', () => {
  it('accepts empty/missing as ok/null', () => {
    expect(validateComment('')).toEqual({ ok: true, value: null });
    expect(validateComment(undefined)).toEqual({ ok: true, value: null });
    expect(validateComment(null)).toEqual({ ok: true, value: null });
  });
  it('rejects whitespace-only', () => {
    expect(validateComment('   ').ok).toBe(false);
    expect(validateComment('\t\n ').ok).toBe(false);
  });
  it('accepts non-blank verbatim', () => {
    expect(validateComment('hi')).toEqual({ ok: true, value: 'hi' });
    expect(validateComment('  keep spaces  ')).toEqual({ ok: true, value: '  keep spaces  ' });
  });
});

describe('buildRunFromPhaseArgs', () => {
  it('maps single/continue and normalizes blanks to null', () => {
    expect(buildRunFromPhaseArgs('id1', 'P02', 'fix it', 'single')).toEqual({
      id: 'id1',
      phaseId: 'P02',
      phaseRunMode: 'single',
      comment: 'fix it',
    });
    expect(buildRunFromPhaseArgs('id1', '', '', 'continue')).toEqual({
      id: 'id1',
      phaseId: null,
      phaseRunMode: 'continue',
      comment: null,
    });
  });
  it('throws on a whitespace-only comment (button is already guarded)', () => {
    expect(() => buildRunFromPhaseArgs('id1', 'P02', '   ', 'continue')).toThrow();
  });
});
