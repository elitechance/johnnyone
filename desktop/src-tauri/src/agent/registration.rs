use serde::{Deserialize, Serialize};

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

    let mut request = client
        .post(&graphql_url)
        .header("Content-Type", "application/json")
        .header("x-tenant-id", tenant_id)
        .header("x-user-id", user_id);
    // Carry the credential. Skipped only when unset, so a token-less local/dev worker still works.
    if !access_token.trim().is_empty() {
        request = request.header("Authorization", format!("Bearer {}", access_token.trim()));
    }
    let response = request
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
