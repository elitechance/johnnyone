use crate::db::models::{
    CreateSessionInput, Message, ProviderConfig, Session, UpsertProviderConfigInput,
};
use crate::events::{ChatCompleteEvent, ChatDeltaEvent};
use crate::services::{
    chat_host,
    planner_prompts::{self as planner_prompt_service, PlannerPromptSettings},
    providers::{self as provider_service, DetectedTool},
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
use tower_http::cors::{Any, CorsLayer};

type JohnnyHostSchema = Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

pub fn router(state: AppState) -> Router {
    let schema = Schema::build(QueryRoot, MutationRoot, SubscriptionRoot)
        .data(state)
        .finish();

    // CORS: the host's GraphQL is reached from the Tauri webview (any origin)
    // and from local dev tools (e.g. host-app at :4201). Permissive by design —
    // the host listens on localhost only, so external origins can't reach it
    // anyway. Methods/headers must include everything async-graphql sends.
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/graphql", get(graphql_playground).post(graphql_handler))
        .route_service("/graphql/ws", GraphQLSubscription::new(schema.clone()))
        .layer(cors)
        .with_state(schema)
}

async fn graphql_handler(
    AxumState(schema): AxumState<JohnnyHostSchema>,
    req: GraphQLRequest,
) -> GraphQLResponse {
    schema.execute(req.into_inner()).await.into()
}

async fn graphql_playground() -> impl IntoResponse {
    Html(playground_source(
        GraphQLPlaygroundConfig::new("/graphql").subscription_endpoint("/graphql/ws"),
    ))
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

    async fn get_setting(&self, ctx: &Context<'_>, key: String) -> async_graphql::Result<String> {
        let state = ctx.data_unchecked::<AppState>();
        Ok(settings_service::get_setting(state, key)?)
    }

    async fn get_planner_prompt_settings(&self) -> async_graphql::Result<GqlPlannerPromptSettings> {
        Ok(planner_prompt_service::load_prompt_settings()?.into())
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

    async fn upsert_provider_config(
        &self,
        ctx: &Context<'_>,
        input: UpsertProviderConfigInputGql,
    ) -> async_graphql::Result<GqlProviderConfig> {
        let state = ctx.data_unchecked::<AppState>();
        Ok(provider_service::upsert_provider_config(state, input.into())?.into())
    }

    async fn delete_provider_config(
        &self,
        ctx: &Context<'_>,
        provider: String,
    ) -> async_graphql::Result<bool> {
        let state = ctx.data_unchecked::<AppState>();
        provider_service::delete_provider_config(state, provider)?;
        Ok(true)
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

    async fn set_setting(
        &self,
        ctx: &Context<'_>,
        key: String,
        value: String,
    ) -> async_graphql::Result<bool> {
        let state = ctx.data_unchecked::<AppState>();
        settings_service::set_setting(state, key, value)?;
        Ok(true)
    }

    async fn set_planner_prompt_settings(
        &self,
        input: PlannerPromptSettingsInput,
    ) -> async_graphql::Result<GqlPlannerPromptSettings> {
        Ok(planner_prompt_service::save_prompt_settings(input.into())?.into())
    }
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
            api_key: value.api_key,
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
#[graphql(name = "PlannerPromptSettings", rename_fields = "camelCase")]
struct GqlPlannerPromptSettings {
    schema: String,
    development: GqlPlannerDevelopmentPrompts,
    planning: GqlPlannerPlanningPrompts,
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "PlannerDevelopmentPrompts", rename_fields = "camelCase")]
struct GqlPlannerDevelopmentPrompts {
    worker: String,
    reviewer: String,
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "PlannerPlanningPrompts", rename_fields = "camelCase")]
struct GqlPlannerPlanningPrompts {
    planner: String,
    reviewer: String,
}

impl From<PlannerPromptSettings> for GqlPlannerPromptSettings {
    fn from(value: PlannerPromptSettings) -> Self {
        Self {
            schema: value.schema,
            development: GqlPlannerDevelopmentPrompts {
                worker: value.development.worker,
                reviewer: value.development.reviewer,
            },
            planning: GqlPlannerPlanningPrompts {
                planner: value.planning.planner,
                reviewer: value.planning.reviewer,
            },
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

#[derive(InputObject)]
#[graphql(name = "CreateAiSessionInput", rename_fields = "camelCase")]
struct CreateAiSessionInput {
    title: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    working_directory: Option<String>,
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
}

#[derive(InputObject)]
#[graphql(name = "PlannerDevelopmentPromptsInput", rename_fields = "camelCase")]
struct PlannerDevelopmentPromptsInput {
    worker: String,
    reviewer: String,
}

#[derive(InputObject)]
#[graphql(name = "PlannerPlanningPromptsInput", rename_fields = "camelCase")]
struct PlannerPlanningPromptsInput {
    planner: String,
    reviewer: String,
}

impl From<PlannerPromptSettingsInput> for PlannerPromptSettings {
    fn from(value: PlannerPromptSettingsInput) -> Self {
        // Build a default so we have the up-to-date amend templates; then
        // overlay the input's worker/reviewer/planner/reviewer onto it. This
        // lets older clients that don't know about amend_planner/amend_reviewer
        // still update prompts without losing the defaults.
        let mut prompts = PlannerPromptSettings::default();
        prompts.schema = String::new();
        prompts.development.worker = value.development.worker;
        prompts.development.reviewer = value.development.reviewer;
        prompts.planning.planner = value.planning.planner;
        prompts.planning.reviewer = value.planning.reviewer;
        prompts
    }
}
