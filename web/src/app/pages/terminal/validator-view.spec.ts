// What a HUMAN VALIDATOR can act on, looked at in a real browser on 2026-10-23.
//
// The console was open on the kart initiative, which was stuck. Two panels were
// in prime real estate and neither carried the fact the person needed.
//
// 1. The lifecycle bar read `HEALTH • needs-attention` and stopped there. The
//    reason — `can't find pane: %2375` — was already on the same page, eleven
//    rows down the Events column. A validator had to scroll a transcript to
//    learn why the run they were looking at had stopped.
//
// 2. The VALIDATION panel read:
//
//        VALIDATION  [configured]                           1 lenses
//        glm-validator
//        [lokalai] [glm] [blocking]
//        Configure →
//
//    That is the CONFIGURATION. The lens had been dispatched once and had died
//    three seconds later without ever reporting, and the panel said nothing
//    about it — it looked exactly the same as it had before the lens ever ran.
//    For the person whose whole job is to decide, the panel named after their
//    job showed them a form.
//
// Both of these are pure derivations over data the page already holds, so they
// are testable without Angular, in the plugin-less web vitest — the same
// pure-seam pattern as `lifecycle-status.ts` and `initiative-events-logic.ts`.

import { describe, it, expect } from 'vitest';

import { healthReason } from '../../../../../ui/src/lib/lifecycle-status';
import { lensOutcomes } from './lens-outcome';
import type { TimelineEvent } from './initiative-events-logic';

function ev(partial: Partial<TimelineEvent>): TimelineEvent {
  return {
    id: Math.random().toString(36).slice(2),
    stage: 'review',
    actor: 'coordinator',
    actorLabel: 'coordinator',
    title: '',
    phaseLabel: null,
    lens: null,
    verdict: null,
    reason: null,
    iso: '2026-10-23T05:40:22Z',
    milestone: false,
    ...partial,
  };
}

describe('healthReason — the badge must say why', () => {
  it('carries the reason when the run is not healthy', () => {
    expect(healthReason('needs-attention', "can't find pane: %2375")).toBe(
      "can't find pane: %2375",
    );
    expect(healthReason('blocked', 'agent reported blocked — needs a human')).toBe(
      'agent reported blocked — needs a human',
    );
  });

  it('says nothing when the run is healthy, and nothing when there is nothing to say', () => {
    // A green run with a stale error string must not display it: that would
    // show an old failure beside a healthy badge, which reads as a live one.
    expect(healthReason('complete', 'some stale error from two phases ago')).toBeNull();
    expect(healthReason('in-progress', "can't find pane: %2375")).toBeNull();
    // Unhealthy with no reason recorded is the honest blank — do NOT invent
    // copy like "something went wrong", which tells the validator less than
    // the badge already did.
    expect(healthReason('needs-attention', null)).toBeNull();
    expect(healthReason('needs-attention', '   ')).toBeNull();
  });
});

describe('lensOutcomes — the validation panel must show outcome, not configuration', () => {
  const lenses = [{ name: 'glm-validator' }, { name: 'qa' }];

  it('reports a lens that was never dispatched as never-run, not as blank', () => {
    const out = lensOutcomes(lenses, []);
    expect(out.map((o) => o.name)).toEqual(['glm-validator', 'qa']);
    expect(out[0].verdict).toBeNull();
    expect(out[0].state).toBe('never-run');
  });

  it('reports a lens that started and never reported as RUNNING, not as never-run', () => {
    // This is the kart case exactly: dispatched at 05:40:22, dead at 05:40:25,
    // no verdict ever. "never ran" would be a lie — it ran and vanished, which
    // is a different thing for the person deciding whether to wait.
    const out = lensOutcomes(lenses, [
      ev({ lens: 'glm-validator', title: 'Lens review started', iso: '2026-10-23T05:40:22Z' }),
    ]);
    const glm = out.find((o) => o.name === 'glm-validator')!;
    expect(glm.state).toBe('running');
    expect(glm.verdict).toBeNull();
    expect(glm.at).toBe('2026-10-23T05:40:22Z');
  });

  it('reports the verdict a lens actually gave', () => {
    const out = lensOutcomes(lenses, [
      ev({ lens: 'glm-validator', verdict: 'NEEDS_CHANGES', iso: '2026-10-23T06:10:00Z' }),
    ]);
    const glm = out.find((o) => o.name === 'glm-validator')!;
    expect(glm.state).toBe('reported');
    expect(glm.verdict).toBe('NEEDS_CHANGES');
  });

  it('takes the LATEST verdict, whichever order the rows arrive in', () => {
    // `consoleTimeline()` is newest-first, but nothing in the type says so.
    // Both orders must give the same answer, or the panel shows a stale
    // verdict whenever the feed order changes.
    const older = ev({ lens: 'glm-validator', verdict: 'NEEDS_CHANGES', iso: '2026-10-23T06:10:00Z' });
    const newer = ev({ lens: 'glm-validator', verdict: 'PASS', iso: '2026-10-23T07:30:00Z' });

    for (const rows of [[newer, older], [older, newer]]) {
      const glm = lensOutcomes(lenses, rows).find((o) => o.name === 'glm-validator')!;
      expect(glm.verdict, `order ${rows.map((r) => r.iso).join(' then ')}`).toBe('PASS');
      expect(glm.at).toBe('2026-10-23T07:30:00Z');
    }
  });

  it('does not attribute one lens’s verdict to another', () => {
    const out = lensOutcomes(lenses, [
      ev({ lens: 'qa', verdict: 'PASS', iso: '2026-10-23T06:00:00Z' }),
    ]);
    expect(out.find((o) => o.name === 'qa')!.verdict).toBe('PASS');
    expect(out.find((o) => o.name === 'glm-validator')!.verdict).toBeNull();
    expect(out.find((o) => o.name === 'glm-validator')!.state).toBe('never-run');
  });

  it('ignores rows belonging to a lens that is no longer configured', () => {
    // Lenses can be reconfigured mid-initiative. A verdict from a lens that is
    // gone must not resurrect it as a row in a panel that claims to list the
    // CURRENT configuration's outcomes.
    const out = lensOutcomes(lenses, [
      ev({ lens: 'lead', verdict: 'PASS', iso: '2026-10-23T06:00:00Z' }),
    ]);
    expect(out.map((o) => o.name)).toEqual(['glm-validator', 'qa']);
  });
});
