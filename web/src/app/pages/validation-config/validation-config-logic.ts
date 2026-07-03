// Pure decision layer behind the §07 "Validation · configure" UI (overhaul P7, D9/D14). Kept
// Angular/Ionic/rxjs-free so the spec runs under the plugin-less web vitest. Mirrors the shipped
// pure-sibling pattern (files-page-logic.ts / shells-page-logic.ts). Every transform returns a NEW
// array (immutable, signal-friendly); the page holds no duplicate logic.

/** A UI-side lens draft. `model` is `''` when unset (the page omits it on save); `vision` is the
 * design-authority §3 flag (D14). Maps to/from the Rust `ValidationLens` wire shape. */
export interface LensDraft {
  name: string;
  provider: string;
  model: string;
  prompt?: string;
  vision: boolean;
  blocking: boolean;
}

/** The real `CliProvider` registry MINUS `shell` (a reviewer can't be a raw shell) — D9. The mock's
 * `kloo`/`gpt-5` are illustrative only and deliberately excluded. */
export const PROVIDER_OPTIONS = [
  'claude_code',
  'codex',
  'cline',
  'ollama',
  'grok',
] as const;

export function providerOptions(): string[] {
  return [...PROVIDER_OPTIONS];
}

/** The default template = today's product/qa/lead triad, all blocking + non-vision (D6/D14). The
 * model is left `''` so the Rust side resolves the real provider default. */
export function defaultLenses(): LensDraft[] {
  return ['product', 'qa', 'lead'].map((name) => ({
    name,
    provider: 'claude_code',
    model: '',
    vision: false,
    blocking: true,
  }));
}

/** Append a new lens with sensible defaults (a fresh new array). */
export function addLens(list: LensDraft[]): LensDraft[] {
  return [
    ...list,
    {
      name: `lens-${list.length + 1}`,
      provider: 'claude_code',
      model: '',
      vision: false,
      blocking: true,
    },
  ];
}

/** Remove the lens at `index` — EXCEPT this is a no-op (returns the list unchanged) when only one
 * lens remains (min 1, no max) or the index is out of range. */
export function removeLens(list: LensDraft[], index: number): LensDraft[] {
  if (list.length <= 1) return list;
  if (index < 0 || index >= list.length) return list;
  return list.filter((_, i) => i !== index);
}

/** Move the lens at `from` to `to` (clamped). No-op when `from` is out of range or already at `to`. */
export function moveLens(list: LensDraft[], from: number, to: number): LensDraft[] {
  if (from < 0 || from >= list.length) return list;
  const target = Math.max(0, Math.min(to, list.length - 1));
  if (from === target) return list;
  const next = [...list];
  const [item] = next.splice(from, 1);
  next.splice(target, 0, item);
  return next;
}

/** Serialize the drafts to the Rust `Vec<ValidationLens>` wire JSON: omit empty `model`/`prompt`,
 * always emit `vision` (a bool) + `blocking`. Order-independent keys (serde reads by name). */
export function toConfigJson(list: LensDraft[]): string {
  const wire = list.map((lens) => {
    const out: Record<string, unknown> = {
      name: lens.name,
      provider: lens.provider,
      vision: lens.vision,
      blocking: lens.blocking,
    };
    const model = lens.model?.trim();
    if (model) out['model'] = model;
    const prompt = lens.prompt?.trim();
    if (prompt) out['prompt'] = prompt;
    return out;
  });
  return JSON.stringify(wire);
}

/** Parse persisted `validationConfig` → drafts. `null`/`''`/invalid/empty-array → `defaultLenses()`.
 * A wire lens missing `vision` becomes `vision:false` (legacy config safety, D14); missing `model`
 * becomes `''`; missing `blocking` defaults to `true` (matches the all-blocking default posture). */
export function fromConfigJson(json: string | null | undefined): LensDraft[] {
  if (!json) return defaultLenses();
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch {
    return defaultLenses();
  }
  if (!Array.isArray(parsed) || parsed.length === 0) return defaultLenses();
  return parsed.map((raw) => {
    const lens = (raw ?? {}) as Record<string, unknown>;
    return {
      name: typeof lens['name'] === 'string' ? (lens['name'] as string) : '',
      provider:
        typeof lens['provider'] === 'string'
          ? (lens['provider'] as string)
          : 'claude_code',
      model: typeof lens['model'] === 'string' ? (lens['model'] as string) : '',
      prompt: typeof lens['prompt'] === 'string' ? (lens['prompt'] as string) : undefined,
      vision: !!lens['vision'],
      blocking: lens['blocking'] !== false,
    };
  });
}
