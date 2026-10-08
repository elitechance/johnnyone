use crate::db::models::{
    CreateSessionInput, Message, ProviderConfig, Session, UpsertProviderConfigInput,
};
use crate::events::{ChatCompleteEvent, ChatDeltaEvent};
use crate::services::{
    agent_plans,
    chat_host,
    planner_prompts::{self as planner_prompt_service, PlannerPromptSettings},
    prompt_library::{self as prompt_library_service, PromptLibraryEntry},
    providers::{self as provider_service, DetectedTool},
    relay as relay_service,
    sessions as session_service, settings as settings_service,
};
use crate::state::app_state::AppState;
use async_graphql::futures_util::StreamExt;
use async_graphql::http::{playground_source, GraphQLPlaygroundConfig};
use async_graphql::{Context, InputObject, Object, Schema, SimpleObject, Subscription};
use async_graphql_axum::{GraphQLRequest, GraphQLResponse, GraphQLSubscription};
use axum::{
    extract::State as AxumState,
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use tokio_stream::wrappers::BroadcastStream;

pub mod origin_guard;

type JohnnyHostSchema = Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

pub fn router(state: AppState) -> Router {
    let schema = Schema::build(QueryRoot, MutationRoot, SubscriptionRoot)
        .data(state)
        .finish();

    // Binding `127.0.0.1` (main.rs) keeps other MACHINES out; it does not keep
    // other WEB PAGES out — any site the user visits can fetch this
    // unauthenticated surface from their own browser, and async-graphql parses a
    // CORS-simple `text/plain` POST as JSON, so there is not even a preflight to
    // fail. `origin_guard::guard_layers` therefore rejects a request whose
    // `Origin` is neither the webview's nor loopback's, and the CORS layer it
    // installs reuses the SAME predicate so `access-control-allow-origin: *` is
    // never echoed.
    //
    // What this buys: a page on `https://evil.example` can no longer drive or
    // read the host API. What it does NOT buy: anything against a non-browser
    // local attacker — a request with no `Origin` is allowed on purpose so the
    // baked agent `curl`s keep working, and a local process can omit `Origin`
    // just as easily. See `origin_guard`'s module docs.
    origin_guard::guard_layers(
        Router::new()
            .route("/graphql", get(graphql_playground).post(graphql_handler))
            .route_service("/graphql/ws", GraphQLSubscription::new(schema.clone())),
    )
    .with_state(schema)
}

/// `HeaderMap` must come BEFORE `GraphQLRequest`: the latter consumes the body,
/// and axum requires the body-consuming extractor last.
///
/// The per-request `WriteTrust` is injected into the GraphQL context because the
/// middleware cannot make this call — it does not parse the body, so it has no
/// idea which setting key is being written. Resolvers that care read it with
/// `data_opt` and default to the restrictive value, so the subscription
/// transport (which never carries it) fails closed.
async fn graphql_handler(
    AxumState(schema): AxumState<JohnnyHostSchema>,
    headers: axum::http::HeaderMap,
    req: GraphQLRequest,
) -> GraphQLResponse {
    let write_trust = origin_guard::write_trust_from_headers(&headers);
    schema
        .execute(req.into_inner().data(write_trust))
        .await
        .into()
}

/// GraphQL never executes on GET — this route only serves the playground's
/// HTML — but without a framing policy `evil.example` can iframe it and
/// clickjack its Run button, which chains with anything the POST rules still
/// allow. `frame-ancestors 'none'` is the modern control; `X-Frame-Options` is
/// the fallback for user agents that predate it. Response headers rather than a
/// `Sec-Fetch-Site` requirement, because WebKitGTK sends no fetch metadata and a
/// header costs nothing in compatibility.
async fn graphql_playground() -> impl IntoResponse {
    (
        [
            (
                axum::http::header::CONTENT_SECURITY_POLICY,
                "frame-ancestors 'none'",
            ),
            (axum::http::header::X_FRAME_OPTIONS, "DENY"),
        ],
        Html(playground_source(
            GraphQLPlaygroundConfig::new("/graphql").subscription_endpoint("/graphql/ws"),
        )),
    )
}

struct QueryRoot;

#[Object(rename_fields = "camelCase")]
impl QueryRoot {
    async fn health(&self) -> bool {
        true
    }

    async fn list_ai_sessions(
        &self,
        ctx: &Context<'_>,
        status: Option<String>,
    ) -> async_graphql::Result<Vec<AiSession>> {
        let state = ctx.data_unchecked::<AppState>();
        Ok(session_service::list_sessions(state, status)?
            .into_iter()
            .map(AiSession::from)
            .collect())
    }

    async fn get_ai_session(
        &self,
        ctx: &Context<'_>,
        id: String,
    ) -> async_graphql::Result<Option<AiSession>> {
        let state = ctx.data_unchecked::<AppState>();
        match session_service::get_session(state, id) {
            Ok(session) => Ok(Some(session.into())),
            Err(err) if err.starts_with("Session not found:") => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    async fn list_ai_messages(
        &self,
        ctx: &Context<'_>,
        session_id: String,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> async_graphql::Result<Vec<AiMessage>> {
        let state = ctx.data_unchecked::<AppState>();
        Ok(session_service::list_messages(
            state,
            session_id,
            limit.map(i64::from),
            offset.map(i64::from),
        )?
        .into_iter()
        .map(AiMessage::from)
        .collect())
    }

    async fn list_provider_configs(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Vec<GqlProviderConfig>> {
        let state = ctx.data_unchecked::<AppState>();
        Ok(provider_service::list_provider_configs(state)?
            .into_iter()
            .map(GqlProviderConfig::from)
            .collect())
    }

    /// Read one setting. The key is filtered against
    /// `settings_service::READABLE_SETTING_KEYS` because this surface is
    /// UNAUTHENTICATED: without the filter an arbitrary key reached
    /// `access_token`, which for a `jk_` credential is a durable worker API key
    /// that never expires (`services/relay.rs::refresh_access_token`). Same
    /// curated-projection idea as `host_settings` below.
    async fn get_setting(&self, ctx: &Context<'_>, key: String) -> async_graphql::Result<String> {
        if !settings_service::is_readable_setting_key(&key) {
            return Err(async_graphql::Error::new(format!(
                "getSetting: {key:?} is not readable over the host API"
            )));
        }
        let state = ctx.data_unchecked::<AppState>();
        Ok(settings_service::get_setting(state, key)?)
    }

    async fn get_planner_prompt_settings(&self) -> async_graphql::Result<GqlPlannerPromptSettings> {
        Ok(planner_prompt_service::load_prompt_settings()?.into())
    }

    async fn list_prompt_library(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Vec<GqlPromptLibraryEntry>> {
        let state = ctx.data_unchecked::<AppState>();
        Ok(prompt_library_service::list_prompt_library(state)?
            .into_iter()
            .map(GqlPromptLibraryEntry::from)
            .collect())
    }

    /// External tmux sessions a new terminal can attach to (excludes the
    /// `johnnyone_<id>` panes JohnnyOne already manages). Mirrors the worker's
    /// `listTmuxSessions` — the console prefers the host when it is reachable
    /// (`queryPreferLocalHost`) and has no worker fallback, so this field must
    /// exist on BOTH surfaces or attach breaks in the desktop app.
    /// A session's persisted reports, oldest→newest. Lets a freshly-opened transcript show what
    /// it missed instead of starting blank. Must exist on BOTH GraphQL surfaces — the console
    /// prefers the host when reachable and has no worker fallback.
    async fn list_session_reports(
        &self,
        ctx: &Context<'_>,
        session_id: String,
        limit: Option<i32>,
    ) -> async_graphql::Result<Vec<GqlSessionReport>> {
        let state = ctx.data_unchecked::<AppState>();
        Ok(
            agent_plans::list_session_reports(state, &session_id, limit.unwrap_or(100) as i64)?
                .into_iter()
                .map(GqlSessionReport::from)
                .collect(),
        )
    }

    async fn list_tmux_sessions(&self) -> async_graphql::Result<Vec<GqlTmuxSession>> {
        Ok(crate::terminal::list_external_tmux_sessions()
            .await?
            .into_iter()
            .map(GqlTmuxSession::from)
            .collect())
    }

    async fn host_settings(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlHostSettings> {
        let state = ctx.data_unchecked::<AppState>();
        Ok(settings_service::load_host_settings(state).into())
    }

    async fn relay_connection_status(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<GqlRelayConnectionStatus> {
        let state = ctx.data_unchecked::<AppState>();
        let status = state.connection_status.lock().await;
        Ok(GqlRelayConnectionStatus {
            connected: status.connected,
            session_id: status.session_id.clone(),
            last_heartbeat: status.last_heartbeat.as_ref().map(|t| t.to_rfc3339()),
        })
    }
}

struct MutationRoot;

#[Object(rename_fields = "camelCase")]
impl MutationRoot {
    async fn create_ai_session(
        &self,
        ctx: &Context<'_>,
        input: CreateAiSessionInput,
    ) -> async_graphql::Result<AiSession> {
        let state = ctx.data_unchecked::<AppState>();
        Ok(session_service::create_session(state, input.into())?.into())
    }

    /// Structured completion signal from a planner/development agent. The agent is
    /// given a baked `curl` for this in its prompt. `sessionId` must be one CO
    /// spawned (worker or reviewer) — unknown ids are rejected. `kind` is
    /// "ready" (worker/planner done) or "verdict" (reviewer), with `verdict` one of
    /// PASS / NEEDS_CHANGES / BLOCKED. Reached over unauthenticated localhost; the
    /// unguessable session id is the verification.
    #[allow(clippy::too_many_arguments)]
    async fn report_agent_result(
        &self,
        ctx: &Context<'_>,
        session_id: String,
        kind: String,
        verdict: Option<String>,
        findings: Option<String>,
        summary: Option<String>,
        severity: Option<String>,
        reason: Option<String>,
        evidence: Option<String>,
    ) -> async_graphql::Result<bool> {
        let state = ctx.data_unchecked::<AppState>();
        agent_plans::record_agent_report(
            state, session_id, kind, verdict, findings, summary, severity, reason, evidence,
        )
        .await?;
        Ok(true)
    }

    async fn update_ai_session_title(
        &self,
        ctx: &Context<'_>,
        id: String,
        title: String,
    ) -> async_graphql::Result<AiSession> {
        let state = ctx.data_unchecked::<AppState>();
        Ok(session_service::update_session_title(state, id, title)?.into())
    }

    async fn update_ai_session_working_directory(
        &self,
        ctx: &Context<'_>,
        id: String,
        working_directory: String,
    ) -> async_graphql::Result<AiSession> {
        let state = ctx.data_unchecked::<AppState>();
        Ok(session_service::update_session_working_directory(state, id, working_directory)?.into())
    }

    async fn update_ai_session_provider(
        &self,
        ctx: &Context<'_>,
        id: String,
        provider: String,
    ) -> async_graphql::Result<AiSession> {
        let state = ctx.data_unchecked::<AppState>();
        Ok(session_service::update_session_provider(state, id, provider)?.into())
    }

    async fn archive_ai_session(
        &self,
        ctx: &Context<'_>,
        id: String,
    ) -> async_graphql::Result<AiSession> {
        let state = ctx.data_unchecked::<AppState>();
        Ok(session_service::archive_session(state, id).await?.into())
    }

    async fn delete_ai_session(
        &self,
        ctx: &Context<'_>,
        id: String,
    ) -> async_graphql::Result<bool> {
        let state = ctx.data_unchecked::<AppState>();
        session_service::delete_session(state, id).await?;
        Ok(true)
    }

    async fn send_ai_chat_message(
        &self,
        ctx: &Context<'_>,
        input: SendAiChatMessageInput,
    ) -> async_graphql::Result<AiChatRunResult> {
        let state = ctx.data_unchecked::<AppState>();
        Ok(
            chat_host::send_chat_message_blocking(state, input.session_id, input.content)
                .await?
                .into(),
        )
    }

    async fn stop_ai_generation(
        &self,
        ctx: &Context<'_>,
        session_id: String,
    ) -> async_graphql::Result<bool> {
        let state = ctx.data_unchecked::<AppState>();
        chat_host::stop_generation(state, session_id).await?;
        Ok(true)
    }

    /// Refused on this surface — see [`provider_config_writes_refused`].
    async fn upsert_provider_config(
        &self,
        _ctx: &Context<'_>,
        _input: UpsertProviderConfigInputGql,
    ) -> async_graphql::Result<GqlProviderConfig> {
        Err(provider_config_writes_refused("upsertProviderConfig"))
    }

    /// Refused on this surface — see [`provider_config_writes_refused`].
    async fn delete_provider_config(
        &self,
        _ctx: &Context<'_>,
        _provider: String,
    ) -> async_graphql::Result<bool> {
        Err(provider_config_writes_refused("deleteProviderConfig"))
    }

    async fn detect_cli_tools(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Vec<GqlDetectedTool>> {
        let state = ctx.data_unchecked::<AppState>();
        Ok(provider_service::detect_cli_tools(state)
            .await?
            .into_iter()
            .map(GqlDetectedTool::from)
            .collect())
    }

    /// Write one setting. Two gates, both because this surface is
    /// UNAUTHENTICATED and reachable from any loopback page in the user's
    /// browser:
    ///
    /// 1. The key must be one a client actually writes
    ///    (`settings_service::WRITABLE_SETTING_KEYS`), so the settings table
    ///    cannot be used as arbitrary attacker-controlled storage.
    /// 2. A **connection** key (`relay::is_connection_key`) additionally needs
    ///    `WriteTrust::Webview`. Without this, a page on `http://localhost:3000`
    ///    could point `worker_url` at its own endpoint, and `apply_setting`'s
    ///    reconnect would hand it the real `jk_` relay credential — which never
    ///    expires. Gating `getSetting` against RETURNING the token is pointless
    ///    while a caller can redirect where the token is SENT.
    async fn set_setting(
        &self,
        ctx: &Context<'_>,
        key: String,
        value: String,
    ) -> async_graphql::Result<bool> {
        if !settings_service::is_writable_setting_key(&key) {
            return Err(async_graphql::Error::new(format!(
                "setSetting: {key:?} is not writable over the host API"
            )));
        }
        // Fail closed: a transport that never injected the marker (the
        // subscription socket) is treated as untrusted.
        let trust = ctx
            .data_opt::<origin_guard::WriteTrust>()
            .copied()
            .unwrap_or(origin_guard::WriteTrust::Loopback);
        if relay_service::is_connection_key(&key)
            && trust != origin_guard::WriteTrust::Webview
        {
            return Err(async_graphql::Error::new(format!(
                "setSetting: {key:?} is a relay-connection key and may only be \
                 written from the JohnnyOne app itself"
            )));
        }
        let state = ctx.data_unchecked::<AppState>();
        crate::services::relay::apply_setting(state, key, value).await?;
        Ok(true)
    }

    async fn set_planner_prompt_settings(
        &self,
        input: PlannerPromptSettingsInput,
    ) -> async_graphql::Result<GqlPlannerPromptSettings> {
        let current = planner_prompt_service::load_prompt_settings()?;
        let small_mode = input.small_mode.map(|sm| planner_prompt_service::SmallModePrompts {
            planner: sm.planner,
            reviewer: sm.reviewer,
            leaf_wrapper: sm.leaf_wrapper,
            amend_planner: sm.amend_planner,
        });
        let merged = planner_prompt_service::overlay_prompt_settings(
            current,
            planner_prompt_service::PlannerDevelopmentOverlay {
                worker: input.development.worker,
                reviewer: input.development.reviewer,
                worker_nudge: input.development.worker_nudge,
            },
            planner_prompt_service::PlannerPlanningOverlay {
                planner: input.planning.planner,
                reviewer: input.planning.reviewer,
                amend_planner: input.planning.amend_planner,
                amend_reviewer: input.planning.amend_reviewer,
            },
            small_mode,
        );
        Ok(planner_prompt_service::save_prompt_settings(merged)?.into())
    }

    async fn connect_relay(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let state = ctx.data_unchecked::<AppState>().clone();
        relay_service::ensure_connected(state).await?;
        Ok(true)
    }
}


/// Provider-config WRITES are refused outright on the host's GraphQL surface,
/// which is unauthenticated and reachable from any loopback page in the user's
/// browser (and, over `/graphql/ws`, from a graphql-transport-ws `subscribe`
/// message — `async-graphql-7.2.1/src/schema.rs:606` executes any
/// non-subscription operation there, so the socket is a second mutation entry
/// point).
///
/// The reason this is refused rather than tiered like `setSetting`:
/// `cli_path` BECOMES THE EXECUTED COMMAND (`providers/claude_code.rs:19`,
/// `providers/ollama_cli.rs:14`, `providers/cline.rs:11` all do
/// `cli_path.unwrap_or(<default>)` and run it), so a write here is local code
/// execution on the next agent run — strictly worse than the credential theft
/// the setting guards close.
///
/// Refusing costs nothing, because NOTHING writes provider configs over this
/// surface. Verified against the built bundles, not the source:
/// - `dist/host-app/browser/*.js` (the Tauri webview, the only client on this
///   surface) contains zero occurrences of `upsertProviderConfig` /
///   `deleteProviderConfig`. It only READS, via
///   `{ listProviderConfigs { provider cliPath isAvailable defaultModel } }`
///   (`chunk-DJ5NYM3W.js`), which still works.
/// - `dist/web/browser/*.js` has both, but as
///   `upsertProviderConfig(i){return this.gql.mutate(...)}` — `gql.mutate`
///   targets the WORKER, never `queryPreferLocalHost`. The console's write
///   therefore travels worker -> WS relay -> `agent/mod.rs:1313`
///   `rpc_upsert_provider_config` -> `provider_service::upsert_provider_config`,
///   which never reaches `graphql_handler`. That path is untouched.
///
/// The fields stay in the schema so the host keeps field-for-field parity with
/// the worker's `ProviderConfig` surface; only execution is refused.
fn provider_config_writes_refused(field: &str) -> async_graphql::Error {
    async_graphql::Error::new(format!(
        "{field}: provider configuration cannot be written over the host API \
         (cliPath is executed). Use the JohnnyOne console, which routes through \
         the authenticated worker relay."
    ))
}

struct SubscriptionRoot;

#[Subscription(rename_fields = "camelCase")]
impl SubscriptionRoot {
    async fn on_ai_chat_delta(
        &self,
        ctx: &Context<'_>,
        session_id: String,
    ) -> impl futures_util::Stream<Item = GqlAiChatDelta> {
        let state = ctx.data_unchecked::<AppState>().clone();
        BroadcastStream::new(state.chat_delta_tx.subscribe()).filter_map(move |result| {
            let session_id = session_id.clone();
            async move {
                match result.ok() {
                    Some(event) if event.session_id == session_id => Some(event.into()),
                    _ => None,
                }
            }
        })
    }

    async fn on_ai_chat_complete(
        &self,
        ctx: &Context<'_>,
        session_id: String,
    ) -> impl futures_util::Stream<Item = GqlAiChatComplete> {
        let state = ctx.data_unchecked::<AppState>().clone();
        BroadcastStream::new(state.chat_complete_tx.subscribe()).filter_map(move |result| {
            let session_id = session_id.clone();
            async move {
                match result.ok() {
                    Some(event) if event.session_id == session_id => Some(event.into()),
                    _ => None,
                }
            }
        })
    }
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "AiSession", rename_fields = "camelCase")]
struct AiSession {
    id: String,
    title: String,
    provider: String,
    model: String,
    working_directory: String,
    status: String,
    total_input_tokens: i64,
    total_output_tokens: i64,
    total_cost_cents: i64,
    created_at: String,
    updated_at: String,
    /// True when this session views an EXTERNAL tmux session rather than its
    /// own `johnnyone_<id>` pane. Mirrors the worker's `attachedTmux` — the
    /// console selects it on the shared `listAiSessions` query that
    /// `queryPreferLocalHost` may route here.
    attached_tmux: bool,
}

impl From<Session> for AiSession {
    fn from(value: Session) -> Self {
        Self {
            id: value.id,
            title: value.title,
            provider: value.provider,
            model: value.model,
            working_directory: value.working_directory,
            status: value.status,
            total_input_tokens: value.total_input_tokens,
            total_output_tokens: value.total_output_tokens,
            total_cost_cents: value.total_cost_cents,
            created_at: value.created_at,
            updated_at: value.updated_at,
            attached_tmux: value.attached_tmux,
        }
    }
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "AiMessage", rename_fields = "camelCase")]
struct AiMessage {
    id: String,
    session_id: String,
    role: String,
    content: String,
    tool_calls: Option<String>,
    finish_reason: Option<String>,
    input_tokens: i64,
    output_tokens: i64,
    cost_cents: i64,
    created_at: String,
}

impl From<Message> for AiMessage {
    fn from(value: Message) -> Self {
        Self {
            id: value.id,
            session_id: value.session_id,
            role: value.role,
            content: value.content,
            tool_calls: value.tool_calls,
            finish_reason: value.finish_reason,
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            cost_cents: value.cost_cents,
            created_at: value.created_at,
        }
    }
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "ProviderConfig", rename_fields = "camelCase")]
struct GqlProviderConfig {
    id: String,
    provider: String,
    cli_path: String,
    api_key: String,
    default_model: String,
    settings: String,
    is_available: bool,
    updated_at: String,
}

impl From<ProviderConfig> for GqlProviderConfig {
    fn from(value: ProviderConfig) -> Self {
        Self {
            id: value.id,
            provider: value.provider,
            cli_path: value.cli_path,
            // Never leave the host. The field stays in the schema for parity
            // with the worker's `ProviderConfig`, but nothing in the clients
            // reads it: `ui/src/services/johnny-api.service.ts:726,765` only
            // SELECTS it, `host-app/src/app/services/host-status.service.ts:110`
            // asks for `provider cliPath isAvailable defaultModel`, and there is
            // no `.apiKey` consumer anywhere in `ui/`, `web/` or `host-app/`.
            // This surface is unauthenticated, so cleartext here is the same
            // leak class as `getSetting("access_token")`.
            api_key: String::new(),
            default_model: value.default_model,
            settings: value.settings,
            is_available: value.is_available,
            updated_at: value.updated_at,
        }
    }
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "DetectedTool", rename_fields = "camelCase")]
struct GqlDetectedTool {
    provider: String,
    command: String,
    found: bool,
    path: Option<String>,
}

impl From<DetectedTool> for GqlDetectedTool {
    fn from(value: DetectedTool) -> Self {
        Self {
            provider: value.provider,
            command: value.command,
            found: value.found,
            path: value.path,
        }
    }
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "HostSettings", rename_fields = "camelCase")]
struct GqlHostSettings {
    worker_url: String,
    tenant_id: String,
    user_id: String,
    planner_methodology_path: String,
    planner_conventions_path: String,
    web_client_url: String,
    discord_webhook_url: String,
}

impl From<settings_service::HostSettings> for GqlHostSettings {
    fn from(value: settings_service::HostSettings) -> Self {
        Self {
            worker_url: value.worker_url,
            tenant_id: value.tenant_id,
            user_id: value.user_id,
            planner_methodology_path: value.planner_methodology_path,
            planner_conventions_path: value.planner_conventions_path,
            web_client_url: value.web_client_url,
            discord_webhook_url: value.discord_webhook_url,
        }
    }
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "RelayConnectionStatus", rename_fields = "camelCase")]
struct GqlRelayConnectionStatus {
    connected: bool,
    session_id: Option<String>,
    last_heartbeat: Option<String>,
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "PromptLibraryEntry", rename_fields = "camelCase")]
struct GqlPromptLibraryEntry {
    id: String,
    key: String,
    name: String,
    role: String,
    scope: String,
    version: String,
    used_count: i32,
    customised: bool,
    read_only: bool,
    engine_reads: bool,
}

impl From<PromptLibraryEntry> for GqlPromptLibraryEntry {
    fn from(value: PromptLibraryEntry) -> Self {
        Self {
            id: value.id,
            key: value.key,
            name: value.name,
            role: value.role,
            scope: value.scope,
            version: value.version,
            used_count: value.used_count,
            customised: value.customised,
            read_only: value.read_only,
            engine_reads: value.engine_reads,
        }
    }
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "PlannerPromptSettings", rename_fields = "camelCase")]
struct GqlPlannerPromptSettings {
    schema: String,
    development: GqlPlannerDevelopmentPrompts,
    planning: GqlPlannerPlanningPrompts,
    small_mode: Option<GqlPlannerSmallModePrompts>,
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "PlannerSmallModePrompts", rename_fields = "camelCase")]
struct GqlPlannerSmallModePrompts {
    planner: String,
    reviewer: String,
    leaf_wrapper: String,
    amend_planner: String,
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "PlannerDevelopmentPrompts", rename_fields = "camelCase")]
struct GqlPlannerDevelopmentPrompts {
    worker: String,
    reviewer: String,
    worker_nudge: Option<String>,
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "PlannerPlanningPrompts", rename_fields = "camelCase")]
struct GqlPlannerPlanningPrompts {
    planner: String,
    reviewer: String,
    amend_planner: Option<String>,
    amend_reviewer: Option<String>,
}

impl From<PlannerPromptSettings> for GqlPlannerPromptSettings {
    fn from(value: PlannerPromptSettings) -> Self {
        Self {
            schema: value.schema,
            development: GqlPlannerDevelopmentPrompts {
                worker: value.development.worker,
                reviewer: value.development.reviewer,
                worker_nudge: value.development.worker_nudge,
            },
            planning: GqlPlannerPlanningPrompts {
                planner: value.planning.planner,
                reviewer: value.planning.reviewer,
                amend_planner: Some(value.planning.amend_planner),
                amend_reviewer: Some(value.planning.amend_reviewer),
            },
            small_mode: Some(GqlPlannerSmallModePrompts {
                planner: value.small_mode.planner,
                reviewer: value.small_mode.reviewer,
                leaf_wrapper: value.small_mode.leaf_wrapper,
                amend_planner: value.small_mode.amend_planner,
            }),
        }
    }
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "AiChatRunResult", rename_fields = "camelCase")]
struct AiChatRunResult {
    user_message: AiMessage,
    assistant_message: AiMessage,
}

impl From<chat_host::ChatRunResult> for AiChatRunResult {
    fn from(value: chat_host::ChatRunResult) -> Self {
        Self {
            user_message: value.user_message.into(),
            assistant_message: value.assistant_message.into(),
        }
    }
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "AiChatDelta", rename_fields = "camelCase")]
struct GqlAiChatDelta {
    session_id: String,
    message_id: String,
    delta: String,
    chunk_type: String,
    is_final: bool,
}

impl From<ChatDeltaEvent> for GqlAiChatDelta {
    fn from(value: ChatDeltaEvent) -> Self {
        Self {
            session_id: value.session_id,
            message_id: value.message_id,
            delta: value.delta,
            chunk_type: value.chunk_type,
            is_final: value.is_final,
        }
    }
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "AiChatComplete", rename_fields = "camelCase")]
struct GqlAiChatComplete {
    session_id: String,
    message_id: String,
}

impl From<ChatCompleteEvent> for GqlAiChatComplete {
    fn from(value: ChatCompleteEvent) -> Self {
        Self {
            session_id: value.session_id,
            message_id: value.message_id,
        }
    }
}

/// One persisted agent report. Mirrors the worker's `SessionReport` field-for-field.
#[derive(SimpleObject)]
#[graphql(name = "SessionReport", rename_fields = "camelCase")]
struct GqlSessionReport {
    id: String,
    session_id: String,
    kind: String,
    role: Option<String>,
    summary: Option<String>,
    markdown: Option<String>,
    created_at: String,
}

impl From<agent_plans::SessionReport> for GqlSessionReport {
    fn from(value: agent_plans::SessionReport) -> Self {
        Self {
            id: value.id,
            session_id: value.session_id,
            kind: value.kind,
            role: value.role,
            summary: value.summary,
            markdown: value.markdown,
            created_at: value.created_at,
        }
    }
}

/// An external tmux session a terminal can attach to. Mirrors the worker's
/// `TmuxSession` type field-for-field so one client query serves both surfaces.
#[derive(SimpleObject)]
#[graphql(name = "TmuxSession", rename_fields = "camelCase")]
struct GqlTmuxSession {
    name: String,
    attached: bool,
    windows: u32,
}

impl From<crate::terminal::ExternalTmuxSession> for GqlTmuxSession {
    fn from(value: crate::terminal::ExternalTmuxSession) -> Self {
        Self {
            name: value.name,
            attached: value.attached,
            windows: value.windows,
        }
    }
}

#[derive(InputObject)]
#[graphql(name = "CreateAiSessionInput", rename_fields = "camelCase")]
struct CreateAiSessionInput {
    title: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    working_directory: Option<String>,
    /// When set, the new session ATTACHES to this existing external tmux
    /// session instead of spawning a `johnnyone_<id>` pane. Closing it only
    /// detaches. Must mirror the worker input — the console's attach flow
    /// sends this field whichever surface the mutation lands on.
    tmux_session_name: Option<String>,
}

impl From<CreateAiSessionInput> for CreateSessionInput {
    fn from(value: CreateAiSessionInput) -> Self {
        Self {
            provider: value.provider,
            model: value.model,
            working_directory: value.working_directory,
            title: value.title,
            // All sessions created via the host's GraphQL surface are user
            // sessions. The planner / development coordinator creates its
            // agent sessions directly via `sessions::create_session` with
            // `kind: Some("agent")`.
            kind: None,
            // Setup commands are a planner/development (shell-worker) concern,
            // not exposed on the host AiSession surface.
            setup_commands: None,
            tmux_session_name: value.tmux_session_name,
        }
    }
}

#[derive(InputObject)]
#[graphql(name = "SendAiChatMessageInput", rename_fields = "camelCase")]
struct SendAiChatMessageInput {
    session_id: String,
    content: String,
}

#[derive(InputObject)]
#[graphql(name = "UpsertProviderConfigInput", rename_fields = "camelCase")]
struct UpsertProviderConfigInputGql {
    provider: String,
    cli_path: Option<String>,
    api_key: Option<String>,
    default_model: Option<String>,
    settings: Option<String>,
}

impl From<UpsertProviderConfigInputGql> for UpsertProviderConfigInput {
    fn from(value: UpsertProviderConfigInputGql) -> Self {
        Self {
            provider: value.provider,
            cli_path: value.cli_path,
            api_key: value.api_key,
            default_model: value.default_model,
            settings: value.settings,
        }
    }
}

#[derive(InputObject)]
#[graphql(name = "PlannerPromptSettingsInput", rename_fields = "camelCase")]
struct PlannerPromptSettingsInput {
    development: PlannerDevelopmentPromptsInput,
    planning: PlannerPlanningPromptsInput,
    small_mode: Option<PlannerSmallModePromptsInput>,
}

#[derive(InputObject)]
#[graphql(name = "PlannerSmallModePromptsInput", rename_fields = "camelCase")]
struct PlannerSmallModePromptsInput {
    planner: String,
    reviewer: String,
    leaf_wrapper: String,
    amend_planner: String,
}

#[derive(InputObject)]
#[graphql(name = "PlannerDevelopmentPromptsInput", rename_fields = "camelCase")]
struct PlannerDevelopmentPromptsInput {
    worker: String,
    reviewer: String,
    worker_nudge: Option<String>,
}

#[derive(InputObject)]
#[graphql(name = "PlannerPlanningPromptsInput", rename_fields = "camelCase")]
struct PlannerPlanningPromptsInput {
    planner: String,
    reviewer: String,
    amend_planner: Option<String>,
    amend_reviewer: Option<String>,
}



#[cfg(test)]
mod router_tests {
    //! End-to-end over the REAL schema + a real migrated SQLite DB, driven
    //! through `router()` with `oneshot`. This is what proves the resolver
    //! guards actually bite — the pure tests in `origin_guard` only cover the
    //! decision, not the wiring.

    use super::*;
    use crate::services::settings as settings_service;
    use crate::test_support::test_state;
    use axum::body::Body;
    use axum::http::{header::CONTENT_TYPE, header::ORIGIN, Method, Request, StatusCode};
    use tower::ServiceExt;

    /// The attacker's shape, as measured: a CORS-**simple** `text/plain` POST,
    /// which needs no preflight, from a page served on loopback.
    fn text_plain_post(origin: Option<&str>, sec_fetch_site: Option<&str>, query: &str) -> Request<Body> {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri("/graphql")
            .header(CONTENT_TYPE, "text/plain");
        if let Some(origin) = origin {
            builder = builder.header(ORIGIN, origin);
        }
        if let Some(site) = sec_fetch_site {
            builder = builder.header("sec-fetch-site", site);
        }
        let body = serde_json::json!({ "query": query }).to_string();
        builder.body(Body::from(body)).unwrap()
    }

    async fn body_text(res: axum::http::Response<Body>) -> String {
        let bytes = axum::body::to_bytes(res.into_body(), 256 * 1024).await.unwrap();
        String::from_utf8_lossy(&bytes).to_string()
    }

    #[tokio::test]
    async fn attacker_cannot_redirect_worker_url_and_steal_the_relay_token() {
        let (state, _root) = test_state();
        let probe = state.clone();
        let res = router(state)
            .oneshot(text_plain_post(
                Some("http://localhost:3000"),
                Some("cross-site"),
                r#"mutation{setSetting(key:"worker_url",value:"https://attacker")}"#,
            ))
            .await
            .unwrap();

        // The request itself is legal (loopback), so it is the RESOLVER that
        // must refuse it — hence a 200 carrying a GraphQL error, not a 403.
        assert_eq!(res.status(), StatusCode::OK);
        let text = body_text(res).await;
        assert!(text.contains("errors"), "expected a GraphQL error, got: {text}");
        assert!(!text.contains("\"setSetting\":true"), "write was not refused: {text}");

        // And the credential's destination is untouched.
        assert_ne!(
            settings_service::get_setting_or(&probe, settings_service::KEY_WORKER_URL, ""),
            "https://attacker"
        );
    }

    #[tokio::test]
    async fn attacker_cannot_write_the_access_token() {
        let (state, _root) = test_state();
        let probe = state.clone();
        let res = router(state)
            .oneshot(text_plain_post(
                Some("http://localhost:3000"),
                Some("cross-site"),
                r#"mutation{setSetting(key:"access_token",value:"jk_attacker")}"#,
            ))
            .await
            .unwrap();
        let text = body_text(res).await;
        assert!(text.contains("errors"), "expected a GraphQL error, got: {text}");
        assert_eq!(
            settings_service::get_setting_or(&probe, settings_service::KEY_ACCESS_TOKEN, ""),
            ""
        );
    }

    #[tokio::test]
    async fn the_webview_can_still_write_a_connection_key() {
        // Breaking the host-app's login / settings save would be worse than the
        // bug, so this is the test that keeps the fix honest.
        let (state, _root) = test_state();
        let probe = state.clone();
        let res = router(state)
            .oneshot(text_plain_post(
                Some("tauri://localhost"),
                None,
                r#"mutation{setSetting(key:"tenant_id",value:"11111111-1111-1111-1111-111111111111")}"#,
            ))
            .await
            .unwrap();
        let text = body_text(res).await;
        assert!(text.contains("\"setSetting\":true"), "webview write was refused: {text}");
        assert_eq!(
            settings_service::get_setting_or(&probe, settings_service::KEY_TENANT_ID, ""),
            "11111111-1111-1111-1111-111111111111"
        );
    }

    #[tokio::test]
    async fn a_bare_loopback_origin_may_still_write_a_non_connection_key() {
        // The demotion is scoped to connection keys; it must not break a local
        // tool writing an ordinary preference.
        let (state, _root) = test_state();
        let probe = state.clone();
        let res = router(state)
            .oneshot(text_plain_post(
                Some("http://localhost:3000"),
                Some("cross-site"),
                r#"mutation{setSetting(key:"discord_webhook_url",value:"https://discord.example/hook")}"#,
            ))
            .await
            .unwrap();
        let text = body_text(res).await;
        assert!(text.contains("\"setSetting\":true"), "non-connection write was refused: {text}");
        assert_eq!(
            settings_service::get_setting_or(&probe, settings_service::KEY_DISCORD_WEBHOOK_URL, ""),
            "https://discord.example/hook"
        );
    }

    #[tokio::test]
    async fn an_unknown_setting_key_is_refused_even_from_the_webview() {
        let (state, _root) = test_state();
        let res = router(state)
            .oneshot(text_plain_post(
                Some("tauri://localhost"),
                None,
                r#"mutation{setSetting(key:"attacker_planted_key",value:"x")}"#,
            ))
            .await
            .unwrap();
        let text = body_text(res).await;
        assert!(text.contains("errors"), "expected a GraphQL error, got: {text}");
    }

    #[tokio::test]
    async fn get_setting_still_refuses_the_token_through_the_real_schema() {
        let (state, _root) = test_state();
        settings_service::set_setting(
            &state,
            settings_service::KEY_ACCESS_TOKEN.to_string(),
            "jk_realsecret".to_string(),
        )
        .unwrap();
        let res = router(state)
            .oneshot(text_plain_post(
                Some("http://localhost:3000"),
                Some("cross-site"),
                r#"query{getSetting(key:"access_token")}"#,
            ))
            .await
            .unwrap();
        let text = body_text(res).await;
        assert!(!text.contains("jk_realsecret"), "token leaked: {text}");
        assert!(text.contains("errors"), "expected a GraphQL error, got: {text}");
    }

    // ── provider configs ──────────────────────────────────────────────────
    // `cli_path` BECOMES THE EXECUTED COMMAND (`providers/claude_code.rs:19`,
    // `ollama_cli.rs:14`, `cline.rs:11`), so a write here is local code
    // execution on the next agent run — strictly worse than the credential
    // theft the setting guards close, and reachable by the identical route.

    fn seed_provider(state: &AppState) {
        crate::services::providers::upsert_provider_config(
            state,
            crate::db::models::UpsertProviderConfigInput {
                provider: "claude_code".to_string(),
                cli_path: Some("/usr/bin/claude".to_string()),
                api_key: Some(String::new()),
                default_model: Some(String::new()),
                settings: Some("{}".to_string()),
            },
        )
        .unwrap();
    }

    fn provider_cli_path(state: &AppState, provider: &str) -> String {
        crate::services::providers::list_provider_configs(state)
            .unwrap()
            .into_iter()
            .find(|config| config.provider == provider)
            .map(|config| config.cli_path)
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn attacker_cannot_repoint_a_provider_cli_path() {
        let (state, _root) = test_state();
        seed_provider(&state);
        let probe = state.clone();
        let res = router(state)
            .oneshot(text_plain_post(
                Some("http://localhost:3000"),
                Some("cross-site"),
                r#"mutation{upsertProviderConfig(input:{provider:"claude_code",cliPath:"/tmp/evil.sh"}){id}}"#,
            ))
            .await
            .unwrap();
        let text = body_text(res).await;
        assert!(text.contains("errors"), "expected a GraphQL error, got: {text}");
        assert_eq!(
            provider_cli_path(&probe, "claude_code"),
            "/usr/bin/claude",
            "the executed command was repointed"
        );
    }

    #[tokio::test]
    async fn provider_config_writes_are_refused_even_from_the_webview() {
        // Unconditional on this surface, not merely tiered: nothing in either
        // built bundle writes provider configs over HTTP, so there is no
        // capability to preserve and no reason to leave the vector open to a
        // local process that omits `Origin`.
        let (state, _root) = test_state();
        seed_provider(&state);
        let probe = state.clone();
        for origin in [Some("tauri://localhost"), None] {
            let res = router(state.clone())
                .oneshot(text_plain_post(
                    origin,
                    None,
                    r#"mutation{upsertProviderConfig(input:{provider:"claude_code",cliPath:"/tmp/evil.sh"}){id}}"#,
                ))
                .await
                .unwrap();
            let text = body_text(res).await;
            assert!(
                text.contains("errors"),
                "origin {origin:?} was allowed to write a provider config: {text}"
            );
        }
        assert_eq!(provider_cli_path(&probe, "claude_code"), "/usr/bin/claude");
    }

    #[tokio::test]
    async fn deleting_a_provider_config_is_refused_on_the_http_surface() {
        let (state, _root) = test_state();
        seed_provider(&state);
        let probe = state.clone();
        let res = router(state)
            .oneshot(text_plain_post(
                Some("http://localhost:3000"),
                Some("cross-site"),
                r#"mutation{deleteProviderConfig(provider:"claude_code")}"#,
            ))
            .await
            .unwrap();
        let text = body_text(res).await;
        assert!(text.contains("errors"), "expected a GraphQL error, got: {text}");
        assert_eq!(
            provider_cli_path(&probe, "claude_code"),
            "/usr/bin/claude",
            "the provider row was deleted"
        );
    }

    #[tokio::test]
    async fn the_webview_can_still_read_provider_configs() {
        // The one thing the host-app actually needs from this surface, with the
        // exact field set its built bundle requests
        // (dist/host-app/browser/chunk-DJ5NYM3W.js).
        let (state, _root) = test_state();
        seed_provider(&state);
        let res = router(state)
            .oneshot(text_plain_post(
                Some("tauri://localhost"),
                None,
                "{ listProviderConfigs { provider cliPath isAvailable defaultModel } }",
            ))
            .await
            .unwrap();
        let text = body_text(res).await;
        assert!(!text.contains("errors"), "read was refused: {text}");
        assert!(text.contains("/usr/bin/claude"), "expected the config, got: {text}");
    }

    #[tokio::test]
    async fn the_relay_path_can_still_write_a_provider_config() {
        // The console's real route: worker -> WS relay -> `rpc_upsert_provider_config`
        // -> `provider_service::upsert_provider_config`, which never touches
        // `graphql_handler`. This is what keeps provider editing working.
        let (state, _root) = test_state();
        seed_provider(&state);
        crate::services::providers::upsert_provider_config(
            &state,
            crate::db::models::UpsertProviderConfigInput {
                provider: "claude_code".to_string(),
                cli_path: Some("/opt/claude".to_string()),
                api_key: None,
                default_model: None,
                settings: None,
            },
        )
        .unwrap();
        assert_eq!(provider_cli_path(&state, "claude_code"), "/opt/claude");
    }

    #[tokio::test]
    async fn the_playground_cannot_be_framed() {
        let (state, _root) = test_state();
        let res = router(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/graphql")
                    .header("sec-fetch-site", "none")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let headers = res.headers().clone();
        let res = res;
        assert_eq!(
            headers
                .get("content-security-policy")
                .map(|v| v.to_str().unwrap()),
            Some("frame-ancestors 'none'")
        );
        assert_eq!(
            headers.get("x-frame-options").map(|v| v.to_str().unwrap()),
            Some("DENY")
        );
        // And the playground itself still renders — the header tuple must not
        // have replaced the body.
        let text = body_text(res).await;
        assert!(
            text.contains("GraphQL") && text.len() > 500,
            "playground HTML missing, got {} bytes",
            text.len()
        );
    }
}
