use serde::{Deserialize, Serialize};
use std::future::Future;

#[derive(Debug, Serialize)]
struct GraphqlRequest {
    query: String,
    variables: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct GraphqlResponse {
    data: Option<RegisterData>,
    errors: Option<Vec<GraphqlError>>,
}

#[derive(Debug, Deserialize)]
struct RegisterData {
    #[serde(rename = "registerDesktopNode")]
    register_desktop_node: RegisteredNode,
}

#[derive(Debug, Deserialize)]
struct GraphqlError {
    message: String,
}

#[derive(Debug, Deserialize)]
pub struct RegisteredNode {
    pub id: String,
    pub hostname: String,
    pub status: String,
}

/// Headers for desktop GraphQL to the CF worker.
/// `x-tenant-id` / `x-user-id` are advisory (not credentials).
/// `Authorization` is omitted when the token is empty (do not send `Bearer `).
pub fn register_headers(
    tenant_id: &str,
    user_id: &str,
    access_token: &str,
) -> Vec<(String, String)> {
    let mut headers = vec![
        ("x-tenant-id".to_string(), tenant_id.to_string()),
        ("x-user-id".to_string(), user_id.to_string()),
    ];
    let token = access_token.trim();
    if !token.is_empty() {
        headers.push((
            "Authorization".to_string(),
            format!("Bearer {}", token),
        ));
    }
    headers
}

pub fn apply_headers(
    mut request: reqwest::RequestBuilder,
    headers: &[(String, String)],
) -> reqwest::RequestBuilder {
    for (name, value) in headers {
        request = request.header(name, value);
    }
    request
}

pub fn apply_register_headers(
    request: reqwest::RequestBuilder,
    tenant_id: &str,
    user_id: &str,
    access_token: &str,
) -> reqwest::RequestBuilder {
    apply_headers(request, &register_headers(tenant_id, user_id, access_token))
}

/// Prefer the token just resolved from settings; fall back to the spawn-time clone.
pub fn access_token_for_attempt(resolved: Option<&str>, spawn_clone: &str) -> String {
    match resolved {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => spawn_clone.to_string(),
    }
}

/// Headers for get/delete attachment GraphQL. The live settings/env token
/// wins over the spawn-time `WorkerRelayConfig` snapshot (same rule as
/// register / the WS reconnect loop).
pub fn attachment_graphql_headers(
    tenant_id: &str,
    user_id: &str,
    live_token: Option<&str>,
    snapshot_token: &str,
) -> Vec<(String, String)> {
    register_headers(
        tenant_id,
        user_id,
        &access_token_for_attempt(live_token, snapshot_token),
    )
}

pub fn is_unauthenticated_register_error(err: &str) -> bool {
    let upper = err.to_ascii_uppercase();
    upper.contains("UNAUTHENTICATED") || upper.contains("NOT AUTHENTICATED")
}

/// Register, and if the error is UNAUTHENTICATED, refresh once and retry
/// with the post-refresh token. `refresh` returning `Ok(false)` (jk_ / no
/// refresh token) does not retry.
pub async fn register_with_optional_refresh<R, RF, F, FF, T, TF>(
    initial_token: &str,
    mut register: R,
    mut refresh: F,
    mut current_token: T,
) -> Result<RegisteredNode, String>
where
    R: FnMut(&str) -> RF,
    RF: Future<Output = Result<RegisteredNode, String>>,
    F: FnMut() -> FF,
    FF: Future<Output = Result<bool, String>>,
    T: FnMut() -> TF,
    TF: Future<Output = String>,
{
    match register(initial_token).await {
        Ok(node) => Ok(node),
        Err(err) if is_unauthenticated_register_error(&err) => match refresh().await {
            Ok(true) => {
                let next = current_token().await;
                register(&next).await
            }
            Ok(false) => Err(err),
            Err(refresh_err) => Err(format!("{err}; refresh failed: {refresh_err}")),
        },
        Err(err) => Err(err),
    }
}

/// Register this desktop node with the CF Worker via GraphQL mutation.
/// Returns the server-assigned node ID (may reuse existing if hostname matches).
/// Register this machine as the user's desktop node.
///
/// The `access_token` is REQUIRED even though the tenant/user headers are still sent: the worker was
/// hardened so identity comes from the credential, not from headers ("identity must come from the
/// credential, not a header"). Registering without an Authorization header returns UNAUTHENTICATED,
/// which stalls the whole relay — the desktop can never attach, and every worker call that proxies to
/// it fails with "Desktop not connected".
pub async fn register_node(
    worker_url: &str,
    user_id: &str,
    tenant_id: &str,
    hostname: &str,
    access_token: &str,
) -> Result<RegisteredNode, String> {
    let client = reqwest::Client::new();

    let query = r#"
        mutation RegisterDesktopNode($input: RegisterDesktopNodeInput!) {
            registerDesktopNode(input: $input) {
                id
                hostname
                status
            }
        }
    "#;

    let variables = serde_json::json!({
        "input": {
            "hostname": hostname,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        }
    });

    let graphql_url = format!("{}/graphql", worker_url.trim_end_matches('/'));

    let response = apply_register_headers(
        client
            .post(&graphql_url)
            .header("Content-Type", "application/json"),
        tenant_id,
        user_id,
        access_token,
    )
    .json(&GraphqlRequest {
        query: query.to_string(),
        variables,
    })
    .send()
    .await
    .map_err(|e| format!("Registration request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "Registration failed with status {}: {}",
            status, body
        ));
    }

    let gql_response: GraphqlResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse registration response: {}", e))?;

    if let Some(errors) = gql_response.errors {
        let msgs: Vec<String> = errors.into_iter().map(|e| e.message).collect();
        return Err(format!("GraphQL errors: {}", msgs.join(", ")));
    }

    gql_response
        .data
        .map(|d| d.register_desktop_node)
        .ok_or_else(|| "No data in registration response".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn register_headers_includes_bearer_when_token_present() {
        let headers = register_headers("tenant-a", "user-a", "jk_abc");
        assert_eq!(header(&headers, "Authorization"), Some("Bearer jk_abc"));
        assert_eq!(header(&headers, "x-tenant-id"), Some("tenant-a"));
        assert_eq!(header(&headers, "x-user-id"), Some("user-a"));
    }

    #[test]
    fn register_headers_omits_authorization_when_token_empty() {
        let headers = register_headers("tenant-a", "user-a", "");
        assert!(header(&headers, "Authorization").is_none());
        assert_eq!(header(&headers, "x-tenant-id"), Some("tenant-a"));
        assert_eq!(header(&headers, "x-user-id"), Some("user-a"));
    }

    #[test]
    fn register_headers_omits_authorization_when_token_whitespace() {
        let headers = register_headers("t", "u", "   ");
        assert!(header(&headers, "Authorization").is_none());
    }

    #[test]
    fn attachment_headers_use_live_token_not_snapshot() {
        let headers = attachment_graphql_headers("t1", "u1", Some("fresh-jwt"), "stale-jwt");
        assert_eq!(header(&headers, "Authorization"), Some("Bearer fresh-jwt"));
        assert_eq!(header(&headers, "x-tenant-id"), Some("t1"));
        assert_eq!(header(&headers, "x-user-id"), Some("u1"));
    }

    #[test]
    fn attachment_headers_fall_back_to_snapshot_when_live_empty() {
        let headers = attachment_graphql_headers("t1", "u1", Some(""), "snapshot-jwt");
        assert_eq!(
            header(&headers, "Authorization"),
            Some("Bearer snapshot-jwt")
        );
        let none = attachment_graphql_headers("t1", "u1", None, "snapshot-jwt");
        assert_eq!(header(&none, "Authorization"), Some("Bearer snapshot-jwt"));
    }

    #[test]
    fn attachment_headers_omit_authorization_when_both_tokens_empty() {
        let headers = attachment_graphql_headers("t1", "u1", None, "");
        assert!(header(&headers, "Authorization").is_none());
    }

    #[test]
    fn access_token_for_attempt_prefers_resolved_over_spawn_clone() {
        assert_eq!(
            access_token_for_attempt(Some("stored-fresh"), "spawn-stale"),
            "stored-fresh"
        );
        assert_eq!(
            access_token_for_attempt(Some(""), "spawn-stale"),
            "spawn-stale"
        );
        assert_eq!(access_token_for_attempt(None, "spawn-stale"), "spawn-stale");
    }

    #[tokio::test]
    async fn unauthenticated_register_refreshes_and_retries_with_new_token() {
        let mut tokens: Vec<String> = Vec::new();
        let mut refresh_calls = 0usize;
        let result = register_with_optional_refresh(
            "expired-jwt",
            |tok| {
                tokens.push(tok.to_string());
                let first = tokens.len() == 1;
                async move {
                    if first {
                        Err("GraphQL errors: UNAUTHENTICATED".to_string())
                    } else {
                        Ok(RegisteredNode {
                            id: "node-1".to_string(),
                            hostname: "host".to_string(),
                            status: "online".to_string(),
                        })
                    }
                }
            },
            || {
                refresh_calls += 1;
                async { Ok(true) }
            },
            || async { "post-refresh-token".to_string() },
        )
        .await
        .expect("retry should succeed");
        assert_eq!(result.id, "node-1");
        assert_eq!(refresh_calls, 1);
        assert_eq!(
            tokens,
            vec!["expired-jwt".to_string(), "post-refresh-token".to_string()]
        );
    }

    #[tokio::test]
    async fn successful_register_does_not_refresh() {
        let mut refresh_calls = 0usize;
        let result = register_with_optional_refresh(
            "live-token",
            |_tok| async {
                Ok(RegisteredNode {
                    id: "n".to_string(),
                    hostname: "h".to_string(),
                    status: "online".to_string(),
                })
            },
            || {
                refresh_calls += 1;
                async { Ok(true) }
            },
            || async { "should-not-run".to_string() },
        )
        .await
        .expect("success");
        assert_eq!(result.id, "n");
        assert_eq!(refresh_calls, 0);
    }

    #[tokio::test]
    async fn refresh_ok_false_does_not_retry_register() {
        let mut register_calls = 0usize;
        let err = register_with_optional_refresh(
            "jk_key",
            |_tok| {
                register_calls += 1;
                async { Err("GraphQL errors: UNAUTHENTICATED".to_string()) }
            },
            || async { Ok(false) },
            || async { "unused".to_string() },
        )
        .await
        .expect_err("jk_ should not loop");
        assert!(is_unauthenticated_register_error(&err));
        assert_eq!(register_calls, 1);
    }
}
