import { Injectable } from '@angular/core';

const HOST_URL = 'http://127.0.0.1:7788/graphql';

export interface HostSettings {
  workerUrl: string;
  tenantId: string;
  userId: string;
  plannerMethodologyPath: string;
  plannerConventionsPath: string;
  webClientUrl: string;
}

export const DEFAULT_HOST_SETTINGS: HostSettings = {
  workerUrl: 'https://johnnyone.ethan-353.workers.dev',
  tenantId: '00000000-0000-0000-0000-000000000001',
  userId: '',
  plannerMethodologyPath: 'lokal/agents/common/methodology.md',
  plannerConventionsPath: 'lokal/agents/common/conventions',
  webClientUrl: 'https://johnnyone.pages.dev/',
};

@Injectable({ providedIn: 'root' })
export class HostSettingsService {
  async load(): Promise<HostSettings> {
    const data = await this.query<{ hostSettings: HostSettings }>(`{
      hostSettings {
        workerUrl
        tenantId
        userId
        plannerMethodologyPath
        plannerConventionsPath
        webClientUrl
      }
    }`);
    return data.hostSettings;
  }

  async save(settings: HostSettings): Promise<void> {
    await Promise.all([
      this.setSetting('worker_url', settings.workerUrl.trim()),
      this.setSetting('tenant_id', settings.tenantId.trim()),
      this.setSetting('user_id', settings.userId.trim()),
      this.setSetting('planner_methodology_path', settings.plannerMethodologyPath.trim()),
      this.setSetting('planner_conventions_path', settings.plannerConventionsPath.trim()),
      this.setSetting('web_client_url', settings.webClientUrl.trim()),
    ]);
  }

  async setSetting(key: string, value: string): Promise<void> {
    await this.mutate(
      `mutation SetSetting($key: String!, $value: String!) {
        setSetting(key: $key, value: $value)
      }`,
      { key, value },
    );
  }

  workerGraphqlUrl(workerUrl: string): string {
    return `${workerUrl.replace(/\/+$/, '')}/graphql`;
  }

  private async query<T>(query: string, variables?: Record<string, unknown>): Promise<T> {
    const res = await fetch(HOST_URL, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Accept: 'application/json',
      },
      body: JSON.stringify({ query, variables }),
    });
    if (!res.ok) {
      throw new Error(`Host settings query failed: ${res.status} ${res.statusText}`);
    }
    const json = (await res.json()) as {
      data?: T;
      errors?: Array<{ message: string }>;
    };
    if (json.errors?.length) {
      throw new Error(json.errors[0].message);
    }
    if (!json.data) {
      throw new Error('Host settings query returned no data');
    }
    return json.data;
  }

  private async mutate(
    query: string,
    variables?: Record<string, unknown>,
  ): Promise<void> {
    await this.query(query, variables);
  }
}