import { Injectable } from '@angular/core';

const HOST_URL = 'http://127.0.0.1:7788/graphql';

export interface RelayConnectionStatus {
  connected: boolean;
  sessionId: string | null;
  lastHeartbeat: string | null;
}

@Injectable({ providedIn: 'root' })
export class HostRelayService {
  async connect(): Promise<void> {
    await this.mutate(`mutation { connectRelay }`);
    await this.invokeStartAgent();
  }

  async status(): Promise<RelayConnectionStatus> {
    const data = await this.query<{ relayConnectionStatus: RelayConnectionStatus }>(`{
      relayConnectionStatus {
        connected
        sessionId
        lastHeartbeat
      }
    }`);
    return data.relayConnectionStatus;
  }

  private async invokeStartAgent(): Promise<void> {
    const tauri = (window as Window & { __TAURI__?: { invoke?: (cmd: string) => Promise<unknown> } })
      .__TAURI__;
    if (!tauri?.invoke) {
      return;
    }
    try {
      await tauri.invoke('start_agent');
    } catch {
      // GraphQL connectRelay already started the loop when running under Tauri.
    }
  }

  private async query<T>(query: string): Promise<T> {
    const res = await fetch(HOST_URL, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Accept: 'application/json',
      },
      body: JSON.stringify({ query }),
    });
    if (!res.ok) {
      throw new Error(`Relay status query failed: ${res.status} ${res.statusText}`);
    }
    const json = (await res.json()) as {
      data?: T;
      errors?: Array<{ message: string }>;
    };
    if (json.errors?.length) {
      throw new Error(json.errors[0].message);
    }
    if (!json.data) {
      throw new Error('Relay status query returned no data');
    }
    return json.data;
  }

  private async mutate(query: string): Promise<void> {
    await this.query(query);
  }
}