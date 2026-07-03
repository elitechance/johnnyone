import { describe, it, expect } from 'vitest';
import {
  statusRows,
  overallHealth,
  providerRows,
  storePathRows,
  sessionSummary,
  relativeTime,
} from './host-dashboard-logic';
import type { DetectedTool } from '../../services/host-status.service';
import type { HostSettings } from '../../services/host-settings.service';

// Pure spec (no Angular/Ionic) — runs under host-app/vitest.config.ts.
// Pins every dashboard row/summary the mock .frame renders (P9 / phase 03).

const NOW = Date.parse('2026-07-03T12:00:00Z');

const upInput = {
  hostUp: true,
  workerUp: true,
  relayConnected: true,
  workerUrl: 'johnnyone.ethan-353.workers.dev',
  relayEmail: 'silverjrom@gmail.com',
  relayHeartbeat: '2026-07-03T11:59:49Z', // 11s before NOW
  activeSessions: 4,
  nowMs: NOW,
};

describe('statusRows', () => {
  it('renders the four mock rows in order', () => {
    const rows = statusRows(upInput);
    expect(rows).toHaveLength(4);
    expect(rows.map((r) => r.title)).toEqual([
      'Host process',
      'Worker reachable',
      'Relay connected',
      'Active sessions',
    ]);
  });

  it('shows ok chips when everything is up', () => {
    const rows = statusRows(upInput);
    expect(rows[0].chip).toEqual({ label: 'Up', tone: 'ok' });
    expect(rows[1].chip).toEqual({ label: 'Reachable', tone: 'ok' });
    expect(rows[2].chip).toEqual({ label: 'Connected', tone: 'ok' });
    expect(rows[3].chip).toEqual({ label: '4 running', tone: 'ok' });
  });

  it('degrades chips when things are down', () => {
    const rows = statusRows({
      ...upInput,
      hostUp: false,
      workerUp: false,
      relayConnected: false,
      activeSessions: 0,
    });
    expect(rows[0].chip).toEqual({ label: 'Down', tone: 'off' });
    expect(rows[1].chip).toEqual({ label: 'Offline', tone: 'warn' });
    expect(rows[2].chip).toEqual({ label: 'Offline', tone: 'warn' });
    expect(rows[3].chip).toEqual({ label: 'Idle', tone: 'off' });
  });

  it('puts the NEW badge only on Active sessions', () => {
    const rows = statusRows(upInput);
    expect(rows[3].badgeNew).toBe(true);
    expect(rows[0].badgeNew).toBeUndefined();
    expect(rows[1].badgeNew).toBeUndefined();
    expect(rows[2].badgeNew).toBeUndefined();
  });

  it('renders the relay email + relative heartbeat (never a token)', () => {
    const rows = statusRows(upInput);
    expect(rows[2].value).toBe('silverjrom@gmail.com · heartbeat 11s ago');
  });

  it('falls back when the worker URL is empty', () => {
    const rows = statusRows({ ...upInput, workerUrl: '' });
    expect(rows[1].value).toBe('Worker URL not configured');
  });
});

describe('overallHealth', () => {
  it('is ok/All systems up when host+worker+relay up', () => {
    expect(overallHealth({ hostUp: true, workerUp: true, relayConnected: true })).toEqual({
      label: 'All systems up',
      tone: 'ok',
    });
  });

  it('is warn/Needs attention when the worker is down but host up', () => {
    expect(overallHealth({ hostUp: true, workerUp: false, relayConnected: true })).toEqual({
      label: 'Needs attention',
      tone: 'warn',
    });
  });

  it('is off/Host down when the host is down', () => {
    expect(overallHealth({ hostUp: false, workerUp: true, relayConnected: true })).toEqual({
      label: 'Host down',
      tone: 'off',
    });
  });
});

describe('providerRows', () => {
  const tools: DetectedTool[] = [
    { provider: 'claude_code', command: 'claude', found: true, path: '~/.local/bin/claude', defaultModel: 'opus-4.8' },
    { provider: 'codex', command: 'codex', found: false, path: null, defaultModel: null },
  ];

  it('maps path (path ?? command), model (default || —) and Ready/Missing', () => {
    const rows = providerRows(tools);
    expect(rows[0]).toEqual({
      provider: 'claude_code',
      path: '~/.local/bin/claude',
      model: 'opus-4.8',
      chip: { label: 'Ready', tone: 'ok' },
    });
    expect(rows[1]).toEqual({
      provider: 'codex',
      path: 'codex', // falls back to command when path is null
      model: '—',
      chip: { label: 'Missing', tone: 'off' },
    });
  });

  it('is data-driven — a kloo row appears iff detectCliTools returns it (D4)', () => {
    const withKloo = providerRows([
      ...tools,
      { provider: 'kloo', command: 'kloo', found: true, path: '~/go/bin/kloo', defaultModel: 'dsv4' },
    ]);
    const kloo = withKloo.find((r) => r.provider === 'kloo');
    expect(kloo).toEqual({
      provider: 'kloo',
      path: '~/go/bin/kloo',
      model: 'dsv4',
      chip: { label: 'Ready', tone: 'ok' },
    });
    // ...and NOT present when the backend doesn't detect it.
    expect(providerRows(tools).some((r) => r.provider === 'kloo')).toBe(false);
  });

  it('returns [] for no detected CLIs (card shows the empty state)', () => {
    expect(providerRows([])).toEqual([]);
  });
});

describe('storePathRows', () => {
  const settings: HostSettings = {
    workerUrl: 'https://johnnyone.ethan-353.workers.dev',
    tenantId: 't',
    userId: 'u',
    plannerMethodologyPath: 'lokal/agents/common/methodology.md',
    plannerConventionsPath: 'lokal/agents/common/conventions',
    webClientUrl: 'https://johnnyone.pages.dev/',
    discordWebhookUrl: '',
    initiativesDir: '/home/creepy/Documents/Workspace/.johnnyone/initiatives',
    filesRoot: '/home/creepy/Documents/Workspace',
  };

  it('renders six rows in order incl. the two P02 paths', () => {
    const rows = storePathRows(settings);
    expect(rows).toHaveLength(6);
    expect(rows.map((r) => r.title)).toEqual([
      'Initiatives store',
      'Files root',
      'Methodology',
      'Conventions',
      'Worker URL',
      'Discord alerts',
    ]);
    expect(rows[0].value).toBe('/home/creepy/Documents/Workspace/.johnnyone/initiatives');
    expect(rows[1].value).toBe('/home/creepy/Documents/Workspace');
  });

  it('NEW-badges the two new store paths only', () => {
    const rows = storePathRows(settings);
    expect(rows[0].badgeNew).toBe(true);
    expect(rows[1].badgeNew).toBe(true);
    expect(rows[2].badgeNew).toBeUndefined();
  });

  it('shows the Discord chip off/not-set when unset, on/configured when set', () => {
    const off = storePathRows(settings)[5];
    expect(off.value).toBe('not set');
    expect(off.chip).toEqual({ label: 'Off', tone: 'off' });

    const on = storePathRows({ ...settings, discordWebhookUrl: 'https://discord.com/api/webhooks/x' })[5];
    expect(on.value).toBe('webhook configured');
    expect(on.chip).toEqual({ label: 'On', tone: 'ok' });
  });
});

describe('sessionSummary', () => {
  it('reads "No active sessions" at 0 and "N active" otherwise', () => {
    expect(sessionSummary(0)).toBe('No active sessions');
    expect(sessionSummary(3)).toBe('3 active');
  });
});

describe('relativeTime', () => {
  it('is — for null / unparseable', () => {
    expect(relativeTime(null, NOW)).toBe('—');
    expect(relativeTime('not-a-date', NOW)).toBe('—');
  });

  it('formats seconds / minutes / hours / days ago', () => {
    expect(relativeTime('2026-07-03T11:59:49Z', NOW)).toBe('11s ago');
    expect(relativeTime('2026-07-03T11:55:00Z', NOW)).toBe('5m ago');
    expect(relativeTime('2026-07-03T09:00:00Z', NOW)).toBe('3h ago');
    expect(relativeTime('2026-07-01T12:00:00Z', NOW)).toBe('2d ago');
  });
});
