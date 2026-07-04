// Stale-tab recovery. After a new deploy, an already-open tab still references the previous build's
// hashed lazy chunks. Those files no longer exist, so Cloudflare Pages' SPA fallback serves index.html
// (HTML) for them → "Failed to load module script … MIME type text/html" and the app wedges.
//
// Fix: when a module/chunk fails to load, reload the page ONCE (guarded) so the browser fetches the
// fresh index.html (served max-age=0) and its current chunk names. The guard prevents a reload loop if
// the failure is real (not just staleness).

const RELOAD_KEY = 'j1-chunk-reload-ts';
const RELOAD_COOLDOWN_MS = 10_000;

function looksLikeChunkFailure(text: string): boolean {
  return /Failed to load module script|Failed to fetch dynamically imported module|error loading dynamically imported module|Importing a module script failed|Loading chunk [\w-]+ failed|ChunkLoadError/i.test(
    text,
  );
}

function reloadOnce(): void {
  try {
    const now = Date.now();
    const last = Number(sessionStorage.getItem(RELOAD_KEY) || '0');
    if (now - last < RELOAD_COOLDOWN_MS) return; // already tried very recently — avoid a loop
    sessionStorage.setItem(RELOAD_KEY, String(now));
  } catch {
    // sessionStorage unavailable (private mode edge cases) — fall through and reload anyway.
  }
  location.reload();
}

/** Install global listeners that reload the page when a lazy chunk / module script fails to load. */
export function installChunkReload(): void {
  // Resource-load failures (a <script>/<link> that 404'd or returned the wrong MIME) surface as an
  // error event on the element during the capture phase, with no message — detect by target tag + src.
  window.addEventListener(
    'error',
    (event: ErrorEvent) => {
      const el = event.target as (HTMLElement & { src?: string; href?: string }) | null;
      if (el && (el.tagName === 'SCRIPT' || el.tagName === 'LINK')) {
        const url = el.src || el.href || '';
        if (/\.(?:js|mjs|css)(?:\?|$)/i.test(url)) {
          reloadOnce();
          return;
        }
      }
      if (looksLikeChunkFailure(event.message || '')) reloadOnce();
    },
    true,
  );

  // A failed dynamic import() rejects — Angular's lazy routes use these.
  window.addEventListener('unhandledrejection', (event: PromiseRejectionEvent) => {
    const reason = event.reason as { message?: string } | string | undefined;
    const text = typeof reason === 'string' ? reason : reason?.message || '';
    if (looksLikeChunkFailure(text)) reloadOnce();
  });
}
