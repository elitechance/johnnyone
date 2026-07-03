/**
 * Pure, framework-free helpers for the two host "Store & paths" settings —
 * `initiatives_dir` (P1) and `files_root` (P2). Kept Angular/Ionic-free so the
 * plugin-less host-app vitest (`host-app/vitest.config.ts`) can pin them
 * (Overhaul P9 / phase 02 / D2).
 *
 * These are generic Rust setting keys read/written through the existing
 * `getSetting`/`setSetting` pair — NOT typed `hostSettings` fields — so the
 * client owns the empty→default fallback here.
 */

/**
 * Defaults mirroring the Rust `DEFAULT_INITIATIVES_DIR` / `DEFAULT_FILES_ROOT`
 * (`desktop/src-tauri/src/services/settings.rs:21-38`). When `getSetting`
 * returns an empty/unset value, the client shows these.
 */
export const DEFAULT_STORE_PATHS = {
  initiativesDir: '/home/creepy/Documents/Workspace/.johnnyone/initiatives',
  filesRoot: '/home/creepy/Documents/Workspace',
} as const;

/** Return `fallback` when `raw` is null/empty/whitespace-only, else `raw` (untrimmed). */
export function resolveStorePath(raw: string | null | undefined, fallback: string): string {
  return raw != null && raw.trim() !== '' ? raw : fallback;
}

/** Trim both store-path fields (used by the settings page snapshot + service save). */
export function trimStorePaths(s: { initiativesDir: string; filesRoot: string }): {
  initiativesDir: string;
  filesRoot: string;
} {
  return {
    initiativesDir: s.initiativesDir.trim(),
    filesRoot: s.filesRoot.trim(),
  };
}
