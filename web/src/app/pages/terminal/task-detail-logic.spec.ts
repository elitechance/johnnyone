import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, it, expect } from 'vitest';
import { taskDetail } from './task-detail-logic';

const here = dirname(fileURLToPath(import.meta.url));

const spec = {
  id: '04-d',
  files: ['src/d.rs'],
  verify: 'cargo test d -- --exact',
  must_contain: ['pub fn add'],
  depends_on: ['03-c'],
};

describe('taskDetail', () => {
  it('failed AttemptRecord (camelCase checks.failed) marks exit and verify fail', () => {
    const attempt = {
      tier: 'qwen3-coder',
      model: 'qwen/qwen3-coder',
      attempt: 1,
      startedAt: '2026-08-14T12:00:00Z',
      endedAt: '2026-08-14T12:00:02Z',
      failureCode: 'verify_failed',
      class: 'model',
      checks: {
        passed: false,
        failed: [
          { check: 'exit', reason: 'exit_code=1' },
          { check: 'verify', reason: 'verify failed' },
        ],
      },
    };
    const v = taskDetail(
      { id: '04-d', status: 'failed', attempts: [attempt], route: 'planner' },
      spec,
      attempt,
      undefined,
      [spec, { id: '05-e', dependsOn: ['04-d'] }],
    );
    const by = Object.fromEntries(v.checks.map((c) => [c.check, c]));
    expect(by.exit.ok).toBe(false);
    expect(by.exit.label).toBe('exit clean');
    expect(by.exit.meaning).toBeTruthy();
    expect(by.verify.ok).toBe(false);
    expect(by.verify.label).toBe('postcheck passed');
    expect(by.scope.ok).toBe(true);
    expect(by.scope.label).toBe('changed subset-of allowed');
    expect(by.changed.ok).toBe(true);
    expect(by.changed.label).toBe('files actually changed');
    expect(by.must_contain.ok).toBe(true);
    expect(by.must_contain.label).toBe('must_contain present');
    expect(v.hasAttempt).toBe(true);
    expect(v.failureCode).toBe('verify_failed');
    expect(v.ruleAlert).toBeTruthy();
    expect(v.blocks).toEqual(['05-e']);
    expect(v.attemptRows).toHaveLength(1);
    expect(v.attemptRows[0].tier).toBe('qwen3-coder');
  });

  it('KLOO_RESULT_JSON attempt file populates machine fields', () => {
    const v = taskDetail(
      { id: '04-d', status: 'failed', attempts: [{ attempt: 1 }] },
      spec,
      {
        success: false,
        failure_code: 'verify_failed',
        files_changed: { count: 1, paths: ['src/d.rs'] },
        off_scope_edits: 0,
        rail_fires: [{ rail: 'x' }],
        postchecks: [{ command: 'c', passed: true }],
        verify: { command: 'cargo test d -- --exact', passed: false },
        transcript_tail: 'boom',
      },
    );
    expect(v.success).toBe(false);
    expect(v.klooCommand).toContain('cargo test');
    expect(v.filesChanged).toEqual(['src/d.rs']);
    expect(v.postchecks).toBeTruthy();
    expect(v.transcriptTail).toBe('boom');
    expect(v.hasAttempt).toBe(true);
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
    expect(v.hasAttempt).toBe(false);
    expect(v.checks).toEqual([]);
    expect(v.success).toBeNull();
  });

  it('missing JSON still shows spec', () => {
    const v = taskDetail({ id: 'x', status: 'blocked' }, spec, undefined);
    expect(v.files[0]).toBe('src/d.rs');
    expect(v.checks).toEqual([]);
  });

});

describe('wiring — task detail', () => {
  const html = readFileSync(resolve(here, 'terminal.page.html'), 'utf8');
  const ts = readFileSync(resolve(here, 'terminal.page.ts'), 'utf8');
  it('mentions taskDetail and back-to-table', () => {
    expect(html + ts).toMatch(/taskDetail/);
    expect(html).toMatch(/back|onBackToTasks|selectedTaskId/i);
    expect(html).toMatch(/hasAttempt/);
    expect(html).toMatch(/c\.label/);
    expect(html).toMatch(/c\.meaning/);
  });
});
