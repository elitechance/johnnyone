// Pure, framework-free model behind the Plan tab (Overhaul P9 / phase P2, mock §03). Kept
// Angular/Ionic/DOM-free so the spec runs under the plugin-less web vitest, mirroring the shipped
// `console-tabs-logic.ts` / `console-logic.ts` pure-seam pattern (D6). Every transform is a projection
// over the EXISTING `getAgentPlan` result — no new data call, no markdown/mermaid re-implementation
// (rendering is `johnny-markdown-view`). Types are `import type` only (erased at build), so no
// `@johnnyone/ui` / Angular runtime dependency leaks into the plugin-less vitest.
import type { AgentPlanRun, AgentPlanTask } from '../../../../../ui/src/services/johnny-api.service';

/** Normalize a task/phase status the same way the host does (`normalize_task_status`): trim, lower,
 *  `_`→`-`. Kept local so a raw/un-normalized value (deploy skew, direct markdown) still classifies. */
function normStatus(status: string | null | undefined): string {
  return (status ?? '').trim().toLowerCase().replace(/_/g, '-');
}

/** The host collapses `passed`/`complete`/`completed` → `done`; accept those aliases defensively. */
const DONE_STATUSES = new Set(['done', 'completed', 'complete', 'passed']);

function isDone(status: string | null | undefined): boolean {
  return DONE_STATUSES.has(normStatus(status));
}

export interface PlanCounts {
  phases: number;
  tasks: number;
  done: number;
}

/** Header counts "N phases · M tasks · K done" — over `AgentPlanRun.phases`/`.tasks`. Null-safe. */
export function planCounts(run: AgentPlanRun | null | undefined): PlanCounts {
  const phases = run?.phases ?? [];
  const tasks = run?.tasks ?? [];
  return {
    phases: phases.length,
    tasks: tasks.length,
    done: tasks.filter((t) => isDone(t.status)).length,
  };
}

/** One task row in the docnav / phase card (§03 `.taskrow`). */
export interface PlanTaskRow {
  id: string;
  title: string;
  status: string;
}

/** Tasks belonging to a phase, preserving the host-provided order. */
function taskRowsFor(run: AgentPlanRun | null | undefined, phaseId: string): PlanTaskRow[] {
  return (run?.tasks ?? [])
    .filter((t: AgentPlanTask) => t.phaseId === phaseId)
    .map((t) => ({ id: t.taskId, title: t.taskTitle, status: t.status }));
}

/** A doc-navigator entry: either a fixed markdown file or an expandable phase (§03 `.docnav`). */
export type DocNavEntry =
  | { kind: 'file'; id: string; label: string }
  | { kind: 'phase'; id: string; label: string; status: string; tasks: PlanTaskRow[] };

/**
 * Doc navigator model. The `brief.md` + `plan.md` planning artifacts (written by the T1 planner) are
 * ALWAYS listed — they exist from the planning stage on, so the Plan tab is never empty while a plan is
 * being produced. The structured development docs (`overview.md`, `status.md`) and per-phase entries are
 * only added once the plan has been decomposed into phases (development), since those files don't exist
 * during planning (§03 `:728-737`).
 */
export function docNavModel(run: AgentPlanRun | null | undefined): DocNavEntry[] {
  const entries: DocNavEntry[] = [
    { kind: 'file', id: 'brief.md', label: 'brief.md' },
    { kind: 'file', id: 'plan.md', label: 'plan.md' },
  ];
  const phases = run?.phases ?? [];
  if (phases.length) {
    entries.push({ kind: 'file', id: 'overview.md', label: 'overview.md' });
    entries.push({ kind: 'file', id: 'status.md', label: 'status.md' });
    for (const ph of phases) {
      entries.push({
        kind: 'phase',
        id: ph.phaseId,
        label: ph.phaseTitle,
        status: ph.status,
        tasks: taskRowsFor(run, ph.phaseId),
      });
    }
  }
  return entries;
}

/**
 * The doc the Plan tab should open by default for a run. During DEVELOPMENT (the plan has phases) the
 * structured `overview.md` is the entry point; during PLANNING (no phases yet) the flat `plan.md` the
 * planner is producing is what to show. Total.
 */
export function defaultPlanDoc(run: AgentPlanRun | null | undefined): string {
  return (run?.phases?.length ?? 0) > 0 ? 'overview.md' : 'plan.md';
}

/** A §03 phase card: `phaseId`, `phaseTitle`, `done/total`, and its task rows. */
export interface PhaseCard {
  phaseId: string;
  phaseTitle: string;
  done: number;
  total: number;
  tasks: PlanTaskRow[];
}

/** Phase-card model — per phase, in host order (§03 `:757-782`). Null-safe → `[]`. */
export function phaseCards(run: AgentPlanRun | null | undefined): PhaseCard[] {
  return (run?.phases ?? []).map((ph) => {
    const tasks = taskRowsFor(run, ph.phaseId);
    return {
      phaseId: ph.phaseId,
      phaseTitle: ph.phaseTitle,
      total: tasks.length,
      done: tasks.filter((t) => isDone(t.status)).length,
      tasks,
    };
  });
}

/** The four fixed top-level plan docs. Selections equal to one of these resolve to `<plan>/<name>`. */
const FIXED_PLAN_DOCS = ['brief.md', 'plan.md', 'overview.md', 'status.md'] as const;

/**
 * Is `seg` a safe single path segment?
 *
 * Rejects the empty segment (which is what a leading `/`, a trailing `/`, a `//` and a UNC `//host`
 * all decompose into), `.`, `..`, and anything containing a backslash — a Windows separator or
 * drive-qualified name (`C:\x`) must never reach the host, where `\` is a legal filename character
 * and would therefore be silently accepted as part of a name.
 */
function isSafeSegment(seg: string): boolean {
  if (!seg || seg === '.' || seg === '..') return false;
  if (seg.includes('\\') || seg.includes('/')) return false;
  return true;
}

/**
 * Split a plan-relative selection into segments, or `null` if any segment is unsafe.
 * Total: a selection with a leading/trailing/double slash yields an empty segment and is rejected.
 */
function safeSegments(id: string): string[] | null {
  const segs = id.split('/');
  return segs.every(isSafeSegment) ? segs : null;
}

/**
 * Is this selection an ARTIFACT sub-path — the one nested shape the navigator is allowed to address?
 *
 * Exactly two forms, both relative to the plan dir:
 *   - `artifacts` / `artifacts/<...>`                     (the plan's own artifacts tree)
 *   - `phases/<phaseId>/artifacts` / `.../artifacts/<...>` (a phase's artifacts tree)
 *
 * Every segment must pass [`isSafeSegment`] first, so `..`, `.`, empty segments (absolute/UNC/double
 * slash) and backslashes are gone before the shape is even considered. Nothing else nested is
 * allowed: `phases/<id>/notes/x.md` and `secrets/key.pem` are NOT artifact paths.
 *
 * Known, accepted edge: a phase whose `phaseId` is literally `artifacts` would have the selection
 * `artifacts` read as the plan's artifacts dir rather than that phase's overview. Phase ids are
 * host-generated slugs (`00-backend-run-from-phase`, `P2-plan-tab`), so this is not reachable in
 * practice, and the consequence is a wrong-but-still-confined path either way.
 */
function artifactSegments(id: string): string[] | null {
  const segs = safeSegments(id);
  if (!segs) return null;
  if (segs[0] === 'artifacts') return segs;
  if (segs.length >= 3 && segs[0] === 'phases' && segs[2] === 'artifacts') return segs;
  return null;
}

/** The plan-relative artifacts dir for a phase id, or `null` if the id is not a safe segment. */
export function phaseArtifactsDir(phaseId: string | null | undefined): string | null {
  const id = (phaseId ?? '').trim();
  return isSafeSegment(id) ? `phases/${id}/artifacts` : null;
}

/**
 * Build the file path for a navigator selection, joined to the server-provided `planPath`.
 *
 * THE single allowlist for everything the Plan tab can open (requirement: one function). A selection
 * is resolvable only if it is one of [`FIXED_PLAN_DOCS`], a safe bare phase id (→ that phase's
 * `overview.md`), or an artifact sub-path per [`artifactSegments`]. Anything else → `null`.
 *
 * Returns `null` for an empty `planPath`. Defense-in-depth even though ids are server data: a phase
 * id containing `/`, `\` or `..` is rejected, and the assembled path is re-checked to be inside
 * `<planPath>/` with no `..` and no backslash before it is handed back.
 */
export function planDocPath(
  planPath: string | null | undefined,
  sel: string | null | undefined,
): string | null {
  const base = (planPath ?? '').trim().replace(/\/+$/, '');
  if (!base) return null;
  const rel = relPlanDocPath(sel);
  if (!rel) return null;
  const full = `${base}/${rel}`;
  // Belt and braces: whatever the branches above produced must still be a plain descendant of the
  // plan dir. A future branch that forgets to validate fails here rather than reaching the host.
  if (full.includes('..') || full.includes('\\')) return null;
  if (!full.startsWith(`${base}/`)) return null;
  return full;
}

/** The plan-dir-RELATIVE suffix a selection resolves to, or `null` if it is not allowed. */
function relPlanDocPath(sel: string | null | undefined): string | null {
  const id = (sel ?? '').trim();
  if (!id) return null;
  // Fixed top-level docs: planning artifacts (brief.md, plan.md) + development artifacts.
  if ((FIXED_PLAN_DOCS as readonly string[]).includes(id)) return id;
  // The one nested shape: a plan or phase artifacts tree, validated segment by segment.
  const art = artifactSegments(id);
  if (art) return art.join('/');
  // Legacy shape: a bare phase id → that phase's overview. `isSafeSegment` already rejects a `/`,
  // so a multi-segment selection that is not an artifact path cannot reach here.
  if (!isSafeSegment(id)) return null;
  return `phases/${id}/overview.md`;
}

/** Display label + class token for a task-row status chip (§03 taskrow states). */
export interface TaskStatusView {
  label: string;
  token: string;
}

export function taskStatusLabel(status: string | null | undefined): TaskStatusView {
  const n = normStatus(status);
  // Done aliases (completed/complete/passed) render as the single DONE chip, matching `isDone`.
  if (isDone(status)) return { label: 'DONE', token: 'done' };
  if (n === 'planned') return { label: 'PLANNED', token: 'planned' };
  if (n === 'in-progress') return { label: 'IN-PROGRESS', token: 'in-progress' };
  return { label: (status ?? '').trim().toUpperCase(), token: n || 'unknown' };
}

// ── Artifacts section (mass-artifacts) ───────────────────────────────────────────────────────────
//
// A plan dir holds more than the five markdown docs: a worker writes its mocks, screenshots, logs
// and reports into `<plan>/artifacts/` and `<plan>/phases/<id>/artifacts/`. Those were unreachable
// from the Plan tab — the only route was the accidental one, a report's markdown happening to
// mention the path. This section lists them through the EXISTING `filesListDir` host API and opens
// them with the EXISTING renderers (`johnny-markdown-view` for markdown, the sandboxed
// `file-viewer-modal` for html/image/text). No new host code, no new renderer, no second
// confinement root — the artifact dirs already sit inside `files_root`.
//
// SUBDIRECTORIES: artifact trees do nest (`artifacts/mocks/`, `artifacts/ui/`,
// `phases/<id>/artifacts/screenshots/`). We DESCEND, lazily, rather than hand off to `/files`:
//   - the nested entries are the interesting ones (the `.html` mocks and the `.png` captures live
//     one level down), so a handoff would bounce the reader out of the plan for the main payload;
//   - `/files` is a separate page with its own navigation stack — on a phone that means losing the
//     initiative you were reading, which is exactly what `file-viewer-modal` exists to avoid;
//   - descending costs one `filesListDir` per directory the reader actually opens (collapsed by
//     default), so a wide `ui/` dir of 20 PNGs is never fetched unless asked for.
// `/files` remains the right tool for browsing outside a plan, and is unchanged.

/** The shape this module needs from a `filesListDir` entry (structurally satisfied by `FileEntry`). */
export interface ArtifactEntryLike {
  name: string;
  /** `'file' | 'dir'` (any other value is treated as a file). */
  kind: string;
}

/** One row in the navigator's Artifacts section. */
export interface ArtifactNode {
  /** Selection id: the path RELATIVE to the plan dir, resolvable by `planDocPath`. */
  id: string;
  name: string;
  kind: 'file' | 'dir';
  /** Nesting level inside its artifacts dir (0 = a direct child), for indentation. */
  depth: number;
  /** True for a directory the reader has opened. Always false for a file. */
  expanded: boolean;
}

/** The Artifacts section, one group per artifacts dir that actually has entries. */
export interface ArtifactGroup {
  /** `'plan'` for the plan's own dir, else the phase title. */
  label: string;
  /** The dir's path relative to the plan dir — the `filesListDir` cache key. */
  dirRel: string;
  nodes: ArtifactNode[];
}

/** Listings keyed by plan-relative dir path, as cached from `filesListDir`. */
export type ArtifactListings = Record<string, ArtifactEntryLike[] | undefined>;
/** Which plan-relative dir paths the reader has expanded. */
export type ArtifactExpanded = Record<string, boolean | undefined>;

/**
 * Every artifacts dir that COULD exist for a run, plan-relative: the plan's own, then one per phase
 * in host order. A phase id that would not survive the path allowlist is skipped rather than
 * silently producing a path the host would reject.
 */
export function artifactDirsFor(run: AgentPlanRun | null | undefined): string[] {
  const dirs = ['artifacts'];
  for (const ph of run?.phases ?? []) {
    const dir = phaseArtifactsDir(ph.phaseId);
    if (dir) dirs.push(dir);
  }
  return dirs;
}

/** Flatten one artifacts dir into rows, splicing an expanded subdirectory's children in place. */
function artifactNodes(
  dirRel: string,
  listings: ArtifactListings,
  expanded: ArtifactExpanded,
  depth: number,
): ArtifactNode[] {
  const out: ArtifactNode[] = [];
  for (const entry of listings[dirRel] ?? []) {
    const name = entry?.name ?? '';
    // A name that is not a single safe segment can never be addressed by `planDocPath`, so it must
    // not be offered as a row — a dead button is worse than an omission.
    if (!isSafeSegment(name)) continue;
    const id = `${dirRel}/${name}`;
    const kind = entry.kind === 'dir' ? 'dir' : 'file';
    const isOpen = kind === 'dir' && expanded[id] === true;
    out.push({ id, name, kind, depth, expanded: isOpen });
    if (isOpen) out.push(...artifactNodes(id, listings, expanded, depth + 1));
  }
  return out;
}

/**
 * The Artifacts section model — a pure projection over the cached `filesListDir` results.
 *
 * A dir with no entries (or one never listed / whose listing failed) contributes NOTHING: no group,
 * no header, no empty shell. That is what keeps the section invisible for the many initiatives that
 * have no artifacts at all.
 */
export function artifactGroups(
  run: AgentPlanRun | null | undefined,
  listings: ArtifactListings,
  expanded: ArtifactExpanded,
): ArtifactGroup[] {
  const titleByPhase = new Map<string, string>();
  for (const ph of run?.phases ?? []) titleByPhase.set(ph.phaseId, ph.phaseTitle);
  const groups: ArtifactGroup[] = [];
  for (const dirRel of artifactDirsFor(run)) {
    const nodes = artifactNodes(dirRel, listings, expanded, 0);
    if (!nodes.length) continue;
    const phaseId = dirRel.startsWith('phases/') ? dirRel.split('/')[1] : null;
    const label = phaseId ? (titleByPhase.get(phaseId) ?? phaseId) : 'plan';
    groups.push({ label, dirRel, nodes });
  }
  return groups;
}

/**
 * How an artifact file should be opened.
 *
 * `markdown` → the Plan tab's own `johnny-markdown-view` (keeps mermaid + the zoom modal working).
 * `viewer`   → `file-viewer-modal`, which already branches html → sandboxed iframe, image → data
 *              URI, anything else → text. No new renderer either way.
 */
export function artifactOpenMode(id: string): 'markdown' | 'viewer' {
  return /\.(md|markdown)$/i.test(id) ? 'markdown' : 'viewer';
}
