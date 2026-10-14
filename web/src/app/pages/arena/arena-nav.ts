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

/**
 * The `queryParams` an arena-ward link must carry so the page it opens can come back here.
 * Existing params on the link are merged in by the caller; this only adds `from`.
 *
 * It CHAINS: a departure URL that itself carries a `from` is kept whole, so console -> workflows ->
 * detail -> arena walks back the way it came rather than collapsing to the console in one jump.
 */
export function arenaReturnParams(currentUrl: string): Record<string, string> {
  if (!currentUrl || !currentUrl.startsWith('/')) return {};
  return { [ARENA_RETURN_PARAM]: currentUrl };
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
