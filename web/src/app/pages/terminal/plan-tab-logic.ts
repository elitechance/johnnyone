// Pure, framework-free model behind the Plan tab (Overhaul P9 / phase P2, mock §03). Kept
// Angular/Ionic/DOM-free so the spec runs under the plugin-less web vitest, mirroring the shipped
// `console-tabs-logic.ts` / `console-logic.ts` pure-seam pattern (D6). Every transform is a projection
// over the EXISTING `getAgentPlan` result — no new data call, no markdown/mermaid re-implementation
// (rendering is `johnny-markdown-view`). Types are `import type` only (erased at build), so no
// `@johnnyone/ui` / Angular runtime dependency leaks into the plugin-less vitest.
import type { AgentPlanRun, AgentPlanTask } from '../../../../../ui/src/services/johnny-api.service';
// The ONLY value import here: a pure, Angular-free helper (`files-page-logic.ts` has no imports at
// all), so the plugin-less web vitest can still load this module.
import { normalizeFileError } from '../files/files-page-logic';

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
 * The document directories a plan (or a phase) may expose, by EXACT name.
 *
 * Deliberately a closed, explicit list rather than a wildcard or "any directory": the whole guard
 * rests on the set of reachable shapes being finite and reviewable, so widening it is a decision
 * someone has to make here, in one place, on purpose.
 *
 * Measured over the real store (71 plans): `artifacts` x29, `mocks` x4, `decisions` x3, `gates` x2,
 * `screens-to-verify` x2, `conventions` x1, `proposed` x1 at plan level; `artifacts` x68 at phase
 * level. `mocks/` and `screens-to-verify/` hold exactly the HTML mocks and PNG captures this
 * feature exists to surface, so leaving them out made the feature miss most of its payload.
 *
 * `tasks/` is EXCLUDED on purpose (172 instances, phase level): it is plan structure, which the
 * navigator already renders as phase + task rows. `.git/` (41 instances) is excluded by the same
 * closed-list rule, without needing a special case.
 */
const DOC_DIRS: readonly string[] = [
  'artifacts',
  'mocks',
  'screens-to-verify',
  'decisions',
  'gates',
  'conventions',
  'proposed',
];

/** Is `name` one of the allowlisted document directories? Exact match — `Mocks`/`mocksx` are not. */
export function isDocDir(name: string): boolean {
  return DOC_DIRS.includes(name);
}

/** The allowlisted document directory names, for the caller that probes which ones exist. */
export function docDirNames(): readonly string[] {
  return DOC_DIRS;
}

/**
 * Is `seg` a safe single path segment?
 *
 * Rejects the empty segment (which is what a leading `/`, a trailing `/`, a `//` and a UNC `//host`
 * all decompose into), `.`, anything CONTAINING `..`, and anything containing a slash or backslash
 * — a Windows separator or drive-qualified name (`C:\x`) must never reach the host, where `\` is a
 * legal filename character and would therefore be silently accepted as part of a name.
 *
 * `..` is rejected as a SUBSTRING, not just as the whole segment, so that this predicate agrees
 * exactly with the re-check at the end of [`planDocPath`]. When it did not agree, a host entry
 * named `v1..2.png` became a navigator row that `planDocPath` then refused — a button that did
 * nothing at all. The cost is that such a file is not offered; an omission beats a dead button.
 */
function isSafeSegment(seg: string): boolean {
  if (!seg || seg === '.' || seg.includes('..')) return false;
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
 * Is this selection a DOCUMENT-DIRECTORY sub-path — the nested shapes the navigator may address?
 *
 * Three forms, all relative to the plan dir, where `<doc>` is one of [`DOC_DIRS`]:
 *   - `<doc>` / `<doc>/<...>`                           (a plan-level document tree)
 *   - `phases/<phaseId>/<doc>` / `.../<doc>/<...>`       (a phase-level document tree)
 *   - `phases/<phaseId>`                                (a phase dir, so the caller can discover
 *                                                        WHICH document dirs it has in one call
 *                                                        instead of probing all seven)
 *
 * Every segment must pass [`isSafeSegment`] first, so `..`, `.`, empty segments (absolute/UNC/double
 * slash) and backslashes are gone before the shape is even considered. Nothing else nested is
 * allowed: `phases/<id>/tasks/x`, `phases/<id>/notes/x.md` and `secrets/key.pem` are all refused.
 *
 * Known, accepted edge: a phase whose `phaseId` is literally one of [`DOC_DIRS`] would have the
 * bare selection read as the plan-level dir of that name rather than that phase's overview. Phase
 * ids are host-generated slugs (`00-backend-run-from-phase`, `P2-plan-tab`) and the real store has
 * ZERO such collisions across every phase of all 71 plans; either way the path stays confined.
 */
function docDirSegments(id: string): string[] | null {
  const segs = safeSegments(id);
  if (!segs) return null;
  if (isDocDir(segs[0])) return segs;
  if (segs[0] !== 'phases') return null;
  if (segs.length === 2) return segs; // the phase dir itself, for the one-call probe
  if (segs.length >= 3 && isDocDir(segs[2])) return segs;
  return null;
}

/** The plan-relative path of a phase's directory, or `null` if the id is not a safe segment. */
export function phaseDir(phaseId: string | null | undefined): string | null {
  const id = (phaseId ?? '').trim();
  return isSafeSegment(id) ? `phases/${id}` : null;
}

/**
 * The plan-relative path of a phase's document dir (default `artifacts`), or `null` when either the
 * phase id is not a safe segment or `docDir` is not allowlisted.
 */
export function phaseArtifactsDir(
  phaseId: string | null | undefined,
  docDir = 'artifacts',
): string | null {
  const dir = phaseDir(phaseId);
  return dir && isDocDir(docDir) ? `${dir}/${docDir}` : null;
}

/**
 * The plan directory itself, normalised — or `null` when there is no plan path.
 *
 * This is the base every selection is joined to, and [`planDocPath`] runs on exactly this: trim,
 * drop trailing slashes, reject empty. Exported because the loader has one listing whose target IS
 * the base (the single call that discovers WHICH document dirs a plan has), and that call used to
 * take a raw path straight from the caller — the one `listDir` in the feature that never passed a
 * validator from this module. Routing it through here keeps "one allowlist, one decision point"
 * literally true instead of true-with-a-footnote, and keeps the two from drifting apart since
 * `planDocPath` now calls it too.
 */
export function planBasePath(planPath: string | null | undefined): string | null {
  const base = (planPath ?? '').trim().replace(/\/+$/, '');
  return base || null;
}

/**
 * Resolve a LISTABLE document dir, where `''` means the plan dir itself.
 *
 * The loader's single entry point for turning a plan-relative dir into a host path, so no path
 * arithmetic survives in the page component at all.
 */
export function planListPath(
  planPath: string | null | undefined,
  dirRel: string,
): string | null {
  return dirRel ? planDocPath(planPath, dirRel) : planBasePath(planPath);
}

/**
 * Build the file path for a navigator selection, joined to the server-provided `planPath`.
 *
 * THE single allowlist for everything the Plan tab can open or list (requirement: one decision
 * point). A selection resolves only if it is one of [`FIXED_PLAN_DOCS`], a safe bare phase id (→
 * that phase's `overview.md`), or a document-dir sub-path per [`docDirSegments`]. Anything else →
 * `null`, and `null` for an empty `planPath`.
 *
 * Defense-in-depth even though ids are server data: EVERY segment of the assembled suffix is
 * re-validated with the same segment guard, and the result is re-checked to start with
 * `<planPath>/` with no `..` and no backslash. A future branch that forgets to validate therefore
 * still fails closed — including on an empty or `.` segment, which the earlier substring-only
 * re-check would have let through as `artifacts//x` / `artifacts/./x`.
 */
export function planDocPath(
  planPath: string | null | undefined,
  sel: string | null | undefined,
): string | null {
  const base = planBasePath(planPath);
  if (!base) return null;
  const rel = relPlanDocPath(sel);
  if (!rel) return null;
  if (!safeSegments(rel)) return null;
  const full = `${base}/${rel}`;
  if (full.includes('..') || full.includes('\\')) return null;
  if (!full.startsWith(`${base}/`)) return null;
  return full;
}

/**
 * The plan-dir-RELATIVE suffix a selection resolves to, or `null` if it is not allowed.
 *
 * Trimming happens ONLY on the fixed-doc branch, where the selection is compared against four
 * literals this code owns. A document-dir selection is assembled from host entry NAMES, and a file
 * really can be called `report.md ` with a trailing space — trimming it there read a path that does
 * not exist, and turned a file named `' '` into a row that could never open.
 */
function relPlanDocPath(sel: string | null | undefined): string | null {
  const raw = sel ?? '';
  // Fixed top-level docs: planning artifacts (brief.md, plan.md) + development artifacts. These
  // four are literals, so trimming the selection is safe and keeps the pre-existing behaviour.
  const trimmed = raw.trim();
  if ((FIXED_PLAN_DOCS as readonly string[]).includes(trimmed)) return trimmed;
  if (!trimmed) return null;
  // The nested shapes: a plan or phase document tree. NOT trimmed — every segment past the first
  // is a real filename.
  const doc = docDirSegments(raw);
  if (doc) return doc.join('/');
  // Legacy shape: a bare phase id → that phase's overview. Trimmed, because the baseline trimmed
  // and a phase id is a host-generated slug rather than a filename.
  if (!isSafeSegment(trimmed)) return null;
  return `phases/${trimmed}/overview.md`;
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

/** The Artifacts section, one group per document dir that actually has entries. */
export interface ArtifactGroup {
  /** What to display: the dir name for a plan-level dir, `'<phase title> / <dir>'` for a phase. */
  label: string;
  /** The dir's path relative to the plan dir — the `filesListDir` cache key. */
  dirRel: string;
  /** Which allowlisted document dir this group is (`artifacts`, `mocks`, …). */
  docDir: string;
  nodes: ArtifactNode[];
}

/** Listings keyed by plan-relative dir path, as cached from `filesListDir`. */
export type ArtifactListings = Record<string, ArtifactEntryLike[] | undefined>;
/** Which plan-relative dir paths the reader has expanded. */
export type ArtifactExpanded = Record<string, boolean | undefined>;

/**
 * Every document dir that COULD exist for a run, plan-relative and in render order: the plan's own
 * seven, then each phase's seven in host order. A phase id that would not survive the path
 * allowlist is skipped rather than silently producing a path the host would reject.
 *
 * This enumerates CANDIDATES for rendering, not a fetch plan — the loader discovers which of these
 * actually exist with one `filesListDir` of the parent dir (see `terminal.page.ts`), and a
 * candidate with no listing contributes nothing.
 */
export function artifactDirsFor(run: AgentPlanRun | null | undefined): string[] {
  const dirs: string[] = [...DOC_DIRS];
  for (const ph of run?.phases ?? []) {
    for (const docDir of DOC_DIRS) {
      const dir = phaseArtifactsDir(ph.phaseId, docDir);
      if (dir) dirs.push(dir);
    }
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
    const segs = dirRel.split('/');
    const isPhase = segs[0] === 'phases';
    const docDir = isPhase ? segs[2] : segs[0];
    const label = isPhase ? `${titleByPhase.get(segs[1]) ?? segs[1]} / ${docDir}` : docDir;
    groups.push({ label, dirRel, docDir, nodes });
  }
  return groups;
}

/**
 * The listing cache, tagged with the initiative it belongs to.
 *
 * D3 was a cross-initiative race: every listing is an await, `'artifacts'` is the same literal
 * cache key for every plan, and a slow fan-out for initiative A landed after the reader had
 * switched to B — so B's navigator rendered A's filenames, and clicking one joined B's `planPath`
 * to A's filename. Tagging the cache with its owner makes that unrepresentable rather than
 * merely guarded: a merge names the owner it believes it is writing for, and a mismatch is
 * discarded. A per-call-site `if` would have had to be remembered at every future call site.
 */
export interface ArtifactCache {
  /** The initiative id these listings were fetched for. */
  ownerId: string;
  listings: ArtifactListings;
}

/** A fresh, empty cache owned by `ownerId`. */
export function emptyArtifactCache(ownerId: string): ArtifactCache {
  return { ownerId, listings: {} };
}

/**
 * Fold successful listings into the cache — but ONLY if `ownerId` still owns it.
 *
 * A stale fan-out is dropped whole, and absent/refused dirs stay absent (so they contribute no
 * group). Returns the SAME object when nothing changes, so a signal holding it does not re-notify.
 */
export function mergeArtifactListings(
  cache: ArtifactCache,
  ownerId: string,
  results: ReadonlyArray<{ dirRel: string; entries?: ArtifactEntryLike[] }>,
): ArtifactCache {
  if (ownerId !== cache.ownerId) return cache;
  const landed = results.filter((r) => r.entries);
  if (!landed.length) return cache;
  const listings: ArtifactListings = { ...cache.listings };
  for (const r of landed) listings[r.dirRel] = r.entries;
  return { ownerId: cache.ownerId, listings };
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

/**
 * Classify a failed `filesListDir` into the one message worth showing the reader, or `null`.
 *
 * A document dir that simply does not exist is the COMMON case (most plans have only `artifacts/`),
 * and reporting it as an error would bury the feature in noise. The one failure that is worth
 * saying out loud is a confinement refusal: `files_root` and `initiatives_dir` are independent user
 * settings, so a plan store configured outside the browse root is reachable config, and the host
 * then refuses every listing. Silence there would read as "this plan has no artifacts", which is a
 * lie — so it gets an explanation instead, and the guard is NOT relaxed to work around it.
 *
 * Pure (hence testable under the plugin-less vitest) and routed through the shared
 * `normalizeFileError`, which knows all three error shapes `graphql-client.ts` can throw — the
 * hand-rolled `String(err.message)` this replaces only understood one of them.
 *
 * The matched literal is the host's own wording from `resolve_within_root`
 * (`desktop/src-tauri/src/services/settings.rs`). It is pinned on BOTH sides: by
 * `plan-tab-logic.spec.ts` here and by `resolve_within_root_rejects_above_root` there, because a
 * reword with nothing pinning it would silently downgrade this explanation back to silence.
 */
export function artifactListError(err: unknown): string | null {
  if (!/outside the configured files_root/i.test(normalizeFileError(err))) return null;
  return (
    'Artifacts cannot be listed: this plan store is outside the configured file-browsing root. ' +
    'Point Settings → files root at a directory that contains the initiatives store.'
  );
}
