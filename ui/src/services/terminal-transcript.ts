const STORAGE_KEY = 'johnnyone_terminal_transcript_cache';
const MAX_SESSIONS = 30;
const MAX_CHARS = 600_000;
const MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;

interface TranscriptEntry {
  text: string;
  savedAt: number;
}

interface TranscriptStore {
  version: 1;
  sessions: Record<string, TranscriptEntry>;
}

export type TerminalTranscriptMode = 'viewport' | 'history';

export interface TerminalTranscriptSegment {
  kind: 'text' | 'mermaid';
  content: string;
  id: string;
  label: string;
  encodedSource?: string;
}

const MERMAID_OPEN_PATTERN = /(`{3,}|~{3,})\s*mermaid\b/i;
const MERMAID_CLOSE_PATTERN = /(`{3,}|~{3,})\s*$/;
const GROK_MERMAID_ACTIONS = /\[(?:Open Image|Copy Image Path|Copy Source)\]/i;
const MERMAID_SOURCE_LINE =
  /(?:^|\s)(flowchart|graph|sequenceDiagram|classDiagram|stateDiagram|erDiagram|gantt|pie)\b|subgraph\b|participant\b|classDef\b|-->|---|<-->|do\s*<-->|^\s*end\s*$/i;

function fenceChar(token: string): '`' | '~' {
  return token.startsWith('~') ? '~' : '`';
}

function isMatchingCloseFence(line: string, opener: string): boolean {
  const match = line.match(MERMAID_CLOSE_PATTERN);
  if (!match) return false;
  return fenceChar(match[1]) === fenceChar(opener);
}

function inlineMermaidSource(line: string, openToken: string): string {
  const index = line.search(MERMAID_OPEN_PATTERN);
  if (index < 0) return '';
  const afterOpen = line.slice(index + openToken.length).trim();
  return afterOpen;
}

function isTuiRedrawNoise(line: string): boolean {
  const trimmed = line.trim();
  if (!trimmed) return false;
  return /^(.)\1{12,}$/.test(trimmed);
}

function isGrokMermaidMarker(line: string): boolean {
  const trimmed = line.trim();
  return trimmed.toLowerCase() === 'mermaid' || (/\bmermaid\b/i.test(trimmed) && GROK_MERMAID_ACTIONS.test(line));
}

function isGrokMermaidActionLine(line: string): boolean {
  const trimmed = line.trim();
  if (!trimmed) return false;
  return GROK_MERMAID_ACTIONS.test(line);
}

function isMermaidSourceLine(line: string): boolean {
  const trimmed = line.trim();
  if (!trimmed) return false;
  if (isGrokMermaidMarker(line) || GROK_MERMAID_ACTIONS.test(line)) return false;
  if (MERMAID_OPEN_PATTERN.test(line)) return false;
  if (isTuiRedrawNoise(line)) return false;
  if (/^Turn completed\b/i.test(trimmed)) return false;
  return MERMAID_SOURCE_LINE.test(line);
}

function normalizeMermaidSource(source: string): string {
  let normalized = source
    .split('\n')
    .map((line) => line.replace(/-->\s*\[([^\]]+)\]\s*\|/g, '-->|"$1"|').trimEnd())
    .join('\n')
    .trim();

  if (!normalized) return '';

  if (!/^\s*(flowchart|graph|sequenceDiagram|classDiagram|stateDiagram|erDiagram|gantt|pie)\b/im.test(normalized)) {
    if (/subgraph|-->|participant|do\s*<-->/i.test(normalized)) {
      normalized = `flowchart TB\n${normalized}`;
    }
  }

  return normalized;
}

function peelTrailingMermaidLines(textBuffer: string[]): string[] {
  const peeled: string[] = [];
  let index = textBuffer.length - 1;

  while (index >= 0) {
    const line = textBuffer[index];
    if (!line.trim()) {
      index -= 1;
      continue;
    }
    if (!isMermaidSourceLine(line)) break;
    peeled.unshift(line);
    index -= 1;
  }

  textBuffer.length = index + 1;
  while (textBuffer.length > 0 && !textBuffer[textBuffer.length - 1].trim()) {
    textBuffer.pop();
  }

  return peeled;
}

function preprocessTranscriptLines(transcript: string): string[] {
  return transcript
    .split('\n')
    .filter((line) => !isTuiRedrawNoise(line))
    .filter((line) => !isGrokMermaidActionLine(line));
}

function pushMermaidSegment(segments: TerminalTranscriptSegment[], source: string): void {
  const normalized = normalizeMermaidSource(source);
  if (!normalized) return;
  const index = segments.filter((segment) => segment.kind === 'mermaid').length + 1;
  segments.push({
    kind: 'mermaid',
    content: normalized,
    encodedSource: encodeURIComponent(normalized),
    id: `history-mermaid-${index}`,
    label: '',
  });
}

/**
 * Split transcript text into plain-text and mermaid segments in document order.
 * Supports markdown fences and Grok CLI mermaid widgets (label + action links).
 */
export function parseTerminalTranscriptSegments(transcript: string): TerminalTranscriptSegment[] {
  if (!transcript) return [];

  const lines = preprocessTranscriptLines(transcript);
  const segments: TerminalTranscriptSegment[] = [];
  const textBuffer: string[] = [];
  const mermaidBuffer: string[] = [];
  let inFenceMermaid = false;
  let openFence = '```';

  const flushText = (): void => {
    if (textBuffer.length === 0) return;
    segments.push({ kind: 'text', content: textBuffer.join('\n'), id: '', label: '' });
    textBuffer.length = 0;
  };

  const flushFenceMermaid = (): void => {
    pushMermaidSegment(segments, mermaidBuffer.join('\n'));
    mermaidBuffer.length = 0;
  };

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];

    if (inFenceMermaid) {
      if (isMatchingCloseFence(line, openFence)) {
        flushFenceMermaid();
        inFenceMermaid = false;
        continue;
      }
      mermaidBuffer.push(line);
      continue;
    }

    if (isGrokMermaidMarker(line)) {
      const peeled = peelTrailingMermaidLines(textBuffer);
      while (index + 1 < lines.length && isGrokMermaidActionLine(lines[index + 1])) {
        index += 1;
      }
      if (peeled.length > 0) {
        flushText();
        pushMermaidSegment(segments, peeled.join('\n'));
        continue;
      }
      continue;
    }

    const openMatch = line.match(MERMAID_OPEN_PATTERN);
    if (openMatch) {
      flushText();
      inFenceMermaid = true;
      openFence = openMatch[1];
      const inline = inlineMermaidSource(line, openMatch[0]);
      if (inline) mermaidBuffer.push(inline);
      continue;
    }

    textBuffer.push(line);
  }

  if (inFenceMermaid) {
    textBuffer.push(...mermaidBuffer);
    mermaidBuffer.length = 0;
  }
  flushText();

  if (segments.length === 0) {
    return [{ kind: 'text', content: transcript, id: '', label: '' }];
  }

  const mermaidTotal = segments.filter((segment) => segment.kind === 'mermaid').length;
  return segments.map((segment) => {
    if (segment.kind !== 'mermaid') return segment;
    const mermaidIndex = Number(segment.id.replace('history-mermaid-', ''));
    return {
      ...segment,
      label: mermaidTotal === 1 ? 'Mermaid' : `Mermaid ${mermaidIndex}`,
    };
  });
}

export function extractMermaidSources(transcript: string, maxBlocks = 8): string[] {
  return parseTerminalTranscriptSegments(normalizeTerminalPlainText(transcript))
    .filter((segment): segment is TerminalTranscriptSegment & { kind: 'mermaid' } => segment.kind === 'mermaid')
    .map((segment) => segment.content)
    .slice(0, maxBlocks);
}

export function stripTerminalAnsi(content: string): string {
  return content
    .replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '')
    .replace(/\x1b\][^\x07]*(?:\x07|\x1b\\)/g, '')
    .replace(/\x1b[@-_][0-?]*[ -/]*[@-~]/g, '')
    .replace(/\r\n/g, '\n')
    .replace(/\r/g, '\n');
}

export function normalizeTerminalPlainText(content: string): string {
  const lines = stripTerminalAnsi(content).split('\n');
  while (lines.length > 0 && lines[lines.length - 1].trim() === '') {
    lines.pop();
  }
  return lines.join('\n');
}

function linesEqual(a: string[], b: string[]): boolean {
  if (a.length !== b.length) return false;
  return a.every((line, index) => line === b[index]);
}

/**
 * Viewport snapshots from TUIs often redraw in place. Only append when line
 * sequences overlap cleanly; never concatenate unrelated frames.
 */
export function mergeViewportTranscript(existing: string, snapshot: string): string {
  const next = normalizeTerminalPlainText(snapshot);
  if (!next) return existing;
  if (!existing) return next;
  if (next === existing) return existing;
  if (next.startsWith(existing)) return truncateTranscript(next);
  if (existing.startsWith(next)) return existing;

  const existingLines = existing.split('\n');
  const viewportLines = next.split('\n');
  const maxLineOverlap = Math.min(existingLines.length, viewportLines.length, 80);

  for (let overlap = maxLineOverlap; overlap >= 1; overlap--) {
    const tail = existingLines.slice(-overlap);
    const head = viewportLines.slice(0, overlap);
    if (linesEqual(tail, head)) {
      const merged = [...existingLines, ...viewportLines.slice(overlap)].join('\n');
      return truncateTranscript(merged);
    }
  }

  // TUI full-pane redraw without line overlap — append only when the viewport
  // carries genuinely new output, not periodic chrome-only frames.
  if (!existing.includes(next)) {
    if (viewportLines.length <= 16 && next.length < existing.length * 0.45) {
      const tail = existingLines.slice(-viewportLines.length);
      if (linesEqual(tail, viewportLines)) return existing;
    }
    const merged = [...existingLines, ...viewportLines].join('\n');
    if (normalizeTerminalPlainText(merged) === existing) return existing;
    return truncateTranscript(merged);
  }

  return existing;
}

/** Full tmux history captures replace the transcript (canonical scrollback). */
export function applyHistoryTranscript(_existing: string, snapshot: string): string {
  const next = normalizeTerminalPlainText(snapshot);
  if (!next) return _existing;
  return truncateTranscript(next);
}

export function applyTerminalSnapshotToTranscript(
  existing: string,
  snapshot: string,
  mode: TerminalTranscriptMode,
): string {
  return mode === 'history'
    ? applyHistoryTranscript(existing, snapshot)
    : mergeViewportTranscript(existing, snapshot);
}

export function readTerminalTranscript(sessionId: string): string {
  const entry = readStore().sessions[sessionId];
  if (!entry) return '';
  if (Date.now() - entry.savedAt > MAX_AGE_MS) {
    removeTerminalTranscript(sessionId);
    return '';
  }
  return entry.text;
}

export function rememberTerminalTranscript(
  sessionId: string,
  snapshot: string,
  mode: TerminalTranscriptMode = 'viewport',
): string {
  const store = readStore();
  const existing = store.sessions[sessionId]?.text ?? '';
  const merged = applyTerminalSnapshotToTranscript(existing, snapshot, mode);
  if (!merged || merged === existing) return existing;

  store.sessions[sessionId] = {
    text: merged,
    savedAt: Date.now(),
  };
  pruneStore(store);
  writeStore(store);
  return merged;
}

export function replaceTerminalTranscript(sessionId: string, snapshot: string): string {
  const text = truncateTranscript(normalizeTerminalPlainText(snapshot));
  const store = readStore();
  store.sessions[sessionId] = {
    text,
    savedAt: Date.now(),
  };
  pruneStore(store);
  writeStore(store);
  return text;
}

export function removeTerminalTranscript(sessionId: string): void {
  const store = readStore();
  if (!store.sessions[sessionId]) return;
  delete store.sessions[sessionId];
  writeStore(store);
}

function truncateTranscript(text: string): string {
  if (text.length <= MAX_CHARS) return text;
  return text.slice(text.length - MAX_CHARS);
}

function pruneStore(store: TranscriptStore): void {
  const now = Date.now();
  for (const [sessionId, entry] of Object.entries(store.sessions)) {
    if (now - entry.savedAt > MAX_AGE_MS) {
      delete store.sessions[sessionId];
    }
  }

  const entries = Object.entries(store.sessions);
  if (entries.length <= MAX_SESSIONS) return;

  entries
    .sort((a, b) => a[1].savedAt - b[1].savedAt)
    .slice(0, entries.length - MAX_SESSIONS)
    .forEach(([sessionId]) => {
      delete store.sessions[sessionId];
    });
}

function readStore(): TranscriptStore {
  if (typeof window === 'undefined') {
    return { version: 1, sessions: {} };
  }

  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return { version: 1, sessions: {} };
    const parsed = JSON.parse(raw) as Partial<TranscriptStore>;
    if (!parsed || parsed.version !== 1 || !parsed.sessions) {
      return { version: 1, sessions: {} };
    }
    return { version: 1, sessions: parsed.sessions };
  } catch {
    return { version: 1, sessions: {} };
  }
}

function writeStore(store: TranscriptStore): void {
  if (typeof window === 'undefined') return;

  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(store));
  } catch {
    // Ignore quota errors.
  }
}