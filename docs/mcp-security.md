# MCP security — the plan agent's `/mcp` endpoint

> Plans tab, initiative 8ad3ae71, phase 02. Required by the user's brief section "MCP security"
> (2026-11-23). This is the durable record of what protects `POST 127.0.0.1:7788/mcp`, which code
> enforces each control, and which test pins it. **Read this before touching `/mcp`, the plan agent's
> launch flags, `repo_search`, or the `mcp_tokens` table.**

The Plans-tab **plan agent** is a claude_code chat session (one `claude --print --resume` process per
turn on the existing chat paths). Each turn runs **visibly** in the agent's own tmux session
`johnnyone_<sessionId>`, but the pane is for watching only: J1 reads the turn's replies and tool calls
from its structured stream-json file, never from the pane (see "Plan agent in tmux" below). Its ONLY write path into J1 is the MCP server at
`POST 127.0.0.1:7788/mcp` (Streamable HTTP, JSON responses). It exposes ten tools: `brief_get`,
`brief_update`, `brief_versions`, the five outline tools, `plan_check` and the read-only
`repo_search`. Everything else on `:7788` (the GraphQL host API) is **unauthenticated by design**.
`/mcp` is the first authenticated route there and must not inherit that trust.

## Plan agent in tmux (user decision 2026-11-23, D1)

`desktop/src-tauri/src/services/plan_agent_tmux.rs`. One tmux session per plan agent,
`johnnyone_<sessionId>` (a J1-owned name, so the PR #75 retire and orphan-sweep rules apply). The
first chat turn creates it (`new-session … ; set-option -w remain-on-exit on`); later turns
`respawn-pane -k` it, so the pane keeps every turn's output. Each turn runs the generated script
`<initiatives_dir>/<id>/plan-agent/turns/<n>.sh`:

```sh
umask 077
printf '▶ you: %s\n' "$(head -n 1 '<n>.prompt' | cut -c1-200)"
{ 'claude' <the same plan-agent args, each single-quoted> "$(cat '<n>.prompt')" </dev/null 2>>'<n>.stderr'; echo $? > '<n>.rc'; } \
  | tee -a '<n>.jsonl' | { if <viewer verified>; then exec '<engine binary>' --plan-agent-view; else exec cat; fi; }
mv -f '<n>.rc' '<n>.exit'
```

- **Data path.** J1 reads replies and tool calls ONLY from `turns/<n>.jsonl` (append-only) through the
  same parser as the headless path (`claude_code::parse_line`). It never reads the pane.
- **Completion.** The turn is complete when the jsonl has the `result` event. `<n>.exit` is written
  after the whole pipeline, so the jsonl is already flushed when it appears. If `<n>.exit` appears
  and there is no `result` event, the turn crashed: J1 shows a chat error with the last stderr line.
  A pane that is gone or dead without `<n>.exit` (killed) is also a chat error. It never hangs.
- **Prompt.** The user's text is written to `<n>.prompt` (0600) and reaches claude as ONE argument
  through `"$(cat …)"`; the shell does not re-parse a command substitution inside double quotes.
  It is never part of the script or of any tmux argument. Every other word is single-quoted.
- **Token.** The pane command is `sh <n>.sh`. The script holds the config PATH, never the token, as
  the headless argv did. `turns/` is 0700. Every file in it is 0600: J1 creates the script, prompt
  and jsonl with `create_new` + mode 0600, and the script starts with `umask 077` for the files the
  shell creates (stderr, rc, exit).
- **One turn at a time.** A plan agent holds a turn slot (`plan_agent_tmux::claim_turn`) from spawn
  until its turn ends. A second send in the meantime is refused on every chat path, before anything
  is stored ("The plan agent is still answering the last message. Try again when it finishes."). Turn numbers are reserved
  with `create_new`, so they never collide. A turn's tail treats a newer turn's script as "this
  turn is over". A chat path only removes or kills its OWN handle from `active_processes`
  (`chat_host::take_own_process`).
- **The pane is view-only.** Only chat turns create or drive it. The terminal paths
  (`terminal.rs::plan_agent_pane_access`) may VIEW an existing pane through the Raw surface. They
  never create one, because a terminal-created pane would start an unrestricted interactive CLI.
  They never send input to it. The check uses the RESOLVED tmux target, so a session attached to
  `johnnyone_<planSid>` is refused too, and `sessions::create_session` refuses to attach to any
  `johnnyone_*` name. The question watcher (`question_watch::watchable_pane_ids`) never captures a
  plan agent's pane. These guards FAIL CLOSED: if the plan-agent lookup itself errors, input is
  refused, the pane is excluded, and the error is logged.
- **Only its plan ends it.** A live plan agent can't be closed, archived or deleted from a terminal
  or the session UI. `plan_agent::refuse_human_teardown` guards `sessions::archive_session`,
  `sessions::delete_session` and the relay's pane kill (`terminal::kill_terminal_session_human`).
  Only the retire path ends the agent, and it revokes the MCP token. The web hides close, rename and
  Watchdog for a plan agent. It decides this from the engine-computed `planAgent` session flag,
  never from the title.
- **No terminal escapes in the pane.** Everything the pane prints is stripped of C0 controls (except
  tab and newline), DEL and C1 controls. That covers the viewer's lines (`strip_controls`), the
  `cat` fallback, the "▶ you:" line and the stderr tail (`SHELL_SANITIZE`). Model, tool or stderr
  text can't send OSC 52 clipboard writes or other escape sequences to whoever is watching. The jsonl
  data path is untouched.
- **Lifecycle.** Stop and retire end an in-flight turn: they cancel the tail and run
  `respawn-pane -k` with a fixed notice. Accept, delete, replace and any other retire kill the
  pane through the pane seam (`retire_agent_session_with`). The orphan sweep reaps a pane whose row
  is archived or gone. Retention: `turns/` survives accept and is deleted with the plan.
- **Live view.** The pane runs the engine binary as a pretty-printer (`--plan-agent-view`), which
  shows the user's line, assistant text, `→ tool(args)` and `✓ v5`. The printer always drains its
  stdin, so it can never break the `tee` that feeds the data path. Before exec'ing it, the script
  checks that the file at the binary's path is still the running engine: its `dev:ino:size:mtime`
  must equal the identity recorded from `/proc/self/exe`, a `stat` check that runs nothing. The
  binary must then answer the `--plan-agent-view-probe` handshake. If either check fails, the pane
  falls back to `cat`, so a replaced or rolled-back binary is never run.

## Threat model

**Who can reach `127.0.0.1:7788`.**

- **Any local process** of any user on this machine. The listener binds loopback, so other
  machines can't connect, but every local program can.
- **Browser pages, indirectly.** Any site the user visits can `fetch()` loopback.
  - A CORS-"simple" `POST` (e.g. `text/plain`) needs no preflight.
  - DNS rebinding lets a hostile domain resolve to `127.0.0.1`, so its requests carry a foreign `Host`.
  - Without the controls below, a page could drive `/mcp` the same way it once could drive `/graphql`.

**What a plan agent could misuse.**

- **Its own tools**, to touch a plan other than its own. Scope must come from the token, never from
  arguments.
- **Other plans' `mcp.json` token files.** They live in the plan store
  (`<initiatives_dir>/<id>/plan-agent/mcp.json`). A workspace can be an ANCESTOR of the plan store
  (e.g. `/home/…/Workspace`), so a generic file-read tool would let plan A's agent read plan B's
  token. This was the round-1 Lead finding.
- **Files outside its workspace** (`~/.ssh`, `.env`, other repos), through a built-in Read/Grep/Glob
  tool or a `..`/symlink escape in a search path.
- **Command execution or file writes**, via a built-in Bash/Write/Edit tool or a tool that shells out.

**Out of scope.**

- **A local attacker running as the same OS user.** Such an attacker can read the `0600` config file
  (or `/proc`, or the SQLite DB) directly; file permissions cannot stop the file's own owner. The
  controls narrow what the *agent* and *other users/pages* can do. They are not a sandbox against
  the user's own account.
- **Model misbehaviour inside the agent's own plan.** It can only edit its own brief through
  versioned, attributed, undoable writes.

## Controls

| id | control | code path | tests |
|---|---|---|---|
| C1 | Per-session bearer token: 256 bits from the OS RNG, only its SHA-256 is stored, compared in constant time (`subtle`); every request (incl. `initialize`) must present it; the token's session must exist, be active and still be the plan's briefing_session_id | desktop/src-tauri/src/services/mcp_tokens.rs::mint, mcp_tokens.rs::verify, host/mcp.rs::authorize | `token_is_256_bits_hex`, `only_hash_is_stored`, `verify_rejects_malformed_unknown_revoked`, `missing_token_401`, `malformed_token_401`, `unknown_token_401`, `revoked_token_401`, `archived_session_rejected`, `deleted_session_rejected`, `replaced_session_rejected` |
| C2 | The raw token lives only in a 0600 `mcp.json` inside a 0700 per-plan dir, exactly once, in `mcpServers.j1.headers.Authorization`; argv carries the file PATH only; never in env, logs or events (`RawToken` has a redacting Debug and no Display/Serialize) | mcp_tokens.rs::write_mcp_config, mcp_tokens.rs::RawToken, plan_agent.rs::plan_agent_extras, cli_runner.rs::log_spawn, plan_agent_tmux.rs::turn_script, plan_agent_tmux.rs::start_turn (the tmux command and turn script carry the PATH only; `turns/` 0700, files 0600) | `mcp_config_file_is_0600_in_0700_dir`, `mcp_config_token_only_in_authorization_header`, `raw_token_debug_is_redacted`, `launch_args_never_contain_token` (incl. the tmux command string and turn script), `turn_command_never_contains_token`, `logged_launch_args_contain_no_token`, `token_absent_from_events_and_logs` |
| C3 | Scope comes from the token only: no tool schema has an initiative/plan id, every schema is `additionalProperties:false`, arguments are validated against the SAME schema `tools/list` advertises; no tool executes a command or writes a file | plan_mcp.rs::TOOLS, plan_mcp.rs::validate, host/mcp.rs::tools_call | `token_a_cannot_touch_plan_b`, `initiative_id_argument_is_a_validation_error`, `every_tool_rejects_unknown_and_mistyped_args`, `every_schema_uses_supported_keywords`, `no_tool_writes_outside_db`, `tools_call_brief_update_increments_version_author_agent` |
| C4 | Revoke on accept (Start planning), delete, any retire, and replacement; a plan that left briefing answers 403 with the frozen version; revoking deletes `mcp.json` | agent_plans.rs::freeze_and_flip_brief, agent_plans.rs::freeze_flip_and_retire_agent, agent_plans.rs::delete_plan_with, agent_plans.rs::retire_agent_session_with, plan_agent.rs::retire_plan_agent_with, plan_agent.rs::replace_plan_agent_session, mcp_tokens.rs::revoke_for_initiative; retiring also kills the agent's `johnnyone_<sid>` pane, and the orphan sweep reaps one left behind | `accept_revokes_token_and_retires_session`, `delete_revokes_token_and_retires_session`, `retire_revokes_token`, `new_session_token_revokes_old`, `mint_revokes_previous_live_token`, `replacement_stops_and_retires_old_session`, `retired_session_cannot_call_mcp`, `call_after_accept_rejected`, `plan_agent_pane_created_on_first_turn_and_killed_on_every_retire`, `plan_agent_pane_kept_while_live_swept_once_retired`, `stop_cancels_tail_and_kills_only_the_agent_pane`, `plan_agent_cannot_be_closed_from_a_terminal_only_retired` |
| C5 | Transport guard before any parsing: POST only (405), ANY Origin refused (403), Host must be 127.0.0.1 or localhost on the bound port (421), Content-Type application/json (415), body at most 256 KiB incl. chunked (413); `/mcp` is merged beside, not inside, the CORS-enabled origin guard so it never emits an access-control header | host/mcp.rs::check_request, host/mcp.rs::mcp_router, host/mod.rs::router | `check_request_table`, `origin_header_rejected`, `bad_host_rejected`, `non_post_rejected`, `options_preflight_rejected_without_cors`, `non_json_rejected`, `oversized_body_rejected`, `no_cors_headers_on_mcp`, `graphql_route_unchanged` |
| C6 | Least-privilege agent. PRIMARY: `--tools ""`, an allow-list of NO built-in tools (the CLI then reports zero tools; it cannot go stale when a CLI release adds one). Extra layers: `--allowedTools` is exactly the ten `mcp__j1__*` tools, `--strict-mcp-config` (only the j1 server), `--disallowedTools` lists every built-in that reads/writes/executes/fetches/delegates (incl. Agent, LSP, Skill), `--permission-mode default` (never bypassPermissions); ordinary chats get none of these flags; refused after accept and on any non-claude provider | providers/claude_code.rs::build_config_with, claude_code.rs::PLAN_AGENT_DISALLOWED_TOOLS, chat_host.rs::plan_agent_turn_extras (used by chat_host, agent/mod.rs::handle_relay_chat and commands/chat.rs::send_chat_message), plan_agent_tmux.rs::spawn_chat_turn (the turn runs in the agent's visible `johnnyone_<sid>` pane with exactly those args; data comes only from the turn's stream-json, never from the pane; the prompt goes through a file, never a shell string), terminal.rs::plan_agent_pane_access (the terminal paths may VIEW the pane, never create it or type into it) | `plan_agent_args_least_privilege`, `allowed_tools_are_mcp_j1_only`, `plain_chat_args_unchanged`, `plan_agent_deny_list_matches_the_probe`, `extras_err_after_accept`, `extras_none_for_non_plan_session`, `system_prompt_mentions_tools_only_and_choices_line`, `plan_agent_pane_is_view_only_and_created_by_turns`, `attached_session_targeting_plan_pane_cannot_type`, `create_session_refuses_attaching_johnnyone_names`, `plan_agent_pane_is_never_captured`, `second_send_while_turn_in_flight_is_refused_and_first_unharmed`, `concurrent_starts_never_share_a_turn_number`, `stale_turn_cleanup_never_touches_a_later_turn`, `superseded_turn_tail_ends_even_with_a_live_pane`, `viewer_runs_only_after_identity_and_probe`, `plan_agent_guards_fail_closed_on_lookup_error`, `view_lines_strip_terminal_controls`, `pane_shell_output_strips_terminal_controls`, `prompt_with_shell_metacharacters_cannot_inject`, `shell_quote_round_trips`, `structured_parse_identical_to_headless_path`, `completion_detected_from_jsonl_result_event`, `crash_without_result_is_an_error_with_last_stderr_line`, `pane_killed_mid_turn_is_an_error_not_a_hang` |
| C7 | `repo_search` is read-only and jailed: relative paths only, no `..`, canonicalized start must stay in the workspace, symlinked dirs never followed, symlinked files only when they resolve inside; hard exclusions on the canonical path (initiatives dir, any `.johnnyone` or `plan-agent` component, `mcp.json`, `.env*`); output, line and time caps; an in-process walker, no subprocess | services/repo_search.rs::repo_search_until, repo_search.rs::is_excluded, repo_search.rs::check_relative | `repo_search_finds_literal_case_insensitive`, `repo_search_dotdot_refused`, `repo_search_absolute_refused`, `repo_search_symlink_escape_refused`, `repo_search_output_capped`, `repo_search_time_capped`, `repo_search_skips_ignored_dirs`, `repo_search_never_returns_mcp_tokens`, `repo_search_excludes_real_initiatives_dir_inside_workspace` |
| C8 | Audit: every accepted write is a brief_version_created event with author agent + sessionId; every refused call that resolves to a plan is an agent_mcp_rejected {reason, method, tool, count} event (fixed labels only — client text is never echoed); planless refusals are a `tracing` warn with a global counter (in the engine log, see Operations); rejections never page Discord | plan_mcp.rs::record_rejection, brief_store.rs::write_version, agent_plans.rs::event_summary | `accepted_write_event_has_agent_and_session`, `rejection_event_has_reason_and_count_no_token`, `rejection_renders_in_timeline`, `unknown_method_32601`, `token_absent_from_events_and_logs` |

The caller always gets ONE undistinguished `401 {"code":-32001,"message":"unauthorized"}` for
any token/session failure. The precise reason goes only to the audit, so the endpoint is not an
oracle for guessing tokens. Tool-level failures the model can fix (unknown outline id, `no change`,
over a cap) return `result.isError = true`. Schema and argument failures return JSON-RPC `-32602`.

### Input limits

Both writers (human GraphQL/RPC and agent MCP) go through `brief_model::apply_field_op` /
apply_outline_op, so one set of caps guards both. The MCP schemas mirror the same numbers as
`maxLength`/`maxItems`:

- a text field holds at most 4,000 characters;
- `where` holds at most 50 items of at most 300 characters each;
- an outline holds at most 20 phases × 30 tasks;
- a title holds at most 200 characters.

Single-line values (`where` items, titles) refuse newlines and control characters. When
render_brief_md builds the planner's input, it escapes any line that would add markdown structure:
a leading `#`, a code fence, or a setext/thematic rule. Pinned by
`over_cap_values_are_rejected_with_a_clear_error` and
`injected_title_cannot_add_a_heading_or_section`.

## Token lifecycle

```mermaid
sequenceDiagram
  participant W as Web (chat turn)
  participant H as Host chat path (chat_host / relay)
  participant T as mcp_tokens
  participant F as plan-agent/mcp.json (0600, dir 0700)
  participant P as tmux pane johnnyone_<sid> (view only)
  participant J as turns/<n>.jsonl (0600, dir 0700)
  participant C as claude --print (plan agent)
  participant M as /mcp (guard + handle_mcp)
  participant S as brief_store
  W->>H: send a message to the plan's briefing session
  H->>T: plan_agent_extras: live token for (plan, session)? file present?
  T->>T: mint: 256-bit token, revoke older live token, store SHA-256 only
  T->>F: write {"mcpServers":{"j1":{"headers":{"Authorization":"Bearer …"}}}}
  H->>P: new-session / respawn-pane: sh turns/<n>.sh (prompt in <n>.prompt; no token in argv/script)
  P->>C: claude --mcp-config <PATH> --strict-mcp-config --allowedTools mcp__j1__* "$(cat <n>.prompt)"
  C->>F: reads the config file
  C->>J: stream-json via tee -a (the pane shows a pretty-printed copy)
  H->>J: tail: parse with claude_code::parse_line, done on the result event
  C->>M: POST /mcp, Authorization: Bearer <token>
  M->>M: transport guard (POST, no Origin, Host, JSON, ≤256 KiB)
  M->>T: verify: 64 hex, hash, constant-time compare, not revoked
  M->>M: session active AND == briefing_session_id; plan in briefing
  M->>S: tool runs with TokenScope (author agent, sessionId)
  S-->>M: {version, changed_fields}
  Note over W,S: Start planning / delete / retire / replace
  H->>T: revoke_for_initiative / revoke_for_session (revoked_at set)
  T->>F: delete mcp.json
  H->>P: stop the turn, kill-session johnnyone_<sid>
  C->>M: any later request with the old token
  M-->>C: 401 unauthorized (or 403 frozen if only the plan flipped)
```

## Operations

All commands assume the default DB `~/.local/share/johnnyone/johnnyone.db` and the plan store
`/home/creepy/Documents/Workspace/.johnnyone/initiatives` (the `initiatives_dir` setting).

**Revoke one plan's token.** The clean way is to delete the plan (it revokes, deletes the file and
retires the agent) or to Start planning. By hand:

```bash
sqlite3 ~/.local/share/johnnyone/johnnyone.db \
  "UPDATE mcp_tokens SET revoked_at=datetime('now') WHERE initiative_id='<id>' AND revoked_at IS NULL"
rm -rf /home/creepy/Documents/Workspace/.johnnyone/initiatives/<id>/plan-agent/
```

The next chat turn mints a fresh token and rewrites the file (only while the plan is in briefing).

**Revoke every token.**

```bash
sqlite3 ~/.local/share/johnnyone/johnnyone.db \
  "UPDATE mcp_tokens SET revoked_at=datetime('now') WHERE revoked_at IS NULL"
rm -rf /home/creepy/Documents/Workspace/.johnnyone/initiatives/*/plan-agent/
```

**Read the audit.** The initiative's timeline shows `MCP call rejected: <reason> (×n)`. Or query it:

```bash
sqlite3 ~/.local/share/johnnyone/johnnyone.db \
  "SELECT created_at, payload_json FROM agent_plan_events WHERE type='agent_mcp_rejected' ORDER BY created_at DESC LIMIT 20"
```

Planless refusals (no/unknown token, transport guard) are only in the engine log (target `mcp`,
`MCP call rejected reason=… count=…`). Where that log is depends on how the engine was started:

- **Production** (the `j1watch` service): the j1watch service's log. `j1watch.sh` sets its path,
  `logs/j1watch/johnnyone-desktop.log` under the service's working directory.
- **Dev / manual launch** (`npm run desktop`): `/tmp/johnnyone-desktop.log`

```bash
grep 'MCP call rejected' <log> | tail
```

**Watch the plan agent.** Its pane is `johnnyone_<briefingSessionId>`. Open the Plans detail page and
use **View pane** (the Raw terminal surface), or run `tmux attach -r -t johnnyone_<sid>`. Attach
read-only; the chat is the only input. A turn's data is in
`<initiatives_dir>/<id>/plan-agent/turns/<n>.jsonl`, and a crash's stderr is in `<n>.stderr`.
The Raw page shows a view-only banner for a plan agent and has no input.

**Live checks** (engine running):

- No token → 401: `curl -s -o /dev/null -w '%{http_code}\n' -X POST http://127.0.0.1:7788/mcp -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'` prints `401`.
- Any Origin → 403: `curl -s -o /dev/null -w '%{http_code}\n' -X POST http://127.0.0.1:7788/mcp -H 'content-type: application/json' -H 'Origin: http://evil.example' -d '{}'` prints `403`.
- A rebinding Host → 421: `curl -s -o /dev/null -w '%{http_code}\n' -X POST http://127.0.0.1:7788/mcp -H 'Host: evil.example:7788' -H 'content-type: application/json' -d '{}'` prints `421`.
- A token used after Start planning → 403 (frozen) or 401 (revoked): take the Bearer value from a plan's `mcp.json` before you accept, then run `curl -s -X POST http://127.0.0.1:7788/mcp -H 'content-type: application/json' -H "Authorization: Bearer $TOKEN" -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'`. After accept it returns `401` `unauthorized`, because accept revokes the token. Only if the plan flipped without the revoke does it return `403` `the plan left briefing; the brief is frozen at v<n>`.

## Never do

- Never add an `initiativeId`, `planId` or any other scope argument to an MCP tool; scope comes from the token.
- Never put the token (or its hash) on a command line, in an env var, in a log line, in an event payload or in an error message.
- Never store the raw token; only `sha256_hex(raw)` goes into `mcp_tokens`.
- Never wrap `/mcp` in `origin_guard::guard_layers` or any CORS layer, and never relax its "any Origin → 403" rule.
- Never let `/mcp` answer a GET, an OPTIONS preflight or a non-JSON body, and never raise the body cap without a reason recorded here.
- Never accept a token for a plan that is not in `briefing`, for an archived or deleted session, or for a session that is no longer the plan's `briefing_session_id`.
- Never give the plan agent a built-in tool: never drop `--tools ""` or `--strict-mcp-config`, and never run it with `bypassPermissions`.
- Never remove a hard exclusion from `repo_search`, never let it follow a symlinked directory, and never make it spawn a process (`rg` or anything else).
- Never write a tool that executes a command or writes a file; brief changes go through `brief_store::write_version` only.
- Never echo client-supplied text (method names, tool names, arguments) into an `agent_mcp_rejected` event.
- Never add a chat or terminal path that runs a plan-agent session without `plan_agent_turn_extras` (every claude spawn for it must carry the j1 flags). The agent gets a visible tmux pane only through `plan_agent_tmux` (chat turns). Never let a terminal path create that pane or type into it. Data flows only through the turn's stream-json; never read the agent's replies, tool calls or completion from the pane.
- Never interpolate chat text into a shell string or a tmux argument; the prompt reaches claude only through its 0600 `<n>.prompt` file.
- Never skip the revoke when adding a new way for a plan to leave briefing or for a plan-agent session to end. Human close/archive/delete paths must call `refuse_human_teardown`. Only the retire path ends a live plan agent.
- Never print model, tool or stderr text to the plan agent's pane without stripping terminal controls.

## Test index

- `token_is_256_bits_hex`
- `only_hash_is_stored`
- `verify_rejects_malformed_unknown_revoked`
- `missing_token_401`
- `malformed_token_401`
- `unknown_token_401`
- `revoked_token_401`
- `archived_session_rejected`
- `deleted_session_rejected`
- `replaced_session_rejected`
- `mcp_config_file_is_0600_in_0700_dir`
- `mcp_config_token_only_in_authorization_header`
- `raw_token_debug_is_redacted`
- `launch_args_never_contain_token`
- `logged_launch_args_contain_no_token`
- `token_absent_from_events_and_logs`
- `token_a_cannot_touch_plan_b`
- `initiative_id_argument_is_a_validation_error`
- `every_tool_rejects_unknown_and_mistyped_args`
- `every_schema_uses_supported_keywords`
- `no_tool_writes_outside_db`
- `tools_call_brief_update_increments_version_author_agent`
- `accept_revokes_token_and_retires_session`
- `delete_revokes_token_and_retires_session`
- `retire_revokes_token`
- `new_session_token_revokes_old`
- `mint_revokes_previous_live_token`
- `replacement_stops_and_retires_old_session`
- `retired_session_cannot_call_mcp`
- `call_after_accept_rejected`
- `check_request_table`
- `origin_header_rejected`
- `bad_host_rejected`
- `non_post_rejected`
- `options_preflight_rejected_without_cors`
- `non_json_rejected`
- `oversized_body_rejected`
- `no_cors_headers_on_mcp`
- `graphql_route_unchanged`
- `plan_agent_args_least_privilege`
- `allowed_tools_are_mcp_j1_only`
- `plain_chat_args_unchanged`
- `plan_agent_deny_list_matches_the_probe`
- `extras_err_after_accept`
- `extras_none_for_non_plan_session`
- `system_prompt_mentions_tools_only_and_choices_line`
- `plan_agent_pane_is_view_only_and_created_by_turns`
- `plan_agent_pane_created_on_first_turn_and_killed_on_every_retire`
- `plan_agent_pane_kept_while_live_swept_once_retired`
- `turn_command_never_contains_token`
- `prompt_with_shell_metacharacters_cannot_inject`
- `shell_quote_round_trips`
- `structured_parse_identical_to_headless_path`
- `completion_detected_from_jsonl_result_event`
- `crash_without_result_is_an_error_with_last_stderr_line`
- `pane_killed_mid_turn_is_an_error_not_a_hang`
- `stop_cancels_tail_and_kills_only_the_agent_pane`
- `second_send_while_turn_in_flight_is_refused_and_first_unharmed`
- `concurrent_starts_never_share_a_turn_number`
- `stale_turn_cleanup_never_touches_a_later_turn`
- `superseded_turn_tail_ends_even_with_a_live_pane`
- `attached_session_targeting_plan_pane_cannot_type`
- `create_session_refuses_attaching_johnnyone_names`
- `plan_agent_pane_is_never_captured`
- `viewer_runs_only_after_identity_and_probe`
- `plan_agent_cannot_be_closed_from_a_terminal_only_retired`
- `plan_agent_guards_fail_closed_on_lookup_error`
- `view_lines_strip_terminal_controls`
- `pane_shell_output_strips_terminal_controls`
- `repo_search_finds_literal_case_insensitive`
- `repo_search_dotdot_refused`
- `repo_search_absolute_refused`
- `repo_search_symlink_escape_refused`
- `repo_search_output_capped`
- `repo_search_time_capped`
- `repo_search_skips_ignored_dirs`
- `repo_search_never_returns_mcp_tokens`
- `repo_search_excludes_real_initiatives_dir_inside_workspace`
- `accepted_write_event_has_agent_and_session`
- `rejection_event_has_reason_and_count_no_token`
- `rejection_renders_in_timeline`
- `unknown_method_32601`
- `over_cap_values_are_rejected_with_a_clear_error`
- `injected_title_cannot_add_a_heading_or_section`
