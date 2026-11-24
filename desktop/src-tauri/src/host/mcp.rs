//! `POST 127.0.0.1:7788/mcp` — the plan agent's MCP endpoint (Plans tab, initiative 8ad3ae71 phase
//! 02 tasks 02-03; master §5.3, D14; brief §"MCP security" 2, 3, 5). Streamable HTTP, JSON response
//! mode.
//!
//! This is the first AUTHENTICATED route on the otherwise unauthenticated `:7788` host, so it does
//! NOT inherit that surface's trust or its CORS:
//!
//! 1. **Transport guard** ([`check_request`], pure, before any parsing): method must be POST (405,
//!    `Allow: POST`); ANY `Origin` header is refused (403 — agents never send one, browsers always
//!    do; `origin_guard` deliberately allows loopback origins, which is exactly wrong here); `Host`
//!    must be `127.0.0.1:<port>` or `localhost:<port>` (421 — DNS-rebinding defence);
//!    `Content-Type` must be `application/json` (415); body ≤ 256 KiB (413, also enforced on a
//!    chunked body by `RequestBodyLimitLayer` + a bounded read).
//! 2. **No CORS.** [`mcp_router`] is MERGED next to `origin_guard::guard_layers(..)`, not inside it,
//!    so no `access-control-*` header is ever emitted on `/mcp` (an OPTIONS preflight gets 405).
//! 3. **Auth on every request**, including `initialize` ([`handle_mcp`]): `Authorization: Bearer
//!    <token>` → `mcp_tokens::verify` → the token's session must exist, be `active`, and still be
//!    the plan's `briefing_session_id` → 401 `-32001 "unauthorized"` (one undistinguished message;
//!    the precise reason goes to the audit only); the plan must be in `briefing` → 403 `-32003`
//!    with the frozen version.
//! 4. **Scope from the token.** The resulting `TokenScope` is passed to the tool; no code path reads
//!    an initiative id from the request body.
//!
//! Every refusal is audited through `plan_mcp::record_rejection` (task 06).

use crate::services::brief_store;
use crate::services::mcp_tokens::{self, Reject, TokenScope};
use crate::services::plan_mcp::{self, ToolOutcome};
use crate::state::app_state::AppState;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Map, Value};
use tower_http::limit::RequestBodyLimitLayer;

/// Max request body (master §5.3).
pub const MAX_BODY_BYTES: usize = 256 * 1024;

/// MCP protocol revisions this server speaks (JSON response mode is the same in both).
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26"];

/// JSON-RPC error codes.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const TRANSPORT_ERROR: i64 = -32000;
const UNAUTHORIZED: i64 = -32001;
const FROZEN: i64 = -32003;

// ───────────────────────────── transport guard (task 02) ─────────────────────────────

/// Why the transport guard refused a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportReject {
    MethodNotAllowed,
    OriginPresent,
    BadHost,
    UnsupportedMediaType,
    TooLarge,
}

impl TransportReject {
    pub fn status(self) -> StatusCode {
        match self {
            TransportReject::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            TransportReject::OriginPresent => StatusCode::FORBIDDEN,
            TransportReject::BadHost => StatusCode::MISDIRECTED_REQUEST,
            TransportReject::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            TransportReject::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        }
    }

    /// Fixed audit reason (also the JSON-RPC error message).
    pub fn reason(self) -> &'static str {
        match self {
            TransportReject::MethodNotAllowed => "method_not_allowed",
            TransportReject::OriginPresent => "origin_present",
            TransportReject::BadHost => "bad_host",
            TransportReject::UnsupportedMediaType => "unsupported_media_type",
            TransportReject::TooLarge => "body_too_large",
        }
    }
}

/// Is `host` exactly `127.0.0.1:<port>` or `localhost:<port>` (host case-insensitive, exact port)?
/// The rule lives in `origin_guard::host_matches`, shared with the GraphQL router.
fn host_allowed(host: &str, bound_port: u16) -> bool {
    super::origin_guard::host_matches(host, bound_port, super::origin_guard::MCP_HOST_NAMES)
}

/// The pure transport check, in the order of master §5.3 / task 02.
pub fn check_request(
    method: &Method,
    headers: &HeaderMap,
    content_length: Option<u64>,
    bound_port: u16,
) -> Result<(), TransportReject> {
    if method != Method::POST {
        return Err(TransportReject::MethodNotAllowed);
    }
    if headers.contains_key(header::ORIGIN) {
        return Err(TransportReject::OriginPresent);
    }
    let host_ok = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(|h| host_allowed(h.trim(), bound_port))
        .unwrap_or(false);
    if !host_ok {
        return Err(TransportReject::BadHost);
    }
    let json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("application/json"))
        .unwrap_or(false);
    if !json {
        return Err(TransportReject::UnsupportedMediaType);
    }
    if content_length.is_some_and(|n| n > MAX_BODY_BYTES as u64) {
        return Err(TransportReject::TooLarge);
    }
    Ok(())
}

#[derive(Clone)]
struct McpRouterState {
    app: AppState,
    port: u16,
}

/// The `/mcp` sub-router. Merged by `host::router` OUTSIDE `origin_guard::guard_layers`, so none of
/// that surface's CORS applies here. `any(..)` so a non-POST reaches the guard (and gets a JSON 405
/// with `Allow: POST`) instead of axum's default.
pub fn mcp_router(state: AppState) -> Router {
    mcp_router_on_port(state, mcp_tokens::host_port())
}

fn mcp_router_on_port(state: AppState, port: u16) -> Router {
    Router::new()
        .route("/mcp", any(mcp_endpoint))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .with_state(McpRouterState { app: state, port })
}

fn transport_response(state: &AppState, r: TransportReject) -> Response {
    plan_mcp::record_rejection(state, None, r.reason(), "-", None);
    let body = json!({ "jsonrpc": "2.0", "id": null, "error": { "code": TRANSPORT_ERROR, "message": r.reason() } });
    let mut res = (r.status(), axum::Json(body)).into_response();
    if r == TransportReject::MethodNotAllowed {
        res.headers_mut().insert(header::ALLOW, HeaderValue::from_static("POST"));
    }
    res
}

async fn mcp_endpoint(State(st): State<McpRouterState>, method: Method, headers: HeaderMap, body: Body) -> Response {
    let content_length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    if let Err(r) = check_request(&method, &headers, content_length, st.port) {
        return transport_response(&st.app, r);
    }
    // Bounded read: a chunked body with no Content-Length is cut off here (and by the layer).
    let Ok(bytes) = axum::body::to_bytes(body, MAX_BODY_BYTES).await else {
        return transport_response(&st.app, TransportReject::TooLarge);
    };
    let auth = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok());
    let reply = handle_mcp(&st.app, auth, &bytes).await;
    let status = StatusCode::from_u16(reply.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut res = match reply.body {
        Some(v) => (status, axum::Json(v)).into_response(),
        None => status.into_response(),
    };
    if status == StatusCode::UNAUTHORIZED {
        res.headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    }
    res
}

// ───────────────────────────── JSON-RPC core (task 03) ─────────────────────────────

/// What [`handle_mcp`] answers: an HTTP status and an optional JSON body (`None` = 202 Accepted).
#[derive(Debug, Clone, PartialEq)]
pub struct McpReply {
    pub status: u16,
    pub body: Option<Value>,
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn rpc_result(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn reply(status: u16, body: Value) -> McpReply {
    McpReply { status, body: Some(body) }
}

/// Why auth refused, with the plan to attribute it to when there is one.
struct AuthFail {
    initiative_id: Option<String>,
    reason: &'static str,
    /// `Some(version)` → 403 frozen; `None` → 401.
    frozen_at: Option<i64>,
}

impl AuthFail {
    fn planless(reason: &'static str) -> Self {
        AuthFail { initiative_id: None, reason, frozen_at: None }
    }
    fn on(plan: &str, reason: &'static str) -> Self {
        AuthFail { initiative_id: Some(plan.to_string()), reason, frozen_at: None }
    }
}

/// Bearer → token → live, current session → briefing plan.
fn authorize(state: &AppState, authorization: Option<&str>) -> Result<TokenScope, AuthFail> {
    let Some(header) = authorization.map(str::trim).filter(|h| !h.is_empty()) else {
        return Err(AuthFail::planless("missing_token"));
    };
    let token = match header.split_once(' ') {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("bearer") => rest.trim(),
        _ => return Err(AuthFail::planless("malformed_token")),
    };
    let scope = match mcp_tokens::verify(state, token) {
        Ok(s) => s,
        Err(Reject::Revoked { initiative_id }) => {
            return Err(AuthFail { initiative_id: Some(initiative_id), reason: "revoked", frozen_at: None })
        }
        Err(r) => {
            return Err(AuthFail::planless(match r {
                Reject::Malformed => "malformed_token",
                _ => "unknown_token",
            }))
        }
    };
    if scope.role != mcp_tokens::ROLE_PLAN_AGENT {
        return Err(AuthFail::on(&scope.initiative_id, "wrong_role"));
    }
    type PlanRow = (String, String, Option<String>, Option<i64>);
    let plan: Option<PlanRow> = state
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT initiative_status, status, briefing_session_id, accepted_brief_version FROM agent_plans WHERE id = ?1",
                params![scope.initiative_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|e| e.to_string())
        })
        .unwrap_or(None);
    let Some((initiative_status, plan_status, briefing_session_id, accepted)) = plan else {
        return Err(AuthFail::planless("plan_missing"));
    };
    let session_status: Option<String> = state
        .db
        .with_conn(|conn| {
            conn.query_row("SELECT status FROM sessions WHERE id = ?1", params![scope.session_id], |r| r.get(0))
                .optional()
                .map_err(|e| e.to_string())
        })
        .unwrap_or(None);
    if session_status.as_deref() != Some("active") {
        return Err(AuthFail::on(&scope.initiative_id, "session_inactive"));
    }
    if briefing_session_id.as_deref() != Some(scope.session_id.as_str()) {
        return Err(AuthFail::on(&scope.initiative_id, "session_replaced"));
    }
    if plan_status == "closed" {
        return Err(AuthFail::on(&scope.initiative_id, "plan_closed"));
    }
    if initiative_status != "briefing" {
        let latest = brief_store::version_count(state, &scope.initiative_id).unwrap_or(0);
        return Err(AuthFail {
            initiative_id: Some(scope.initiative_id.clone()),
            reason: "not_briefing",
            frozen_at: Some(accepted.unwrap_or(latest)),
        });
    }
    Ok(scope)
}

/// The whole protocol, minus HTTP: the axum handler only extracts the `Authorization` header and
/// the body and calls this, so every case is testable in-process.
pub async fn handle_mcp(state: &AppState, authorization: Option<&str>, body: &[u8]) -> McpReply {
    let null = Value::Null;
    let msg: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => {
            plan_mcp::record_rejection(state, None, "parse_error", "-", None);
            return reply(400, rpc_error(&null, PARSE_ERROR, "parse error"));
        }
    };
    let Some(obj) = msg.as_object() else {
        let what = if msg.is_array() { "batch" } else { "invalid_request" };
        plan_mcp::record_rejection(state, None, what, "-", None);
        let text = if msg.is_array() { "batch requests are not supported" } else { "invalid request" };
        return reply(400, rpc_error(&null, INVALID_REQUEST, text));
    };
    let id = obj.get("id").cloned().unwrap_or(Value::Null);
    let method = obj.get("method").and_then(Value::as_str).unwrap_or("");

    // Auth BEFORE any dispatch, for every method.
    let scope = match authorize(state, authorization) {
        Ok(s) => s,
        Err(fail) => {
            plan_mcp::record_rejection(state, fail.initiative_id.as_deref(), fail.reason, method, None);
            return match fail.frozen_at {
                Some(v) => reply(403, rpc_error(&id, FROZEN, &brief_store::frozen_error(v))),
                None => reply(401, rpc_error(&id, UNAUTHORIZED, "unauthorized")),
            };
        }
    };

    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") || method.is_empty() {
        plan_mcp::record_rejection(state, Some(&scope.initiative_id), "invalid_request", method, None);
        return reply(400, rpc_error(&id, INVALID_REQUEST, "invalid request"));
    }
    // Notifications (incl. `notifications/initialized`) get 202 with no body.
    if method.starts_with("notifications/") || !obj.contains_key("id") {
        return McpReply { status: 202, body: None };
    }
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "initialize" => reply(200, rpc_result(&id, initialize_result(&params))),
        "ping" => reply(200, rpc_result(&id, json!({}))),
        "tools/list" => reply(200, rpc_result(&id, plan_mcp::tools_list())),
        "tools/call" => tools_call(state, &scope, &id, &params).await,
        other => {
            plan_mcp::record_rejection(state, Some(&scope.initiative_id), "unknown_method", other, None);
            reply(200, rpc_error(&id, METHOD_NOT_FOUND, &format!("method not found: {}", other)))
        }
    }
}

fn initialize_result(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked
        .filter(|v| SUPPORTED_PROTOCOL_VERSIONS.contains(v))
        .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "j1", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "These tools read and edit THIS plan's brief and draft outline only; repo_search is read-only.",
    })
}

async fn tools_call(state: &AppState, scope: &TokenScope, id: &Value, params: &Value) -> McpReply {
    let invalid = |tool: Option<&str>, reason: &'static str, msg: String| {
        plan_mcp::record_rejection(state, Some(&scope.initiative_id), reason, "tools/call", tool);
        reply(200, rpc_error(id, INVALID_PARAMS, &msg))
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return invalid(None, "invalid_arguments", "params.name must be a tool name".to_string());
    };
    let Some(tool) = plan_mcp::find_tool(name) else {
        return invalid(Some(name), "unknown_tool", format!("unknown tool: {}", name));
    };
    let args = match params.get("arguments") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(v) => v.clone(),
    };
    if let Err(e) = plan_mcp::validate(&(tool.input_schema)(), &args) {
        return invalid(Some(tool.name), "invalid_arguments", e);
    }
    let Value::Object(args) = args else {
        return invalid(Some(tool.name), "invalid_arguments", "arguments must be an object".to_string());
    };

    // Tools touch SQLite and (repo_search) the filesystem: run off the async workers, carrying the
    // caller's tracing dispatcher so log capture (and per-request spans) follow the work.
    let dispatch = tracing::dispatcher::get_default(|d| d.clone());
    let (st, sc, run) = (state.clone(), scope.clone(), tool.run);
    let outcome = tokio::task::spawn_blocking(move || {
        tracing::dispatcher::with_default(&dispatch, || run(&st, &sc, &args))
    })
    .await
    .unwrap_or_else(|e| ToolOutcome::Error(format!("tool failed: {}", e)));

    match outcome {
        ToolOutcome::Ok(v) => reply(
            200,
            rpc_result(
                id,
                json!({ "content": [{ "type": "text", "text": v.to_string() }], "structuredContent": v, "isError": false }),
            ),
        ),
        ToolOutcome::Error(msg) => reply(
            200,
            rpc_result(
                id,
                json!({ "content": [{ "type": "text", "text": msg }], "structuredContent": { "error": msg }, "isError": true }),
            ),
        ),
        ToolOutcome::InvalidArgs(msg) => invalid(Some(tool.name), "invalid_arguments", msg),
    }
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::plan_mcp::fixture::{self, events, plan, use_store, version_count, Plan};
    use crate::test_support::test_state;
    use axum::http::Request;
    use tower::ServiceExt;

    // ── helpers ──

    /// Rejection tests assert that nothing was written.
    fn assert_no_new_version(state: &AppState, id: &str, before: i64) {
        assert_eq!(version_count(state, id), before, "a rejected call must not write a brief version");
    }

    fn rpc(method: &str, params: Value) -> Vec<u8> {
        json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params }).to_string().into_bytes()
    }

    fn write_call() -> Vec<u8> {
        rpc("tools/call", json!({ "name": "brief_update", "arguments": { "field": "why", "add": "one more reason" } }))
    }

    async fn plan_with_state() -> (AppState, std::path::PathBuf, Plan) {
        let (state, root) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        (state, root, p)
    }

    fn assert_401(r: &McpReply) {
        assert_eq!(r.status, 401, "{:?}", r.body);
        let b = r.body.as_ref().unwrap();
        assert_eq!(b["error"]["code"], UNAUTHORIZED);
        assert_eq!(b["error"]["message"], "unauthorized", "one undistinguished message");
    }

    fn last_rejection(state: &AppState, id: &str) -> Value {
        events(state, id, "agent_mcp_rejected").pop().expect("a rejection event")
    }

    fn req(method: Method, host: Option<&str>, ct: Option<&str>, origin: Option<&str>, body: Body) -> Request<Body> {
        let mut b = Request::builder().method(method).uri("/mcp");
        if let Some(h) = host {
            b = b.header(header::HOST, h);
        }
        if let Some(c) = ct {
            b = b.header(header::CONTENT_TYPE, c);
        }
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        b.body(body).unwrap()
    }

    fn good(body: &[u8]) -> Request<Body> {
        req(Method::POST, Some("127.0.0.1:7788"), Some("application/json"), None, Body::from(body.to_vec()))
    }

    async fn send(state: &AppState, r: Request<Body>) -> Response {
        crate::host::router(state.clone()).oneshot(r).await.unwrap()
    }

    fn assert_no_cors(res: &Response) {
        for (k, _) in res.headers() {
            assert!(!k.as_str().starts_with("access-control-"), "CORS header {k} on /mcp");
        }
    }

    // ── task 02: transport guard ──

    #[test]
    fn check_request_table() {
        let h = |pairs: &[(&str, &str)]| {
            let mut m = HeaderMap::new();
            for (k, v) in pairs {
                m.insert(header::HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
            }
            m
        };
        let ok = [("host", "127.0.0.1:7788"), ("content-type", "application/json")];
        assert_eq!(check_request(&Method::POST, &h(&ok), Some(10), 7788), Ok(()));
        assert_eq!(check_request(&Method::POST, &h(&[("host", "LOCALHOST:7788"), ("content-type", "application/json; charset=utf-8")]), None, 7788), Ok(()));
        let cases: Vec<(Method, Vec<(&str, &str)>, Option<u64>, TransportReject)> = vec![
            (Method::GET, ok.to_vec(), None, TransportReject::MethodNotAllowed),
            (Method::OPTIONS, ok.to_vec(), None, TransportReject::MethodNotAllowed),
            (Method::PUT, ok.to_vec(), None, TransportReject::MethodNotAllowed),
            (Method::POST, [&ok[..], &[("origin", "http://127.0.0.1:7788")]].concat(), None, TransportReject::OriginPresent),
            (Method::POST, [&ok[..], &[("origin", "null")]].concat(), None, TransportReject::OriginPresent),
            (Method::POST, [&ok[..], &[("origin", "https://evil.example")]].concat(), None, TransportReject::OriginPresent),
            (Method::POST, vec![("host", "127.0.0.1:7788.evil.com"), ("content-type", "application/json")], None, TransportReject::BadHost),
            (Method::POST, vec![("host", "localhost:80"), ("content-type", "application/json")], None, TransportReject::BadHost),
            (Method::POST, vec![("host", "127.0.0.1"), ("content-type", "application/json")], None, TransportReject::BadHost),
            (Method::POST, vec![("host", "evil.example:7788"), ("content-type", "application/json")], None, TransportReject::BadHost),
            (Method::POST, vec![("host", "0.0.0.0:7788"), ("content-type", "application/json")], None, TransportReject::BadHost),
            (Method::POST, vec![("content-type", "application/json")], None, TransportReject::BadHost),
            (Method::POST, vec![("host", "127.0.0.1:7788")], None, TransportReject::UnsupportedMediaType),
            (Method::POST, vec![("host", "127.0.0.1:7788"), ("content-type", "text/plain")], None, TransportReject::UnsupportedMediaType),
            (Method::POST, vec![("host", "127.0.0.1:7788"), ("content-type", "application/jsonx")], None, TransportReject::UnsupportedMediaType),
            (Method::POST, ok.to_vec(), Some(MAX_BODY_BYTES as u64 + 1), TransportReject::TooLarge),
        ];
        for (m, hs, cl, want) in cases {
            assert_eq!(check_request(&m, &h(&hs), cl, 7788), Err(want), "{m} {hs:?} {cl:?}");
        }
        assert_eq!(check_request(&Method::POST, &h(&ok), Some(MAX_BODY_BYTES as u64), 7788), Ok(()));
        // Ordering: method first, then Origin.
        assert_eq!(
            check_request(&Method::GET, &h(&[("origin", "https://x")]), None, 7788),
            Err(TransportReject::MethodNotAllowed)
        );
        assert_eq!(check_request(&Method::POST, &h(&ok), None, 9999), Err(TransportReject::BadHost));
    }

    #[tokio::test]
    async fn origin_header_rejected() {
        let (state, _r, p) = plan_with_state().await;
        let before = version_count(&state, &p.id);
        for origin in ["http://127.0.0.1:7788", "null", "https://evil.example", "tauri://localhost"] {
            let mut r = req(Method::POST, Some("127.0.0.1:7788"), Some("application/json"), Some(origin), Body::from(write_call()));
            r.headers_mut().insert(header::AUTHORIZATION, HeaderValue::from_str(&p.bearer()).unwrap());
            let res = send(&state, r).await;
            assert_eq!(res.status(), StatusCode::FORBIDDEN, "{origin}");
            assert_no_cors(&res);
        }
        assert_no_new_version(&state, &p.id, before);
    }

    #[tokio::test]
    async fn bad_host_rejected() {
        let (state, _r, p) = plan_with_state().await;
        let before = version_count(&state, &p.id);
        for host in [Some("127.0.0.1:7788.evil.com"), Some("localhost:80"), Some("rebind.evil.example:7788"), None] {
            let mut r = req(Method::POST, host, Some("application/json"), None, Body::from(write_call()));
            r.headers_mut().insert(header::AUTHORIZATION, HeaderValue::from_str(&p.bearer()).unwrap());
            let res = send(&state, r).await;
            assert_eq!(res.status(), StatusCode::MISDIRECTED_REQUEST, "{host:?}");
        }
        assert_no_new_version(&state, &p.id, before);
    }

    #[tokio::test]
    async fn non_post_rejected() {
        let (state, _r) = test_state();
        for m in [Method::GET, Method::PUT, Method::DELETE, Method::PATCH] {
            let res = send(&state, req(m.clone(), Some("127.0.0.1:7788"), Some("application/json"), None, Body::empty())).await;
            assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED, "{m}");
            assert_eq!(res.headers().get(header::ALLOW).unwrap(), "POST");
            let body = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
            let v: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(v["error"]["message"], "method_not_allowed");
        }
    }

    #[tokio::test]
    async fn options_preflight_rejected_without_cors() {
        let (state, _r) = test_state();
        let mut r = req(Method::OPTIONS, Some("127.0.0.1:7788"), None, Some("http://127.0.0.1:7788"), Body::empty());
        r.headers_mut().insert("access-control-request-method", HeaderValue::from_static("POST"));
        r.headers_mut().insert("access-control-request-headers", HeaderValue::from_static("authorization,content-type"));
        let res = send(&state, r).await;
        assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_no_cors(&res);
    }

    #[tokio::test]
    async fn non_json_rejected() {
        let (state, _r, p) = plan_with_state().await;
        let before = version_count(&state, &p.id);
        for ct in [Some("text/plain"), Some("application/x-www-form-urlencoded"), Some("multipart/form-data"), None] {
            let mut r = req(Method::POST, Some("127.0.0.1:7788"), ct, None, Body::from(write_call()));
            r.headers_mut().insert(header::AUTHORIZATION, HeaderValue::from_str(&p.bearer()).unwrap());
            let res = send(&state, r).await;
            assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE, "{ct:?}");
        }
        assert_no_new_version(&state, &p.id, before);
    }

    #[tokio::test]
    async fn oversized_body_rejected() {
        let (state, _r, p) = plan_with_state().await;
        let before = version_count(&state, &p.id);
        let big = vec![b' '; 300 * 1024];
        // With Content-Length.
        let mut r = good(&big);
        r.headers_mut().insert(header::CONTENT_LENGTH, HeaderValue::from(big.len()));
        r.headers_mut().insert(header::AUTHORIZATION, HeaderValue::from_str(&p.bearer()).unwrap());
        let res = send(&state, r).await;
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_no_cors(&res);
        // Without Content-Length (a chunked stream).
        let chunks: Vec<Result<axum::body::Bytes, std::io::Error>> =
            (0..30).map(|_| Ok(axum::body::Bytes::from(vec![b' '; 10 * 1024]))).collect();
        let mut r = req(
            Method::POST,
            Some("127.0.0.1:7788"),
            Some("application/json"),
            None,
            Body::from_stream(futures_util::stream::iter(chunks)),
        );
        assert!(r.headers().get(header::CONTENT_LENGTH).is_none());
        r.headers_mut().insert(header::AUTHORIZATION, HeaderValue::from_str(&p.bearer()).unwrap());
        let res = send(&state, r).await;
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_no_new_version(&state, &p.id, before);
    }

    #[tokio::test]
    async fn no_cors_headers_on_mcp() {
        let (state, _r, p) = plan_with_state().await;
        let mut ok = good(&rpc("ping", json!({})));
        ok.headers_mut().insert(header::AUTHORIZATION, HeaderValue::from_str(&p.bearer()).unwrap());
        let mut reqs = vec![
            ok,
            good(&rpc("ping", json!({}))),
            req(Method::GET, Some("127.0.0.1:7788"), None, Some("http://127.0.0.1:7788"), Body::empty()),
            req(Method::POST, Some("127.0.0.1:7788"), Some("application/json"), Some("http://localhost:7788"), Body::from("{}")),
            req(Method::POST, Some("evil:7788"), Some("application/json"), None, Body::from("{}")),
            req(Method::POST, Some("127.0.0.1:7788"), Some("text/plain"), None, Body::from("{}")),
            req(Method::OPTIONS, Some("127.0.0.1:7788"), None, Some("tauri://localhost"), Body::empty()),
        ];
        let statuses = [200, 401, 405, 403, 421, 415, 405];
        for (r, want) in reqs.drain(..).zip(statuses) {
            let res = send(&state, r).await;
            assert_eq!(res.status().as_u16(), want);
            assert_no_cors(&res);
        }
    }

    #[tokio::test]
    async fn graphql_route_unchanged() {
        let (state, _r) = test_state();
        // An agent's origin-less JSON POST still reaches GraphQL…
        let r = Request::builder()
            .method(Method::POST)
            .uri("/graphql")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "query": "{ health }" }).to_string()))
            .unwrap();
        let res = send(&state, r).await;
        assert_eq!(res.status(), StatusCode::OK);
        let v: Value = serde_json::from_slice(&axum::body::to_bytes(res.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(v["data"]["health"], true);
        // …a hostile origin is still refused by origin_guard…
        let r = Request::builder()
            .method(Method::POST)
            .uri("/graphql")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ORIGIN, "https://evil.example")
            .body(Body::from(json!({ "query": "{ health }" }).to_string()))
            .unwrap();
        assert_eq!(send(&state, r).await.status(), StatusCode::FORBIDDEN);
        // …and the webview origin still gets its CORS echo (the policy /mcp must NOT inherit).
        let r = Request::builder()
            .method(Method::POST)
            .uri("/graphql")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ORIGIN, "tauri://localhost")
            .body(Body::from(json!({ "query": "{ health }" }).to_string()))
            .unwrap();
        let res = send(&state, r).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(res.headers().contains_key("access-control-allow-origin"));
    }

    // ── task 03: auth + protocol ──

    #[tokio::test]
    async fn missing_token_401() {
        let (state, _r, p) = plan_with_state().await;
        let before = version_count(&state, &p.id);
        for auth in [None, Some(""), Some("   ")] {
            assert_401(&handle_mcp(&state, auth, &write_call()).await);
        }
        assert_no_new_version(&state, &p.id, before);
        // Through HTTP too, with the WWW-Authenticate hint.
        let res = send(&state, good(&write_call())).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(res.headers().get(header::WWW_AUTHENTICATE).unwrap(), "Bearer");
        assert_no_new_version(&state, &p.id, before);
    }

    #[tokio::test]
    async fn malformed_token_401() {
        let (state, _r, p) = plan_with_state().await;
        let before = version_count(&state, &p.id);
        let basic = format!("Basic {}", p.token);
        let raw_only = p.token.clone();
        let short = format!("Bearer {}", &p.token[..63]);
        for auth in ["Bearer abc", "Bearer", "Bearer ", basic.as_str(), raw_only.as_str(), short.as_str(), "Token xyz"] {
            assert_401(&handle_mcp(&state, Some(auth), &write_call()).await);
        }
        assert_no_new_version(&state, &p.id, before);
    }

    #[tokio::test]
    async fn unknown_token_401() {
        let (state, _r, p) = plan_with_state().await;
        let before = version_count(&state, &p.id);
        let unknown = format!("Bearer {}", "0123456789abcdef".repeat(4));
        assert_401(&handle_mcp(&state, Some(&unknown), &write_call()).await);
        assert_no_new_version(&state, &p.id, before);
    }

    #[tokio::test]
    async fn revoked_token_401() {
        let (state, _r, p) = plan_with_state().await;
        let before = version_count(&state, &p.id);
        mcp_tokens::revoke_for_initiative(&state, &p.id, "test").unwrap();
        assert_401(&handle_mcp(&state, Some(&p.bearer()), &write_call()).await);
        assert_no_new_version(&state, &p.id, before);
        assert_eq!(last_rejection(&state, &p.id)["reason"], "revoked");
    }

    #[tokio::test]
    async fn archived_session_rejected() {
        let (state, _r, p) = plan_with_state().await;
        let before = version_count(&state, &p.id);
        crate::services::sessions::mark_session_archived(&state, &p.session).unwrap();
        for body in [write_call(), rpc("initialize", json!({})), rpc("tools/list", json!({}))] {
            assert_401(&handle_mcp(&state, Some(&p.bearer()), &body).await);
        }
        assert_no_new_version(&state, &p.id, before);
        assert_eq!(last_rejection(&state, &p.id)["reason"], "session_inactive");
    }

    #[tokio::test]
    async fn deleted_session_rejected() {
        let (state, _r, p) = plan_with_state().await;
        let before = version_count(&state, &p.id);
        state
            .db
            .with_conn(|c| c.execute("DELETE FROM sessions WHERE id = ?1", params![p.session]).map_err(|e| e.to_string()))
            .unwrap();
        assert_401(&handle_mcp(&state, Some(&p.bearer()), &write_call()).await);
        assert_no_new_version(&state, &p.id, before);
        assert_eq!(last_rejection(&state, &p.id)["reason"], "session_inactive");
    }

    #[tokio::test]
    async fn replaced_session_rejected() {
        let (state, _r, p) = plan_with_state().await;
        let before = version_count(&state, &p.id);
        state
            .db
            .with_conn(|c| {
                c.execute("UPDATE agent_plans SET briefing_session_id = 'someone-else' WHERE id = ?1", params![p.id])
                    .map_err(|e| e.to_string())
            })
            .unwrap();
        assert_401(&handle_mcp(&state, Some(&p.bearer()), &write_call()).await);
        assert_no_new_version(&state, &p.id, before);
        assert_eq!(last_rejection(&state, &p.id)["reason"], "session_replaced");
    }

    #[tokio::test]
    async fn call_after_accept_rejected() {
        let (state, _r, p) = plan_with_state().await;
        // One agent write, then Start planning flips the row (the DB half of accept: even a token
        // that somehow survived the revoke cannot write a frozen brief).
        let r = handle_mcp(&state, Some(&p.bearer()), &write_call()).await;
        assert_eq!(r.body.as_ref().unwrap()["result"]["isError"], json!(false));
        let frozen = version_count(&state, &p.id);
        state
            .db
            .with_conn(|c| crate::services::agent_plans::apply_brief_acceptance_at(c, &p.id, "brief", Some(frozen)))
            .unwrap();
        for body in [write_call(), rpc("tools/list", json!({})), rpc("tools/call", json!({ "name": "brief_get" }))] {
            let r = handle_mcp(&state, Some(&p.bearer()), &body).await;
            assert_eq!(r.status, 403);
            let b = r.body.unwrap();
            assert_eq!(b["error"]["code"], FROZEN);
            assert_eq!(b["error"]["message"], format!("the plan left briefing; the brief is frozen at v{frozen}"));
        }
        assert_no_new_version(&state, &p.id, frozen);
        assert_eq!(last_rejection(&state, &p.id)["reason"], "not_briefing");
    }

    #[tokio::test]
    async fn initialize_returns_capabilities() {
        let (state, _r, p) = plan_with_state().await;
        let r = handle_mcp(&state, Some(&p.bearer()), &rpc("initialize", json!({ "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "claude-code", "version": "x" } }))).await;
        assert_eq!(r.status, 200);
        let res = &r.body.as_ref().unwrap()["result"];
        assert_eq!(res["protocolVersion"], "2025-03-26");
        assert_eq!(res["capabilities"]["tools"]["listChanged"], false);
        assert_eq!(res["serverInfo"]["name"], "j1");
        assert_eq!(res["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
        assert!(res["instructions"].as_str().unwrap().contains("THIS plan"));
        assert_eq!(r.body.as_ref().unwrap()["id"], 7);
        let r = handle_mcp(&state, Some(&p.bearer()), &rpc("initialize", json!({ "protocolVersion": "1999-01-01" }))).await;
        assert_eq!(r.body.unwrap()["result"]["protocolVersion"], "2025-06-18");
        let r = handle_mcp(&state, Some(&p.bearer()), &rpc("ping", json!({}))).await;
        assert_eq!(r.body.unwrap()["result"], json!({}));
        // initialize is authenticated too.
        let before = version_count(&state, &p.id);
        assert_401(&handle_mcp(&state, None, &rpc("initialize", json!({}))).await);
        assert_no_new_version(&state, &p.id, before);
    }

    #[tokio::test]
    async fn notifications_return_202() {
        let (state, _r, p) = plan_with_state().await;
        let n = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string().into_bytes();
        let r = handle_mcp(&state, Some(&p.bearer()), &n).await;
        assert_eq!(r, McpReply { status: 202, body: None });
        // Through HTTP: 202, empty body.
        let mut req = good(&n);
        req.headers_mut().insert(header::AUTHORIZATION, HeaderValue::from_str(&p.bearer()).unwrap());
        let res = send(&state, req).await;
        assert_eq!(res.status(), StatusCode::ACCEPTED);
        assert!(axum::body::to_bytes(res.into_body(), 64).await.unwrap().is_empty());
        // A notification still needs a token.
        let before = version_count(&state, &p.id);
        assert_401(&handle_mcp(&state, None, &n).await);
        assert_no_new_version(&state, &p.id, before);
    }

    #[tokio::test]
    async fn batch_rejected() {
        let (state, _r, p) = plan_with_state().await;
        let before = version_count(&state, &p.id);
        let batch = format!("[{}]", String::from_utf8(write_call()).unwrap());
        let r = handle_mcp(&state, Some(&p.bearer()), batch.as_bytes()).await;
        assert_eq!(r.status, 400);
        assert_eq!(r.body.unwrap()["error"]["code"], INVALID_REQUEST);
        assert_no_new_version(&state, &p.id, before);
        let r = handle_mcp(&state, Some(&p.bearer()), b"{not json").await;
        assert_eq!(r.body.unwrap()["error"]["code"], PARSE_ERROR);
        assert_no_new_version(&state, &p.id, before);
    }

    #[tokio::test]
    async fn unknown_method_32601() {
        let (state, _r, p) = plan_with_state().await;
        let before = version_count(&state, &p.id);
        for m in ["resources/list", "prompts/get", "tools/delete", "shell/exec"] {
            let r = handle_mcp(&state, Some(&p.bearer()), &rpc(m, json!({}))).await;
            assert_eq!(r.body.unwrap()["error"]["code"], METHOD_NOT_FOUND, "{m}");
        }
        assert_no_new_version(&state, &p.id, before);
        let ev = last_rejection(&state, &p.id);
        assert_eq!(ev["reason"], "unknown_method");
        assert_eq!(ev["method"], "other", "client method text is never echoed into an event");
        let r = handle_mcp(&state, Some(&p.bearer()), &rpc("tools/call", json!({ "name": "exec", "arguments": {} }))).await;
        assert_eq!(r.body.unwrap()["error"]["code"], INVALID_PARAMS);
        let ev = last_rejection(&state, &p.id);
        assert_eq!(ev["reason"], "unknown_tool");
        assert_eq!(ev["tool"], "unknown");
    }

    #[tokio::test]
    async fn tools_list_lists_the_tools() {
        let (state, _r, p) = plan_with_state().await;
        let r = handle_mcp(&state, Some(&p.bearer()), &rpc("tools/list", json!({}))).await;
        assert_eq!(r.status, 200);
        let tools = r.body.unwrap()["result"]["tools"].as_array().unwrap().clone();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            vec![
                "brief_get", "brief_update", "brief_versions", "outline_add_phase", "outline_add_task",
                "outline_update", "outline_move", "outline_delete", "plan_check", "repo_search"
            ]
        );
        for t in &tools {
            assert_eq!(t["inputSchema"]["type"], "object");
            assert!(t["description"].as_str().is_some_and(|d| !d.is_empty()));
        }
        // tools/list serializes the SAME table tools/call enforces.
        for (t, def) in tools.iter().zip(plan_mcp::TOOLS) {
            assert_eq!(t["inputSchema"], (def.input_schema)());
        }
    }

    // ── task 06: audit ──

    #[tokio::test]
    async fn accepted_write_event_has_agent_and_session() {
        let (state, _r, p) = plan_with_state().await;
        let r = handle_mcp(&state, Some(&p.bearer()), &write_call()).await;
        let version = r.body.unwrap()["result"]["structuredContent"]["version"].clone();
        let ev = events(&state, &p.id, "brief_version_created").pop().unwrap();
        assert_eq!(ev["author"], "agent");
        assert_eq!(ev["sessionId"], json!(p.session));
        assert_eq!(ev["version"], version);
        assert_eq!(ev["changedFields"], json!(["why"]));
        assert!(!ev.to_string().contains(&p.token));
    }

    #[tokio::test]
    async fn rejection_event_has_reason_and_count_no_token() {
        let (state, _r, p) = plan_with_state().await;
        let bad = rpc("tools/call", json!({ "name": "brief_update", "arguments": { "field": "why", "value": "x", "initiativeId": "other" } }));
        for _ in 0..3 {
            handle_mcp(&state, Some(&p.bearer()), &bad).await;
        }
        let ev = events(&state, &p.id, "agent_mcp_rejected");
        assert_eq!(ev.len(), 3);
        for (i, e) in ev.iter().enumerate() {
            assert_eq!(e["reason"], "invalid_arguments");
            assert_eq!(e["method"], "tools/call");
            assert_eq!(e["tool"], "brief_update");
            assert_eq!(e["count"], json!(i as u64 + 1));
            let s = e.to_string();
            assert!(plan_mcp::redact_payload_has_no_secret(&s, &p.token));
            assert!(plan_mcp::redact_payload_has_no_secret(&s, &mcp_tokens::sha256_hex(&p.token)));
        }
        // The counter is seeded from the table after a restart (fresh in-memory map).
        state.mcp_rejections.lock().unwrap().clear();
        handle_mcp(&state, Some(&p.bearer()), &bad).await;
        assert_eq!(events(&state, &p.id, "agent_mcp_rejected").pop().unwrap()["count"], 4);
        // Rejections never page anyone.
        for e in &ev {
            assert!(crate::services::agent_plans::discord_message_for("agent_mcp_rejected", None, e).is_none());
        }
    }

    #[test]
    fn token_absent_from_events_and_logs() {
        let (state, _r) = test_state();
        let ((token, hash, ids), text) = fixture::capture_logs(|rt| {
            rt.block_on(async {
                let store = use_store(&state);
                let p = plan(&state, &store, "A").await;
                let other = plan(&state, &store, "B").await;
                mcp_tokens::write_mcp_config_on_port(&store, &p.id, &RawTokenForTest(p.token.clone()).raw(), 7788).unwrap();
                let bearer = p.bearer();
                // Happy write + every read.
                handle_mcp(&state, Some(&bearer), &write_call()).await;
                handle_mcp(&state, Some(&bearer), &rpc("tools/list", json!({}))).await;
                handle_mcp(&state, Some(&bearer), &rpc("tools/call", json!({ "name": "repo_search", "arguments": { "query": &p.token } }))).await;
                // Rejection suite.
                handle_mcp(&state, None, &write_call()).await;
                handle_mcp(&state, Some(&format!("Basic {}", p.token)), &write_call()).await;
                handle_mcp(&state, Some(&format!("Bearer {}x", p.token)), &write_call()).await;
                handle_mcp(&state, Some(&format!("Bearer {}", "f".repeat(64))), &write_call()).await;
                handle_mcp(&state, Some(&bearer), &rpc("tools/call", json!({ "name": "brief_update", "arguments": { "field": p.token } }))).await;
                handle_mcp(&state, Some(&bearer), &rpc(&p.token, json!({}))).await;
                handle_mcp(&state, Some(&bearer), format!("[{}]", p.token).as_bytes()).await;
                for r in [
                    req(Method::GET, Some("127.0.0.1:7788"), None, None, Body::empty()),
                    req(Method::POST, Some("127.0.0.1:7788"), Some("application/json"), Some("https://evil.example"), Body::from(write_call())),
                    req(Method::POST, Some("evil:7788"), Some("application/json"), None, Body::from(write_call())),
                    req(Method::POST, Some("127.0.0.1:7788"), Some("text/plain"), None, Body::from(write_call())),
                ] {
                    let mut r = r;
                    r.headers_mut().insert(header::AUTHORIZATION, HeaderValue::from_str(&bearer).unwrap());
                    send(&state, r).await;
                }
                crate::services::sessions::mark_session_archived(&state, &p.session).unwrap();
                handle_mcp(&state, Some(&bearer), &write_call()).await;
                mcp_tokens::revoke_for_initiative(&state, &p.id, "test").unwrap();
                handle_mcp(&state, Some(&bearer), &write_call()).await;
                handle_mcp(&state, Some(&other.bearer()), &write_call()).await;
                (p.token.clone(), mcp_tokens::sha256_hex(&p.token), (p.id.clone(), other.token.clone()))
            })
        });
        assert!(text.contains("MCP call rejected"), "the capture works:\n{text}");
        assert!(text.contains("plan-agent MCP token minted"), "INFO lines are captured too");
        for secret in [token.as_str(), hash.as_str(), ids.1.as_str(), mcp_tokens::sha256_hex(&ids.1).as_str()] {
            assert!(plan_mcp::redact_payload_has_no_secret(&text, secret), "a log line holds token material");
            for payload in fixture::all_event_payloads(&state) {
                assert!(plan_mcp::redact_payload_has_no_secret(&payload, secret), "event holds token material: {payload}");
            }
        }
        assert!(!events(&state, &ids.0, "agent_mcp_rejected").is_empty());
    }

    /// Test-only bridge: build a `RawToken` from a known string (the real ones come from `mint`).
    struct RawTokenForTest(String);
    impl RawTokenForTest {
        fn raw(self) -> mcp_tokens::RawToken {
            mcp_tokens::RawToken::for_test(self.0)
        }
    }

    #[tokio::test]
    async fn rejection_renders_in_timeline() {
        let (state, _r, p) = plan_with_state().await;
        mcp_tokens::revoke_for_initiative(&state, &p.id, "test").unwrap();
        handle_mcp(&state, Some(&p.bearer()), &write_call()).await;
        handle_mcp(&state, Some(&p.bearer()), &write_call()).await;
        let run = crate::services::agent_plans::get_plan(&state, &p.id).unwrap();
        let summaries: Vec<String> = run
            .events
            .iter()
            .filter(|e| e.event_type == "agent_mcp_rejected")
            .map(|e| e.summary.clone())
            .collect();
        assert!(summaries.contains(&"MCP call rejected: revoked".to_string()), "{summaries:?}");
        assert!(summaries.contains(&"MCP call rejected: revoked (×2)".to_string()), "{summaries:?}");
        let ev = run.events.iter().find(|e| e.event_type == "agent_mcp_rejected").unwrap();
        assert_eq!(ev.actor, "plan_agent");
    }
}
