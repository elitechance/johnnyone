# Files manager (overhaul P5)

> A real **two-pane file manager** over the host filesystem, reachable at
> **`/files`**: browse the `files_root`-rooted tree on the left, open a file into a
> preview/editor on the right, run the full CRUD toolbar (New file · New folder ·
> Rename · Delete), and drag-drop **upload** with per-file progress.
>
> Landed on branch `overhaul/2026-07` (commit `overhaul P5: Files manager UI (CRUD +
> upload)`). **This phase is UI-only.** It ships no new backend, relay, worker, or
> Rust surface — it consumes the P2 host file ops and their `JohnnyApiService`
> client methods, and previews through the P3 render core. Everything below is web
> (`web/src/app/pages/files/`) + reuse of `@johnnyone/ui`.

## What it delivers

Before P5, the `files_root`-rooted host file ops shipped in **P2** (transport +
types only — no screen) and the markdown/code render core shipped in **P3**. P5 is
the operator-facing surface that wires them together:

1. **Browse + reachability.** Left pane lists the current directory (`filesListDir`)
   with **breadcrumb** navigation; dirs sort first, then files by name. `/files` is
   an `authGuard`, lazily-loaded route with a **Files** entry (`folder-outline`) in
   the app menu.
2. **Preview via the P3 core.** Selecting a file reads it (`filesRead`) and routes
   it to a preview mode: markdown → `johnny-markdown-view`, source code →
   `highlightCode`, plain text → a `<pre>`, binary → a "download/upload only"
   notice.
3. **Inline editor + Save.** Text/code files open in a plain `<textarea>` with a
   **Save** button (`filesWrite`) and a **`● unsaved`** dirty indicator. Navigating
   away while dirty prompts to discard.
4. **Toolbar CRUD.** New file (`filesWrite` empty), New folder (`filesMkdir`),
   Rename (`filesRename`), Delete (`filesDelete`, behind a confirm dialog). Each op
   refreshes the listing.
5. **Upload with progress.** A drop zone + Upload button chunk each file into ≤1 MiB
   base64 chunks streamed via `filesUploadChunk`, showing a per-file progress chip.

**Root, guard, and caps stay on the host.** Every path is `files_root`-relative and
the client does **not** pre-judge path safety. It relies on the P2 host guard
(`resolve_within_root`) and size caps (5 MiB read/write, 1 MiB/chunk, 50 MiB total
per destination) and **surfaces their errors inline**. See
[`docs/host-transport.md`](host-transport.md) for the transport itself.

## Where the code lives

```
web/src/app/pages/files/
  files.page.ts            FilesPage (selector app-files, standalone) — the two-pane component
  files.page.html / .scss  layout: toolbar · breadcrumb · list | preview/editor · dropzone
  files-page-logic.ts      pure, Angular/Ionic-free navigation + CRUD decision helpers (+ .spec.ts)
  file-chunker.ts          pure upload chunk math + base64 encode (+ .spec.ts)
```

The page delegates every decision to the two **pure** modules so they are
unit-tested under the plugin-less web vitest (`web/vitest.config.ts`) without
pulling in Ionic or the renderer. Reused verbatim from `@johnnyone/ui`:
`JohnnyApiService` (the P2 `listDir`/`readFile`/`writeFile`/`mkdir`/`rename`/
`deleteFile`/`uploadChunk` + `getSetting`) and the P3 `MarkdownViewComponent` +
`highlightCode`. No `@johnnyone/ui` code was modified.

## Preview classification

`previewMode(name, contentType, encoding)` picks the preview surface in a **fixed
order** so the modes are mutually exclusive:

1. `encoding === 'base64'` **or** a non-textual `contentType` → **`binary`** (notice
   only; not editable).
2. `.md` / `.markdown` extension **or** a markdown content type → **`markdown`**
   (rendered via `johnny-markdown-view`).
3. a recognized source extension (`codeLanguage(name) !== ''`) → **`code`**
   (highlighted via `highlightCode`; read-only render, editable as plain text).
4. everything else → **`text`** (plain `<textarea>` / `<pre>`).

`code` is driven by the **extension**, not by `text/*`: `x.ts` → `code`, but
`notes.txt` → `text`. Only `code` and `text` modes are editable — markdown is
preview-only, binary is a notice. Highlighting is **preview-only**; the editor
itself is a plain textarea (no in-place syntax highlighting while typing).

## The pure logic seams

### `files-page-logic.ts` — navigation & CRUD decisions

| Helper | Purpose |
|---|---|
| `breadcrumbs(path, rootLabel)` | cumulative crumbs; index 0 is always the root (`{label: rootLabel, path: ''}`) |
| `joinPath(dir, name)` / `parentPath(path)` | POSIX-style path math (no `..` synthesis — the host guards) |
| `previewMode(name, contentType, encoding)` | classify preview surface (table above) |
| `codeLanguage(name)` | extension → `highlightCode` language hint (`''` if unknown) |
| `formatSize(bytes)` | human-readable `B` / `KiB` / `MiB` |
| `isDirty(original, current)` | unsaved-edits flag, ignoring a single trailing `\n` |
| `validateNewName(name, siblings)` | reject empty, `/`, `..`, or a duplicate sibling name (UX only; the host re-guards) |
| `siblingPath(dir, name)` / `renameTarget(path, newName)` | build the destination path for create/rename |
| `fileActionIntent(action, ctx)` | pure `action → {op, args}` descriptor the toolbar dispatches |
| `normalizeFileError(err)` | pull a human message out of the three error shapes so host guard/cap text surfaces inline |

`fileActionIntent` maps the toolbar to the P2 ops without rendering:

```
newFile   → { op:'writeFile',  args:[siblingPath(cwd,name), ''] }
newFolder → { op:'mkdir',      args:[siblingPath(cwd,name)] }
rename    → { op:'rename',     args:[selectedPath, renameTarget(selectedPath, name)] }
delete    → { op:'deleteFile', args:[selectedPath] }
```

### `file-chunker.ts` — upload math

| Helper | Purpose |
|---|---|
| `planChunks(size, chunkBytes=1 MiB)` | contiguous `[start,end)` ranges covering the file; a 0-byte file yields one empty chunk so it still uploads |
| `uploadCallSequence(totalChunks)` | ordered, payload-free `{chunkIndex, totalChunks, done}[]` — `done` is true **only** on the last chunk |
| `bytesToBase64(bytes)` | byte-wise latin1 walk (in 32 KiB blocks) before `btoa` — the correctness-critical step; passing `btoa` a raw `ArrayBuffer` would corrupt any byte > 127 |
| `uploadProgress(received, total)` | `0..1` fraction (`total <= 0` ⇒ complete) |
| `exceedsTotalCap(size, cap=50 MiB)` | friendly per-**file** pre-check against the host's per-destination total cap |

`CHUNK_BYTES` (1 MiB) and `TOTAL_CAP_BYTES` (50 MiB) mirror the host's
`MAX_UPLOAD_CHUNK_BYTES` / `MAX_UPLOAD_TOTAL_BYTES`. The host remains the authority;
these are UX pre-checks, and the cap is **per file** (each file in a multi-file drop
is an independent 50 MiB upload against its own destination path).

## Security posture

- **Never trust the client.** Path safety and size caps are the host's
  (`resolve_within_root` + the caps in `host_files.rs`); the client only navigates
  and surfaces errors. Client-side name validation is UX, not a security boundary.
- **Never log payloads.** File `content` and upload `dataBase64` are never logged
  (inherits the P2 no-log posture).
- **Delete is confirm-gated** via Ionic `AlertController`.
- **Rendered-markdown preview** inherits the P3 `bypassSecurityTrustHtml` trust
  posture (the operator's own trusted workspace), the same as the Planning preview.

## Scope notes

- **UI-only.** No new transport, re-implemented file op, forked renderer, Rust /
  worker / relay surface, or npm dependency was added. The running desktop app is
  **not** rebuilt — because P5 touches no Rust, the live host already serves the P2
  ops, so the feature is exercisable against it without a rebuild.
- **Visual verification is deferred** (no browser in the runner). Acceptance is
  `nx build web`/`ui` + the pure specs (`files-page-logic.spec.ts`,
  `file-chunker.spec.ts`), with mock §05 as the layout contract.
- **Deferred to Phase 8:** the full Work/Files/Shells/Settings nav rail and its `+`
  launcher, and the "from inside briefing" entry point to the manager. P5 ships only
  the standalone `/files` route + a single nav entry.
