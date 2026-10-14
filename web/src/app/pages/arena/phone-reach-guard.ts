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

/**
 * Declarations that force a box wider than the viewport, which is the other way a phone loses a
 * surface: it is on screen but off to the right, with the page scrolling sideways.
 *
 * Both unit families are checked, because both have been injected into this project's own
 * stylesheet and gone unnoticed: absolute `px` wider than the viewport, and viewport-relative
 * widths over 100vw (`120vw` is 20% off-screen at EVERY width, so no `width` argument saves you).
 */
export function overflowRisks(css: string, width = 400): OverflowRisk[] {
  const out: OverflowRisk[] = [];
  for (const b of blocks(css)) {
    if (b.query !== '' && !mediaQueryMatches(b.query, width)) continue;
    for (const m of b.body.matchAll(/(?:^|[;{\s])(min-width|width)\s*:\s*(\d+(?:\.\d+)?)px/gi)) {
      if (Number(m[2]) > width) out.push({ selector: b.selector, declaration: m[0].trim() });
    }
    for (const m of b.body.matchAll(/(?:^|[;{\s])(min-width|width)\s*:\s*(\d+(?:\.\d+)?)(vw|vmin|vmax)/gi)) {
      if (Number(m[2]) > 100) out.push({ selector: b.selector, declaration: m[0].trim() });
    }
  }
  return out;
}

export interface ClipRisk {
  selector: string;
  declaration: string;
}

/**
 * Rules that CLIP their own content: a fixed `height`/`max-height` together with a hidden overflow.
 * This is the third way a phone loses a surface and the subtlest — the element is on screen, at the
 * right width, and simply ends, with no scrollbar and nothing to drag.
 *
 * A fixed height with `auto`/`scroll` overflow is fine and is how `pre.a-out` shows a gate's output,
 * so the pair is what is flagged, never either half alone.
 */
export function clipRisks(css: string, width = 400): ClipRisk[] {
  const out: ClipRisk[] = [];
  for (const b of blocks(css)) {
    if (b.query !== '' && !mediaQueryMatches(b.query, width)) continue;
    // EVERY length unit, not px|rem|em. The rule that caused this whole round is Ionic's own
    // `.ion-page { contain: size …; overflow: hidden }` at a PERCENTAGE height, and the arena's own
    // console-tab scroller is `height: 100%` — one word (`auto` → `hidden`) from being the bug the
    // guard exists to catch. A unit allow-list that excludes `%` and `vh` excludes the real cases.
    const bounded =
      /(?:^|[;{\s])(max-height|height)\s*:\s*(\d+(?:\.\d+)?)(px|rem|em|%|vh|dvh|svh|lvh|vmin|vmax|ch|ex)/i.exec(
        b.body,
      );
    if (!bounded) continue;
    const hidden = /(?:^|[;{\s])overflow(?:-x|-y)?\s*:\s*(?:hidden|clip)\b/i.exec(b.body);
    if (hidden) {
      out.push({ selector: b.selector, declaration: `${bounded[0].trim()} + ${hidden[0].trim()}` });
    }
  }
  return out;
}

export interface OverlayRisk {
  selector: string;
  declaration: string;
}

/**
 * An element pinned over the whole viewport: `position: fixed|absolute` with all four sides at 0
 * (as `inset: 0` or as the four longhands). It covers everything beneath it, so on a phone the page
 * is there, is the right size, scrolls — and cannot be touched.
 *
 * Only a FULL cover is flagged. A pinned chip or a sticky bar is normal and is not.
 */
export function overlayRisks(css: string, width = 400): OverlayRisk[] {
  const out: OverlayRisk[] = [];
  for (const b of blocks(css)) {
    if (b.query !== '' && !mediaQueryMatches(b.query, width)) continue;
    const pinned = /(?:^|[;{\s])position\s*:\s*(fixed|absolute)\b/i.exec(b.body);
    if (!pinned) continue;
    const insetZero = /(?:^|[;{\s])inset\s*:\s*0(?:px)?\s*(?:;|$|\})/i.test(b.body);
    const sides = ['top', 'right', 'bottom', 'left'].every((side) =>
      new RegExp(`(?:^|[;{\\s])${side}\\s*:\\s*0(?:px)?\\s*(?:;|$|\\})`, 'i').test(b.body),
    );
    if (insetZero || sides) {
      out.push({ selector: b.selector, declaration: `${pinned[0].trim()} + inset 0` });
    }
  }
  return out;
}

export interface TransformRisk {
  selector: string;
  declaration: string;
}

/**
 * A transform that moves content away from where the layout put it, by enough to matter: a scale
 * other than ~1, or a percentage translate large enough to push a box off its own parent. The
 * element is laid out correctly and painted somewhere else, so neither a width check nor a height
 * check can see it.
 *
 * OUT OF SCOPE here, deliberately — and this note used to claim the browser harness covered it,
 * which was MEASURED FALSE: `opacity: 0`, `clip-path` and `visibility: hidden` all passed the
 * harness, because its reachability check was pure geometry. The harness now checks paint
 * (`checkVisibility`, which covers opacity, visibility and content-visibility) and a centre-point
 * HIT TEST (which covers clip-path, masks and anything painted on top), and each of the four shapes
 * is re-run against it as a mutation.
 *
 * What is outside BOTH, stated rather than implied: rotations and 3-D transforms that leave a
 * control hittable but unreadable, colour-contrast failures, a control that is reachable only after
 * an animation finishes, and anything that needs a real touch device (hover-only affordances, the
 * software keyboard covering a focused field). Those want a human or a device lab, and nothing in
 * this branch claims them.
 */
export function transformRisks(css: string, width = 400): TransformRisk[] {
  const out: TransformRisk[] = [];
  for (const b of blocks(css)) {
    if (b.query !== '' && !mediaQueryMatches(b.query, width)) continue;
    for (const m of b.body.matchAll(/transform\s*:\s*([^;}]+)/gi)) {
      const value = m[1].trim();
      for (const scale of value.matchAll(/scale[XY3d]*\(\s*([\d.]+)/gi)) {
        const n = Number(scale[1]);
        if (Number.isFinite(n) && (n > 1.05 || n < 0.95)) {
          out.push({ selector: b.selector, declaration: `transform: ${value}` });
        }
      }
      for (const tr of value.matchAll(/translate[XY]?\(\s*(-?[\d.]+)%/gi)) {
        if (Math.abs(Number(tr[1])) >= 100) {
          out.push({ selector: b.selector, declaration: `transform: ${value}` });
        }
      }
    }
  }
  return out;
}
