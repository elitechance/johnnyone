# Host ↔ web transport — files · shells · stream events

> Overhaul phase 2 (2026-07). This documents the **transport primitives** that
> later UI phases consume. Everything here is plumbing, types, and programmatic
> round-trips — **no screens ship in this phase**. The file-manager UI, the
> raw-shell launcher, and the transcript renderer are later phases; this layer is
> what they call.

Every primitive rides the **existing relay** (web → worker WSS relay →
desktop binary), never a new parallel transport and never a direct connection to
`127.0.0.1:7788` (localhost-only, unreachable from a remote browser). There are
two relay flavors, and each primitive picks the one that fits:

| Flavor | Shape | Used for |
|---|---|---|
| **relay-RPC** (request/response, 15 s timeout) | `desktopRpc(ctx,'method',params)` in a worker resolver → `ChatRelayDO` → host `handle_rpc` match | file CRUD, one-shot terminal capture |
| **envelope push** (streaming) | persistent WSS `/api/relay/ws`; DO tag-broadcasts `desktop`↔`mobile`/`api` | live terminal I/O, stream events |

File ops are request/response → relay-RPC. Stream events are fire-and-forget
push → envelope. Shell input already rides the envelope (`terminal_command`); a
relay-RPC read-output (`capture_terminal`) was added for deterministic reads.

## 1. Host file manager

Full CRUD over a single, user-configurable browse root, reachable from the web
client. The host engine is `desktop/src-tauri/src/services/host_files.rs`; it is
deliberately **separate** from the plan-scoped readers in `agent_plans.rs`
(`readHostFile`/`browseHostDirectory`) — those are rooted at a plan's workspace,
this one is rooted at `files_root`.

### The `files_root` setting

- Setting key: **`files_root`**. Default: **`/home/creepy/Documents/Workspace`**.
- Absolute, user-configurable through the existing `getSetting` / `setSetting`
  surface — there is no dedicated command. Empty/unset falls back to the default.
- Resolved by `settings.rs::resolve_files_root`, mirroring
  `resolve_initiatives_dir`. Distinct from the plan store (`initiatives_dir`).

### Operations

Seven ops, each rooted at `files_root` and guarded per-path:

| Op | Behavior |
|---|---|
| `list_dir(path)` | Lists a directory. Dirs first, then case-insensitive by name. Capped at 5000 entries. |
| `read_file(path)` | Reads a file ≤ **5 MiB**. utf8 text returned inline; binary returned base64 (`encoding` field says which). |
| `write_file(path, content, encoding)` | `encoding` is `utf8` (default) or `base64`. Creates parent dirs. Content ≤ 5 MiB. Lands **atomically** (temp sibling + rename). |
| `mkdir(path)` | Creates a directory and any missing parents. |
| `rename(from, to)` | Moves within the root. **Never overwrites** an existing target. |
| `delete(path)` | Removes a file or directory (recursive). **Refuses to delete `files_root` itself.** |
| `upload_chunk(path, chunkIndex, totalChunks, dataBase64, done)` | Appends one base64 chunk to a deterministic `.part` sidecar; on `done` (or the last index) the sidecar is atomically renamed into place. |

### Security model (non-negotiable)

- **Path guard** — every path argument passes `settings.rs::resolve_within_root`
  before any filesystem call. It rejects any `..` segment and any path (relative
  or absolute) that resolves outside `files_root`. It canonicalizes the deepest
  existing ancestor first, so **symlink escapes are defeated** and the guard runs
  *before* any directory is created for a not-yet-existing target.
- **Size caps** — reads/writes ≤ 5 MiB; upload chunk ≤ 1 MiB; total upload
  ≤ 50 MiB; directory listing ≤ 5000 entries.
- **No secrets in logs** — nothing here logs file content, decoded upload bytes,
  or base64 payloads.
- **Auth on every call** — the worker resolvers call
  `authorizeForAltToken(ctx, 'files:read' | 'files:write')`. Two new API scopes,
  `files:read` and `files:write`, were added to `worker/lib/auth/scopes.ts`.

These invariants are each pinned by a dedicated `cargo test` (traversal, above-root,
symlink escape, size caps, chunked-upload round-trip, camelCase wire contract) —
run against a temp dir, never the live filesystem.

### Wire path

```
JohnnyApiService.listDir(...)      ui/web
  → filesListDir GraphQL query     worker/schema/johnnyone-ai.graphql
  → files-list-dir.ts resolver     desktopRpc(ctx,'files_list_dir')  + files:read
  → ChatRelayDO relay-RPC          rpc_request
  → agent/mod.rs handle_rpc        "files_list_dir" arm
  → host_files::list_dir           files_root-rooted, guarded
```

GraphQL surface (`worker/schema/johnnyone-ai.graphql`):

- **Queries:** `filesListDir(path): DirListing!`, `filesRead(path): FileContent!`
- **Mutations:** `filesWrite(path, content, encoding): FileOpResult!`,
  `filesMkdir(path): FileOpResult!`, `filesRename(from, to): FileOpResult!`,
  `filesDelete(path): FileOpResult!`,
  `filesUploadChunk(path, chunkIndex, totalChunks, dataBase64, done): UploadChunkResult!`
- **Types:** `FileEntry`, `DirListing`, `FileContent`, `FileOpResult`,
  `UploadChunkResult` — field names mirror the host serde (camelCase), so the
  resolvers in `worker/resolvers/ai/files-*.ts` are pure pass-throughs.

Relay-RPC method names: `files_list_dir`, `files_read`, `files_write`,
`files_mkdir`, `files_rename`, `files_delete`, `files_upload_chunk`.

ui client methods (`ui/src/services/johnny-api.service.ts`): `listDir`,
`readFile`, `writeFile`, `mkdir`, `rename`, `deleteFile`, `uploadChunk`. The
`FileEntry` / `DirListing` / `FileContent` / `FileOpResult` / `UploadChunkResult`
interfaces are re-exported from the `@johnnyone/ui` barrel.

> The two-pane file-manager UI (tree + inline editor + drop zone) that consumes
> these is **Phase 5** — no screen ships here.

## 2. Shell I/O over the relay

A plain-shell session (`CliProvider::Shell`) already crosses the relay: its
input/output uses the **same** `terminal_command` / `terminal_screen` envelope
path as agent CLIs, with no shell-specific branch (confirmed by `cargo test` —
`Shell` resolves like the agent providers and launches its command with no agent
args). This phase adds a **programmatic** surface so tests and later phases can
drive a shell without the UI:

- **send-input** — rides the existing `terminal_command` envelope (unchanged).
- **read-output** — a new deterministic one-shot capture:
  - Query `captureTerminal(sessionId, historyLines): TerminalScreen!`
  - Resolver `worker/resolvers/ai/capture-terminal.ts` → `desktopRpc('capture_terminal')`,
    gated by the `terminal:read` scope.
  - Host arm `capture_terminal` in `agent/mod.rs` snapshots the current pane.
  - ui method `JohnnyApiService.captureTerminal(sessionId, historyLines?)`.

The live stream still uses `relay-terminal.service.ts`; `captureTerminal` is the
request/response read for automation and tests.

> The raw-shell launcher UI (rail `+` menu) is **Phase 6** — not built here.

## 3. Stream-event channel

A structured, per-session event push so the host can stream provider/agent events
to the web client — mirroring the terminal-screen push, on the same WSS envelope
lane. This phase ships the **type + channel + subscribe control only**; the
transcript renderer that consumes it is **Phase 3** (no rendering here).

### The `StreamEvent` type

Defined in Rust (`desktop/src-tauri/src/events.rs`) and TypeScript
(`ui/src/models/stream-event.model.ts`), camelCase on the wire — the Rust struct,
the DO envelope, and the TS interface must agree:

| Field | Type | Notes |
|---|---|---|
| `sessionId` | string | |
| `seq` | number | monotonic per turn/session, for ordering/dedup |
| `kind` | string | `text` \| `tool_call` \| `tool_result` \| `code` \| `mermaid` \| `error` |
| `text?` | string | prose / code body / mermaid source / error message |
| `language?` | string | for `kind:"code"` |
| `toolName?` | string | for tool_call / tool_result |
| `data?` | unknown | structured payload (tool args/result), opaque this phase |
| `final?` | boolean | last event of a turn |

### Flow

- Host: an `AppState.stream_event_tx` broadcast channel; a `process_messages`
  select arm forwards each event as `AgentMessage::StreamEvent`, whose wire
  `type` is `stream_event`.
- Worker (`chat-relay-do.ts`): `stream_event` is added to the envelope union and
  broadcast **desktop → clients only** (a client cannot inject one). The
  per-session controls `stream_subscribe` / `stream_unsubscribe` ride the same
  client→desktop forward and **ownership gate** as the terminal visual controls;
  for API-key sockets they require the `terminal:read` scope (a subscription is a
  read — no new scope invented).
- ui (`web/src/app/services/relay-terminal.service.ts`): a `streamEvents()`
  observable plus `subscribeStream(sessionId)` / `unsubscribeStream(sessionId)`.
  Subscriptions are **ref-counted** and **replayed on reconnect**, exactly like
  the terminal visual subscriptions. `StreamEvent` is re-exported from
  `@johnnyone/ui`.

A `cargo test` pins the camelCase wire contract (a `StreamEvent` broadcast
serializes to the expected `AgentMessage::StreamEvent` JSON).

## Deploy-skew safety

The whole surface is purely **additive** — new GraphQL queries/mutations/types,
new relay-RPC method names, a new envelope `type`. A worker deploy carrying these
against a still-running (pre-rebuild) desktop binary only affects the *new* calls;
every existing query/mutation/terminal path is untouched, and the host's
unknown-method arm rejects an unrecognized `files_*` call cleanly. Deploy ordering
is therefore not load-bearing for this phase.

Over-the-wire confirmation (a `curl` through the live relay to a rebuilt desktop)
is a **deferred operator smoke test** run after the next desktop rebuild — it
never gated the phase. Phase verification was `cargo test` + `nx build` +
programmatic round-trips against a temp dir.

## Source map

| Concern | Files |
|---|---|
| `files_root` setting + path guard | `desktop/.../services/settings.rs` (`KEY_FILES_ROOT`, `resolve_files_root`, `resolve_within_root`) |
| File engine (7 ops) | `desktop/.../services/host_files.rs` |
| Relay-RPC dispatch | `desktop/.../agent/mod.rs` (`files_*`, `capture_terminal` arms) |
| Stream event + channel | `desktop/.../events.rs` (`StreamEvent`), `state/app_state.rs` (`stream_event_tx`), `agent/message_types.rs` (`AgentMessage::StreamEvent`) |
| Shell provider confirmation | `desktop/.../providers/mod.rs`, `desktop/.../terminal.rs` |
| GraphQL schema | `worker/schema/johnnyone-ai.graphql` |
| Worker resolvers | `worker/resolvers/ai/files-*.ts`, `capture-terminal.ts` |
| API scopes | `worker/lib/auth/scopes.ts` (`files:read`, `files:write`) |
| Envelope + subscribe controls | `worker/lib/runtime/chat-relay-do.ts` |
| ui client + types | `ui/src/services/johnny-api.service.ts`, `ui/src/models/stream-event.model.ts`, `ui/src/index.ts` |
| ui stream service | `web/src/app/services/relay-terminal.service.ts` |
