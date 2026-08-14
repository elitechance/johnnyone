// Pure seam for /briefing/new Agent=kloo as a mode (phase 05-01 / 05-02).
// Angular/Ionic/DOM-free so vitest can import it without the page.

export const FALLBACK_EXECUTOR = {
  mode: 'local-small' as const,
  provider: 'openrouter',
  model: 'qwen/qwen3-coder',
  ctx: 32768,
  profile: '/home/creepy/etc/kloo.json',
};

export const ESCALATION_COPY = 'qwen3-coder ×2 → claude ×1 → opus ×1';
export const COMMIT_COPY = 'one commit per passing task';

export type AgentKind = 'provider' | 'mode';

export interface AgentChoice {
  value: string;
  label: string;
  kind: AgentKind;
}

export interface DoctorSnapshot {
  provider?: string;
  model?: string;
  ctx?: number;
  profile?: string;
  endpoint?: string;
  api_key?: { set?: boolean } | boolean;
  verify?: { source?: string };
}

export interface ProbeSnapshot {
  tool_call?: boolean;
  file_edit?: boolean;
  json_only?: boolean;
  context?: { configured?: number; advertised?: number; source?: string };
}

export interface CreatePayload {
  workerProvider: string;
  reviewerProvider: string;
  executorConfig?: string;
  title?: string;
  workspacePath: string;
  brief: string;
}

export type PreflightState =
  | 'idle'
  | 'running'
  | { doctor: DoctorSnapshot; probe?: ProbeSnapshot | null }
  | { error: string };

export interface PreflightAlert {
  kind: 'ok' | 'warn' | 'info' | 'danger';
  text: string;
}

export interface PreflightView {
  phase: 'idle' | 'running' | 'ok' | 'error';
  lines: string[];
  alerts: PreflightAlert[];
  buttonDisabled: boolean;
  cardCopy?: string;
}

export function agentChoices(
  detected: { provider: string; found?: boolean }[] | null | undefined,
  known: string[] | null | undefined,
): AgentChoice[] {
  const found = (detected ?? [])
    .filter((t) => t.found !== false && t.provider && t.provider !== 'shell' && t.provider !== 'kloo')
    .map((t) => t.provider);
  const providers = [...new Set([...found, ...(known ?? [])])].filter((p) => p !== 'kloo');
  const choices: AgentChoice[] = providers.map((value) => ({
    value,
    label: value,
    kind: 'provider',
  }));
  choices.push({ value: 'kloo', label: 'kloo', kind: 'mode' });
  return choices;
}

function firstCommercial(detected: { provider: string; found?: boolean }[] | null | undefined): string {
  const hit = (detected ?? []).find(
    (t) => t.found !== false && t.provider && t.provider !== 'shell' && t.provider !== 'kloo',
  );
  return hit?.provider || 'claude_code';
}

export function executorFromDoctor(doctor?: DoctorSnapshot | null): typeof FALLBACK_EXECUTOR {
  if (!doctor) return { ...FALLBACK_EXECUTOR };
  return {
    mode: 'local-small',
    provider: doctor.provider || FALLBACK_EXECUTOR.provider,
    model: doctor.model || FALLBACK_EXECUTOR.model,
    ctx: typeof doctor.ctx === 'number' ? doctor.ctx : FALLBACK_EXECUTOR.ctx,
    profile: doctor.profile || FALLBACK_EXECUTOR.profile,
  };
}

export function createPayload(
  agent: string,
  commercialProvider: string | null | undefined,
  fields: { title?: string; workspacePath: string; brief: string },
  doctor?: DoctorSnapshot | null,
): CreatePayload {
  if (agent === 'kloo') {
    const exec = executorFromDoctor(doctor);
    const provider = (commercialProvider || '').trim() || 'claude_code';
    return {
      workerProvider: provider === 'kloo' ? 'claude_code' : provider,
      reviewerProvider: provider === 'kloo' ? 'claude_code' : provider,
      executorConfig: JSON.stringify(exec),
      ...fields,
    };
  }
  return {
    workerProvider: agent,
    reviewerProvider: agent,
    ...fields,
  };
}

function apiKeySet(doctor: DoctorSnapshot): boolean | undefined {
  const k = doctor.api_key;
  if (typeof k === 'boolean') return k;
  if (k && typeof k === 'object') return k.set;
  return undefined;
}

export function preflightView(state: PreflightState): PreflightView {
  if (state === 'idle') {
    return { phase: 'idle', lines: [], alerts: [], buttonDisabled: false };
  }
  if (state === 'running') {
    return {
      phase: 'running',
      lines: [],
      alerts: [],
      buttonDisabled: true,
      cardCopy: 'Running doctor + probe…',
    };
  }
  if ('error' in state) {
    const raw = (state.error || '').trim() || 'kloo not found';
    const text = /not found|spawn|enoent/i.test(raw) ? 'kloo not found' : raw;
    return {
      phase: 'error',
      lines: [],
      alerts: [{ kind: 'danger', text }],
      buttonDisabled: false,
    };
  }
  const { doctor, probe } = state;
  const lines: string[] = [];
  if (doctor.endpoint) lines.push(`endpoint: ${doctor.endpoint}`);
  const set = apiKeySet(doctor);
  if (set !== undefined) lines.push(`api_key.set: ${set}`);
  if (doctor.model) lines.push(`model: ${doctor.model}`);
  if (typeof doctor.ctx === 'number') lines.push(`ctx: ${doctor.ctx}`);
  if (doctor.provider) lines.push(`provider: ${doctor.provider}`);
  if (doctor.verify?.source) lines.push(`verify.source: ${doctor.verify.source}`);
  if (probe) {
    if (probe.tool_call !== undefined) lines.push(`tool_call: ${probe.tool_call}`);
    if (probe.file_edit !== undefined) lines.push(`file_edit: ${probe.file_edit}`);
    if (probe.json_only !== undefined) lines.push(`json_only: ${probe.json_only}`);
    if (probe.context?.advertised !== undefined) {
      lines.push(`context.advertised: ${probe.context.advertised}`);
    }
    if (probe.context?.source) lines.push(`context.source: ${probe.context.source}`);
  }

  const alerts: PreflightAlert[] = [];
  if (set === false) alerts.push({ kind: 'warn', text: 'API key not set' });
  if (doctor.verify?.source === 'auto-detect') {
    alerts.push({ kind: 'warn', text: 'verify source is auto-detect' });
  }
  if (probe?.context?.source === 'not-advertised') {
    alerts.push({ kind: 'warn', text: 'context not advertised — silent over-compaction risk' });
  }
  alerts.push({ kind: 'info', text: 'kloo cannot see the J1 console or plan store' });
  if (probe) {
    for (const check of ['tool_call', 'file_edit', 'json_only'] as const) {
      if (probe[check] === false) {
        alerts.push({ kind: 'warn', text: `probe failed: ${check}` });
      }
    }
    const failed = ['tool_call', 'file_edit', 'json_only'].some(
      (k) => probe[k as 'tool_call'] === false,
    );
    if (!failed) alerts.unshift({ kind: 'ok', text: 'probe passed' });
  }

  return { phase: 'ok', lines, alerts, buttonDisabled: false };
}

export function parseDoctorJson(raw: string | null | undefined): DoctorSnapshot | null {
  if (!raw) return null;
  try {
    const v = JSON.parse(raw) as DoctorSnapshot;
    return v && typeof v === 'object' ? v : null;
  } catch {
    return null;
  }
}

export function parseProbeJson(raw: string | null | undefined): ProbeSnapshot | null {
  if (!raw) return null;
  try {
    const v = JSON.parse(raw) as ProbeSnapshot;
    return v && typeof v === 'object' ? v : null;
  } catch {
    return null;
  }
}

export { firstCommercial };
