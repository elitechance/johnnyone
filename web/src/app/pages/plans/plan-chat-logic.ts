/**
 * Pure decisions of the plan detail's chat column (Plans tab, initiative 8ad3ae71 phase 03 task 07).
 *
 * - MCP tool calls render like the 08 mock (`brief.update outcome · v2 ✓`) instead of raw JSON.
 * - The plan agent ends a message with `CHOICES: a | b | c` when it offers options (its system
 *   prompt, phase 02 task 07); that line becomes suggestion chips and is hidden from the text.
 * - The composer and the agent pill follow the plan state (retired once planning started).
 * DOM-free and Angular-free.
 */

import { shellRoutePath } from '../shells/shells-route';

/** One entry of `messages.tool_calls` (phase 02 task 08: `{id, name, input, result?}`). */
export interface ToolCall {
  id?: string;
  name: string;
  input?: unknown;
  result?: string;
}

/** What the bubble shows for one tool call. */
export interface ToolRow {
  /** `brief.update`. */
  label: string;
  /** `outcome` / `"query" → 7 hits` / `00/04`. */
  detail: string;
  /** `v2` / `✓` / `!` / `…` (no result yet). */
  mark: string;
  tone: 'ok' | 'warn' | 'pending';
}

const J1_PREFIX = 'mcp__j1__';

/** `mcp__j1__brief_update` → `brief.update`; `repo_search` → `repo.search`; other names verbatim. */
export function toolLabel(name: string | null | undefined): string {
  const n = name ?? '';
  const bare = n.startsWith(J1_PREFIX) ? n.slice(J1_PREFIX.length) : n;
  const known = [
    'brief_get',
    'brief_update',
    'brief_versions',
    'outline_add_phase',
    'outline_add_task',
    'outline_update',
    'outline_move',
    'outline_delete',
    'plan_check',
    'repo_search',
  ];
  if (!known.includes(bare)) return n;
  const i = bare.indexOf('_');
  return `${bare.slice(0, i)}.${bare.slice(i + 1)}`;
}

function obj(v: unknown): Record<string, unknown> {
  return v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {};
}

function str(v: unknown): string {
  return typeof v === 'string' ? v : '';
}

function clip(s: string, n = 60): string {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}

/** Parse a tool result if it is (complete) JSON; null when clipped, plain text or absent. */
function resultJson(result: string | undefined): Record<string, unknown> | null {
  if (!result) return null;
  try {
    const v = JSON.parse(result);
    return v && typeof v === 'object' ? (v as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

/** The one-line summary of a tool call. */
export function toolSummary(call: ToolCall): ToolRow {
  const label = toolLabel(call.name);
  const bare = (call.name ?? '').replace(J1_PREFIX, '');
  const input = obj(call.input);
  const res = resultJson(call.result);

  let detail = '';
  switch (bare) {
    case 'brief_update': {
      const field = str(input['field']);
      if (input['add'] !== undefined) detail = `${field} += "${clip(str(input['add']), 40)}"`;
      else if (input['remove'] !== undefined) detail = `${field} −= "${clip(str(input['remove']), 40)}"`;
      else detail = field;
      break;
    }
    case 'brief_get':
      detail = input['version'] ? `v${input['version']}` : 'latest';
      break;
    case 'outline_add_phase':
      detail = clip(str(input['title']));
      break;
    case 'outline_add_task':
      detail = `${str(input['phaseId'])} · ${clip(str(input['title']))}`;
      break;
    case 'outline_update':
      detail = `${str(input['id'])} · ${clip(str(input['title']))}`;
      break;
    case 'outline_move':
      detail = `${str(input['id'])} → ${input['toIndex'] ?? ''}`;
      break;
    case 'outline_delete':
      detail = str(input['id']);
      break;
    case 'repo_search': {
      const q = `"${clip(str(input['query']), 40)}"`;
      const matches = res && Array.isArray(res['matches']) ? (res['matches'] as unknown[]).length : null;
      detail = matches === null ? q : `${q} → ${matches} ${matches === 1 ? 'hit' : 'hits'}`;
      break;
    }
    case 'plan_check': {
      const items = res && Array.isArray(res['items']) ? (res['items'] as { status?: string }[]) : null;
      if (items) {
        const ok = items.filter((i) => i.status === 'ok').length;
        const warn = items.filter((i) => i.status === 'warn').length;
        const fail = items.filter((i) => i.status === 'fail').length;
        detail = [`${ok} ✓`, warn ? `${warn} ${warn === 1 ? 'warning' : 'warnings'}` : '', fail ? `${fail} failed` : '']
          .filter(Boolean)
          .join(' · ');
      }
      break;
    }
    default:
      detail = '';
  }

  if (call.result === undefined || call.result === null) return { label, detail, mark: '…', tone: 'pending' };
  // Our MCP server answers every success with a JSON object; anything else is an error text (an
  // `isError` result or a JSON-RPC refusal the CLI relayed).
  const looksJson = call.result.trim().startsWith('{');
  if (!looksJson || (res && typeof res['error'] === 'string')) return { label, detail, mark: '!', tone: 'warn' };
  if (bare === 'plan_check' && res && Array.isArray(res['items'])) {
    const bad = (res['items'] as { status?: string }[]).some((i) => i.status !== 'ok');
    return { label, detail, mark: bad ? '!' : '✓', tone: bad ? 'warn' : 'ok' };
  }
  const version = res && typeof res['version'] === 'number' ? (res['version'] as number) : null;
  return { label, detail, mark: version !== null && bare !== 'brief_get' ? `v${version}` : '✓', tone: 'ok' };
}

/** Parse a message's `tool_calls` JSON (tolerant: malformed → []). */
export function parseToolCalls(raw: string | null | undefined): ToolCall[] {
  if (!raw) return [];
  try {
    const v = JSON.parse(raw);
    return Array.isArray(v) ? v.filter((c) => c && typeof c.name === 'string') : [];
  } catch {
    return [];
  }
}

/** Max chips shown for one message. */
export const MAX_CHOICES = 4;

/**
 * Split a message into its text and its choices. Only a LAST non-empty line of the form
 * `CHOICES: a | b | c` counts; items are trimmed, empties dropped, at most four kept.
 */
export function extractChoices(content: string | null | undefined): { text: string; choices: string[] } {
  const src = content ?? '';
  const lines = src.split('\n');
  let last = lines.length - 1;
  while (last >= 0 && !lines[last].trim()) last--;
  if (last < 0) return { text: src, choices: [] };
  const m = /^\s*CHOICES:\s*(.*)$/.exec(lines[last]);
  if (!m) return { text: src, choices: [] };
  const choices = m[1]
    .split('|')
    .map((c) => c.trim())
    .filter(Boolean)
    .slice(0, MAX_CHOICES);
  return { text: lines.slice(0, last).join('\n').trimEnd(), choices };
}

export interface ComposerState {
  disabled: boolean;
  placeholder: string;
}

export const RETIRED_PLACEHOLDER = 'The plan agent is retired — this plan is in planning.';

/** The composer: disabled once the plan left briefing, or while a turn streams. */
export function composerState(initiativeStatus: string | null | undefined, streaming: boolean): ComposerState {
  if ((initiativeStatus ?? '') !== 'briefing') return { disabled: true, placeholder: RETIRED_PLACEHOLDER };
  if (streaming) return { disabled: true, placeholder: 'Waiting for the plan agent…' };
  return { disabled: false, placeholder: 'Discuss the plan…' };
}

/** The chat card pill label (D1, 2026-11-23: the plan agent runs visibly in its own tmux pane). */
export const AGENT_PILL_LABEL = 'Plan agent · claude_code · tmux';

/** The agent's tmux pane, as the chat card shows it: its session name and the Raw view link. */
export interface AgentPane {
  /** `johnnyone_<sid>` — for `tmux attach -r -t <name>` (the copy button). */
  name: string;
  /** The existing Raw terminal surface for the session (`/shells/<sid>/raw`). */
  link: string;
}

/**
 * The agent's pane, or `null` when there is nothing to show: no session, no chat message yet (the
 * pane is created by the FIRST turn), or the agent is retired (its pane is killed on
 * accept/delete/replace). The pane is view-only — chat stays the only way to talk to the agent,
 * and J1 never reads data from the pane.
 */
export function agentPane(
  sessionId: string | null | undefined,
  initiativeStatus: string | null | undefined,
  messageCount: number,
): AgentPane | null {
  const sid = (sessionId ?? '').trim();
  if (!sid || (initiativeStatus ?? '') !== 'briefing' || !(messageCount >= 1)) return null;
  return { name: `johnnyone_${sid}`, link: shellRoutePath(sid, 'raw') };
}

/** State of the pane-name Copy button. */
export type CopyState = 'idle' | 'copied' | 'failed';

/** The Copy button's text. A failed clipboard write says so (and the page selects the name). */
export function copyLabel(state: CopyState): string {
  if (state === 'copied') return 'Copied';
  if (state === 'failed') return 'Copy failed: name selected, press Ctrl/⌘+C';
  return 'Copy';
}

/** The chat card pill suffix: `idle` / `working` / `retired`. */
export function agentState(initiativeStatus: string | null | undefined, streaming: boolean): 'idle' | 'working' | 'retired' {
  if ((initiativeStatus ?? '') !== 'briefing') return 'retired';
  return streaming ? 'working' : 'idle';
}
