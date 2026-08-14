// Pure inline task-detail seam (S5). Angular/Ionic/DOM-free.

import { tookOf } from './tasks-tab-logic';

export interface DetailSpec {
  id?: string;
  files?: string[];
  verify?: string;
  must_contain?: string[];
  mustContain?: string[];
  depends_on?: string[];
  dependsOn?: string[];
}

export interface DetailRow {
  id: string;
  status: string;
  commitSha?: string | null;
  commit_sha?: string | null;
  route?: string | null;
  attempts?: unknown[];
}

export interface CheckCell {
  check: string;
  label: string;
  meaning: string;
  ok: boolean;
  reason: string;
}

export interface AttemptRow {
  n: number;
  tier: string;
  outcome: string;
  took: string;
}

export interface TaskDetailView {
  files: string[];
  verify: string;
  mustContain: string[];
  dependsOn: string[];
  blocks: string[];
  attempts: unknown[];
  attemptRows: AttemptRow[];
  attemptsCopy: string | null;
  hasAttempt: boolean;
  klooCommand: string;
  success: boolean | null;
  failureCode: string | null;
  filesChanged: string[];
  offScopeEdits: string[];
  railFires: unknown;
  postchecks: unknown;
  checks: CheckCell[];
  transcriptTail: string;
  commitSha: string | null;
  ruleAlert: string | null;
}

const FIVE = ['exit', 'scope', 'changed', 'must_contain', 'verify'] as const;

export const CHECK_COPY: Record<(typeof FIVE)[number], { label: string; meaning: string }> = {
  exit: { label: 'exit clean', meaning: 'process exited 0 and reported success' },
  scope: { label: 'changed subset-of allowed', meaning: 'edits stayed inside files[]' },
  changed: { label: 'files actually changed', meaning: 'at least one allowed file was written' },
  must_contain: { label: 'must_contain present', meaning: 'required needles exist in the files' },
  verify: { label: 'postcheck passed', meaning: 'the task verify command passed' },
};

interface LooseAttempt {
  attempt?: number;
  tier?: string;
  failureCode?: string;
  failure_code?: string;
  class?: string;
  startedAt?: string;
  started_at?: string;
  endedAt?: string;
  ended_at?: string;
  checks?: { failed?: { check: string; reason?: string }[] };
  failed?: { check: string; reason?: string }[];
  verify?: { command?: string };
  command?: string;
  success?: boolean;
  files_changed?: unknown;
  filesChanged?: unknown;
  off_scope_edits?: unknown;
  offScopeEdits?: unknown;
  rail_fires?: unknown;
  railFires?: unknown;
  postchecks?: unknown;
  transcript_tail?: string;
  transcriptTail?: string;
}

export function dependentsOf(
  id: string,
  specs: DetailSpec[] | null | undefined,
): string[] {
  return (specs ?? [])
    .filter((s) => (s.dependsOn ?? s.depends_on ?? []).includes(id))
    .map((s) => s.id || '')
    .filter(Boolean);
}

export function taskDetail(
  row: DetailRow | null | undefined,
  spec: DetailSpec | null | undefined,
  lastAttemptJson: unknown,
  checks?: { check: string; reason?: string; passed?: boolean }[] | null,
  allSpecs?: DetailSpec[] | null,
): TaskDetailView {
  const files = spec?.files ?? [];
  const verify = spec?.verify ?? '';
  const mustContain = spec?.mustContain ?? spec?.must_contain ?? [];
  const dependsOn = spec?.dependsOn ?? spec?.depends_on ?? [];
  const attempts = Array.isArray(row?.attempts) ? row!.attempts! : [];
  const noAttempt = attempts.length === 0 && !lastAttemptJson;
  const parsed = parseAttempt(lastAttemptJson);
  const fromRow = parseAttempt(attempts[attempts.length - 1] ?? null);
  const failedList = checks ?? (parsed.failedChecks.length ? parsed.failedChecks : fromRow.failedChecks);
  const checkCells: CheckCell[] = [];
  if (!noAttempt) {
    const failed = new Map<string, string>();
    for (const c of failedList) {
      failed.set(c.check, c.reason ?? '');
    }
    for (const name of FIVE) {
      const reason = failed.get(name);
      const copy = CHECK_COPY[name];
      checkCells.push({
        check: name,
        label: copy.label,
        meaning: copy.meaning,
        ok: reason === undefined,
        reason: reason ?? '',
      });
    }
  }
  const status = (row?.status ?? '').toLowerCase();
  const commitSha = row?.commitSha ?? row?.commit_sha ?? null;
  let ruleAlert: string | null = null;
  if (status === 'failed') {
    ruleAlert = parsed.failureCode || fromRow.failureCode || row?.route || 'failed';
  }
  return {
    files,
    verify,
    mustContain,
    dependsOn,
    blocks: dependentsOf(row?.id ?? '', allSpecs ?? (spec ? [spec] : [])),
    attempts,
    attemptRows: attemptRowsOf(attempts),
    attemptsCopy: noAttempt ? 'No attempt yet' : null,
    hasAttempt: !noAttempt,
    klooCommand: parsed.command || fromRow.command,
    success: parsed.success,
    failureCode: parsed.failureCode || fromRow.failureCode,
    filesChanged: parsed.filesChanged.length ? parsed.filesChanged : fromRow.filesChanged,
    offScopeEdits: parsed.offScopeEdits.length ? parsed.offScopeEdits : fromRow.offScopeEdits,
    railFires: parsed.railFires ?? fromRow.railFires,
    postchecks: parsed.postchecks ?? fromRow.postchecks,
    checks: checkCells,
    transcriptTail: parsed.transcriptTail || fromRow.transcriptTail,
    commitSha,
    ruleAlert: status === 'done' ? null : ruleAlert,
  };
}

export function attemptRowsOf(attempts: unknown[]): AttemptRow[] {
  return attempts.map((raw, i) => {
    const a = asAttempt(raw);
    const n = typeof a.attempt === 'number' ? a.attempt : i + 1;
    const tier = String(a.tier ?? '');
    const code = String(a.failureCode ?? a.failure_code ?? '');
    const outcome = code || (a.class ? String(a.class) : 'ok');
    return { n, tier, outcome, took: tookOf(a.startedAt ?? a.started_at, a.endedAt ?? a.ended_at) };
  });
}

export { tookOf };

function asAttempt(raw: unknown): LooseAttempt {
  if (raw && typeof raw === 'object') return raw as LooseAttempt;
  return {};
}

function parseAttempt(raw: unknown): {
  command: string;
  success: boolean | null;
  failureCode: string | null;
  filesChanged: string[];
  offScopeEdits: string[];
  railFires: unknown;
  postchecks: unknown;
  transcriptTail: string;
  failedChecks: { check: string; reason?: string }[];
} {
  const empty = {
    command: '',
    success: null as boolean | null,
    failureCode: null as string | null,
    filesChanged: [] as string[],
    offScopeEdits: [] as string[],
    railFires: null,
    postchecks: null,
    transcriptTail: '',
    failedChecks: [] as { check: string; reason?: string }[],
  };
  if (raw == null) return empty;
  let obj: LooseAttempt;
  if (typeof raw === 'string') {
    try {
      obj = JSON.parse(raw) as LooseAttempt;
    } catch {
      return empty;
    }
  } else if (typeof raw === 'object') {
    obj = raw as LooseAttempt;
  } else {
    return empty;
  }
  const nested = obj.checks;
  const failedRaw = nested?.failed ?? obj.failed;
  const failed = Array.isArray(failedRaw) ? failedRaw : [];
  const verify = obj.verify;
  const changed = obj.files_changed ?? obj.filesChanged;
  return {
    command: typeof obj.command === 'string' ? obj.command : typeof verify?.command === 'string' ? verify.command : '',
    success: typeof obj.success === 'boolean' ? obj.success : null,
    failureCode:
      typeof obj.failure_code === 'string'
        ? obj.failure_code
        : typeof obj.failureCode === 'string'
          ? obj.failureCode
          : null,
    filesChanged: filesChangedOf(changed),
    offScopeEdits: offScopeOf(obj.off_scope_edits ?? obj.offScopeEdits),
    railFires: obj.rail_fires ?? obj.railFires ?? null,
    postchecks: obj.postchecks ?? null,
    transcriptTail: String(obj.transcript_tail ?? obj.transcriptTail ?? ''),
    failedChecks: failed,
  };
}

function filesChangedOf(v: unknown): string[] {
  if (Array.isArray(v)) return v.map(String);
  if (v && typeof v === 'object' && Array.isArray((v as { paths?: unknown }).paths)) {
    return ((v as { paths: unknown[] }).paths).map(String);
  }
  return [];
}

function offScopeOf(v: unknown): string[] {
  if (Array.isArray(v)) return v.map(String);
  if (typeof v === 'number') return v ? [`${v}`] : [];
  return [];
}

export function clearSelectedId(): null {
  return null;
}
