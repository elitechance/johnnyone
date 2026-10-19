//! One startup read of the provider's model list, compared with the seat caps.
//!
//! **Why this exists.** [`leases::families_absent_from_provider`] and
//! [`leases::provider_serves_no_capped_family`] have been tested since the lease pool landed, but
//! nothing called them — so a cap for a family the provider will not serve produced no warning
//! anywhere. A cap for an absent family is a pool that can be leased and never used: the arena
//! happily reserves two seats on `deepseek-v4.1-flash`, the run starts, and every request to that
//! family is answered by a model that is not the one the prompt was searched on. The count is
//! honest and the comparison is correct; the missing piece was anybody asking.
//!
//! **The real data this is written against.** The kloo profile at `~/etc/kloo.json` looks like:
//!
//! ```json
//! {"defaultProvider":"lokalai",
//!  "providers":{"lokalai":{"endpoint":"https://lokalai.silverjrom.app/v1",
//!                          "apiKey":"…",
//!                          "defaultModel":"next",
//!                          "models":{…}}}}
//! ```
//!
//! and that endpoint's `GET /v1/models` returns
//!
//! ```json
//! {"object":"list","data":[{"id":"glm-5.3-flash",…},{"id":"qwen3.8-flash-next",…},…]}
//! ```
//!
//! `deepseek-v4.1-flash` — which [`MEASURED_SEAT_CAPS`](leases::MEASURED_SEAT_CAPS) gives 2 seats
//! — is **not** among the ids. That is the mismatch this module reports, and it is the reason the
//! check runs at all.
//!
//! **Shape of the module.** Everything that decides is pure and takes its data as a `&str`:
//! profile JSON in, endpoint out; models JSON in, id list out; caps + ids in, one-line report out.
//! The impure part is `run_once` alone, and it is thin. That split is not decoration — the
//! difference between "the provider serves none of these" and "we could not read the provider" is
//! the whole content of the warning, and it has to be testable without a network.
//!
//! **Every failure is silent-but-logged at debug.** A preflight that cannot reach the provider
//! must not block the host and must not alarm the operator: the host has work to do whether or not
//! lokalai answers, and a warning on every laptop that is offline at boot trains nobody to read
//! warnings. An unreadable answer means *unknown*, and unknown reports nothing.

use crate::services::leases::{
    families_absent_from_provider, provider_serves_no_capped_family, SeatCaps,
};

/// How long to wait for the model list. Short on purpose: this runs at boot and the answer is a
/// log line, so a provider that is thinking is a provider we skip rather than a host that waits.
const PREFLIGHT_TIMEOUT_SECS: u64 = 5;

/// The `endpoint` of a named provider block in a kloo profile, or None when the profile does not
/// have that provider. None means "do not check", never "check against nothing".
///
/// Decides *whether* the comparison runs at all, which is why it returns `Option` rather than
/// defaulting to some plausible URL: guessing an endpoint would turn a misconfigured profile into
/// a confident claim about a fleet nobody configured. Trailing slashes are trimmed so the caller's
/// `{endpoint}/models` cannot produce `//models`.
pub fn provider_endpoint_from_profile(profile_json: &str, provider: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(profile_json).ok()?;
    let endpoint = value.get("providers")?.get(provider)?.get("endpoint")?.as_str()?;
    let trimmed = endpoint.trim().trim_end_matches('/');
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The model ids in a `/v1/models` body, read from `data[].id`.
///
/// Returns None when the body cannot be parsed or has no `data` array — an unreadable answer is NOT
/// an empty fleet, and the difference decides whether the mismatch check runs at all. Collapsing
/// "unreadable" into `vec![]` would let the caller either report every capped family as absent
/// (a fleet-wide alarm from one bad HTTP response) or report nothing by accident rather than by
/// design. `None` makes the skip an explicit decision at the call site.
///
/// Ids that are not strings are skipped rather than fatal, since a provider adding a field shape
/// is not a reason to stop checking the ids it did send.
pub fn provider_ids_from_models_json(body: &str) -> Option<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let data = value.get("data")?.as_array()?;
    Some(
        data.iter()
            .filter_map(|entry| entry.get("id")?.as_str())
            .map(String::from)
            .collect(),
    )
}

/// The one-line operator warning, or None when there is nothing to say.
///
/// Uses [`families_absent_from_provider`] and [`provider_serves_no_capped_family`] from the leases
/// module — it does NOT reimplement either comparison. That is not tidiness: both comparisons carry
/// deliberate rules (an empty provider list reports nothing; matching is exact, never prefix), and
/// a second copy of them here would be a second place to get those rules wrong.
///
/// Mentions each absent family by name, and says explicitly when the provider serves none of the
/// capped families, because that means the arena cannot run at all rather than running narrower —
/// a different message for an operator who has to fix it before starting anything.
pub fn mismatch_report(caps: &SeatCaps, provider_ids: &[String]) -> Option<String> {
    let absent = families_absent_from_provider(caps, provider_ids);
    if absent.is_empty() {
        return None;
    }
    // Both comparisons are re-asked rather than derived from `absent`, so this function cannot
    // drift from the leases module's own definition of "serves none".
    if provider_serves_no_capped_family(caps, provider_ids) {
        Some(format!(
            "provider serves NONE of the capped model families ({}); the arena cannot run at all \
             against this provider, not merely narrower",
            absent.join(", ")
        ))
    } else {
        Some(format!(
            "provider does not serve capped model family/{}: those seats can be leased but never \
             used",
            absent.join("/")
        ))
    }
}

/// Read the profile, fetch the provider's model list once, and log the report.
///
/// Every failure is silent-but-logged at debug: a preflight that cannot reach the provider must not
/// block or alarm the host, and an empty/unreadable answer means "unknown", which reports nothing.
/// Nothing here returns a `Result` and nothing is `warn!` except the report itself — the host's
/// boot path must not be able to fail because of this.
pub async fn run_once(profile_path: &str, provider: &str, caps: &SeatCaps) {
    let profile_json = match std::fs::read_to_string(profile_path) {
        Ok(text) => text,
        Err(e) => {
            tracing::debug!(
                path = %profile_path,
                %e,
                "provider preflight skipped: could not read the kloo profile"
            );
            return;
        }
    };
    let endpoint = match provider_endpoint_from_profile(&profile_json, provider) {
        Some(endpoint) => endpoint,
        None => {
            tracing::debug!(
                path = %profile_path,
                provider,
                "provider preflight skipped: profile has no endpoint for this provider"
            );
            return;
        }
    };
    let url = format!("{}/models", endpoint);
    let client = reqwest::Client::new();
    let response = match client
        .get(&url)
        .timeout(std::time::Duration::from_secs(PREFLIGHT_TIMEOUT_SECS))
        .send()
        .await
    {
        Ok(response) => response,
        Err(e) => {
            tracing::debug!(%url, %e, "provider preflight skipped: model list unreachable");
            return;
        }
    };
    if !response.status().is_success() {
        tracing::debug!(
            %url,
            status = %response.status(),
            "provider preflight skipped: model list request was not 2xx"
        );
        return;
    }
    let body = match response.text().await {
        Ok(body) => body,
        Err(e) => {
            tracing::debug!(%url, %e, "provider preflight skipped: model list body unreadable");
            return;
        }
    };
    let provider_ids = match provider_ids_from_models_json(&body) {
        Some(ids) => ids,
        None => {
            tracing::debug!(%url, "provider preflight skipped: model list is not parseable");
            return;
        }
    };
    if provider_ids.is_empty() {
        // A parseable `{"data":[]}` is as much "unknown" as a body we could not read; the leases
        // comparisons already report nothing for an empty list, and skipping keeps the reason here.
        tracing::debug!(%url, "provider preflight skipped: model list named no ids");
        return;
    }
    match mismatch_report(caps, &provider_ids) {
        Some(report) => tracing::warn!(%url, %report, "provider/seat-cap mismatch"),
        None => tracing::debug!(
            %url,
            ids = provider_ids.len(),
            "provider preflight: every capped family is served"
        ),
    }
}

/// Spawn [`run_once`] on Tauri's runtime.
///
/// MUST be `tauri::async_runtime::spawn` and NEVER the bare per-crate spawn: Tauri's `setup`
/// closure runs with no tokio reactor of its own, and spawning directly onto that runtime there
/// panics `there is no reactor running` — which killed this host on boot earlier today. The caps
/// are taken BY VALUE because the spawned task outlives `setup` and cannot borrow the caller's
/// table.
pub fn spawn_provider_preflight(caps: SeatCaps) {
    let profile_path = crate::providers::kloo_cli::default_profile_path();
    tauri::async_runtime::spawn(async move {
        run_once(&profile_path, crate::providers::kloo_cli::DEFAULT_PROVIDER, &caps).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real profile shape at `~/etc/kloo.json`.
    const PROFILE: &str = r#"{"defaultProvider":"lokalai","providers":{"lokalai":{"endpoint":"https://lokalai.silverjrom.app/v1","apiKey":"sk-…","defaultModel":"next","models":{"next":{"contextLength":262144}}}}}"#;

    /// The real body of `GET https://lokalai.silverjrom.app/v1/models`.
    const MODELS: &str = r#"{"object":"list","data":[{"id":"glm-5.3-flash","object":"model","owned_by":"lokalai"},{"id":"qwen3.8-flash-next","object":"model","owned_by":"lokalai"},{"id":"next","object":"model","owned_by":"lokalai"}]}"#;

    fn ids(body: &str) -> Vec<String> {
        provider_ids_from_models_json(body).expect("parseable")
    }

    #[test]
    fn preflight_the_endpoint_comes_from_the_named_provider_block() {
        assert_eq!(
            provider_endpoint_from_profile(PROFILE, "lokalai").as_deref(),
            Some("https://lokalai.silverjrom.app/v1")
        );
        // The endpoint is read from the block named, not from `defaultProvider` — a caller that
        // asked about `openrouter` must not get lokalai's fleet.
        assert_eq!(
            provider_endpoint_from_profile(PROFILE, "openrouter"),
            None
        );
    }

    #[test]
    fn preflight_a_profile_without_that_provider_is_none() {
        // None means "do not check". An unparseable profile, a profile with no `providers` at all,
        // and a provider block with no `endpoint` all answer the same way, and none of them answer
        // with an empty string that a caller could concatenate into a URL.
        assert_eq!(provider_endpoint_from_profile("not json at all", "lokalai"), None);
        assert_eq!(provider_endpoint_from_profile("{}", "lokalai"), None);
        assert_eq!(
            provider_endpoint_from_profile(
                r#"{"providers":{"lokalai":{"apiKey":"sk-…"}}}"#,
                "lokalai"
            ),
            None
        );
        // A trailing slash is trimmed so `{endpoint}/models` cannot become `//models`.
        assert_eq!(
            provider_endpoint_from_profile(
                r#"{"providers":{"lokalai":{"endpoint":"https://lokalai.silverjrom.app/v1/"}}}"#,
                "lokalai"
            )
            .as_deref(),
            Some("https://lokalai.silverjrom.app/v1")
        );
    }

    #[test]
    fn preflight_model_ids_are_read_from_the_data_array() {
        assert_eq!(
            ids(MODELS),
            vec![
                "glm-5.3-flash".to_string(),
                "qwen3.8-flash-next".to_string(),
                "next".to_string()
            ]
        );
        // `deepseek-v4.1-flash` is genuinely absent from the real fleet's ids — the fact that
        // makes the mismatch check worth running.
        assert!(!ids(MODELS).iter().any(|id| id == "deepseek-v4.1-flash"));
    }

    #[test]
    fn preflight_unparseable_models_json_is_none_not_an_empty_list() {
        // Each of these is "unknown", which reports nothing, as opposed to `Some(vec![])` which a
        // caller could mistake for "the provider serves nothing".
        assert_eq!(provider_ids_from_models_json("<html>502 Bad Gateway</html>"), None);
        assert_eq!(provider_ids_from_models_json(""), None);
        assert_eq!(provider_ids_from_models_json(r#"{"object":"list"}"#), None);
        assert_eq!(provider_ids_from_models_json(r#"{"data":"nope"}"#), None);
        // A `data` array that exists but names nothing IS a list — empty, and the caller's
        // "unknown" guard turns it into a skip.
        assert_eq!(
            provider_ids_from_models_json(r#"{"object":"list","data":[]}"#),
            Some(Vec::new())
        );
    }

    #[test]
    fn preflight_a_missing_family_is_named_in_the_report() {
        let caps = SeatCaps::measured();
        let provider = vec!["glm-5.3-flash".to_string(), "qwen3.8-flash-next".to_string()];
        let report = mismatch_report(&caps, &provider).expect("a mismatch is reported");
        assert!(
            report.contains("deepseek-v4.1-flash"),
            "the report has to name the absent family: {report}"
        );
        // Two of three families ARE served, so this is "runs narrower", not "cannot run at all".
        assert!(
            !report.contains("cannot run at all"),
            "a partial mismatch must not claim the arena is dead: {report}"
        );

        // The other branch: nothing served, which is a different message because it is a different
        // condition — the arena cannot run at all rather than running narrower.
        let none_served = vec!["next".to_string(), "some-other-model".to_string()];
        let report = mismatch_report(&caps, &none_served).expect("a total mismatch is reported");
        assert!(report.contains("cannot run at all"), "{report}");
        assert!(report.contains("deepseek-v4.1-flash"), "{report}");
        assert!(report.contains("glm-5.3-flash"), "{report}");
        assert!(report.contains("qwen3.8-flash-next"), "{report}");

        // An empty provider list is "could not read the provider", so nothing is said — the
        // leases module's rule, inherited here rather than restated.
        assert_eq!(mismatch_report(&caps, &[]), None);
    }

    #[test]
    fn preflight_nothing_missing_produces_no_report() {
        let caps = SeatCaps::measured();
        let served: Vec<String> = caps
            .families()
            .map(|family| format!("{}-extra", family))
            .chain(caps.families().map(String::from))
            .collect();
        assert_eq!(mismatch_report(&caps, &served), None);
        // Exact matching, not prefix: `qwen3.8-flash-next-132k-fast` is NOT `qwen3.8-flash-next`.
        let variants_only = vec![
            "glm-5.3-flash-132k-fast".to_string(),
            "deepseek-v4.1-flash-fast".to_string(),
            "qwen3.8-flash-next-132k-fast".to_string(),
        ];
        let report = mismatch_report(&caps, &variants_only).expect("a variant is not the family");
        assert!(report.contains("cannot run at all"), "{report}");
    }
}