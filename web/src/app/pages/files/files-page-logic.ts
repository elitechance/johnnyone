// Pure, Angular/Ionic-free Files decision logic (overhaul P5, decisions D2/D3/D6) so it can be
// unit-tested under the plugin-less web vitest — the real `FilesPage` pulls in Ionic + the P3
// renderer, which that config cannot import. The page delegates its navigation/preview decisions to
// exactly these functions; this file ships the Phase-01 read-path helpers (Phase 02 extends it with
// the edit/CRUD helpers).
//
// Paths here are `files_root`-relative (P2: `DirListing.entries[].path` are root-relative, and
// `listDir('')` lists the root). These helpers do NO path-safety enforcement — guarding is the
// host's (`resolve_within_root`, D2); the client only navigates and surfaces the host's errors.

/** A single breadcrumb crumb: a display label and the `files_root`-relative path it navigates to. */
export interface Breadcrumb {
  label: string;
  path: string;
}

/** The preview surface a file routes to (mutually exclusive, evaluated in a fixed order — D3). */
export type PreviewMode = 'markdown' | 'code' | 'binary' | 'text';

/**
 * Split a root-relative `path` into cumulative breadcrumbs. Index 0 is always the root
 * (`{ label: rootLabel, path: '' }`); each following crumb carries the cumulative root-relative path.
 * Empty / `.` / leading-slash inputs collapse to just the root crumb.
 *
 *   breadcrumbs('a/b/c', 'Workspace')
 *     → [{Workspace,''},{a,'a'},{b,'a/b'},{c,'a/b/c'}]
 *   breadcrumbs('', 'Workspace') → [{Workspace,''}]
 */
export function breadcrumbs(path: string, rootLabel: string): Breadcrumb[] {
  const crumbs: Breadcrumb[] = [{ label: rootLabel, path: '' }];
  const segments = (path ?? '')
    .split('/')
    .map((segment) => segment.trim())
    .filter((segment) => segment.length > 0 && segment !== '.');
  let cumulative = '';
  for (const segment of segments) {
    cumulative = cumulative ? `${cumulative}/${segment}` : segment;
    crumbs.push({ label: segment, path: cumulative });
  }
  return crumbs;
}

/**
 * POSIX-style join of a root-relative `dir` and a child `name`, trimming redundant slashes.
 * `joinPath('','c')==='c'`, `joinPath('a/b','c')==='a/b/c'`. No `..` synthesis — the host guards (D2).
 */
export function joinPath(dir: string, name: string): string {
  const left = (dir ?? '').replace(/\/+$/, '');
  const right = (name ?? '').replace(/^\/+/, '');
  if (!left) return right;
  if (!right) return left;
  return `${left}/${right}`;
}

/**
 * Drop the last segment of a root-relative path.
 * `parentPath('a/b/c')==='a/b'`, `parentPath('a')===''`, `parentPath('')===''`.
 */
export function parentPath(path: string): string {
  const trimmed = (path ?? '').replace(/\/+$/, '');
  const idx = trimmed.lastIndexOf('/');
  return idx < 0 ? '' : trimmed.slice(0, idx);
}

/** Lowercased extension (without the dot) of a filename, or `''` if it has none. */
function extensionOf(name: string): string {
  const base = (name ?? '').split('/').pop() ?? '';
  const dot = base.lastIndexOf('.');
  if (dot <= 0 || dot === base.length - 1) return '';
  return base.slice(dot + 1).toLowerCase();
}

// Extension → highlight-language hint for the P3 `highlightCode`. The keys double as the "recognized
// source extension" set that `previewMode` uses to decide `code` vs. `text` — an extension absent
// here yields `''`, which routes an unknown-extension text file to `'text'` (D3).
const CODE_LANGUAGES: Record<string, string> = {
  ts: 'typescript',
  tsx: 'typescript',
  mts: 'typescript',
  cts: 'typescript',
  js: 'javascript',
  jsx: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  json: 'json',
  jsonc: 'json',
  rs: 'rust',
  py: 'python',
  go: 'go',
  sh: 'bash',
  bash: 'bash',
  zsh: 'bash',
  html: 'html',
  htm: 'html',
  css: 'css',
  scss: 'scss',
  sass: 'scss',
  less: 'less',
  yaml: 'yaml',
  yml: 'yaml',
  toml: 'toml',
  md: 'markdown',
  markdown: 'markdown',
  sql: 'sql',
  graphql: 'graphql',
  gql: 'graphql',
  xml: 'xml',
  java: 'java',
  kt: 'kotlin',
  c: 'c',
  h: 'c',
  cpp: 'cpp',
  cc: 'cpp',
  hpp: 'cpp',
  cs: 'csharp',
  rb: 'ruby',
  php: 'php',
  swift: 'swift',
  dockerfile: 'dockerfile',
  ini: 'ini',
};

/**
 * Map a filename to a highlight-language hint for `highlightCode`. Unknown extension ⇒ `''` (and the
 * empty string is exactly what makes `previewMode` classify an unknown-extension file as `'text'`).
 */
export function codeLanguage(name: string): string {
  return CODE_LANGUAGES[extensionOf(name)] ?? '';
}

/** True when a `contentType` names a textual payload (so it should NOT be treated as binary). */
function isTextualContentType(contentType: string): boolean {
  const type = (contentType ?? '').toLowerCase().trim();
  if (!type) return true; // unknown → let extension/encoding decide, don't force binary
  if (type.startsWith('text/')) return true;
  if (type.includes('markdown')) return true;
  if (type.endsWith('+json') || type.endsWith('+xml')) return true;
  return /(json|xml|javascript|ecmascript|typescript|yaml|toml|x-sh|x-shellscript|graphql)/.test(type);
}

/**
 * Classify how a file should be previewed, in a FIXED order so the modes are mutually exclusive (D3):
 *   1. `encoding==='base64'` OR a non-textual `contentType`      ⇒ 'binary'
 *   2. `.md`/`.markdown` extension OR a markdown `contentType`   ⇒ 'markdown'
 *   3. a recognized source extension (`codeLanguage(name)!==''`) ⇒ 'code'
 *   4. everything else                                            ⇒ 'text'
 *
 * `code` is driven by the EXTENSION, not by `text/*` — a bare `text/plain` with no recognized source
 * extension falls through to `'text'` (so `x.ts` ⇒ `code` but `notes.txt` ⇒ `text`).
 */
export function previewMode(name: string, contentType: string, encoding: string): PreviewMode {
  if (encoding === 'base64' || !isTextualContentType(contentType)) return 'binary';

  const ext = extensionOf(name);
  const type = (contentType ?? '').toLowerCase();
  if (ext === 'md' || ext === 'markdown' || type.includes('markdown')) return 'markdown';

  if (codeLanguage(name) !== '') return 'code';

  return 'text';
}

/**
 * Human-readable byte size (B / KiB / MiB); `undefined` ⇒ `''`.
 *   formatSize(1536) === '1.5 KiB'
 */
export function formatSize(bytes?: number): string {
  if (bytes === undefined || bytes === null || Number.isNaN(bytes)) return '';
  if (bytes < 1024) return `${bytes} B`;
  const kib = bytes / 1024;
  if (kib < 1024) return `${oneDecimal(kib)} KiB`;
  return `${oneDecimal(kib / 1024)} MiB`;
}

/** Round to one decimal, dropping a trailing `.0` (e.g. `1.5`→`'1.5'`, `2`→`'2'`). */
function oneDecimal(n: number): string {
  const rounded = Math.round(n * 10) / 10;
  return Number.isInteger(rounded) ? String(rounded) : rounded.toFixed(1);
}

/**
 * Extract a human-readable message from the three error shapes callers receive from
 * `graphql-client.ts` (D2), so the host's guard/cap text ("outside the configured files_root",
 * "exceeds 5 MiB", …) surfaces inline:
 *   (a) a `GraphQLRequestError`-like object with `errors: { message }[]` → the joined messages
 *       (matches `graphql-client.ts` `errors.map(e => e.message).join('; ')`);
 *   (b) an `Error` → its `.message`;
 *   (c) a plain `string` → itself;
 *   fallback → `String(err)`.
 */
export function normalizeFileError(err: unknown): string {
  if (typeof err === 'string') return err;
  if (err && typeof err === 'object') {
    const obj = err as { errors?: unknown; message?: unknown };
    if (Array.isArray(obj.errors)) {
      const messages = obj.errors
        .map((entry) =>
          entry && typeof entry === 'object'
            ? (entry as { message?: unknown }).message
            : undefined,
        )
        .filter((message): message is string => typeof message === 'string' && message.length > 0);
      if (messages.length > 0) return messages.join('; ');
    }
    if (typeof obj.message === 'string' && obj.message.length > 0) return obj.message;
  }
  return String(err);
}

// ── Phase 02 — edit / CRUD decision helpers (still pure; D4/D8) ────────────────────────────────

/** Drop a single trailing `\n` so a benign editor-appended newline is not counted as an edit. */
function stripTrailingNewline(text: string): string {
  return text.endsWith('\n') ? text.slice(0, -1) : text;
}

/**
 * Whether `current` differs from `original`, after normalizing a single trailing newline on both
 * sides (so a textarea that appends one `\n` is not a false-positive dirty). `isDirty('a','a')` and
 * `isDirty('a\n','a')` are both `false`; `isDirty('a','b')` is `true`.
 */
export function isDirty(original: string, current: string): boolean {
  return stripTrailingNewline(original ?? '') !== stripTrailingNewline(current ?? '');
}

/**
 * Validate a proposed new/renamed entry name for UX (the host re-guards the resolved path — D2):
 * reject empty/whitespace, names containing `/` or `..`, and names duplicating an existing sibling.
 */
export function validateNewName(
  name: string,
  siblings: string[],
): { ok: boolean; error?: string } {
  const trimmed = (name ?? '').trim();
  if (!trimmed) return { ok: false, error: 'Name is required' };
  if (trimmed.includes('/') || trimmed.includes('..')) {
    return { ok: false, error: 'Name cannot contain / or ..' };
  }
  if ((siblings ?? []).includes(trimmed)) {
    return { ok: false, error: 'A file or folder with that name already exists' };
  }
  return { ok: true };
}

/** Path for a new entry `name` inside directory `dir` (root-relative). */
export function siblingPath(dir: string, name: string): string {
  return joinPath(dir, name);
}

/** Rename target for `path` → `newName`, staying in the same directory. */
export function renameTarget(path: string, newName: string): string {
  return siblingPath(parentPath(path), newName);
}

export type FileAction = 'newFile' | 'newFolder' | 'rename' | 'delete';

export interface FileActionContext {
  cwd: string;
  name?: string;
  selectedPath?: string;
}

export interface FileActionIntent {
  op: 'writeFile' | 'mkdir' | 'rename' | 'deleteFile';
  args: unknown[];
}

/**
 * Pure action → intended `JohnnyApiService` call descriptor (D6), so the toolbar's op+args mapping is
 * unit-pinned without rendering. The component just dispatches `this.api[intent.op](...intent.args)`:
 *   newFile   → { op:'writeFile',  args:[siblingPath(cwd,name), ''] }
 *   newFolder → { op:'mkdir',      args:[siblingPath(cwd,name)] }
 *   rename    → { op:'rename',     args:[selectedPath, renameTarget(selectedPath, name)] }
 *   delete    → { op:'deleteFile', args:[selectedPath] }
 */
export function fileActionIntent(action: FileAction, ctx: FileActionContext): FileActionIntent {
  const cwd = ctx.cwd ?? '';
  const name = ctx.name ?? '';
  const selectedPath = ctx.selectedPath ?? '';
  switch (action) {
    case 'newFile':
      return { op: 'writeFile', args: [siblingPath(cwd, name), ''] };
    case 'newFolder':
      return { op: 'mkdir', args: [siblingPath(cwd, name)] };
    case 'rename':
      return { op: 'rename', args: [selectedPath, renameTarget(selectedPath, name)] };
    case 'delete':
      return { op: 'deleteFile', args: [selectedPath] };
  }
}
