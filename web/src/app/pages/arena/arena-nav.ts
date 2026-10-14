/**
 * Where "back" goes from an arena page.
 *
 * THE DEFECT THIS REPLACES, measured in Chrome at 400x860 against the built bundle: the four arena
 * pages had four different hard-coded back links, and not one of them meant "back to where I was".
 *
 *   /workflows                 "‹ console"              -> /initiatives
 *   /workflows/w2              "‹ workflows"            -> /workflows
 *   /workflows/w2/arena        "‹ the four parts"       -> /workflows/w2
 *   /initiatives/p1/arena      "‹ back to the console"  -> /initiatives?initiativeId=p1&tab=arena
 *
 * A person who entered `/workflows` from an initiative's Arena tab was returned to the console with
 * no initiative and no tab — a different place from the one they left. Every arena-ward link now
 * carries the departure URL in `?from=`, and the back link spends it. The static link stays as the
 * FALLBACK, because `from` is absent on a pasted URL, a bookmark and a reload of a link someone
 * sent you, and those have to go somewhere sensible rather than nowhere.
 *
 * Pure (no Angular import) so `arena-nav.spec.ts` can pin it under the plugin-less web vitest —
 * the same constraint that keeps `@ionic/angular/standalone` out of the page bodies.
 */

/** The query parameter that carries the departure URL. */
export const ARENA_RETURN_PARAM = 'from';

export interface ArenaBackTarget {
  /** Link text, including the leading chevron. */
  label: string;
  /** `routerLink` commands array. */
  link: (string | number)[];
  /** `queryParams` for the link; `{}` when the target carries none. */
  queryParams: Record<string, string>;
  /** True when this came from `?from=` rather than the page's static fallback. */
  fromHistory: boolean;
}

/**
 * Split an app-relative URL into what `routerLink` + `queryParams` need.
 *
 * REFUSES anything that is not app-relative: no scheme, no protocol-relative `//host`, no
 * backslash-smuggled host. `from` arrives in a query string, so it is attacker-controlled by
 * construction — a link someone sends can set it — and a back button that renders it unchecked is
 * an open redirect. Refusing falls back to the page's static link, which is the safe outcome.
 */
export function parseReturnUrl(
  from: string | null | undefined,
): { link: string[]; queryParams: Record<string, string> } | null {
  if (!from) return null;
  let raw = from.trim();
  if (!raw.startsWith('/')) return null;
  // `//host`, `/\host` and `/\/host` are all read as protocol-relative by one browser or another.
  if (/^[/\\]{2}/.test(raw.replace(/\\/g, '/'))) return null;
  if (raw.includes('\\')) return null;
  raw = raw.split('#')[0];
  const [pathPart, queryPart] = [raw.split('?')[0], raw.split('?').slice(1).join('?')];
  const segments = pathPart.split('/').filter((s) => s.length > 0);
  if (segments.length === 0) return null;
  const queryParams: Record<string, string> = {};
  if (queryPart) {
    for (const pair of queryPart.split('&')) {
      if (!pair) continue;
      const eq = pair.indexOf('=');
      const key = decodeURIComponent(eq === -1 ? pair : pair.slice(0, eq));
      if (!key) continue;
      queryParams[key] = eq === -1 ? '' : decodeURIComponent(pair.slice(eq + 1));
    }
  }
  return { link: [`/${segments[0]}`, ...segments.slice(1)], queryParams };
}

/**
 * A human name for a returned-to URL, so the back link says WHERE it goes rather than "back".
 * Unknown shapes get the generic word; a wrong name is worse than a plain one.
 */
export function returnLabel(path: string): string {
  const segments = path.split('?')[0].split('/').filter(Boolean);
  if (segments[0] === 'initiatives') {
    if (segments[2] === 'arena') return 'the arena setup';
    return 'the console';
  }
  if (segments[0] === 'workflows') {
    if (segments.length === 1) return 'workflows';
    if (segments[2] === 'arena') return 'the arena';
    return 'the four parts';
  }
  if (segments[0] === 'files') return 'files';
  if (segments[0] === 'shells') return 'shells';
  return 'where you were';
}

/**
 * The back target for an arena page: the departure URL when there is one, else the page's own
 * static parent. The label is uniform across all four pages on purpose — four different words for
 * one gesture was half of what made the switch feel like a different app.
 */
export function arenaBackTarget(
  from: string | null | undefined,
  fallback: { link: (string | number)[]; queryParams?: Record<string, string>; label: string },
): ArenaBackTarget {
  const parsed = parseReturnUrl(from);
  if (parsed) {
    return {
      label: `‹ back to ${returnLabel(parsed.link.join('/'))}`,
      link: parsed.link,
      queryParams: parsed.queryParams,
      fromHistory: true,
    };
  }
  return {
    label: `‹ back to ${fallback.label}`,
    link: fallback.link,
    queryParams: fallback.queryParams ?? {},
    fromHistory: false,
  };
}

/** The same URL with any `from` removed. Everything else — path, order, other params — is kept. */
export function stripReturnParam(url: string): string {
  if (!url) return url;
  const hash = url.includes('#') ? url.slice(url.indexOf('#')) : '';
  const noHash = hash ? url.slice(0, url.indexOf('#')) : url;
  const qAt = noHash.indexOf('?');
  if (qAt === -1) return noHash + hash;
  const kept = noHash
    .slice(qAt + 1)
    .split('&')
    .filter((pair) => pair && decodeURIComponent(pair.split('=')[0]) !== ARENA_RETURN_PARAM);
  return noHash.slice(0, qAt) + (kept.length ? `?${kept.join('&')}` : '') + hash;
}

/** The decoded `from` a URL already carries, or null. */
export function readReturnParam(url: string): string | null {
  const qAt = (url ?? '').indexOf('?');
  if (qAt === -1) return null;
  for (const pair of url.slice(qAt + 1).split('&')) {
    const eq = pair.indexOf('=');
    if (eq === -1) continue;
    if (decodeURIComponent(pair.slice(0, eq)) === ARENA_RETURN_PARAM) {
      return decodeURIComponent(pair.slice(eq + 1));
    }
  }
  return null;
}

/**
 * The `queryParams` an arena-ward link must carry so the page it opens can come back where the
 * person came FROM. Other params on the link are merged in by the caller; this only adds `from`.
 *
 * IT PASSES THE ORIGIN THROUGH, AND IT NEVER NESTS. Two measured failures shaped this:
 *
 *  1. NESTING. The first version used the current URL whole, `from` and all, so every hop
 *     re-encoded the entire previous address into the next one and the escaping gained a level each
 *     time (%2F -> %252F -> %25252F). Measured in Chrome, phone width, entering by the global Arena
 *     item: 51 chars, then 78 after "the four parts", then 119 after "Open the arena" — growing
 *     faster than linearly, into a history entry and anything the person pastes to someone else.
 *
 *  2. A ONE-LEVEL `from` THAT POINTS AT THE PREDECESSOR cannot retrace. Pointing `from` at the page
 *     you are leaving bounds the length, but back then lands on that page with NO `from` of its
 *     own, so its link falls back to its static parent and the next step up falls back to a bare
 *     `/initiatives` — losing the initiative, which is the defect this whole mechanism exists to
 *     fix. Bounded length and a surviving origin are not the same property and a predecessor
 *     pointer only buys the first.
 *
 * So `from` means THE ORIGIN, not the previous step: once a page carries one, every link out of it
 * forwards that same value unchanged, and only a page without one puts its own identity there
 * (minus any `from`, so the value is one level deep by construction). The origin therefore survives
 * a wander of any length at constant cost, and back goes to the console from anywhere in the arena
 * rather than making the person click up through pages they are finished with. The back LABEL names
 * the destination (`returnLabel`), so that jump is never a surprise.
 */
export function arenaReturnParams(currentUrl: string): Record<string, string> {
  if (!currentUrl || !currentUrl.startsWith('/')) return {};
  // An origin already pinned to this page wins — forwarded verbatim, never re-encoded. It is
  // re-validated because it arrived in a query string and a link someone sends can set it.
  const origin = readReturnParam(currentUrl);
  if (origin && parseReturnUrl(origin)) return { [ARENA_RETURN_PARAM]: origin };
  return { [ARENA_RETURN_PARAM]: stripReturnParam(currentUrl) };
}

/**
 * The copy a workflow page shows when the id in the URL is not a workflow.
 *
 * MEASURED on `/workflows/w9` and `/workflows/w9/arena` against the built bundle, with a host that
 * answers `workflow: null` — which is what a stale link, a bookmark or a deleted champion produces:
 *
 *   Could not read this workflow: TypeError: Cannot read properties of null (reading 'repo')
 *
 * That is a raw JS exception rendered as body copy, and the project's own rule is that a missing
 * input is never surfaced as an internal error. The cause is that the page treated "the host
 * answered, with nothing" the same as "the host failed": the `null` flowed on until a property read
 * threw inside the try, and `String(e)` went on the screen.
 *
 * The two cases are now told apart. A null answer is NOT an error — it is an absence, and this is
 * what it says.
 */
export const WORKFLOW_NOT_FOUND =
  'That workflow is not here. The link may be old, or the workflow may have been removed. The list of champions and challengers is one tap away.';

/** True when the host answered successfully with no workflow — an absence, not a failure. */
export function isMissingWorkflow(answer: unknown): boolean {
  return answer === null || answer === undefined;
}
