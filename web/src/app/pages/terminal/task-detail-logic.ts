// Pure inline task-detail seam (S5). Angular/Ionic/DOM-free.

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
  ok: boolean;
  reason: string;
}

export interface TaskDetailView {
  files: string[];
  verify: string;
  mustContain: string[];
  dependsOn: string[];
  blocks: string[];
  attempts: unknown[];
  attemptsCopy: string | null;
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

export function taskDetail(
  row: DetailRow | null | undefined,
  spec: DetailSpec | null | undefined,
  lastAttemptJson: unknown,
  checks?: { check: string; reason?: string; passed?: boolean }[] | null,
): TaskDetailView {
  const files = spec?.files ?? [];
  const verify = spec?.verify ?? '';
  const mustContain = spec?.mustContain ?? spec?.must_contain ?? [];
  const dependsOn = spec?.dependsOn ?? spec?.depends_on ?? [];
  const attempts = Array.isArray(row?.attempts) ? row!.attempts! : [];
  const noAttempt = attempts.length === 0 && !lastAttemptJson;
  const parsed = parseAttempt(lastAttemptJson);
  const checkCells: CheckCell[] = [];
  if (!noAttempt) {
    const failed = new Map<string, string>();
    for (const c of checks ?? parsed.failedChecks) {
      failed.set(c.check, c.reason ?? '');
    }
    for (const name of FIVE) {
      const reason = failed.get(name);
      checkCells.push({
        check: name,
        ok: reason === undefined,
        reason: reason ?? '',
      });
    }
  }
  const status = (row?.status ?? '').toLowerCase();
  const commitSha = row?.commitSha ?? row?.commit_sha ?? null;
  let ruleAlert: string | null = null;
  if (status === 'failed') {
    ruleAlert = parsed.failureCode || row?.route || 'failed';
  }
  return {
    files,
    verify,
    mustContain,
    dependsOn,
    blocks: [],
    attempts,
    attemptsCopy: noAttempt ? 'No attempt yet' : null,
    klooCommand: parsed.command,
    success: parsed.success,
    failureCode: parsed.failureCode,
    filesChanged: parsed.filesChanged,
    offScopeEdits: parsed.offScopeEdits,
    railFires: parsed.railFires,
    postchecks: parsed.postchecks,
    checks: checkCells,
    transcriptTail: parsed.transcriptTail,
    commitSha,
    ruleAlert: status === 'done' ? null : ruleAlert,
  };
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
  let obj: Record<string, unknown>;
  if (typeof raw === 'string') {
    try {
      obj = JSON.parse(raw) as Record<string, unknown>;
    } catch {
      return empty;
    }
  } else if (typeof raw === 'object') {
    obj = raw as Record<string, unknown>;
  } else {
    return empty;
  }
  const failed = Array.isArray(obj.failed)
    ? (obj.failed as { check: string; reason?: string }[])
    : [];
  return {
    command: typeof obj.command === 'string' ? obj.command : '',
    success: typeof obj.success === 'boolean' ? obj.success : null,
    failureCode:
      typeof obj.failure_code === 'string'
        ? obj.failure_code
        : typeof obj.failureCode === 'string'
          ? obj.failureCode
          : null,
    filesChanged: arr(obj.files_changed ?? obj.filesChanged),
    offScopeEdits: arr(obj.off_scope_edits ?? obj.offScopeEdits),
    railFires: obj.rail_fires ?? obj.railFires ?? null,
    postchecks: obj.postchecks ?? null,
    transcriptTail: String(obj.transcript_tail ?? obj.transcriptTail ?? ''),
    failedChecks: failed,
  };
}

function arr(v: unknown): string[] {
  return Array.isArray(v) ? v.map(String) : [];
}

export function clearSelectedId(): null {
  return null;
}
