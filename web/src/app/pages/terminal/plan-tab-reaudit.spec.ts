import { describe, it, expect } from 'vitest';
import {
  planDocPath,
  artifactGroups,
  isDocDir,
  docDirNames,
  phaseDir,
  phaseArtifactsDir,
  emptyArtifactCache,
  mergeArtifactListings,
} from './plan-tab-logic';
import type { AgentPlanRun } from '../../../../../ui/src/services/johnny-api.service';

// QA re-audit of 82af132. Attacks the three things the fix newly made possible: an owner-tagged
// cache (D3), a two-segment `phases/<id>` shape (R3), and a seven-name doc-dir list (R2).

const BASE = '/store/.johnnyone/initiatives/abc/plan';
const RUN = { plan: {}, phases: [], tasks: [], events: [] } as unknown as AgentPlanRun;

describe('D3 — ownership under adversarial switch sequences', () => {
  const A = 'init-A';
  const B = 'init-B';
  const aRows = [{ name: 'A-only.html', kind: 'file' }];
  const bRows = [{ name: 'B-only.html', kind: 'file' }];

  it('A→B→A: a stale A fan-out is re-admitted, and that is CORRECT (it is A data under A)', () => {
    // t0 select A (fan-out A starts) → t1 select B → t2 select A again (fresh cache owned by A)
    // → t3 the ORIGINAL A fan-out lands. ownerId matches again, so it merges — but the data is A's
    // and the displayed initiative is A, so this is a duplicate fetch, not the cross-plan leak.
    let cache = emptyArtifactCache(A);
    cache = emptyArtifactCache(B);
    cache = emptyArtifactCache(A); // t2 reset
    cache = mergeArtifactListings(cache, A, [{ dirRel: 'artifacts', entries: aRows }]);
    expect(cache.listings['artifacts']).toEqual(aRows);
    // and the path it builds uses A's own planPath
    const id = artifactGroups(RUN, cache.listings, {})[0].nodes[0].id;
    expect(planDocPath('/store/A/plan', id)).toBe('/store/A/plan/artifacts/A-only.html');
  });

  it('two concurrent fan-outs for the SAME initiative merge instead of clobbering', () => {
    // the old code did a wholesale `.set(listings)`, which lost the other fan-out's dirs
    let cache = emptyArtifactCache(A);
    const f1 = [{ dirRel: 'artifacts', entries: aRows }];
    const f2 = [{ dirRel: 'mocks', entries: bRows }];
    cache = mergeArtifactListings(cache, A, f1);
    cache = mergeArtifactListings(cache, A, f2);
    expect(Object.keys(cache.listings).sort()).toEqual(['artifacts', 'mocks']);
    // and in the other landing order
    let other = emptyArtifactCache(A);
    other = mergeArtifactListings(other, A, f2);
    other = mergeArtifactListings(other, A, f1);
    expect(Object.keys(other.listings).sort()).toEqual(['artifacts', 'mocks']);
  });

  it('the initial unowned cache (ownerId "") cannot be written by any real initiative', () => {
    const c0 = emptyArtifactCache('');
    expect(mergeArtifactListings(c0, A, [{ dirRel: 'artifacts', entries: aRows }])).toBe(c0);
    expect(artifactGroups(RUN, c0.listings, {})).toEqual([]);
  });

  it('a partial fan-out (some dirs refused) still merges the ones that landed', () => {
    let cache = emptyArtifactCache(A);
    cache = mergeArtifactListings(cache, A, [
      { dirRel: 'artifacts', entries: aRows },
      { dirRel: 'mocks' }, // refused/absent
      { dirRel: 'gates', entries: [] }, // present but empty
    ]);
    expect(Object.keys(cache.listings).sort()).toEqual(['artifacts', 'gates']);
    // an empty dir still contributes no group
    expect(artifactGroups(RUN, cache.listings, {}).map((g) => g.dirRel)).toEqual(['artifacts']);
  });

  it('the cache is never mutated in place (a stale merge cannot corrupt the live object)', () => {
    const c0 = emptyArtifactCache(A);
    const snapshot = JSON.stringify(c0);
    const c1 = mergeArtifactListings(c0, A, [{ dirRel: 'artifacts', entries: aRows }]);
    expect(JSON.stringify(c0)).toBe(snapshot);
    expect(c1).not.toBe(c0);
  });
});

describe('R3 — the new two-segment `phases/<id>` shape is not a lever', () => {
  const SHAPES = [
    'phases/P1', 'phases/P1/', 'phases//P1', 'phases/P1/..', 'phases/..', 'phases/.',
    'phases/../phases/P1', 'phases/P1/../P2', 'phases/artifacts', 'phases/mocks',
    'phases/phases', 'phases/phases/artifacts', 'phases/tasks', 'phases/.git',
    'phases/P1\\x', 'phases/C:', 'phases', 'phases/', '/phases/P1', 'phases/P1/x',
    'phases/P1/overview.md', 'phases/P1/artifacts/../../../etc',
  ];
  for (const sel of SHAPES) {
    it(`confines or refuses ${JSON.stringify(sel)}`, () => {
      const p = planDocPath(BASE, sel);
      if (p === null) return;
      expect(p.startsWith(`${BASE}/`)).toBe(true);
      expect(p.includes('..')).toBe(false);
      expect(p.includes('\\')).toBe(false);
      // every segment of the suffix must be a real, non-traversing name
      for (const seg of p.slice(BASE.length + 1).split('/')) {
        expect(seg, `bad segment in ${p}`).not.toBe('');
        expect(seg).not.toBe('.');
        expect(seg.includes('..')).toBe(false);
      }
    });
  }
  it('a two-segment phase dir resolves to exactly the phase dir, nothing more', () => {
    expect(planDocPath(BASE, 'phases/P1')).toBe(`${BASE}/phases/P1`);
    // and it is the parent of the already-reachable overview, so no new depth is exposed
    expect(planDocPath(BASE, 'P1')).toBe(`${BASE}/phases/P1/overview.md`);
  });
  it('`phases/<id>/<non-doc-dir>` stays refused even though `phases/<id>` is now allowed', () => {
    for (const d of ['tasks', 'notes', '.git', 'secrets', 'Artifacts']) {
      expect(planDocPath(BASE, `phases/P1/${d}`), d).toBe(null);
      expect(planDocPath(BASE, `phases/P1/${d}/x`), d).toBe(null);
    }
  });
});

describe('R2 — the doc-dir list stays closed', () => {
  it('is exactly seven names and nothing case- or prefix-adjacent', () => {
    expect(docDirNames()).toHaveLength(7);
    for (const n of [
      'tasks', '.git', 'Artifacts', 'ARTIFACTS', 'artifact', 'artifactss', 'artifacts ',
      ' artifacts', 'mock', 'Mocks', 'screens_to_verify', 'screensToVerify', 'gate', 'decision',
      'convention', 'propose', '', '.', '..', 'phases',
    ]) {
      expect(isDocDir(n), n).toBe(false);
    }
  });
  it('each of the seven is reachable at plan AND phase level, and only those', () => {
    for (const d of docDirNames()) {
      expect(planDocPath(BASE, d)).toBe(`${BASE}/${d}`);
      expect(planDocPath(BASE, `phases/P1/${d}`)).toBe(`${BASE}/phases/P1/${d}`);
      expect(phaseArtifactsDir('P1', d)).toBe(`phases/P1/${d}`);
    }
  });
  it('`phases` is not itself a doc dir, so `phases/x/phases/y` is refused', () => {
    expect(isDocDir('phases')).toBe(false);
    expect(planDocPath(BASE, 'phases/P1/phases/y')).toBe(null);
  });
});

describe('residual: a NUL-bearing host name is still offered as a row', () => {
  it('resolves (so the sweep passes) but the host cannot open it', () => {
    const groups = artifactGroups(RUN, { artifacts: [{ name: 'a\u0000b.png', kind: 'file' }] }, {});
    expect(groups[0].nodes[0].id).toBe('artifacts/a\u0000b.png');
    expect(planDocPath(BASE, 'artifacts/a\u0000b.png')).toBe(`${BASE}/artifacts/a\u0000b.png`);
  });
});

describe('phase-id trimming is deliberate and consistent', () => {
  it('phaseDir/phaseArtifactsDir trim, matching the bare-phase-id branch of planDocPath', () => {
    expect(phaseDir(' ok ')).toBe('phases/ok');
    expect(phaseArtifactsDir(' ok ')).toBe('phases/ok/artifacts');
    expect(planDocPath(BASE, ' ok ')).toBe(`${BASE}/phases/ok/overview.md`);
    // so the dir a phase is probed at and the doc its row opens agree
    expect(planDocPath(BASE, phaseDir(' ok ')!)).toBe(`${BASE}/phases/ok`);
  });
});
