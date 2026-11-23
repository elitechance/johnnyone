/**
 * Pure decisions of the shared workspace-folder picker (Plans tab phase 03 task 03), moved out of
 * `briefing.page.ts` so `/briefing/new` and the New-plan modal render ONE picker (no fork).
 *
 * DOM-free and Angular-free so the plugin-less web vitest can pin it. The listing shape is the host's
 * `listDir` result: `root` is the absolute browse root, `path` the directory relative to it ('' = the
 * root itself), `entries` its children.
 */
import type { DirListing, FileEntry } from '../../../../../ui/src/services/johnny-api.service';

/** One clickable crumb: its label and the relative path `listDir` takes to go there. */
export interface Breadcrumb {
  label: string;
  relPath: string;
}

/** The sub-folders of a listing (files are hidden — the picker chooses a folder). */
export function dirEntries(listing: DirListing | null | undefined): FileEntry[] {
  return (listing?.entries ?? []).filter((e) => e.kind === 'dir');
}

/** Parent of a relative path; '' for a top-level folder; null at the root (no "up"). */
export function parentPath(relPath: string | null | undefined): string | null {
  const cur = (relPath ?? '').replace(/^\/+|\/+$/g, '');
  if (!cur) return null;
  return cur.includes('/') ? cur.slice(0, cur.lastIndexOf('/')) : '';
}

/** Absolute path of the directory being shown (root + relative), '' when nothing is loaded. */
export function absolutePath(listing: DirListing | null | undefined): string {
  if (!listing) return '';
  const root = (listing.root || '').replace(/\/+$/, '');
  const rel = (listing.path || '').replace(/^\/+|\/+$/g, '');
  return rel ? `${root}/${rel}` : root;
}

/** What the header shows: the absolute path, or an ellipsis while the first listing loads. */
export function displayPath(listing: DirListing | null | undefined): string {
  return absolutePath(listing) || '…';
}

/** Root crumb (the root's last segment, or `/`) followed by one crumb per relative segment. */
export function breadcrumbs(listing: DirListing | null | undefined): Breadcrumb[] {
  if (!listing) return [];
  const root = (listing.root || '').replace(/\/+$/, '');
  const rootLabel = root.split('/').filter(Boolean).pop() || '/';
  const crumbs: Breadcrumb[] = [{ label: rootLabel, relPath: '' }];
  const segs = (listing.path || '').split('/').filter(Boolean);
  segs.forEach((seg, i) => crumbs.push({ label: seg, relPath: segs.slice(0, i + 1).join('/') }));
  return crumbs;
}

/** "Use this folder" is enabled only on a loaded listing, not mid-load and not on an error. */
export function canChoose(
  listing: DirListing | null | undefined,
  loading: boolean,
  error: string | null | undefined,
): boolean {
  return !!listing && !loading && !error && absolutePath(listing) !== '';
}
