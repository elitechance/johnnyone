import { describe, it, expect } from 'vitest';
import type {
  AgentPlanRun,
  AgentPlanPhase,
} from '../../../../../ui/src/services/johnny-api.service';
import {
  planDocPath,
  planListPath,
  artifactGroups,
  artifactDirsFor,
  artifactOpenMode,
} from './plan-tab-logic';

// QA B — the one back-compat claim in Dev B's report that was READ and never RUN: that
// `docDirSegments` / `planDocPath` already accept the run-scoped artifact layout
// `phases/<id>/artifacts/runs/<run>/x.html` with no change to the web. Reading the code says yes.
// This file MEASURES it, and attacks the same shape for a way out of the allowlist.
//
// It is deliberately a new file rather than an edit to plan-tab-logic.spec.ts, so none of the
// existing specs can be disturbed.

function phase(over: Partial<AgentPlanPhase>): AgentPlanPhase {
  return {
    phaseId: 'P',
    phaseTitle: 'Phase',
    phaseIndex: 0,
    status: 'planned',
    ...over,
  } as AgentPlanPhase;
}
function run(over: Partial<AgentPlanRun>): AgentPlanRun {
  return { plan: {} as any, phases: [], tasks: [], events: [], ...over } as AgentPlanRun;
}

const PLAN = '/home/u/.johnnyone/initiatives/324ad239/plan';
const RUN = run({
  phases: [phase({ phaseId: '04-leases', phaseTitle: 'Lease pool', phaseIndex: 0 })],
});

describe('run-scoped artifact paths (host layout phases/<id>/artifacts/runs/<run_id>/)', () => {
  it('resolves a phase-level run-scoped artifact file', () => {
    expect(planDocPath(PLAN, 'phases/04-leases/artifacts/runs/4f0821/seat-meter.html')).toBe(
      `${PLAN}/phases/04-leases/artifacts/runs/4f0821/seat-meter.html`,
    );
  });

  it('resolves a plan-level run-scoped artifact file', () => {
    expect(planDocPath(PLAN, 'artifacts/runs/4f0821/summary.md')).toBe(
      `${PLAN}/artifacts/runs/4f0821/summary.md`,
    );
  });

  it('resolves arbitrarily deep inside a run directory', () => {
    expect(
      planDocPath(PLAN, 'phases/04-leases/artifacts/runs/4f0821/screens/mobile/aux-open.png'),
    ).toBe(`${PLAN}/phases/04-leases/artifacts/runs/4f0821/screens/mobile/aux-open.png`);
  });

  it('makes every level of the run-scoped tree LISTABLE, so the navigator can descend', () => {
    for (const dir of [
      'phases/04-leases/artifacts',
      'phases/04-leases/artifacts/runs',
      'phases/04-leases/artifacts/runs/4f0821',
      'phases/04-leases/artifacts/runs/4f0821/screens',
    ]) {
      expect(planListPath(PLAN, dir), dir).toBe(`${PLAN}/${dir}`);
    }
  });

  it('keeps two runs that deliver the SAME basename on distinct paths', () => {
    const a = planDocPath(PLAN, 'phases/04-leases/artifacts/runs/aaaa/seat-meter.html');
    const b = planDocPath(PLAN, 'phases/04-leases/artifacts/runs/bbbb/seat-meter.html');
    const champion = planDocPath(PLAN, 'phases/04-leases/artifacts/seat-meter.html');
    expect(new Set([a, b, champion]).size).toBe(3);
    expect(a).not.toBeNull();
  });

  it('still resolves the OLD flat layout unchanged', () => {
    expect(planDocPath(PLAN, 'phases/04-leases/artifacts/aux-open.png')).toBe(
      `${PLAN}/phases/04-leases/artifacts/aux-open.png`,
    );
    expect(planDocPath(PLAN, 'artifacts/summary.md')).toBe(`${PLAN}/artifacts/summary.md`);
    expect(planDocPath(PLAN, 'brief.md')).toBe(`${PLAN}/brief.md`);
  });

  it('opens a run-scoped .html through the sandboxed viewer and .md as markdown', () => {
    expect(artifactOpenMode('phases/04-leases/artifacts/runs/4f0821/seat-meter.html')).toBe(
      'viewer',
    );
    expect(artifactOpenMode('phases/04-leases/artifacts/runs/4f0821/notes.md')).toBe('markdown');
  });

  it('lets the navigator descend runs/ -> <run id> and hand back an openable selection', () => {
    // Exactly what `filesListDir` returns for the three levels, with both opened.
    const listings = {
      'phases/04-leases/artifacts': [
        { name: 'runs', kind: 'dir' },
        { name: 'champion.html', kind: 'file' },
      ],
      'phases/04-leases/artifacts/runs': [{ name: '4f0821', kind: 'dir' }],
      'phases/04-leases/artifacts/runs/4f0821': [{ name: 'seat-meter.html', kind: 'file' }],
    };
    const expanded = {
      'phases/04-leases/artifacts/runs': true,
      'phases/04-leases/artifacts/runs/4f0821': true,
    };
    const groups = artifactGroups(RUN, listings, expanded);
    expect(groups).toHaveLength(1);
    const ids = groups[0].nodes.map((n) => n.id);
    expect(ids).toContain('phases/04-leases/artifacts/runs/4f0821/seat-meter.html');
    // And the deepest row the navigator produced really is openable by the same allowlist.
    const leaf = groups[0].nodes.find((n) => n.name === 'seat-meter.html' && n.depth === 2);
    expect(leaf, 'the run-scoped file must be a row at depth 2').toBeTruthy();
    expect(planDocPath(PLAN, leaf!.id)).toBe(
      `${PLAN}/phases/04-leases/artifacts/runs/4f0821/seat-meter.html`,
    );
  });

  it('does not enumerate runs/ as a candidate dir — it is reached by descending, not by probing', () => {
    const dirs = artifactDirsFor(RUN);
    expect(dirs).toContain('phases/04-leases/artifacts');
    expect(dirs.some((d) => d.includes('/runs'))).toBe(false);
  });
});

describe('the run-scoped shape must not become a way out of the allowlist', () => {
  it('refuses a traversal at every position in the run-scoped path', () => {
    for (const bad of [
      'phases/04-leases/artifacts/runs/../../../overview.md',
      'phases/04-leases/artifacts/runs/4f0821/../../../../overview.md',
      'phases/04-leases/artifacts/../../../etc/passwd',
      'phases/../artifacts/runs/4f0821/x.html',
      'artifacts/runs/../../overview.md',
      'phases/04-leases/artifacts/runs/..',
      'phases/04-leases/artifacts/runs/4f..21/x.html',
    ]) {
      expect(planDocPath(PLAN, bad), bad).toBeNull();
      expect(planListPath(PLAN, bad), bad).toBeNull();
    }
  });

  it('refuses absolute, UNC, empty, dot and backslash segments in the run-scoped path', () => {
    for (const bad of [
      '/phases/04-leases/artifacts/runs/4f0821/x.html',
      '//host/artifacts/runs/4f0821/x.html',
      'phases/04-leases/artifacts/runs//4f0821/x.html',
      'phases/04-leases/artifacts/runs/./4f0821/x.html',
      'phases/04-leases/artifacts/runs/4f0821/',
      'phases\\04-leases\\artifacts\\runs\\4f0821\\x.html',
      'phases/04-leases/artifacts/runs/C:\\x.html',
    ]) {
      expect(planDocPath(PLAN, bad), bad).toBeNull();
    }
  });

  it('does not let `runs` act as a document dir of its own', () => {
    // `runs` is not in DOC_DIRS, so it is addressable only UNDER an `artifacts` dir.
    expect(planDocPath(PLAN, 'runs/4f0821/x.html')).toBeNull();
    expect(planDocPath(PLAN, 'phases/04-leases/runs/4f0821/x.html')).toBeNull();
    // ...and not as a sibling of `artifacts` at plan level either. What it DOES resolve to is a
    // pre-existing quirk QA measured rather than guessed: `relPlanDocPath`'s last branch treats
    // any single safe segment as a phase id, so the bare selection `runs` becomes
    // `phases/runs/overview.md` — a FILE path handed back from a function whose job is to resolve
    // a dir to LIST. Confined, and wrong only in kind; pinned so it is visible. (Not introduced
    // by the run-scoping branch: `runs` is just the first plausible name to collide with it.)
    expect(planListPath(PLAN, 'runs')).toBe(`${PLAN}/phases/runs/overview.md`);
    expect(planDocPath(PLAN, 'runs')).toBe(`${PLAN}/phases/runs/overview.md`);
  });

  it('does not offer a navigator row for a run id the host path builder would refuse', () => {
    // The host returns the entry; the navigator must drop it rather than render a dead button.
    const listings = {
      'phases/04-leases/artifacts': [{ name: 'runs', kind: 'dir' }],
      'phases/04-leases/artifacts/runs': [
        { name: '..', kind: 'dir' },
        { name: 'v1..2', kind: 'dir' },
        { name: 'a/b', kind: 'dir' },
        { name: 'a\\b', kind: 'dir' },
        { name: '', kind: 'dir' },
        { name: '.', kind: 'dir' },
        { name: '4f0821', kind: 'dir' },
      ],
    };
    const nodes = artifactGroups(RUN, listings, {
      'phases/04-leases/artifacts/runs': true,
    })[0].nodes;
    const runRows = nodes.filter((n) => n.depth === 1).map((n) => n.name);
    expect(runRows).toEqual(['4f0821']);
    // Every row that IS offered resolves.
    for (const n of nodes) expect(planDocPath(PLAN, n.id), n.id).not.toBeNull();
  });
});

describe('the navigator and the host agree on what a safe segment is', () => {
  // The cross-language half of the Rust test `plan_artifacts::tests::
  // the_segment_rule_matches_the_navigators`, which pins the HOST side against a hand-copied list
  // of what the navigator does. That list was never executed against the navigator. This is the
  // same corpus, run through the real TS entry point, so the two rules cannot drift apart with
  // both suites green.
  // `a/b` and `a\\b` are NOT in this list, and that is a correction to QA's own first draft of
  // this file: interpolating them into a path string does not test the segment rule at all. `a/b`
  // interpolated becomes TWO legitimate segments and resolves — correctly. The segment rule is
  // probed where a segment really is a segment instead: the `artifactGroups` row test above feeds
  // `a/b`, `a\\b`, `..`, `.`, `''` and `v1..2` in as host entry NAMES, which is the only place
  // either side ever receives one, and every one of them is dropped.
  const HOST_REFUSES = ['', '.', '..', 'x..y', 'C:\\x'];
  const HOST_ACCEPTS = ['01-shell', '4f0821', 'P2-plan-tab', 'v1.2.png', 'runs'];

  it('refuses every segment the host refuses, used as a run id', () => {
    for (const seg of HOST_REFUSES) {
      expect(
        planDocPath(PLAN, `phases/04-leases/artifacts/runs/${seg}/x.html`),
        JSON.stringify(seg),
      ).toBeNull();
    }
  });

  it('accepts every segment the host accepts, used as a run id', () => {
    for (const seg of HOST_ACCEPTS) {
      expect(
        planDocPath(PLAN, `phases/04-leases/artifacts/runs/${seg}/x.html`),
        JSON.stringify(seg),
      ).toBe(`${PLAN}/phases/04-leases/artifacts/runs/${seg}/x.html`);
    }
  });

  it('agrees with the host that a percent-encoded traversal is an ordinary segment name', () => {
    // Pinned on BOTH sides deliberately: the guard is a byte rule, so `%2e%2e` is a directory
    // literally called `%2e%2e`. Safe only while nothing between the navigator and the filesystem
    // URL-decodes the path. See the matching Rust test
    // `plan_artifacts::qa_b_tests::qa_documents_encoded_and_lookalike_traversals_stay_literal_segments`.
    expect(planDocPath(PLAN, 'phases/04-leases/artifacts/runs/%2e%2e/x.html')).toBe(
      `${PLAN}/phases/04-leases/artifacts/runs/%2e%2e/x.html`,
    );
  });
});
