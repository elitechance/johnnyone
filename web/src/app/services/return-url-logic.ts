/**
 * DOM-free `returnUrl` rules, shared by the producer (authGuard + the expiry
 * logout in AuthService) and the consumer (LoginPage).
 *
 * No Angular, Ionic, browser storage or `Date.now()` — pure string predicates.
 */

/** `scheme:` at the very start — `https:`, `javascript:`, `data:`, … */
const SCHEME_RE = /^[a-zA-Z][a-zA-Z0-9+.-]*:/;

/**
 * Characters that must never appear in a path handed to a navigation API: C0
 * controls and space (NUL–SP), DEL and the C1 range, and the Unicode
 * whitespace/format characters browsers trim or ignore while resolving a URL
 * (NBSP, Ogham space, the general-punctuation space + bidi block, line and
 * paragraph separators, narrow/medium NBSP, ideographic space, BOM). A
 * legitimate in-app path contains none of them; an attacker uses them to hide
 * a scheme or a `//` from a naive check.
 */
const UNSAFE_CHARS_RE =
  /[\x00-\x20\x7f-\x9f\xa0\u1680\u2000-\u200f\u2028\u2029\u202f\u205f\u3000\ufeff]/;

/**
 * Is `raw` an in-app, same-origin path we are willing to navigate to?
 *
 * Accepts only a rooted path (`/settings`, `/settings?x=1#y`). Rejects anything that
 * could leave the origin: a scheme (`https://evil.com`, `javascript:alert(1)`),
 * a protocol-relative path (`//evil.com`), the backslash variant browsers
 * normalise to one (`/\evil.com`), and anything carrying whitespace or control
 * characters used to smuggle the above past a naive check.
 *
 * Defence in depth, not a live-hole patch. The only consumer today is
 * `Router.navigateByUrl`, which cannot leave the origin: it parses the string
 * into a `UrlTree` and `Location` only ever receives
 * `urlSerializer.serialize(tree)`, so `'//evil.com/x'` degrades to
 * `/evil.com/x` and `'https://evil.com/x'` to `/https:`. This predicate exists
 * so the first person to write `window.location.href = returnUrl` — which WOULD
 * be an open redirect — inherits the check instead of introducing the bug.
 */
export function isSafeReturnUrl(raw: string | null | undefined): raw is string {
  if (typeof raw !== 'string') return false;
  const url = raw.trim();
  if (!url) return false;
  if (UNSAFE_CHARS_RE.test(url)) return false;
  if (SCHEME_RE.test(url)) return false;
  if (url[0] !== '/') return false;
  if (url.includes('\\')) return false;
  if (url[1] === '/') return false;
  return true;
}

/**
 * Where to land when there is no usable `returnUrl`.
 *
 * `/initiatives` and not `/chat`: there is no `chat` route: `app.routes.ts`
 * declares `{ path: '', redirectTo: 'initiatives' }` and ends with
 * `{ path: '**', redirectTo: 'initiatives' }`, so `/chat` only ever worked by
 * falling through the wildcard. This matches the empty route's own target.
 */
export const DEFAULT_RETURN_URL = '/initiatives';

/**
 * The URL to navigate to after a successful sign-in: the requested `returnUrl`
 * when it is a safe in-app path, else `fallback`.
 */
export function safeReturnUrl(
  raw: string | null | undefined,
  fallback: string = DEFAULT_RETURN_URL,
): string {
  return isSafeReturnUrl(raw) ? raw.trim() : fallback;
}

/**
 * `/login`, `/login?…`, `/login/…`, `/login#…` — but not `/loginhistory`.
 *
 * Case-insensitive. Angular's own route matching is case-SENSITIVE, so `/LOGIN`
 * actually hits the `**` wildcard and this is unreachable today; comparing
 * case-insensitively costs nothing and keeps the predicate true to its name if
 * that ever changes.
 */
function isLoginUrl(url: string): boolean {
  if (!url.toLowerCase().startsWith('/login')) return false;
  const next = url[6];
  return next === undefined || next === '?' || next === '/' || next === '#';
}

/**
 * The `returnUrl` worth remembering for the page the user is currently on, or
 * `undefined` when there is nothing useful to remember.
 *
 * Skips the empty URL and `/` (the login page's own fallback already lands
 * somewhere sensible) and skips `/login*`, which would otherwise be
 * self-referential — and, on a second expiry, would nest an encoded
 * `returnUrl` inside a `returnUrl`.
 */
export function captureReturnUrl(currentUrl: string | null | undefined): string | undefined {
  if (!isSafeReturnUrl(currentUrl)) return undefined;
  const url = currentUrl.trim();
  if (url === '/') return undefined;
  if (isLoginUrl(url)) return undefined;
  return url;
}

/**
 * `/login`, with `?returnUrl=…` appended when the current URL is worth keeping.
 * Same shape the guard's `createUrlTree(['/login'], { queryParams })` produces,
 * so `LoginPage` needs no second code path.
 */
export function loginUrlWithReturn(currentUrl: string | null | undefined): string {
  const returnUrl = captureReturnUrl(currentUrl);
  return returnUrl ? `/login?returnUrl=${encodeURIComponent(returnUrl)}` : '/login';
}
