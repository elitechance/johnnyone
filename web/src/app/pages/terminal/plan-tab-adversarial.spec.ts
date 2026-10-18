import { describe, it, expect } from 'vitest';
import {
  planDocPath,
  artifactGroups,
  artifactDirsFor,
  phaseArtifactsDir,
  artifactOpenMode,
} from './plan-tab-logic';
import type { AgentPlanRun } from '../../../../../ui/src/services/johnny-api.service';

// QA adversarial sweep over the ONE allowlist (`planDocPath`). Every selection below must either
// resolve strictly inside `<planPath>/` or resolve to null. Nothing may escape.

const BASE = '/home/u/Workspace/.johnnyone/initiatives/abc/plan';

const ATTACKS: string[] = [
  // URL / percent encodings (nothing in the chain decodes — assert they stay literal or null)
  '%2e%2e/secret', '%2e%2e%2fsecret', 'artifacts/%2e%2e/%2e%2e/etc/passwd',
  'artifacts/%2e%2e%2f%2e%2e%2fetc/passwd', '%2E%2E/x', 'artifacts/..%2fx',
  // double / triple encoding
  'artifacts/%252e%252e/x',
  // dot-variants
  '..', '.', '...', '....', '....//', 'artifacts/....//x', 'artifacts/....', 'artifacts/...',
  '..;/x', '.../...//x', 'artifacts/.../..//x',
  // NUL + control bytes
  'artifacts/a\u0000b', 'artifacts/\u0000', 'artifacts/..\u0000/x', '\u0000artifacts/x',
  // unicode lookalikes & normalization
  'artifacts/\uFF0E\uFF0E/x', 'artifacts/\u2215etc\u2215passwd', 'artifacts\u2215../x',
  'artifacts/\u002e\u002e/x', 'artifacts/\uFF0F..\uFF0Fx',
  'artifacts/\u202E..\u202E/x', 'artifacts/e\u0301.md',
  // separators / absolute / UNC / drive
  '/', '//', '///', '/artifacts', 'artifacts/', 'artifacts//', '/artifacts/x', '\\',
  'C:/artifacts/x', 'C:\\artifacts\\x', '\\\\host\\share\\x', 'artifacts\\x',
  // shape confusion
  'artifacts', 'Artifacts', 'ARTIFACTS/x', 'artifactsx/y', 'artifacts.x/y',
  'phases', 'phases/', 'phases/x', 'phases//artifacts/x', 'phases/x/artifacts',
  'phases/./artifacts/x', 'phases/../artifacts/x', 'phases/x/../artifacts/y',
  'phases/x/Artifacts/y', 'phases/x/artifactsy/z', 'phases/x/y/artifacts/z',
  // whitespace
  ' ', '  ', ' artifacts/x ', 'artifacts/ ', 'artifacts/x ', 'artifacts/\t/x', 'artifacts/\n../x',
  // deep nesting
  'artifacts/' + 'a/'.repeat(500) + 'x',
  'artifacts/' + '../'.repeat(50) + 'etc/passwd',
  // the fixed docs with suffixes
  'brief.md/../../../etc/passwd', 'brief.md ', ' brief.md', ' brief.md ', 'brief.md/x',
  // misc
  'secrets/key.pem', '.ssh/id_rsa', 'artifacts/../../../../../../../../etc/passwd',
  // R2: the doc-dir allowlist grew from one name to seven. Near-misses must still be refused as
  // doc dirs (they may fall through to the bare-phase-id shape, which the sweep also confines).
  'mocks', 'mocks/a.html', 'Mocks', 'Mocks/a.html', 'MOCKS/a.html', 'mocksx', 'mocksx/a.html',
  'mocks x/a.html', 'mocks.x/y', 'screens-to-verify', 'screens-to-verify/s.png',
  'screens-to-verify/../x', 'screens-to-verify/..', 'Screens-To-Verify/x', 'screens_to_verify/x',
  'screensToVerify/x', 'decisions/d.md', 'gates/g.md', 'conventions/c.md', 'proposed/p.md',
  'decisionsx/x', 'gates/../../x', 'conventions\\x', 'proposed//x', 'proposed/./x',
  // `tasks/` is deliberately NOT a doc dir (plan structure, 172 instances in the real store)
  'tasks', 'tasks/t.md', 'phases/P1/tasks', 'phases/P1/tasks/t.md', 'phases/P1/tasks/../../../x',
  '.git', '.git/config', 'phases/P1/.git/config',
  // the phase dir itself is listable (one-call probe); its traversals are not
  'phases/P1', 'phases/P1/', 'phases/P1/..', 'phases/P1/../P2', 'phases/P1/../../../etc',
];

describe('planDocPath — QA adversarial sweep', () => {
  for (const sel of ATTACKS) {
    it(`confines or rejects ${JSON.stringify(sel)}`, () => {
      const p = planDocPath(BASE, sel);
      if (p === null) return;
      expect(p.startsWith(`${BASE}/`), `escaped prefix: ${JSON.stringify(p)}`).toBe(true);
      expect(p.includes('..'), `contains ..: ${JSON.stringify(p)}`).toBe(false);
      expect(p.includes('\\'), `contains backslash: ${JSON.stringify(p)}`).toBe(false);
      // normalising the result must not move it out of BASE
      const parts: string[] = [];
      for (const seg of p.split('/')) {
        if (seg === '' || seg === '.') continue;
        if (seg === '..') { parts.pop(); continue; }
        parts.push(seg);
      }
      const norm = '/' + parts.join('/');
      expect(norm.startsWith(`${BASE}/`), `normalised escape: ${norm}`).toBe(true);
    });
  }

});

describe('phase id edge cases', () => {
  it('rejects a .. / empty / dot phase id and confines an `artifacts` phase id', () => {
    expect(phaseArtifactsDir('..')).toBe(null);
    expect(phaseArtifactsDir('')).toBe(null);
    expect(phaseArtifactsDir('   ')).toBe(null);
    expect(phaseArtifactsDir('.')).toBe(null);
    expect(phaseArtifactsDir(null)).toBe(null);
    expect(phaseArtifactsDir('a/b')).toBe(null);
    expect(phaseArtifactsDir('artifacts')).toBe('phases/artifacts/artifacts');
    expect(phaseArtifactsDir('P1', 'mocks')).toBe('phases/P1/mocks');
    expect(phaseArtifactsDir('P1', 'tasks')).toBe(null);
    expect(phaseArtifactsDir('P1', '..')).toBe(null);
    expect(phaseArtifactsDir('P1', 'Mocks')).toBe(null);
    expect(planDocPath(BASE, 'phases/artifacts/artifacts')).toBe(
      `${BASE}/phases/artifacts/artifacts`,
    );
  });
  it('a phase id with surrounding whitespace survives the dir but NOT the path', () => {
    // phaseArtifactsDir trims; so this is consistent
    expect(phaseArtifactsDir(' ok ')).toBe('phases/ok/artifacts');
  });
});

// Was QA's "DEAD ROW" block: these three inputs characterised defects D1/D2 (a row the allowlist
// then refused, and a trimmed id that read the wrong path). The INPUTS are unchanged and the
// invariant is now the fixed contract: every row a group offers must resolve, and resolve to the
// name the host actually reported. A row that cannot open must not be offered at all.
describe('NO DEAD ROW: every offered row resolves, and resolves to the host-reported name', () => {
  const RUN = { plan: {}, phases: [], tasks: [], events: [] } as unknown as AgentPlanRun;

  it('omits a name containing `..` instead of offering a button that does nothing (D1)', () => {
    const groups = artifactGroups(
      RUN,
      { artifacts: [{ name: 'v1..2.png', kind: 'file' }, { name: 'ok.png', kind: 'file' }] },
      {},
    );
    expect(groups[0].nodes.map((n) => n.id)).toEqual(['artifacts/ok.png']);
    // and the allowlist still refuses it, so filter and allowlist now agree
    expect(planDocPath(BASE, 'artifacts/v1..2.png')).toBe(null);
  });

  it('omits a directory containing `..` rather than failing silently to expand (D1)', () => {
    const groups = artifactGroups(RUN, { artifacts: [{ name: 'a..b', kind: 'dir' }] }, {});
    expect(groups).toEqual([]);
    expect(planDocPath(BASE, 'artifacts/a..b')).toBe(null);
  });

  it('keeps a trailing space, because the real file has one (D2)', () => {
    const groups = artifactGroups(RUN, { artifacts: [{ name: 'report.md ', kind: 'file' }] }, {});
    expect(groups[0].nodes[0].id).toBe('artifacts/report.md ');
    expect(planDocPath(BASE, 'artifacts/report.md ')).toBe(`${BASE}/artifacts/report.md `);
  });

  it('a space-only name is a live row, not a dead one (D2)', () => {
    const groups = artifactGroups(RUN, { artifacts: [{ name: ' ', kind: 'file' }] }, {});
    expect(groups[0].nodes[0].id).toBe('artifacts/ ');
    expect(planDocPath(BASE, 'artifacts/ ')).toBe(`${BASE}/artifacts/ `);
    expect(artifactOpenMode('artifacts/ ')).toBe('viewer');
  });

  it('EVERY row of EVERY group resolves, over a hostile listing', () => {
    const hostile = [
      'ok.md', 'report.md ', ' ', 'v1..2.png', 'a..b', '..', '.', 'a/b', 'a\\b', '',
      'x\u0000y', 'e\u0301.md', '\uFF0E\uFF0E', 'C:x', 'très long' + 'g'.repeat(200),
    ].map((name) => ({ name, kind: 'file' }));
    const groups = artifactGroups(RUN, { artifacts: hostile, mocks: hostile }, {});
    expect(groups.length).toBe(2);
    let rows = 0;
    for (const g of groups) {
      for (const n of g.nodes) {
        const p = planDocPath(BASE, n.id);
        expect(p, `dead row: ${JSON.stringify(n.id)}`).not.toBe(null);
        expect(p).toBe(`${BASE}/${g.dirRel}/${n.name}`);
        rows++;
      }
    }
    expect(rows).toBeGreaterThan(0);
  });
});

describe('artifactDirsFor — duplicate phase ids', () => {
  it('emits the same dir twice when the host repeats a phase id', () => {
    const r = {
      plan: {}, tasks: [], events: [],
      phases: [{ phaseId: 'P1' }, { phaseId: 'P1' }],
    } as unknown as AgentPlanRun;
    const PLAN = [
      'artifacts', 'mocks', 'screens-to-verify', 'decisions', 'gates', 'conventions', 'proposed',
    ];
    expect(artifactDirsFor(r)).toEqual([
      ...PLAN,
      ...PLAN.map((d) => `phases/P1/${d}`),
      ...PLAN.map((d) => `phases/P1/${d}`),
    ]);
    // the point QA pinned: a repeated phase id is NOT deduped
    expect(artifactDirsFor(r).filter((d) => d === 'phases/P1/artifacts')).toHaveLength(2);
  });
  it('artifactGroups then renders that dir twice (duplicate track key)', () => {
    const r = {
      plan: {}, tasks: [], events: [],
      phases: [{ phaseId: 'P1', phaseTitle: 'One' }, { phaseId: 'P1', phaseTitle: 'One' }],
    } as unknown as AgentPlanRun;
    const g = artifactGroups(r, { 'phases/P1/artifacts': [{ name: 'a.txt', kind: 'file' }] }, {});
    expect(g.map((x) => x.dirRel)).toEqual(['phases/P1/artifacts', 'phases/P1/artifacts']);
  });
});
