import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, it, expect } from 'vitest';
import { providerOptions } from '../validation-config/validation-config-logic';
import {
  agentChoices,
  createPayload,
  executorFromDoctor,
  FALLBACK_EXECUTOR,
  parseDoctorJson,
  preflightView,
} from './briefing-logic';

const here = dirname(fileURLToPath(import.meta.url));

describe('agentChoices', () => {
  it('appends kloo as a mode and never lists it as a provider', () => {
    const choices = agentChoices(
      [{ provider: 'claude_code', found: true }, { provider: 'kloo', found: true }],
      providerOptions(),
    );
    expect(choices.some((c) => c.value === 'kloo' && c.kind === 'mode')).toBe(true);
    expect(choices.filter((c) => c.kind === 'provider').map((c) => c.value)).not.toContain('kloo');
  });
});

describe('createPayload', () => {
  const fields = { workspacePath: '/w', brief: 'do it', title: 't' };

  it('kloo payload never has workerProvider kloo and has executorConfig', () => {
    const p = createPayload('kloo', 'claude_code', fields, null);
    expect(p.workerProvider).toBe('claude_code');
    expect(p.workerProvider).not.toBe('kloo');
    expect(p.executorConfig).toBeTruthy();
    const exec = JSON.parse(p.executorConfig!);
    expect(exec.mode).toBe('local-small');
    expect(exec.provider).toBe(FALLBACK_EXECUTOR.provider);
    expect(exec.model).toBe(FALLBACK_EXECUTOR.model);
    expect(exec.ctx).toBe(FALLBACK_EXECUTOR.ctx);
  });

  it('commercial payload has no executorConfig', () => {
    const p = createPayload('claude_code', 'claude_code', fields);
    expect(p.workerProvider).toBe('claude_code');
    expect(p.executorConfig).toBeUndefined();
  });

  it('normalizes doctor.profile object to a path string', () => {
    const p = createPayload('kloo', 'claude_code', fields, {
      provider: 'openrouter',
      model: 'qwen/qwen3-coder',
      ctx: 32768,
      profile: { path: '/home/creepy/.config/kloo/profiles.json', exists: true },
    });
    const exec = JSON.parse(p.executorConfig!);
    expect(typeof exec.profile).toBe('string');
    expect(exec.profile).toBe('/home/creepy/.config/kloo/profiles.json');
  });

  it('doctor-derived values win over fallback literals', () => {
    const p = createPayload('kloo', 'grok', fields, {
      provider: 'other',
      model: 'other/model',
      ctx: 8192,
      profile: '/tmp/p.json',
    });
    const exec = JSON.parse(p.executorConfig!);
    expect(exec.provider).toBe('other');
    expect(exec.model).toBe('other/model');
    expect(exec.ctx).toBe(8192);
    expect(exec.model).not.toBe(FALLBACK_EXECUTOR.model);
    expect(p.workerProvider).toBe('grok');
  });

  it('doctor null uses fallback literals', () => {
    const exec = executorFromDoctor(null);
    expect(exec).toEqual(FALLBACK_EXECUTOR);
  });
});

describe('providerOptions still excludes kloo', () => {
  it('does not contain kloo', () => {
    expect(providerOptions()).not.toContain('kloo');
  });
});

describe('preflightView', () => {
  const okDoctor = {
    endpoint: 'https://openrouter.ai',
    api_key: { set: true },
    model: 'other/model',
    ctx: 8192,
    provider: 'other',
    verify: { source: 'profile' },
  };
  const okProbe = {
    tool_call: true,
    file_edit: true,
    json_only: true,
    context: { configured: 32768, advertised: 32768, source: 'advertised' },
  };

  it('ok fixture → probe passed and shows doctor model not the fallback', () => {
    const v = preflightView({ doctor: okDoctor, probe: okProbe });
    expect(v.phase).toBe('ok');
    expect(v.alerts.some((a) => a.kind === 'ok' && a.text === 'probe passed')).toBe(true);
    expect(v.lines.some((l) => l.includes('context.configured'))).toBe(true);
    expect(v.lines.some((l) => l.includes('other/model'))).toBe(true);
    expect(v.lines.some((l) => l.includes('qwen/qwen3-coder'))).toBe(false);
  });

  it('not-advertised context warns about over-compaction', () => {
    const v = preflightView({
      doctor: okDoctor,
      probe: { ...okProbe, context: { source: 'not-advertised' } },
    });
    expect(v.alerts.some((a) => /over-compaction/i.test(a.text))).toBe(true);
  });

  it('verify auto-detect warns', () => {
    const v = preflightView({
      doctor: { ...okDoctor, verify: { source: 'auto-detect' } },
      probe: okProbe,
    });
    expect(v.alerts.some((a) => a.kind === 'warn' && /auto-detect/.test(a.text))).toBe(true);
  });

  it('running has no JSON lines and disables the button', () => {
    const v = preflightView('running');
    expect(v.phase).toBe('running');
    expect(v.buttonDisabled).toBe(true);
    expect(v.lines).toEqual([]);
    expect(v.cardCopy).toContain('Running doctor + probe');
  });

  it('spawn error is danger, no throw', () => {
    const v = preflightView({ error: 'spawn failed: ENOENT' });
    expect(v.phase).toBe('error');
    expect(v.alerts[0]).toEqual({ kind: 'danger', text: 'kloo not found' });
  });

  it('api_key.set false warns', () => {
    const v = preflightView({
      doctor: { ...okDoctor, api_key: { set: false } },
      probe: okProbe,
    });
    expect(v.alerts.some((a) => a.text === 'API key not set')).toBe(true);
  });

  it('probe file_edit false names the check', () => {
    const v = preflightView({
      doctor: okDoctor,
      probe: { ...okProbe, file_edit: false },
    });
    expect(v.alerts.some((a) => a.text === 'probe failed: file_edit')).toBe(true);
  });
});

describe('wiring — briefing page uses the seam', () => {
  const html = readFileSync(resolve(here, 'briefing.page.html'), 'utf8');
  const ts = readFileSync(resolve(here, 'briefing.page.ts'), 'utf8');

  it('template and page mention createPayload / executor card / preflightView', () => {
    expect(ts).toMatch(/createPayload|briefing-logic/);
    expect(html).toMatch(/executor|kloo|preflight/i);
    expect(ts).toMatch(/preflightView|runPreflight/);
    expect(html).toMatch(/runPreflight|preflightView/);
  });
});

describe('parseDoctorJson', () => {
  it('returns null on junk', () => {
    expect(parseDoctorJson('nope')).toBeNull();
    expect(parseDoctorJson(null)).toBeNull();
  });
  it('treats {"error":"kloo not found"} as missing, not a doctor', () => {
    expect(parseDoctorJson('{"error":"kloo not found"}')).toBeNull();
    expect(preflightView({ error: 'kloo not found' }).alerts[0]).toEqual({
      kind: 'danger',
      text: 'kloo not found',
    });
  });
  it('keeps the real kloo doctor profile object so executorFromDoctor can normalize it', () => {
    const d = parseDoctorJson(
      '{"provider":"openrouter","model":"qwen/qwen3-coder","ctx":32768,"profile":{"path":"/home/creepy/.config/kloo/profiles.json","exists":true},"api_key":{"set":true}}',
    );
    expect(d?.profile).toEqual({
      path: '/home/creepy/.config/kloo/profiles.json',
      exists: true,
    });
    expect(executorFromDoctor(d).profile).toBe('/home/creepy/.config/kloo/profiles.json');
  });
});
