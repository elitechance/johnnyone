import { describe, it, expect } from 'vitest';

// The Files DECISION logic is extracted into an Angular/Ionic-free module (overhaul P5, D3/D6) so it
// can be unit-tested directly under the plugin-less web vitest — the real FilesPage pulls in Ionic +
// the P3 renderer. The page delegates its navigation/preview decisions to exactly these functions.
import {
  breadcrumbs,
  joinPath,
  parentPath,
  previewMode,
  codeLanguage,
  formatSize,
  normalizeFileError,
  isDirty,
  validateNewName,
  siblingPath,
  renameTarget,
  fileActionIntent,
} from './files-page-logic';

describe('files page — pure logic', () => {
  describe('breadcrumbs', () => {
    it('splits a root-relative path into cumulative crumbs, root first', () => {
      expect(breadcrumbs('a/b/c', 'Workspace')).toEqual([
        { label: 'Workspace', path: '' },
        { label: 'a', path: 'a' },
        { label: 'b', path: 'a/b' },
        { label: 'c', path: 'a/b/c' },
      ]);
    });

    it('collapses empty / dot / leading-slash inputs to just the root crumb', () => {
      expect(breadcrumbs('', 'Workspace')).toEqual([{ label: 'Workspace', path: '' }]);
      expect(breadcrumbs('.', 'Workspace')).toEqual([{ label: 'Workspace', path: '' }]);
      expect(breadcrumbs('/a', 'Workspace')).toEqual([
        { label: 'Workspace', path: '' },
        { label: 'a', path: 'a' },
      ]);
    });
  });

  describe('joinPath / parentPath', () => {
    it('joins root-relative paths, trimming redundant slashes and never synthesizing `..`', () => {
      expect(joinPath('', 'c')).toBe('c');
      expect(joinPath('a/b', 'c')).toBe('a/b/c');
      expect(joinPath('a/b/', '/c')).toBe('a/b/c');
      expect(joinPath('a/b', '')).toBe('a/b');
    });

    it('drops the last segment to compute the parent', () => {
      expect(parentPath('a/b/c')).toBe('a/b');
      expect(parentPath('a')).toBe('');
      expect(parentPath('')).toBe('');
    });
  });

  describe('previewMode (ordered, mutually exclusive)', () => {
    it('classifies markdown by extension regardless of a text contentType', () => {
      expect(previewMode('x.md', 'text/markdown', 'utf8')).toBe('markdown');
      expect(previewMode('x.md', 'text/plain', 'utf8')).toBe('markdown');
    });

    it('pins the code-vs-text boundary: extension yields code, bare text/plain does not', () => {
      expect(previewMode('x.ts', 'text/plain', 'utf8')).toBe('code');
      expect(previewMode('notes.txt', 'text/plain', 'utf8')).toBe('text');
    });

    it('renders .html/.htm as a page (html mode), ahead of code', () => {
      expect(previewMode('page.html', 'text/html', 'utf8')).toBe('html');
      expect(previewMode('page.htm', 'text/plain', 'utf8')).toBe('html');
      // markdown still wins over html when both could apply (order)
      expect(previewMode('x.md', 'text/html', 'utf8')).toBe('markdown');
    });

    it('renders images inline (image mode), ahead of the binary gate', () => {
      expect(previewMode('shot.png', 'image/png', 'base64')).toBe('image');
      expect(previewMode('pic.jpeg', 'image/jpeg', 'base64')).toBe('image');
      expect(previewMode('logo.svg', 'image/svg+xml', 'utf8')).toBe('image');
      // extension alone is enough even without an image contentType
      expect(previewMode('x.webp', 'application/octet-stream', 'base64')).toBe('image');
    });

    it('classifies non-image base64 / non-text payloads as binary', () => {
      expect(previewMode('x.bin', 'application/octet-stream', 'base64')).toBe('binary');
      // A non-textual contentType alone (even utf8-labelled) is binary.
      expect(previewMode('x.bin', 'application/octet-stream', 'utf8')).toBe('binary');
    });
  });

  describe('codeLanguage', () => {
    it('maps a recognized source extension to a hint and an unknown one to empty', () => {
      expect(codeLanguage('x.ts')).toBe('typescript');
      expect(codeLanguage('x.py')).toBe('python');
      expect(codeLanguage('x.unknownext')).toBe('');
      expect(codeLanguage('README')).toBe('');
    });
  });

  describe('formatSize', () => {
    it('formats bytes human-readably and returns empty for undefined', () => {
      expect(formatSize(1536)).toBe('1.5 KiB');
      expect(formatSize(512)).toBe('512 B');
      expect(formatSize(1024)).toBe('1 KiB');
      expect(formatSize(1024 * 1024 * 2)).toBe('2 MiB');
      expect(formatSize(undefined)).toBe('');
    });
  });

  describe('normalizeFileError (one case per shape)', () => {
    it('returns a plain string as-is', () => {
      expect(normalizeFileError('Path is not a directory')).toBe('Path is not a directory');
    });

    it('returns an Error message', () => {
      expect(normalizeFileError(new Error('File exceeds 5 MiB read cap'))).toBe(
        'File exceeds 5 MiB read cap',
      );
    });

    it('returns the GraphQL errors[].message', () => {
      expect(
        normalizeFileError({
          errors: [{ message: 'Path is outside the configured files_root' }],
        }),
      ).toBe('Path is outside the configured files_root');
    });
  });

  // ── Phase 02 — edit / CRUD helpers ──────────────────────────────────────────────────────────────

  describe('isDirty', () => {
    it('is false when equal, ignores a single trailing newline, true when content differs', () => {
      expect(isDirty('a', 'a')).toBe(false);
      expect(isDirty('a\n', 'a')).toBe(false);
      expect(isDirty('a', 'b')).toBe(true);
    });
  });

  describe('validateNewName', () => {
    it('rejects empty, slash/dotdot, and duplicate names; accepts a fresh valid name', () => {
      expect(validateNewName('', ['x.ts']).ok).toBe(false);
      expect(validateNewName('   ', ['x.ts']).ok).toBe(false);
      expect(validateNewName('a/b', ['x.ts']).ok).toBe(false);
      expect(validateNewName('..', ['x.ts']).ok).toBe(false);
      expect(validateNewName('x.ts', ['x.ts']).ok).toBe(false);
      const dup = validateNewName('x.ts', ['x.ts']);
      expect(dup.error).toBe('A file or folder with that name already exists');
      const ok = validateNewName('new.ts', ['x.ts']);
      expect(ok).toEqual({ ok: true });
    });
  });

  describe('siblingPath / renameTarget', () => {
    it('builds a sibling path and keeps a rename in the same directory', () => {
      expect(siblingPath('a/b', 'c.ts')).toBe('a/b/c.ts');
      expect(renameTarget('a/b/c.ts', 'd.ts')).toBe('a/b/d.ts');
    });
  });

  describe('fileActionIntent (op + args mapping)', () => {
    it('maps newFile → writeFile(siblingPath, "")', () => {
      expect(fileActionIntent('newFile', { cwd: 'a/b', name: 'n.ts' })).toEqual({
        op: 'writeFile',
        args: ['a/b/n.ts', ''],
      });
    });

    it('maps newFolder → mkdir(siblingPath)', () => {
      expect(fileActionIntent('newFolder', { cwd: 'a/b', name: 'sub' })).toEqual({
        op: 'mkdir',
        args: ['a/b/sub'],
      });
    });

    it('maps rename → rename(selectedPath, renameTarget)', () => {
      expect(
        fileActionIntent('rename', { cwd: 'a/b', name: 'd.ts', selectedPath: 'a/b/c.ts' }),
      ).toEqual({ op: 'rename', args: ['a/b/c.ts', 'a/b/d.ts'] });
    });

    it('maps delete → deleteFile(selectedPath)', () => {
      expect(fileActionIntent('delete', { cwd: 'a/b', selectedPath: 'a/b/c.ts' })).toEqual({
        op: 'deleteFile',
        args: ['a/b/c.ts'],
      });
    });
  });
});
