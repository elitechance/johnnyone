/**
 * Pure, framework-free row/summary shaping for the consolidated host dashboard
 * (`status.page`, Overhaul P9 / phase 03). Angular/Ionic-free so the plugin-less
 * host-app vitest (`host-app/vitest.config.ts`) can pin every row.
 *
 * Type-only imports (erased at compile) keep this module runtime-independent of
 * the Angular services that own the data shapes.
 */
import type { DetectedTool } from '../../services/host-status.service';
import type { HostSettings } from '../../services/host-settings.service';

/** A status pill: a label + a semantic tone driving its color (P01 tokens). */
export type ChipTone = 'ok' | 'warn' | 'off';
export interface StatusChip {
  label: string;
  tone: ChipTone;
}

export interface StatusRowsInput {
  hostUp: boolean;
  workerUp: boolean;
  relayConnected: boolean;
  workerUrl: string;
  relayEmail: string;
  relayHeartbeat: string | null;
  activeSessions: number;
  /** Explicit clock for deterministic heartbeat formatting (defaults to now). */
  nowMs?: number;
}

export interface StatusRow {
  title: string;
  value: string;
  chip: StatusChip;
  badgeNew?: boolean;
}

/** The four mock Status rows (mock lines 142-148), each chip derived from state. */
export function statusRows(input: StatusRowsInput): StatusRow[] {
  const {
    hostUp,
    workerUp,
    relayConnected,
    workerUrl,
    relayEmail,
    relayHeartbeat,
    activeSessions,
    nowMs,
  } = input;
  return [
    {
      title: 'Host process',
      value: '127.0.0.1:7788 · GraphQL + host API',
      chip: hostUp ? { label: 'Up', tone: 'ok' } : { label: 'Down', tone: 'off' },
    },
    {
      title: 'Worker reachable',
      value: workerUrl || 'Worker URL not configured',
      chip: workerUp ? { label: 'Reachable', tone: 'ok' } : { label: 'Offline', tone: 'warn' },
    },
    {
      title: 'Relay connected',
      value: `${relayEmail} · heartbeat ${relativeTime(relayHeartbeat, nowMs)}`,
      chip: relayConnected
        ? { label: 'Connected', tone: 'ok' }
        : { label: 'Offline', tone: 'warn' },
    },
    {
      title: 'Active sessions',
      value: sessionSummary(activeSessions),
      chip:
        activeSessions > 0
          ? { label: `${activeSessions} running`, tone: 'ok' }
          : { label: 'Idle', tone: 'off' },
      badgeNew: true,
    },
  ];
}

/** The toolbar summary chip: ok/all-up, warn/needs-attention, off/host-down. */
export function overallHealth(input: {
  hostUp: boolean;
  workerUp: boolean;
  relayConnected: boolean;
}): StatusChip {
  if (!input.hostUp) return { label: 'Host down', tone: 'off' };
  if (input.hostUp && input.workerUp && input.relayConnected) {
    return { label: 'All systems up', tone: 'ok' };
  }
  return { label: 'Needs attention', tone: 'warn' };
}

export interface ProviderRow {
  provider: string;
  path: string;
  model: string;
  chip: StatusChip;
}

/**
 * One row per detected provider CLI — data-driven, so kloo (or any provider)
 * appears iff `detectCliTools()` returns it (no hardcode, D4). Empty → [].
 */
export function providerRows(tools: DetectedTool[]): ProviderRow[] {
  return tools.map((tool) => ({
    provider: tool.provider,
    path: tool.path ?? tool.command,
    model: tool.defaultModel || '—',
    chip: tool.found ? { label: 'Ready', tone: 'ok' } : { label: 'Missing', tone: 'off' },
  }));
}

export interface StorePathRow {
  title: string;
  value: string;
  badgeNew?: boolean;
  editable: boolean;
  chip?: StatusChip;
}

/** The six Store & paths rows (mock lines 160-166), values from HostSettings (P02). */
export function storePathRows(s: HostSettings): StorePathRow[] {
  return [
    { title: 'Initiatives store', value: s.initiativesDir, badgeNew: true, editable: true },
    { title: 'Files root', value: s.filesRoot, badgeNew: true, editable: true },
    { title: 'Methodology', value: s.plannerMethodologyPath, editable: true },
    { title: 'Conventions', value: s.plannerConventionsPath, editable: true },
    { title: 'Worker URL', value: s.workerUrl, editable: true },
    {
      title: 'Discord alerts',
      value: s.discordWebhookUrl ? 'webhook configured' : 'not set',
      editable: false,
      chip: s.discordWebhookUrl
        ? { label: 'On', tone: 'ok' }
        : { label: 'Off', tone: 'off' },
    },
  ];
}

/** Active-sessions summary line — a count only, never a fabricated breakdown (D8). */
export function sessionSummary(count: number): string {
  return count === 0 ? 'No active sessions' : `${count} active`;
}

/**
 * Tiny dependency-free "Ns/Nm/Nh/Nd ago" formatter for the relay heartbeat.
 * Deterministic given an explicit `nowMs` (defaults to Date.now() at call time,
 * evaluated per-call — never at module load — so it stays test-stable).
 */
export function relativeTime(iso: string | null, nowMs: number = Date.now()): string {
  if (!iso) return '—';
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return '—';
  const secs = Math.max(0, Math.round((nowMs - then) / 1000));
  if (secs < 60) return `${secs}s ago`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}h ago`;
  const days = Math.floor(hrs / 24);
  return `${days}d ago`;
}
