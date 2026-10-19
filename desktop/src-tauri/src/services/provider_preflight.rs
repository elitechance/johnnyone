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

/// The context lengths the arena's own assumptions are built on, as `(model, tokens)`.
///
/// These are the figures read from the provider's own `/v1/models` on 2026-10-19. The check exists
/// so that a provider-side change to a model's context is REPORTED rather than silently breaking a
/// context budget that was sized against the old number. `135168` is the number the provider gives
/// for `qwen3.8-flash-next` — not the round power-of-two figure the code assumed before it was
/// measured, which was never that model's context length at all.
pub const ASSUMED_CONTEXT_LENGTHS: &[(&str, u32)] = &[
    ("glm-5.3-flash", 262144),
    ("qwen3.8-flash-next", 135168),
];

/// Turn a stored `apiKey` value into the token to actually send.
///
/// On this machine the profile stores an **environment variable reference**, not a secret:
/// `"apiKey":"${LOKALAI_API}"`. kloo expands that at launch; a caller that attaches the stored
/// string verbatim sends the literal `${LOKALAI_API}` as the bearer token and is answered 401 —
/// indistinguishable from sending no key at all, which is what left this check inert.
///
/// `raw` exactly of the form `${NAME}` (leading `${`, trailing `}`, non-empty `NAME` between) is
/// resolved through `lookup`, trimmed, and is None when the variable is missing or blank — an unset
/// variable means "send the request unauthenticated", and NEVER the literal `${NAME}`, because a
/// placeholder sent as a bearer token is a guaranteed 401. Anything else is a literal key: returned
/// trimmed, None when blank.
///
/// The lookup is a PARAMETER rather than a direct environment read so the tests can drive it with a
/// closure — a test that mutated the process environment would race every other test in the same
/// binary.
pub fn expand_key_reference(
    raw: &str,
    lookup: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.starts_with("${") {
        // Anything with a leading `${` is trying to be a reference, so it is judged as one: an
        // empty name or a missing closing brace is a broken reference and answers None, NOT the
        // string itself — a half-written `${NAME` sent as a bearer token 401s exactly like the
        // whole placeholder does.
        let name = trimmed
            .strip_prefix("${")
            .and_then(|inner| inner.strip_suffix('}'))
            .map(|inner| inner.trim());
        let name = match name {
            // `${}` names no variable, and `${NAME` has no closing brace: both are broken
            // references and answer None rather than a string a caller could attach.
            Some(name) if !name.is_empty() => name,
            _ => return None,
        };
        return lookup(name)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
    }
    // `$NAME` (no braces) is not the reference syntax either, and is still not a token worth
    // sending — it names a variable rather than holding a key.
    if trimmed.starts_with('$') {
        return None;
    }
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The `apiKey` of the SAME named provider block the endpoint came from, or None when that block
/// carries no usable key. None means "send the request unauthenticated" — a public provider must
/// keep working — and never `Some("")`, which would send an empty bearer token instead.
///
/// The model list is not public on the provider the profile names: an unauthenticated
/// `GET /v1/models` is answered 401 every time, which left the whole check inert (and hid a
/// wrong-provider bug, because the wrong provider served `/models` without a key). The key is read
/// from the block named by `provider`, exactly like the endpoint, so a key is never sent to a
/// provider it does not belong to. It is a secret: it is attached to the request and nothing else —
/// never a tracing field, never part of a message.
///
/// The stored value is a `${VAR}` environment-variable reference on this machine, so it is expanded
/// through [`expand_key_reference`] before it is attached.
pub fn api_key_from_profile(profile_json: &str, provider: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(profile_json).ok()?;
    let key = value.get("providers")?.get(provider)?.get("apiKey")?.as_str()?;
    expand_key_reference(key, |name| std::env::var(name).ok())
}

/// The provider the profile itself names as default. None when the profile does
/// not name one — and None must mean "check nothing", not "guess a provider":
/// checking the wrong provider produced a false "serves NONE of the capped
/// families" alarm, which is worse than no check at all.
pub fn default_provider_from_profile(profile_json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(profile_json).ok()?;
    let name = value.get("defaultProvider")?.as_str()?;
    let trimmed = name.trim();
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

/// The `(id, context_length)` pairs in a `/v1/models` body, read from `data[]`.
///
/// Same shape and the same rule as [`provider_ids_from_models_json`]: None only when the body cannot
/// be parsed or has no `data` array, because an unreadable answer is not an empty fleet — the
/// difference is between "we do not know" and "the provider says every model is tiny", and only the
/// first one is true of a body we could not read.
///
/// An entry is paired only when BOTH an `id` and a usable `context_length` are present, so a model
/// that reports no context length is SKIPPED rather than reported as zero: "the provider did not
/// say" and "the provider says 0 tokens" are different facts, and reading the first as the second
/// would produce a mismatch line about a model the provider never made a claim about. A
/// `context_length` that is not a non-negative integer fitting a `u32` is likewise not a claim.
pub fn provider_context_lengths_from_models_json(body: &str) -> Option<Vec<(String, u32)>> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let data = value.get("data")?.as_array()?;
    Some(
        data.iter()
            .filter_map(|entry| {
                let id = entry.get("id")?.as_str()?;
                let length = entry.get("context_length")?.as_u64()?;
                let length = u32::try_from(length).ok()?;
                Some((id.to_string(), length))
            })
            .collect(),
    )
}

/// One line per model whose context length the provider reports as DIFFERENT from what the code
/// assumes, in `assumed`'s own order.
///
/// A context length is not a cosmetic number: a Hop's budget, and the point at which a run starts
/// compacting, are sized against it. If the provider shrinks a model — 262144 down to something
/// much smaller — nothing else in the host notices: the run just starts losing its head, so this is
/// the only place that condition can be reported at all.
///
/// A model the provider does not report is NOT a mismatch and is skipped: an absence is already
/// named by the family check in [`mismatch_report`], and naming the same model twice in two
/// different voices is how an operator learns to skip the log. An empty `provider` slice therefore
/// yields no lines at all, for the same reason the family check treats an empty list as "unknown"
/// rather than as "everything is wrong" — a provider we could not read has not contradicted anybody.
pub fn context_length_mismatches(
    assumed: &[(&str, u32)],
    provider: &[(String, u32)],
) -> Vec<String> {
    assumed
        .iter()
        .filter_map(|(model, assumed_ctx)| {
            let reported = provider
                .iter()
                .find(|(id, _)| id == model)?
                .1;
            (reported != *assumed_ctx).then(|| {
                format!(
                    "{model}: the arena assumes {assumed_ctx} tokens, the provider reports \
                     {reported}"
                )
            })
        })
        .collect()
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

/// What the preflight is about to ask, decided before any IO happens.
///
/// The url and the token are decided TOGETHER, out of the SAME provider block of the same profile,
/// so neither can drift from the other: the endpoint that gets `/models` appended and the key that
/// authenticates it are read from one block named by one `provider` argument, and a caller cannot
/// hold one without the other. That coupling is the entire point of this function.
///
/// This is the one question the three shipped-and-green versions of this check each answered
/// wrongly — and every one of them was wrong HERE, with everything around it passing:
///   1. it asked openrouter, a compiled-in constant, while the profile names lokalai;
///   2. then the right provider, but with no Authorization header — 401 every time;
///   3. then a header carrying the literal string `${LOKALAI_API}`, because the profile stores an
///      env reference and not a secret — the same 401.
/// Each was found only by running the binary, because no test ever asked "what request is this
/// about to send?". Making that question a pure, testable decision is what this struct is for: the
/// answer is now inspectable without a network, a profile on disk, or a process environment.
///
/// No endpoint means None — there is nothing to ask, and guessing a URL would turn a misconfigured
/// profile into a confident claim about a fleet nobody configured. A missing or unusable key is
/// `token: None`, which means "send it unauthenticated" and NEVER a placeholder: a `${VAR}` string
/// attached as a bearer token is a guaranteed 401 (failure 3 above). The token is a secret — it is
/// attached to the request and nothing else, never a tracing field, never part of a message.
///
/// `lookup` resolves a `${NAME}` reference and is a PARAMETER rather than a direct environment read
/// so a test can drive it; a test that mutated the process environment would race every other test
/// in the same binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightRequest {
    pub url: String,
    pub token: Option<String>,
}

/// Decide [`PreflightRequest`] — the url and the token the preflight is about to send — from the
/// profile JSON alone, before any IO happens. See [`PreflightRequest`] for why the two are decided
/// in one place.
pub fn preflight_request_plan(
    profile_json: &str,
    provider: &str,
    lookup: impl Fn(&str) -> Option<String>,
) -> Option<PreflightRequest> {
    let endpoint = provider_endpoint_from_profile(profile_json, provider)?;
    // The raw stored value, read from the SAME block the endpoint came from. A block with no
    // `apiKey` at all — or a non-string one — is not a reason to skip: it is a request with no
    // token, so the key is defaulted to "" (which expands to None) rather than short-circuiting
    // the whole plan.
    let raw = serde_json::from_str::<serde_json::Value>(profile_json)
        .ok()
        .and_then(|value| {
            value
                .get("providers")?
                .get(provider)?
                .get("apiKey")?
                .as_str()
                .map(str::to_string)
        })
        .unwrap_or_default();
    Some(PreflightRequest {
        url: format!("{}/models", endpoint),
        token: expand_key_reference(&raw, lookup),
    })
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
    // The whole request — which url, with which token — is decided by [`preflight_request_plan`]
    // before any IO happens, so the composition itself is testable rather than only observable in
    // the running binary. Nothing below rebuilds the url or re-reads the key: the plan is the only
    // source of both.
    let plan = match preflight_request_plan(&profile_json, provider, |name| std::env::var(name).ok())
    {
        Some(plan) => plan,
        None => {
            tracing::debug!(
                path = %profile_path,
                provider,
                "provider preflight skipped: profile has no endpoint for this provider"
            );
            return;
        }
    };
    let url = plan.url;
    let client = reqwest::Client::new();
    // Authenticated with the token from THE SAME provider block the url came from — the model list
    // is not public on the provider the profile names, so an unauthenticated GET is answered 401 and
    // the check never runs. A profile with no usable key still sends the request unauthenticated, so
    // a public provider keeps working; a 401 is already handled as "skip" below, never as an empty
    // fleet. The token itself is never logged and never part of a message.
    let mut request = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(PREFLIGHT_TIMEOUT_SECS));
    if let Some(token) = plan.token {
        request = request.bearer_auth(token);
    }
    let response = match request.send().await {
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
    // The SAME body already in hand — a second HTTP request for a field that is already sitting in
    // this response would be a second chance to fail, and a second thing to time out at boot.
    // A distinct message from the seat-cap one, so "the provider does not serve this family" and
    // "the provider resized a family it does serve" are separable in the log: they are different
    // failures with different fixes. No lines means no warning at all — a provider that agrees with
    // the table has earned silence, not a debug line per model on every boot.
    let lengths = provider_context_lengths_from_models_json(&body).unwrap_or_default();
    for report in context_length_mismatches(ASSUMED_CONTEXT_LENGTHS, &lengths) {
        tracing::warn!(%url, %report, "provider/context-length mismatch");
    }
}

/// Spawn [`run_once`] on Tauri's runtime, for the provider the profile names as its own default.
///
/// MUST be `tauri::async_runtime::spawn` and NEVER the bare per-crate spawn: Tauri's `setup`
/// closure runs with no tokio reactor of its own, and spawning directly onto that runtime there
/// panics `there is no reactor running` — which killed this host on boot earlier today. The caps
/// are taken BY VALUE because the spawned task outlives `setup` and cannot borrow the caller's
/// table.
///
/// The provider is read from the profile's own `defaultProvider`, NOT from any compiled-in
/// constant: the arena runs against whatever the profile names (`lokalai` on this machine), and
/// asking a provider the arena never uses raised a fleet-wide "serves NONE of the capped families"
/// alarm about a fleet that is not the arena's. The profile is read once here so the name and the
/// endpoint come from the same file. When the profile names no default provider there is nothing
/// to check, and this returns WITHOUT spawning — checking a guessed provider is worse than
/// checking nothing.
pub fn spawn_provider_preflight(caps: SeatCaps) {
    let profile_path = crate::providers::kloo_cli::default_profile_path();
    let profile_json = match std::fs::read_to_string(&profile_path) {
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
    let provider = match default_provider_from_profile(&profile_json) {
        Some(provider) => provider,
        None => {
            tracing::debug!(
                path = %profile_path,
                "provider preflight skipped: profile names no default provider, nothing to check"
            );
            return;
        }
    };
    tauri::async_runtime::spawn(async move {
        run_once(&profile_path, &provider, &caps).await;
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
    fn preflight_the_key_comes_from_the_same_provider_block_as_the_endpoint() {
        // The request must authenticate with the key of the block whose endpoint it is calling —
        // the model list is not public on the provider the profile names, so an unauthenticated
        // GET is answered 401 and the check never runs.
        assert_eq!(
            api_key_from_profile(PROFILE, "lokalai").as_deref(),
            Some("sk-…"),
            "the key comes from the same block whose endpoint provider_endpoint_from_profile returns"
        );
        assert_eq!(
            provider_endpoint_from_profile(PROFILE, "lokalai").as_deref(),
            Some("https://lokalai.silverjrom.app/v1")
        );
        // A key belongs to a NAMED block and is never shared across providers: asking about a
        // provider the profile has no block for must not leak lokalai's key to its endpoint.
        assert_eq!(api_key_from_profile(PROFILE, "openrouter"), None);
    }

    #[test]
    fn preflight_an_env_reference_key_is_expanded_from_the_environment() {
        // The profile stores `"apiKey":"${LOKALAI_API}"` — an ENVIRONMENT VARIABLE REFERENCE, not a
        // secret — and kloo expands it at launch. The preflight must expand it too: attaching the
        // stored string verbatim sent the literal `${LOKALAI_API}` as the bearer token and the host
        // logged the SAME 401 it logged with no key at all, so the check stayed inert.
        assert_eq!(
            expand_key_reference("${LOKALAI_API}", |name| {
                (name == "LOKALAI_API").then(|| "sk-real".to_string())
            })
            .as_deref(),
            Some("sk-real"),
            "the reference resolves to the environment's value, never the literal that 401s"
        );
        assert_ne!(
            expand_key_reference("${LOKALAI_API}", |_| Some("sk-real".to_string())).as_deref(),
            Some("${LOKALAI_API}"),
            "the placeholder itself is the bug and must never come back as a token"
        );
        // The lookup is asked for the name INSIDE the braces — `LOKALAI_API`, not the whole
        // `${LOKALAI_API}` string, which no environment would ever answer.
        let asked = std::cell::RefCell::new(Vec::new());
        let resolved = expand_key_reference("${LOKALAI_API}", |name| {
            asked.borrow_mut().push(name.to_string());
            Some("sk-real".to_string())
        });
        assert_eq!(resolved.as_deref(), Some("sk-real"));
        assert_eq!(*asked.borrow(), vec!["LOKALAI_API".to_string()]);
        // A key that is NOT a reference is a literal and comes back unchanged — the lookup is never
        // consulted for it, so a closure that would panic on any call proves it is not called.
        assert_eq!(
            expand_key_reference("sk-literal", |name| panic!(
                "a literal key must not be looked up in the environment (asked for {name:?})"
            ))
            .as_deref(),
            Some("sk-literal")
        );
    }

    #[test]
    fn preflight_an_unset_env_reference_is_none_not_the_literal_placeholder() {
        // None means "send the request UNAUTHENTICATED", never "send the placeholder": a `${…}`
        // string attached as a bearer token is a guaranteed 401, which is exactly the bug this
        // function exists to fix. An unset variable is therefore None…
        assert_eq!(expand_key_reference("${LOKALAI_API}", |_| None), None);
        // …and so is one whose value is blank or whitespace — a blank token is the same dead
        // request wearing a different hat.
        assert_eq!(
            expand_key_reference("${LOKALAI_API}", |_| Some(String::new())),
            None
        );
        assert_eq!(
            expand_key_reference("${LOKALAI_API}", |_| Some("   ".to_string())),
            None
        );
        // The malformed shapes are None too rather than a string a caller could send: `${}` names
        // no variable, `${LOKALAI_API` has no closing brace, and `$LOKALAI_API` is not the
        // reference syntax kloo expands — all three would be sent as nonsense tokens.
        assert_eq!(expand_key_reference("${}", |_| Some("sk-real".to_string())), None);
        assert_eq!(expand_key_reference("${LOKALAI_API", |_| Some("sk-real".to_string())), None);
        assert_eq!(expand_key_reference("$LOKALAI_API", |_| Some("sk-real".to_string())), None);
    }

    #[test]
    fn preflight_a_provider_block_with_no_key_is_none_not_an_empty_key() {
        // None means "send it unauthenticated", which keeps a public provider working. Some("")
        // would send an empty bearer token, which is a different and worse thing — so a block with
        // an endpoint but no `apiKey`, a blank key, a non-string key, and an unparseable profile
        // all answer None rather than a string a caller could attach to the request.
        assert_eq!(
            api_key_from_profile(
                r#"{"providers":{"lokalai":{"endpoint":"https://lokalai.silverjrom.app/v1"}}}"#,
                "lokalai"
            ),
            None
        );
        assert_eq!(
            api_key_from_profile(r#"{"providers":{"lokalai":{"apiKey":""}}}"#, "lokalai"),
            None
        );
        assert_eq!(
            api_key_from_profile(r#"{"providers":{"lokalai":{"apiKey":"  "}}}"#, "lokalai"),
            None
        );
        assert_eq!(
            api_key_from_profile(r#"{"providers":{"lokalai":{"apiKey":42}}}"#, "lokalai"),
            None
        );
        assert_eq!(api_key_from_profile("not json at all", "lokalai"), None);
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
    fn preflight_the_provider_checked_is_the_profiles_own_default() {
        // The arena runs against whatever the profile NAMES, not against a compiled-in constant.
        assert_eq!(
            default_provider_from_profile(PROFILE).as_deref(),
            Some("lokalai"),
            "the profile's own defaultProvider is the provider to check"
        );
        // …and specifically NOT the other provider the profile also carries a block for: the
        // presence of an `openrouter` block does not make it the default, and checking it raised a
        // fleet-wide alarm about a fleet the arena never uses.
        assert_ne!(
            default_provider_from_profile(PROFILE).as_deref(),
            Some("openrouter")
        );
        assert_eq!(
            default_provider_from_profile(
                r#"{"defaultProvider":"openrouter","providers":{"openrouter":{},"lokalai":{}}}"#
            )
            .as_deref(),
            Some("openrouter")
        );
    }

    #[test]
    fn preflight_a_profile_with_no_default_provider_checks_nothing() {
        // None means "check nothing", never "guess". A profile with providers but no
        // `defaultProvider`, an unparseable profile, and a profile naming nothing usable all answer
        // None, and the spawner returns without spawning for each of them.
        assert_eq!(
            default_provider_from_profile(
                r#"{"providers":{"lokalai":{"endpoint":"https://lokalai.silverjrom.app/v1"},"openrouter":{}}}"#
            ),
            None
        );
        assert_eq!(default_provider_from_profile("not json at all"), None);
        assert_eq!(default_provider_from_profile(""), None);
        assert_eq!(default_provider_from_profile("{}"), None);
        // A non-string or blank name is not a provider either.
        assert_eq!(default_provider_from_profile(r#"{"defaultProvider":42}"#), None);
        assert_eq!(default_provider_from_profile(r#"{"defaultProvider":"  "}"#), None);
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
    fn preflight_a_context_length_the_provider_reports_differently_is_named() {
        // A context length is not cosmetic: a Hop's budget and the point a run starts compacting
        // are sized against it, so a provider-side change has to be reported rather than silently
        // shrinking the window a run was planned for.
        let lines = context_length_mismatches(
            &[("qwen3.8-flash-next", 131072)],
            &[("qwen3.8-flash-next".to_string(), 135168)],
        );
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("qwen3.8-flash-next"));
        assert!(lines[0].contains("131072"));
        assert!(lines[0].contains("135168"));
        // A length the provider reports as the code assumes is not a mismatch.
        assert!(context_length_mismatches(
            &[("qwen3.8-flash-next", 131072)],
            &[("qwen3.8-flash-next".to_string(), 131072)],
        )
        .is_empty());
        // …and the table this module actually ships is TRUE as of 2026-10-19, read from the
        // provider's own /v1/models. If this assert ever fails the provider moved, which is
        // precisely the condition the check exists to surface.
        let reported = provider_context_lengths_from_models_json(
            r#"{"data":[{"id":"glm-5.3-flash","context_length":262144},{"id":"qwen3.8-flash-next","context_length":135168}]}"#,
        )
        .expect("a parseable body with a data array");
        assert_eq!(
            reported,
            vec![
                ("glm-5.3-flash".to_string(), 262144),
                ("qwen3.8-flash-next".to_string(), 135168)
            ]
        );
        assert!(context_length_mismatches(ASSUMED_CONTEXT_LENGTHS, &reported).is_empty());
    }

    #[test]
    fn preflight_a_model_the_provider_does_not_report_a_context_for_is_not_a_mismatch() {
        // An entry with no `context_length` made no claim, so it is SKIPPED - never read as 0,
        // which would manufacture a mismatch about a model the provider said nothing about.
        let reported = provider_context_lengths_from_models_json(
            r#"{"data":[{"id":"no-context-reported"},{"id":"glm-5.3-flash","context_length":262144}]}"#,
        )
        .expect("a parseable body with a data array");
        assert_eq!(reported, vec![("glm-5.3-flash".to_string(), 262144)]);
        // A provider we could not read has not contradicted anybody.
        assert!(context_length_mismatches(ASSUMED_CONTEXT_LENGTHS, &[]).is_empty());
        // An assumed model absent from the provider's list is already named by the family check
        // in `mismatch_report`; saying it twice in two different voices is how an operator learns
        // to skip the log.
        assert!(context_length_mismatches(
            &[("deepseek-v4.1-flash", 262144)],
            &[("glm-5.3-flash".to_string(), 262144)],
        )
        .is_empty());
        // None is "unreadable", which is NOT Some(vec![]): an empty list would read as a provider
        // that reports no contexts at all.
        assert_eq!(provider_context_lengths_from_models_json("not json"), None);
        assert_eq!(provider_context_lengths_from_models_json(r#"{"models":[]}"#), None);
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

    #[test]
    fn preflight_the_request_plan_for_the_real_profile_targets_lokalai_with_a_token() {
        // A profile of the REAL shape: the key is stored as an environment-variable REFERENCE, which
        // is what this machine actually holds (PROFILE's own key is a literal, so it cannot show the
        // expansion happening). The plan IS the whole request — which url, with which token — and it
        // is decided out of ONE provider block, so the two cannot drift apart.
        let profile = r#"{"defaultProvider":"lokalai","providers":{"lokalai":{"endpoint":"https://lokalai.silverjrom.app/v1","apiKey":"${LOKALAI_API}","defaultModel":"next","models":{"next":{"contextLength":262144}}}}}"#;
        let plan = preflight_request_plan(profile, "lokalai", |name| {
            (name == "LOKALAI_API").then(|| "sk-real".to_string())
        })
        .expect("the profile has an endpoint for the provider it names, so there is something to ask");
        assert_eq!(plan.url, "https://lokalai.silverjrom.app/v1/models");
        assert_eq!(
            plan.token.as_deref(),
            Some("sk-real"),
            "the stored reference is expanded, not sent as-is"
        );

        // Each of today's three failures, asserted explicitly, so this one test would have caught
        // all three of them:
        //   1. the wrong provider — openrouter was a compiled-in constant while the profile names
        //      lokalai, so the check asked a fleet the arena never uses.
        assert!(
            !plan.url.contains("openrouter"),
            "the url must be the profile's own provider's, not a compiled-in constant: {}",
            plan.url
        );
        //   2. no Authorization header at all — the model list is not public on lokalai, so an
        //      unauthenticated GET is answered 401 and the check never ran.
        assert!(
            plan.token.is_some(),
            "a plan with no token sends the request unauthenticated and is answered 401 every time"
        );
        //   3. the literal placeholder as the bearer token — the same 401, indistinguishable from
        //      failure 2 in the log.
        assert_ne!(
            plan.token.as_deref(),
            Some("${LOKALAI_API}"),
            "the stored reference string is not a token; sending it is a guaranteed 401"
        );

        // Planning the same profile for a provider it has NO block for is None — nothing to ask. Not
        // lokalai's endpoint, and definitely not lokalai's key: a key is never sent to a provider it
        // does not belong to.
        assert_eq!(preflight_request_plan(profile, "openrouter", |_| None), None);
        assert_eq!(preflight_request_plan(PROFILE, "openrouter", |_| None), None);
    }

    #[test]
    fn preflight_a_plan_is_none_when_there_is_nothing_to_ask() {
        // No endpoint means there is nothing to ask, EVEN when the block carries a usable key — the
        // key has nowhere to be sent, and guessing a url would turn a misconfigured profile into a
        // confident claim about a fleet nobody configured.
        assert_eq!(
            preflight_request_plan(
                r#"{"providers":{"lokalai":{"apiKey":"sk-…"}}}"#,
                "lokalai",
                |_| None
            ),
            None
        );
        // An unparseable profile is the same answer: no invented url, no invented token.
        assert_eq!(preflight_request_plan("not json at all", "lokalai", |_| None), None);
        assert_eq!(preflight_request_plan("", "lokalai", |_| None), None);

        // An endpoint but no key plans SOME with token None — a public provider must keep working
        // unauthenticated, so "no key" is a request, not a skip.
        let public = preflight_request_plan(
            r#"{"providers":{"lokalai":{"endpoint":"https://lokalai.silverjrom.app/v1"}}}"#,
            "lokalai",
            |_| None,
        )
        .expect("there is an endpoint, so there is something to ask");
        assert_eq!(public.url, "https://lokalai.silverjrom.app/v1/models");
        assert_eq!(public.token, None, "no key means unauthenticated, never Some(\"\")");

        // An unresolvable reference is likewise token None and SPECIFICALLY NOT the placeholder: a
        // `${MISSING}` sent as a bearer token is a guaranteed 401, which was failure 3 above.
        let unset = preflight_request_plan(
            r#"{"providers":{"lokalai":{"endpoint":"https://lokalai.silverjrom.app/v1","apiKey":"${MISSING}"}}}"#,
            "lokalai",
            |_| None,
        )
        .expect("the endpoint is there even though the key is not");
        assert_eq!(unset.url, "https://lokalai.silverjrom.app/v1/models");
        assert_eq!(unset.token, None, "an unset variable means unauthenticated");
        assert_ne!(
            unset.token.as_deref(),
            Some("${MISSING}"),
            "a placeholder sent as a bearer token is a guaranteed 401"
        );
    }
}
