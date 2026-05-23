import { Injectable, signal } from '@angular/core';

/**
 * Service that pings local host + worker and lists detected CLIs.
 *
 * The Tauri shell will eventually expose richer status via IPC (process supervisor
 * state, child PID, log tail). For Phase 3 we use HTTP polling against the
 * host's GraphQL endpoint — good enough for the structural shell.
 */

const HOST_URL = 'http://127.0.0.1:7788/graphql';
const WORKER_URL = 'https://johnnyone-dev-hub.ethan-353.workers.dev/graphql';

interface DetectedTool {
  provider: string;
  command: string;
  found: boolean;
  path?: string | null;
}

@Injectable({ providedIn: 'root' })
export class HostStatusService {
  readonly hostUp = signal(false);
  readonly workerUp = signal(false);
  readonly nodeRegistered = signal(false);
  readonly lastError = signal<string>('');

  async refresh(): Promise<void> {
    this.lastError.set('');
    await Promise.all([this.pingHost(), this.pingWorker()]);
  }

  private async pingHost(): Promise<void> {
    try {
      const res = await fetch(HOST_URL, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ query: '{ health }' }),
      });
      this.hostUp.set(res.ok);
    } catch {
      this.hostUp.set(false);
    }
  }

  private async pingWorker(): Promise<void> {
    try {
      const res = await fetch(WORKER_URL, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ query: '{ __typename }' }),
      });
      this.workerUp.set(res.ok);
    } catch {
      this.workerUp.set(false);
    }
  }

  async detectCliTools(): Promise<DetectedTool[]> {
    // `detectCliTools` is a Mutation in the host's GraphQL schema (it actively
    // re-scans the system PATH). Use `listProviderConfigs` to read the
    // already-detected CLIs without re-scanning; fall back to detect if empty.
    try {
      const res = await fetch(HOST_URL, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          query: '{ listProviderConfigs { provider cliPath isAvailable } }',
        }),
      });
      if (!res.ok) return [];
      const json = (await res.json()) as {
        data?: {
          listProviderConfigs: Array<{
            provider: string;
            cliPath: string;
            isAvailable: boolean;
          }>;
        };
      };
      const configs = json.data?.listProviderConfigs ?? [];
      return configs.map((c) => ({
        provider: c.provider,
        command: c.cliPath,
        found: c.isAvailable,
        path: c.cliPath || null,
      }));
    } catch {
      return [];
    }
  }
}
