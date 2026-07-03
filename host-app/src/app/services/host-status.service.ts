import { Injectable, inject, signal } from '@angular/core';
import { HostRelayService } from './host-relay.service';
import { HostSettingsService } from './host-settings.service';

const HOST_URL = 'http://127.0.0.1:7788/graphql';

export interface DetectedTool {
  provider: string;
  command: string;
  found: boolean;
  path?: string | null;
  defaultModel?: string | null;
}

@Injectable({ providedIn: 'root' })
export class HostStatusService {
  private readonly settings = inject(HostSettingsService);
  private readonly relay = inject(HostRelayService);

  readonly hostUp = signal(false);
  readonly workerUp = signal(false);
  readonly relayConnected = signal(false);
  readonly relayHeartbeat = signal<string | null>(null);
  readonly activeSessions = signal<number>(0);
  readonly workerUrl = signal('');
  readonly webClientUrl = signal('https://johnnyone.pages.dev/');
  readonly lastError = signal('');

  async refresh(): Promise<void> {
    this.lastError.set('');
    try {
      const hostSettings = await this.settings.load();
      this.workerUrl.set(hostSettings.workerUrl);
      this.webClientUrl.set(hostSettings.webClientUrl);
      await Promise.all([
        this.pingHost(),
        this.pingWorker(hostSettings.workerUrl),
        this.refreshRelayStatus(),
        this.countSessions(),
      ]);
    } catch (err) {
      this.lastError.set(err instanceof Error ? err.message : String(err));
    }
  }

  private async refreshRelayStatus(): Promise<void> {
    try {
      const status = await this.relay.status();
      this.relayConnected.set(status.connected);
      this.relayHeartbeat.set(status.lastHeartbeat);
    } catch {
      this.relayConnected.set(false);
      this.relayHeartbeat.set(null);
    }
  }

  private async countSessions(): Promise<void> {
    try {
      const res = await fetch(HOST_URL, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ query: '{ listAiSessions { id } }' }),
      });
      if (!res.ok) {
        this.activeSessions.set(0);
        return;
      }
      const json = (await res.json()) as {
        data?: { listAiSessions: Array<{ id: string }> };
      };
      this.activeSessions.set(json.data?.listAiSessions?.length ?? 0);
    } catch {
      this.activeSessions.set(0);
    }
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

  private async pingWorker(workerUrl: string): Promise<void> {
    try {
      const graphqlUrl = this.settings.workerGraphqlUrl(workerUrl);
      const res = await fetch(graphqlUrl, {
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
    try {
      const res = await fetch(HOST_URL, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          query: '{ listProviderConfigs { provider cliPath isAvailable defaultModel } }',
        }),
      });
      if (!res.ok) return [];
      const json = (await res.json()) as {
        data?: {
          listProviderConfigs: Array<{
            provider: string;
            cliPath: string;
            isAvailable: boolean;
            defaultModel?: string | null;
          }>;
        };
      };
      const configs = json.data?.listProviderConfigs ?? [];
      return configs.map((c) => ({
        provider: c.provider,
        command: c.cliPath,
        found: c.isAvailable,
        path: c.cliPath || null,
        defaultModel: c.defaultModel ?? null,
      }));
    } catch {
      return [];
    }
  }
}