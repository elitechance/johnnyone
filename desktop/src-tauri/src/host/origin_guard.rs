//! Cross-site request rejection for the embedded host GraphQL listener.
//!
//! ## What this buys, and what it does not
//!
//! The host binds `127.0.0.1:7788` (`main.rs`, `JOHNNYONE_HOST_ADDR`) and its
//! GraphQL surface is UNAUTHENTICATED. Binding loopback keeps remote machines
//! out; it does NOT keep remote *web pages* out. Any page the user visits can
//! `fetch('http://127.0.0.1:7788/graphql')` from their own browser, and because
//! `async-graphql` parses any non-multipart body as JSON
//! (`async-graphql-7.2.1/src/http/mod.rs`, the `_ => receive_batch_json(body)`
//! arm), a CORS-**simple** `content-type: text/plain` POST executes with no
//! preflight at all. The old `CorsLayer::allow_origin(Any)` then echoed
//! `access-control-allow-origin: *`, so the attacker page could read the reply.
//!
//! So this guard rejects a request whose `Origin` is neither the webview's nor
//! loopback's, and the CORS layer uses the same predicate so `*` is never
//! echoed again.
//!
//! What it buys: a page on `https://evil.example` can no longer drive or read
//! the host API through the user's browser.
//!
//! What it does NOT buy: any protection against a non-browser local attacker.
//! A request with **no** `Origin` header is allowed on purpose — that is what
//! keeps every baked agent `curl` to `reportAgentResult` working. A local
//! process can omit `Origin` just as easily, so an Origin check only ever
//! constrains browsers. Real authentication on this surface is a separate job.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use axum::{
    extract::Request,
    http::{header::ORIGIN, HeaderName, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Router,
};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

/// Fetch-metadata header browsers send on every request. Chromium-based
/// webviews send it; WebKitGTK does not, which is why the `Origin` allow-list
/// below has to stand on its own.
const SEC_FETCH_SITE: HeaderName = HeaderName::from_static("sec-fetch-site");

/// Extra origins the user can allow without a rebuild, comma-separated. Same
/// style as `JOHNNYONE_HOST_ADDR` (`main.rs`). This is the recovery path if a
/// platform's webview turns out to send an origin we did not predict — the
/// rejection is logged with the exact value to paste in here.
pub const ENV_ALLOWED_ORIGINS: &str = "JOHNNYONE_HOST_ALLOWED_ORIGINS";

/// Origins the Tauri webview and the local dev servers legitimately use.
///
/// Derived, not guessed:
/// - `tauri://localhost` — the production webview origin on Linux/macOS/iOS.
///   `tauri-2.10.2/src/manager/mod.rs:331-338` (`tauri_protocol_url`) returns it
///   for every target except Windows and Android. This box is Linux, so this is
///   the origin the shipped host-app actually uses for
///   `host-app/src/app/services/host-settings.service.ts:4`
///   (`http://127.0.0.1:7788/graphql`).
/// - `http://tauri.localhost` / `https://tauri.localhost` — the same function's
///   wry workaround URL on Windows/Android. Listed so a Windows build is not
///   broken by this guard.
/// - `http://localhost:4201` — `devUrl` in `desktop/src-tauri/tauri.conf.json`
///   (`npx nx serve host-app --port 4201`). Also covered by the loopback rule;
///   listed because it is the one dev origin the config names outright.
///
/// Everything else loopback (the `web` dev server on :4200, a locally served
/// prod bundle on any port) is accepted by [`is_loopback_origin`] rather than
/// pinned to a port, because `ui/src/services/graphql-client.ts:194` only talks
/// to the host at all when `window.location.hostname` is `localhost` or
/// `127.0.0.1` — the deployed console on `johnnyone.pages.dev` never does.
pub const BUILTIN_ALLOWED_ORIGINS: &[&str] = &[
    "tauri://localhost",
    "http://tauri.localhost",
    "https://tauri.localhost",
    "http://localhost:4201",
];

/// Whether one request may reach the host GraphQL surface. Pure, so it is
/// testable without a server.
pub fn allow_request_with(
    origin: Option<&str>,
    sec_fetch_site: Option<&str>,
    extra: &[String],
) -> bool {
    // A browser will not let a page forge Sec-Fetch-*, so these two values are
    // conclusive. A non-browser client can forge them, but it could equally
    // just omit Origin, which is allowed anyway.
    if matches!(sec_fetch_site, Some("same-origin") | Some("none")) {
        return true;
    }

    // No Origin at all: the baked agent `curl`s, the coordinator, `wscat`.
    // Deliberate — see the module docs.
    let Some(origin) = origin else {
        return true;
    };

    is_allowed_origin_with(origin, extra)
}

/// Whether one `Origin` header value is on the allow-list. Shared with the CORS
/// layer so the two can never disagree.
pub fn is_allowed_origin_with(origin: &str, extra: &[String]) -> bool {
    BUILTIN_ALLOWED_ORIGINS.contains(&origin)
        || extra.iter().any(|allowed| allowed == origin)
        || is_loopback_origin(origin)
}

/// `http(s)://` with a loopback host and an optional numeric port. Rejects
/// `null` (sandboxed iframe), lookalikes such as `localhost.evil.example`, and
/// anything carrying a path or userinfo.
fn is_loopback_origin(origin: &str) -> bool {
    let rest = match origin.strip_prefix("http://") {
        Some(rest) => rest,
        None => match origin.strip_prefix("https://") {
            Some(rest) => rest,
            None => return false,
        },
    };
    // An `Origin` is only ever scheme://host[:port]; anything else is hostile
    // or malformed.
    if rest.is_empty() || rest.contains('/') || rest.contains('@') {
        return false;
    }

    let host = if rest.starts_with('[') {
        // IPv6 literal: the port colon is the one after the closing bracket.
        let Some(close) = rest.find(']') else {
            return false;
        };
        let (host, tail) = rest.split_at(close + 1);
        if !tail.is_empty() && !tail.strip_prefix(':').is_some_and(is_port) {
            return false;
        }
        host
    } else if let Some((host, port)) = rest.split_once(':') {
        if !is_port(port) {
            return false;
        }
        host
    } else {
        rest
    };

    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

fn is_port(value: &str) -> bool {
    !value.is_empty() && value.len() <= 5 && value.bytes().all(|b| b.is_ascii_digit())
}

/// Parsed once — the process has to restart to pick up a new value anyway.
fn extra_allowed_origins() -> &'static [String] {
    static EXTRA: OnceLock<Vec<String>> = OnceLock::new();
    EXTRA.get_or_init(|| {
        let raw = std::env::var(ENV_ALLOWED_ORIGINS).unwrap_or_default();
        let origins: Vec<String> = raw
            .split(',')
            .map(|entry| entry.trim().to_string())
            .filter(|entry| !entry.is_empty())
            .collect();
        // Matching is exact, so `*` is inert — it would silently reject
        // everything instead of allowing everything. Say so rather than leaving
        // the user to guess. Wildcards are deliberately NOT supported: the whole
        // point of this list is that it cannot be widened to `*` by accident.
        for entry in &origins {
            if entry.contains('*') {
                tracing::warn!(
                    entry,
                    "{} entry contains `*`: wildcards are NOT supported and this \
                     entry will never match. List each origin in full \
                     (e.g. http://localhost:3000).",
                    ENV_ALLOWED_ORIGINS
                );
            }
        }
        if !origins.is_empty() {
            tracing::info!(
                origins = ?origins,
                "Host GraphQL: extra allowed origins from {}",
                ENV_ALLOWED_ORIGINS
            );
        }
        origins
    })
}

/// The predicate the CORS layer uses, so a rejected origin never gets an
/// `access-control-allow-origin` header either.
pub fn is_allowed_origin_value(origin: &axum::http::HeaderValue) -> bool {
    origin
        .to_str()
        .is_ok_and(|origin| is_allowed_origin_with(origin, extra_allowed_origins()))
}

/// Remember `origin` and say whether this is the first time it has been seen in
/// `bucket`, with a hard cap so an attacker cycling origins cannot flood the log
/// or grow the set without bound. Returns `false` once the cap is passed.
fn first_sighting(bucket: &'static OnceLock<Mutex<HashSet<String>>>, origin: &str) -> bool {
    const CAP: usize = 64;
    let seen = bucket.get_or_init(|| Mutex::new(HashSet::new()));
    let mut guard = match seen.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.len() >= CAP {
        return false;
    }
    guard.insert(origin.to_string())
}

/// Log a rejection the FIRST time each distinct origin is seen. The log line is
/// the recovery path: it names the exact value to put in
/// `JOHNNYONE_HOST_ALLOWED_ORIGINS` if the origin turns out to be legitimate.
fn log_rejection_once(origin: &str) {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    if !first_sighting(&SEEN, origin) {
        return;
    }
    tracing::warn!(
        origin,
        "Host GraphQL: rejected a cross-site request. If this origin is legitimate, \
         add it to {}",
        ENV_ALLOWED_ORIGINS
    );
}

/// Log an ACCEPTED origin once, so the log says positively which origin the
/// webview uses on this platform rather than only reporting failures. The
/// `tauri://localhost` entry in [`BUILTIN_ALLOWED_ORIGINS`] is derived from
/// tauri's source, not measured on every platform; this line is how you confirm
/// it after a cutover.
fn log_accepted_origin_once(origin: &str) {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    if !first_sighting(&SEEN, origin) {
        return;
    }
    tracing::info!(origin, "Host GraphQL: accepting requests from this origin");
}

/// Axum middleware. Must sit OUTSIDE the CORS layer so it also gates the
/// `OPTIONS` preflight, which `CorsLayer` otherwise answers without calling the
/// inner service.
pub async fn reject_cross_site(req: Request, next: Next) -> Response {
    let headers = req.headers();
    let origin = headers.get(ORIGIN).and_then(|v| v.to_str().ok());
    let sec_fetch_site = headers.get(&SEC_FETCH_SITE).and_then(|v| v.to_str().ok());

    if allow_request_with(origin, sec_fetch_site, extra_allowed_origins()) {
        if let Some(origin) = origin {
            log_accepted_origin_once(origin);
        }
        return next.run(req).await;
    }

    log_rejection_once(origin.unwrap_or("<unreadable>"));
    (StatusCode::FORBIDDEN, "cross-site request rejected\n").into_response()
}

/// How far one request is trusted to WRITE connection-critical settings.
///
/// The read guard stops a hostile site reading the relay token. It does nothing
/// about a caller REDIRECTING where that token is sent: `setSetting` with
/// `worker_url` is a connection key (`services/relay.rs::is_connection_key`),
/// so `apply_setting` reconnects the relay and the host hands the real `jk_`
/// credential — a durable API key that never expires — to whatever endpoint was
/// just written. Gating reads while leaving writes open is no gate at all, so
/// connection keys need their own, narrower tier.
///
/// Why a tier and not simply "refuse connection keys", or "require
/// `Sec-Fetch-Site: same-origin`":
/// - Refusing them outright breaks the host-app. Its login writes
///   `access_token` + `refresh_token` through this very mutation
///   (`host-app/src/app/services/host-auth.service.ts:88-89`) and its settings
///   save writes `worker_url` / `tenant_id` / `user_id`. Breaking login is
///   worse than the bug.
/// - Requiring `Sec-Fetch-Site` breaks it too, on this platform: WebKitGTK does
///   not implement fetch metadata, so the production webview sends no
///   `Sec-Fetch-*` header at all — only `Origin: tauri://localhost`.
///
/// So the dividing line is the ORIGIN tier: ours (the webview, an allow-listed
/// or env-added origin, a genuine same-origin request, or a non-browser client
/// that sent no `Origin`) versus merely loopback. A page on
/// `http://localhost:3000` passes the cross-site guard because it is loopback,
/// but it is not the host-app and has no business rewriting the relay target.
/// Nothing legitimate loses a capability: the only client that writes settings
/// to this surface is the host-app webview, and the web console's `setSetting`
/// goes to the WORKER (`updateSetting`), never here — confirmed in the built
/// bundles, not just the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteTrust {
    /// The Tauri webview, an allow-listed origin, a same-origin request, or a
    /// non-browser client that sent no `Origin` at all.
    Webview,
    /// Passed the cross-site guard only because it is loopback. Not ours.
    Loopback,
}

/// Pure, so it is unit-testable without a server. Fails closed: anything not
/// positively recognised is [`WriteTrust::Loopback`].
pub fn write_trust_with(
    origin: Option<&str>,
    sec_fetch_site: Option<&str>,
    extra: &[String],
) -> WriteTrust {
    // A browser will not let a page forge Sec-Fetch-*, so `same-origin` is
    // conclusive. `none` (a user-initiated navigation) cannot carry a GraphQL
    // POST body from a page, so it is equally safe.
    if matches!(sec_fetch_site, Some("same-origin") | Some("none")) {
        return WriteTrust::Webview;
    }
    match origin {
        // No Origin: the baked agent curls and local scripts. Same deliberate
        // trade-off as the read side — see the module docs.
        None => WriteTrust::Webview,
        Some(origin)
            if BUILTIN_ALLOWED_ORIGINS.contains(&origin)
                || extra.iter().any(|allowed| allowed == origin) =>
        {
            WriteTrust::Webview
        }
        Some(_) => WriteTrust::Loopback,
    }
}

pub fn write_trust_from_headers(headers: &axum::http::HeaderMap) -> WriteTrust {
    let origin = headers.get(ORIGIN).and_then(|v| v.to_str().ok());
    let sec_fetch_site = headers.get(&SEC_FETCH_SITE).and_then(|v| v.to_str().ok());
    write_trust_with(origin, sec_fetch_site, extra_allowed_origins())
}

/// Wrap the host's routes in the CORS layer and the cross-site guard.
///
/// ORDER MATTERS. `.layer(cors)` then `.layer(guard)` makes the guard the
/// OUTERMOST layer, which is required: `CorsLayer` answers the `OPTIONS`
/// preflight itself without calling the inner service, so a guard placed inside
/// would never see a preflight. Being outermost also covers `/graphql/ws`, which
/// CORS does not apply to at all.
///
/// Methods are narrowed to what the webview and the playground actually use.
/// Headers stay `Any` (async-graphql clients send content-type/accept and the
/// console may add an auth header); with no credentialed requests on this
/// surface, `*` for headers is harmless once the origin itself is gated.
pub fn guard_layers<S>(router: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, _parts| {
            is_allowed_origin_value(origin)
        }))
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers(Any);

    router
        .layer(cors)
        .layer(axum::middleware::from_fn(reject_cross_site))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow(origin: Option<&str>, sfs: Option<&str>) -> bool {
        allow_request_with(origin, sfs, &[])
    }

    #[test]
    fn same_origin_and_none_fetch_site_are_allowed() {
        assert!(allow(Some("http://127.0.0.1:7788"), Some("same-origin")));
        // A browser navigation to the GraphQL playground: no Origin, site=none.
        assert!(allow(None, Some("none")));
        // Sec-Fetch-Site wins even when the Origin looks odd, because a browser
        // will not let a page forge that header.
        assert!(allow(Some("tauri://localhost"), Some("same-origin")));
    }

    #[test]
    fn absent_origin_is_allowed_so_baked_agent_curls_keep_working() {
        // Deliberate, documented trade-off: an Origin check only ever
        // constrains browsers. Every agent `curl` to 127.0.0.1:7788 sends no
        // Origin and no Sec-Fetch-* headers.
        assert!(allow(None, None));
    }

    #[test]
    fn webview_and_loopback_origins_are_allowed() {
        // Linux/macOS production webview (tauri 2.10.2 manager/mod.rs:336).
        assert!(allow(Some("tauri://localhost"), None));
        // Windows/Android wry workaround URL (manager/mod.rs:334).
        assert!(allow(Some("http://tauri.localhost"), None));
        assert!(allow(Some("https://tauri.localhost"), None));
        // devUrl (tauri.conf.json) and the web dev server, both loopback.
        assert!(allow(Some("http://localhost:4201"), Some("cross-site")));
        assert!(allow(Some("http://localhost:4200"), Some("cross-site")));
        assert!(allow(Some("http://127.0.0.1:4200"), Some("cross-site")));
        assert!(allow(Some("http://[::1]:4200"), Some("cross-site")));
        assert!(allow(Some("http://localhost"), None));
    }

    #[test]
    fn hostile_origins_are_rejected() {
        assert!(!allow(Some("https://evil.example"), Some("cross-site")));
        assert!(!allow(Some("https://evil.example"), None));
        assert!(!allow(Some("http://evil.example:7788"), None));
    }

    #[test]
    fn null_origin_is_rejected() {
        // A sandboxed iframe reports `Origin: null`; allowing it would reopen
        // the hole this guard closes.
        assert!(!allow(Some("null"), None));
        assert!(!allow(Some("null"), Some("cross-site")));
    }

    #[test]
    fn lookalike_loopback_origins_are_rejected() {
        assert!(!allow(Some("http://localhost.evil.example"), None));
        assert!(!allow(Some("http://127.0.0.1.evil.example"), None));
        // Port position must really be a port.
        assert!(!allow(Some("http://localhost:4200.evil.example"), None));
        // Non-loopback private addresses are not the webview.
        assert!(!allow(Some("http://192.168.1.10:4200"), None));
        // A userinfo-looking origin must not smuggle a host past the check.
        assert!(!allow(Some("http://evil.example@localhost"), None));
        // Only http(s) and the tauri scheme; a file page is opaque anyway.
        assert!(!allow(Some("ftp://localhost"), None));
    }

    #[test]
    fn env_override_adds_one_origin() {
        let extra = vec!["https://console.example".to_string()];
        assert!(allow_request_with(
            Some("https://console.example"),
            Some("cross-site"),
            &extra
        ));
        // and does not widen anything else
        assert!(!allow_request_with(
            Some("https://other.example"),
            Some("cross-site"),
            &extra
        ));
    }

    #[test]
    fn is_allowed_origin_is_the_same_predicate_cors_uses() {
        assert!(is_allowed_origin_with("tauri://localhost", &[]));
        assert!(is_allowed_origin_with("http://localhost:4201", &[]));
        assert!(!is_allowed_origin_with("https://evil.example", &[]));
        assert!(!is_allowed_origin_with("null", &[]));
    }

    // ── wiring ────────────────────────────────────────────────────────────
    // Drives the assembled router so layer ORDER and the real 403 are covered
    // without touching the live host.

    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::routing::get;
    use tower::ServiceExt;

    fn test_router() -> Router {
        guard_layers(Router::new().route("/graphql", get(|| async { "ok" })))
    }

    async fn send(req: HttpRequest<Body>) -> axum::http::Response<Body> {
        test_router().oneshot(req).await.unwrap()
    }

    #[tokio::test]
    async fn hostile_origin_gets_403_and_no_allow_origin_header() {
        let res = send(
            HttpRequest::builder()
                .method(Method::GET)
                .uri("/graphql")
                .header(ORIGIN, "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        assert!(res
            .headers()
            .get("access-control-allow-origin")
            .is_none());
    }

    #[tokio::test]
    async fn hostile_preflight_is_rejected_by_the_outer_guard() {
        // This is the case that proves the ordering: `CorsLayer` on its own
        // would answer OPTIONS with 200 before the guard ever ran.
        let res = send(
            HttpRequest::builder()
                .method(Method::OPTIONS)
                .uri("/graphql")
                .header(ORIGIN, "https://evil.example")
                .header("access-control-request-method", "POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn webview_origin_is_echoed_not_starred() {
        let res = send(
            HttpRequest::builder()
                .method(Method::GET)
                .uri("/graphql")
                .header(ORIGIN, "tauri://localhost")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()
                .get("access-control-allow-origin")
                .map(|v| v.to_str().unwrap()),
            Some("tauri://localhost")
        );
    }

    #[tokio::test]
    async fn an_origin_less_request_still_reaches_the_handler() {
        let res = send(
            HttpRequest::builder()
                .method(Method::GET)
                .uri("/graphql")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
    }


    // ── write trust ───────────────────────────────────────────────────────

    fn trust(origin: Option<&str>, sfs: Option<&str>) -> WriteTrust {
        write_trust_with(origin, sfs, &[])
    }

    #[test]
    fn the_webview_tier_may_write_connection_keys() {
        // Production webview on Linux/macOS, which sends NO Sec-Fetch-* at all
        // (WebKitGTK does not implement fetch metadata).
        assert_eq!(trust(Some("tauri://localhost"), None), WriteTrust::Webview);
        assert_eq!(trust(Some("http://tauri.localhost"), None), WriteTrust::Webview);
        // host-app devUrl.
        assert_eq!(trust(Some("http://localhost:4201"), None), WriteTrust::Webview);
        // Baked agent curls / local scripts: no Origin at all.
        assert_eq!(trust(None, None), WriteTrust::Webview);
        assert_eq!(
            trust(Some("http://127.0.0.1:7788"), Some("same-origin")),
            WriteTrust::Webview
        );
    }

    #[test]
    fn a_bare_loopback_origin_is_demoted_for_connection_keys() {
        // THE MEASURED PROBE: a page on localhost:3000 redirecting worker_url
        // to the attacker so the host hands it the real `jk_` token.
        assert_eq!(
            trust(Some("http://localhost:3000"), Some("cross-site")),
            WriteTrust::Loopback
        );
        assert_eq!(trust(Some("http://localhost:3000"), None), WriteTrust::Loopback);
        // The web dev server may read via the host but never writes settings to
        // it, so demoting it costs nothing.
        assert_eq!(trust(Some("http://localhost:4200"), None), WriteTrust::Loopback);
        assert_eq!(trust(Some("http://127.0.0.1:9999"), None), WriteTrust::Loopback);
    }

    #[test]
    fn write_trust_fails_closed_for_anything_hostile() {
        // These never reach a resolver (the middleware 403s them), but the trust
        // value must not be the permissive one if that ever changes.
        assert_eq!(trust(Some("https://evil.example"), Some("cross-site")), WriteTrust::Loopback);
        assert_eq!(trust(Some("null"), None), WriteTrust::Loopback);
    }

    #[test]
    fn an_env_allowed_origin_gets_the_webview_tier() {
        let extra = vec!["https://console.example".to_string()];
        assert_eq!(
            write_trust_with(Some("https://console.example"), Some("cross-site"), &extra),
            WriteTrust::Webview
        );
        assert_eq!(
            write_trust_with(Some("http://localhost:3000"), Some("cross-site"), &extra),
            WriteTrust::Loopback
        );
    }

    #[test]
    fn ipv6_loopback_edge_cases() {
        assert!(is_allowed_origin_with("http://[::1]", &[]));
        assert!(!is_allowed_origin_with("http://[::1]:notaport", &[]));
        assert!(!is_allowed_origin_with("http://[2001:db8::1]:4200", &[]));
        assert!(!is_allowed_origin_with("http://[::1", &[]));
    }
}
