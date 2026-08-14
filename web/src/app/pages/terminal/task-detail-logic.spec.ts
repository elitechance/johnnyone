import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, it, expect } from 'vitest';
import { clearSelectedId, taskDetail } from './task-detail-logic';

const here = dirname(fileURLToPath(import.meta.url));

const spec = {
  id: '04-d',
  files: ['src/d.rs'],
  verify: 'cargo test d -- --exact',
  must_contain: ['pub fn add'],
  depends_on: ['03-c'],
};

describe('taskDetail', () => {
  it('failed attempt marks exit and verify fail', () => {
    const v = taskDetail(
      { id: '04-d', status: 'failed', attempts: [{}], route: 'planner' },
      spec,
      {
        success: false,
        failure_code: 'verify_failed',
        files_changed: ['src/d.rs'],
        off_scope_edits: [],
        rail_fires: [],
        postchecks: {},
        transcript_tail: 'boom',
        failed: [
          { check: 'exit', reason: 'exit_code=1' },
          { check: 'verify', reason: 'verify failed' },
        ],
      },
    );
    const by = Object.fromEntries(v.checks.map((c) => [c.check, c]));
    expect(by.exit.ok).toBe(false);
    expect(by.verify.ok).toBe(false);
    expect(by.scope.ok).toBe(true);
    expect(by.changed.ok).toBe(true);
    expect(by.must_contain.ok).toBe(true);
    expect(v.failureCode).toBe('verify_failed');
    expect(v.filesChanged).toEqual(['src/d.rs']);
    expect(v.ruleAlert).toBeTruthy();
  });

  it('done fixture: five checks pass, commit sha, no red alert', () => {
    const v = taskDetail(
      { id: '01-a', status: 'done', attempts: [{}], commitSha: 'abc123def' },
      spec,
      { success: true, failed: [] },
    );
    expect(v.checks).toHaveLength(5);
    expect(v.checks.every((c) => c.ok)).toBe(true);
    expect(v.commitSha).toBe('abc123def');
    expect(v.ruleAlert).toBeNull();
  });

  it('no-attempt pending: spec fields, copy, empty checks, no throw', () => {
    const v = taskDetail({ id: '02-b', status: 'pending', attempts: [] }, spec, null);
    expect(v.files).toEqual(['src/d.rs']);
    expect(v.verify).toContain('cargo test');
    expect(v.attemptsCopy).toBe('No attempt yet');
    expect(v.checks).toEqual([]);
    expect(v.success).toBeNull();
  });

  it('missing JSON still shows spec', () => {
    const v = taskDetail({ id: 'x', status: 'blocked' }, spec, undefined);
    expect(v.files[0]).toBe('src/d.rs');
    expect(v.checks).toEqual([]);
  });

  it('back action is selectedId null', () => {
    expect(clearSelectedId()).toBeNull();
  });
});

describe('wiring — task detail', () => {
  const html = readFileSync(resolve(here, 'terminal.page.html'), 'utf8');
  const ts = readFileSync(resolve(here, 'terminal.page.ts'), 'utf8');
  it('mentions taskDetail and back-to-table', () => {
    expect(html + ts).toMatch(/taskDetail/);
    expect(html).toMatch(/back|clearSelected|selectedTaskId/i);
  });
});
