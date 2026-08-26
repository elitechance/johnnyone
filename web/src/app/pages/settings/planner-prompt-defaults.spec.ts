import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { SHIPPED_PROMPT_DEFAULTS } from './planner-prompt-defaults';

const PINNED: Record<string, bigint> = {
  'planning.planner': 0x0fc962193643a37bn,
  'planning.reviewer': 0x42e3b3d0e4178fe5n,
  'development.worker': 0xbb568e9822273934n,
  'development.reviewer': 0x882713aa907d5d7fn,
  'planning.amendPlanner': 0x7f841cf263c8c761n,
  'planning.amendReviewer': 0xea96c82d6fcfac66n,
  'smallMode.planner': 0xf4d9e4c41b8ee5e3n,
  'smallMode.reviewer': 0x37ef8923327e716dn,
  'smallMode.leafWrapper': 0x464e7f9226edda96n,
  'smallMode.amendPlanner': 0x9f3c6a26c7cfc32dn,
};

function fnv1a64(s: string): bigint {
  let h = 0xcbf29ce484222325n;
  const bytes = new TextEncoder().encode(s);
  for (const b of bytes) {
    h ^= BigInt(b);
    h = (h * 0x100000001b3n) & 0xffffffffffffffffn;
  }
  return h;
}

const MARKERS: Record<string, string> = {
  'planning.planner': 'Create or update a methodology-compliant plan',
  'planning.reviewer': 'ROLE: T2_PLAN_REVIEWER',
  'planning.amendPlanner': 'T1_PLANNER (AMEND',
  'planning.amendReviewer': 'T2_PLAN_REVIEWER (AMEND MODE)',
  'development.worker': 'ROLE: T1_WORKER',
  'development.reviewer': 'ROLE: T2_REVIEWER',
  'smallMode.planner': 'ROLE: T1_PLANNER (LOCAL-SMALL)',
  'smallMode.reviewer': 'The plan-check script has already validated',
  'smallMode.leafWrapper': 'You are a leaf executor.',
  'smallMode.amendPlanner': 'ROLE: T1_PLANNER (LOCAL-SMALL AMEND)',
};

describe('planner-prompt-defaults lockstep', () => {
  it('has no Angular / Ionic import', () => {
    const src = readFileSync(
      resolve(dirname(fileURLToPath(import.meta.url)), 'planner-prompt-defaults.ts'),
      'utf8',
    );
    const imports = src.split('\n').filter((line) => /^\s*import\b/.test(line)).join('\n');
    expect(imports).not.toMatch(/@angular\/|@ionic\//);
  });

  it('pins ten fnv1a64 hashes matching Rust commercial_templates_match_pinned_fingerprints', () => {
    const keys = Object.keys(PINNED);
    expect(keys).toHaveLength(10);
    expect(SHIPPED_PROMPT_DEFAULTS['development.workerNudge']).toBeUndefined();
    for (const key of keys) {
      const body = SHIPPED_PROMPT_DEFAULTS[key];
      expect(body, key).toBeTypeOf('string');
      expect(fnv1a64(body), key).toBe(PINNED[key]);
    }
  });

  it('each defaulted key contains its marker', () => {
    for (const [key, marker] of Object.entries(MARKERS)) {
      expect(SHIPPED_PROMPT_DEFAULTS[key]).toContain(marker);
    }
  });

  it('does not invent DEFAULT_DEVELOPMENT_WORKER_NUDGE', () => {
    const src = readFileSync(
      resolve(dirname(fileURLToPath(import.meta.url)), 'planner-prompt-defaults.ts'),
      'utf8',
    );
    expect(src).not.toMatch(/DEFAULT_DEVELOPMENT_WORKER_NUDGE/);
    expect(src).not.toMatch(/["']development\.workerNudge["']\s*:/);
  });
});
