/**
 * DOM-free seam for the Prompt Library list. No Angular, Ionic, window, or network —
 * the page is the container.
 */
import type { PromptLibraryEntry } from '@johnnyone/ui';
import type { LoadState } from '../settings/settings-prompts-logic';

export type LibraryRow = PromptLibraryEntry;
export type LibraryLoadView = 'loading' | 'error' | 'empty' | 'ready';
export type RowAction = { kind: 'duplicate' | 'open'; key: string };

export interface LibraryFilters {
  role?: string;
  scope?: string;
  query?: string;
}

export function filterRows(
  rows: LibraryRow[],
  { role, scope, query }: LibraryFilters,
): LibraryRow[] {
  const roleF = (role ?? '').trim();
  const scopeF = (scope ?? '').trim();
  const q = (query ?? '').trim().toLowerCase();
  return rows.filter((row) => {
    if (roleF && row.role !== roleF) return false;
    if (scopeF && row.scope !== scopeF) return false;
    if (q) {
      const name = row.name.toLowerCase();
      const key = row.key.toLowerCase();
      if (!name.includes(q) && !key.includes(q)) return false;
    }
    return true;
  });
}

export function rowAction(row: Pick<LibraryRow, 'key' | 'customised' | 'readOnly'>): RowAction {
  if (row.customised && !row.readOnly) {
    return { kind: 'open', key: row.key };
  }
  return { kind: 'duplicate', key: row.key };
}

export function editorHref(key: string): string {
  return `/settings?prompt=${encodeURIComponent(key)}`;
}

export function editorQueryParams(key: string): { prompt: string } {
  return { prompt: key };
}

export function rowActionLabel(row: Pick<LibraryRow, 'key' | 'customised' | 'readOnly'>): string {
  return rowAction(row).kind === 'duplicate' ? 'Duplicate to edit' : 'Open editor';
}

export function newHref(): string {
  return '/settings';
}

export function formatUsed(n: number): string {
  if (n === 1) return '1 run';
  return `${n} runs`;
}

export function scopeLabel(scope: string): string {
  return scope === 'builtin' ? 'built-in' : scope;
}

export function loadView(loadState: LoadState, filteredLength: number): LibraryLoadView {
  if (loadState === 'idle' || loadState === 'loading') return 'loading';
  if (loadState === 'load-error') return 'error';
  if (filteredLength === 0) return 'empty';
  return 'ready';
}
