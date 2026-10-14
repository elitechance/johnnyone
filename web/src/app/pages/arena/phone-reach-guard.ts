// A static reachability guard over stylesheet TEXT. See `phone-reach-guard.spec.ts` for why this is
// static rather than rendered: jsdom applies no stylesheets and its `matchMedia` matches nothing, so
// a mounted component cannot prove that a surface is visible at 400px. The measured precedent this
// guards against is real — the Plan tab's document navigator was `display: none` below 720px, which
// made every plan document unreachable on a phone.
//
// Intentionally a small, honest parser rather than a CSS AST dependency: it understands @media
// blocks, width conditions and declarations, and nothing else.

interface Block {
  /** The media condition this rule sits under, or '' at top level. */
  query: string;
  selector: string;
  body: string;
}

/** Does a media condition apply at `width`? Only width features are understood; anything else
 *  (`prefers-color-scheme`, `print`, …) is treated as NOT matching, so the guard never guesses. */
export function mediaQueryMatches(query: string, width: number): boolean {
  const alternatives = query.split(',').map((a) => a.trim()).filter(Boolean);
  if (alternatives.length === 0) return false;
  return alternatives.some((alt) => {
    const features = [...alt.matchAll(/\(([^)]*)\)/g)].map((m) => m[1].trim());
    if (features.length === 0) return false;
    let sawWidth = false;
    for (const f of features) {
      const max = /^max-width\s*:\s*(\d+(?:\.\d+)?)px$/i.exec(f);
      const min = /^min-width\s*:\s*(\d+(?:\.\d+)?)px$/i.exec(f);
      if (max) {
        sawWidth = true;
        if (width > Number(max[1])) return false;
      } else if (min) {
        sawWidth = true;
        if (width < Number(min[1])) return false;
      } else {
        return false; // an unknown feature: do not claim a match
      }
    }
    return sawWidth;
  });
}

/** Flatten a stylesheet into (query, selector, body) triples. */
function blocks(css: string): Block[] {
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, '');
  const out: Block[] = [];
  const walk = (text: string, query: string): void => {
    let i = 0;
    while (i < text.length) {
      const open = text.indexOf('{', i);
      if (open < 0) break;
      const head = text.slice(i, open).trim();
      // find the matching close brace
      let depth = 1;
      let j = open + 1;
      while (j < text.length && depth > 0) {
        if (text[j] === '{') depth++;
        else if (text[j] === '}') depth--;
        j++;
      }
      const body = text.slice(open + 1, j - 1);
      if (head.startsWith('@media')) {
        const inner = head.slice('@media'.length).trim();
        walk(body, query ? `${query} and ${inner}` : inner);
      } else if (head.startsWith('@')) {
        // @supports / @keyframes / … : descend with the same query, selectors inside are not layout
        walk(body, query);
      } else if (head.length > 0) {
        out.push({ query, selector: head, body });
      }
      i = j;
    }
  };
  walk(stripped, '');
  return out;
}

export interface PhoneHidden {
  selector: string;
  query: string;
}

/**
 * Selectors a stylesheet HIDES at `width` because of a width-conditioned media query. A
 * `display: none` outside any media query is a state (`.collapsed`, `.off`), not a width decision,
 * so it is not flagged — the bug class this guards is "the phone is too narrow, so the surface is
 * gone", which is always expressed as a width query.
 */
export function phoneHiddenSelectors(css: string, width = 400): PhoneHidden[] {
  return blocks(css)
    .filter((b) => b.query !== '' && mediaQueryMatches(b.query, width))
    .filter((b) => /(?:^|[;{\s])display\s*:\s*none\b/i.test(b.body) || /visibility\s*:\s*hidden\b/i.test(b.body))
    .map((b) => ({ selector: b.selector, query: b.query }));
}

export interface OverflowRisk {
  selector: string;
  declaration: string;
}

/** Declarations that force a box wider than the viewport, which is the other way a phone loses a
 *  surface: it is on screen but off to the right, with the page scrolling sideways. */
export function overflowRisks(css: string, width = 400): OverflowRisk[] {
  const out: OverflowRisk[] = [];
  for (const b of blocks(css)) {
    if (b.query !== '' && !mediaQueryMatches(b.query, width)) continue;
    for (const m of b.body.matchAll(/(min-width|width)\s*:\s*(\d+(?:\.\d+)?)px/gi)) {
      if (Number(m[2]) > width) out.push({ selector: b.selector, declaration: m[0] });
    }
  }
  return out;
}
