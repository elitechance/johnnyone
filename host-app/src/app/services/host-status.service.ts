import { Injectable, inject, signal } from '@angular/core';
import { HostRelayService } from './host-relay.service';
import { HostSettingsService } from './host-settings.service';

const HOST_URL = 'http://127.0.0.1:7788/graphql';

interface DetectedTool {
  provider: string;
  command: string;
  found: boolean;
  path?: string | null;
}

@Injectable({ providedIn: 'root' })
export class HostStatusService {
  private readonly settings = inject(HostSettingsService);
  private readonly relay = inject(HostRelayService);

  readonly hostUp = signal(false);
  readonly workerUp = signal(false);
  readonly relayConnected = signal(false);
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
      ]);
    } catch (err) {
      this.lastError.set(err instanceof Error ? err.message : String(err));
    }
  }

  private async refreshRelayStatus(): Promise<void> {
    try {
      const status = await this.relay.status();
      this.relayConnected.set(status.connected);
    } catch {
      this.relayConnected.set(false);
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