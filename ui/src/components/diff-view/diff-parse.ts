// Pure diff helpers for `johnny-diff-view` (overhaul P7, D10). Angular/Ionic/rxjs-free so the spec
// under `web/` can import it directly (the type import below is erased at build). Generalizes the
// planner page's inline `diffLines` classifier (`planner.page.ts`) and adds extension→hljs mapping.

import type { GitDiffFile } from '../../services/johnny-api.service';

export type DiffLineKind = 'add' | 'del' | 'hunk' | 'meta' | 'context';

export interface DiffLine {
  kind: DiffLineKind;
  /** Raw line text, including its leading marker (`+`/`-`/` `), so the view can strip/style it. */
  text: string;
}

/** Classify one unified-diff line. Order matters: hunk + meta headers are checked before the
 * bare `+`/`-` content markers so `+++`/`---` file headers are not mistaken for add/del lines. */
function classifyLine(line: string): DiffLineKind {
  if (line.startsWith('@@')) return 'hunk';
  if (
    line.startsWith('diff --git') ||
    line.startsWith('index ') ||
    line.startsWith('+++ ') ||
    line.startsWith('--- ') ||
    line.startsWith('new file') ||
    line.startsWith('deleted file') ||
    line.startsWith('old mode') ||
    line.startsWith('new mode') ||
    line.startsWith('rename ') ||
    line.startsWith('copy ') ||
    line.startsWith('similarity ') ||
    line.startsWith('Binary files')
  ) {
    return 'meta';
  }
  if (line.startsWith('+')) return 'add';
  if (line.startsWith('-')) return 'del';
  return 'context';
}

/** Parse a unified-diff blob into classified lines (raw text preserved). Empty input → `[]`. */
export function parseUnifiedDiff(diff: string): DiffLine[] {
  if (!diff) return [];
  // Drop a single trailing newline's empty tail so we don't emit a blank context line.
  const body = diff.endsWith('\n') ? diff.slice(0, -1) : diff;
  return body.split('\n').map((text) => ({ kind: classifyLine(text), text }));
}

/** Map a file path's extension → a highlight.js language id. Unknown → `''` (auto/plain). */
export function langForPath(path: string): string {
  const ext = path.split('.').pop()?.toLowerCase() ?? '';
  const map: Record<string, string> = {
    ts: 'typescript',
    tsx: 'typescript',
    js: 'javascript',
    jsx: 'javascript',
    mjs: 'javascript',
    cjs: 'javascript',
    rs: 'rust',
    py: 'python',
    go: 'go',
    json: 'json',
    html: 'xml',
    htm: 'xml',
    xml: 'xml',
    css: 'css',
    scss: 'css',
    md: 'markdown',
    markdown: 'markdown',
    sql: 'sql',
    sh: 'bash',
    bash: 'bash',
    yml: 'yaml',
    yaml: 'yaml',
    toml: 'ini',
  };
  return map[ext] ?? '';
}

/** Sum per-file additions/deletions across a changed-file list (drives the diff header). */
export function fileTotals(files: GitDiffFile[]): {
  count: number;
  additions: number;
  deletions: number;
} {
  return files.reduce(
    (acc, f) => ({
      count: acc.count + 1,
      additions: acc.additions + (f.additions ?? 0),
      deletions: acc.deletions + (f.deletions ?? 0),
    }),
    { count: 0, additions: 0, deletions: 0 },
  );
}
