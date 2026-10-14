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
    expect(planDocPath(BASE, 'phases/artifacts/artifacts')).toBe(
      `${BASE}/phases/artifacts/artifacts`,
    );
  });
  it('a phase id with surrounding whitespace survives the dir but NOT the path', () => {
    // phaseArtifactsDir trims; so this is consistent
    expect(phaseArtifactsDir(' ok ')).toBe('phases/ok/artifacts');
  });
});

describe('DEAD ROW: a name offered by artifactGroups that planDocPath rejects', () => {
  const RUN = { plan: {}, phases: [], tasks: [], events: [] } as unknown as AgentPlanRun;
  it('offers a row for a file whose name contains .. but cannot open it', () => {
    const groups = artifactGroups(RUN, { artifacts: [{ name: 'v1..2.png', kind: 'file' }] }, {});
    expect(groups[0].nodes.map((n) => n.id)).toEqual(['artifacts/v1..2.png']);
    // the row exists, but the allowlist refuses the path → a dead button
    expect(planDocPath(BASE, 'artifacts/v1..2.png')).toBe(null);
  });
  it('offers a row for a trailing-space name that resolves to the WRONG path', () => {
    const groups = artifactGroups(RUN, { artifacts: [{ name: 'report.md ', kind: 'file' }] }, {});
    expect(groups[0].nodes[0].id).toBe('artifacts/report.md ');
    // trim() in relPlanDocPath silently drops the trailing space the real file has
    expect(planDocPath(BASE, 'artifacts/report.md ')).toBe(`${BASE}/artifacts/report.md`);
  });
  it('offers a row for a space-only name that is a DEAD button', () => {
    const groups = artifactGroups(RUN, { artifacts: [{ name: ' ', kind: 'file' }] }, {});
    expect(groups[0].nodes[0].id).toBe('artifacts/ ');
    // trim() turns this into 'artifacts/', which has an empty segment → rejected. Dead row.
    expect(planDocPath(BASE, 'artifacts/ ')).toBe(null);
    expect(artifactOpenMode('artifacts/ ')).toBe('viewer');
  });
});

describe('artifactDirsFor — duplicate phase ids', () => {
  it('emits the same dir twice when the host repeats a phase id', () => {
    const r = {
      plan: {}, tasks: [], events: [],
      phases: [{ phaseId: 'P1' }, { phaseId: 'P1' }],
    } as unknown as AgentPlanRun;
    expect(artifactDirsFor(r)).toEqual([
      'artifacts', 'phases/P1/artifacts', 'phases/P1/artifacts',
    ]);
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
