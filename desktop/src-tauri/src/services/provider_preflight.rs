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
//! `deepseek-v4.1-flash` — which [`DEFAULT_SEAT_CAPS`](leases::DEFAULT_SEAT_CAPS) gives 2 seats
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
    families_absent_from_provider, provider_serves_no_capped_family, SeatCaps, SharedLeasePool,
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

/// Whether an entry in the provider's `data[]` is a model the provider will actually SERVE.
///
/// **"Listed" is not "available."** `provider_ids_from_models_json` used to answer the first claim
/// and the callers read it as the second, which is the same mistake that reported the payload
/// carrying "no concurrency/slot/queue keys" because those three words were absent rather than
/// because nothing about readiness was in it. Re-measured 2026-10-19, every entry of the real
/// `/v1/models` payload carries readiness fields, under these field names:
///
/// ```text
/// glm-5.3-flash       status="servable"  resident=true  admissible=true  configured=true
/// qwen3.8-flash-next  status="servable"  resident=true  admissible=true  configured=true
/// ```
///
/// (all nine entries read the same that day) — so this excludes nothing today and changes no
/// current behaviour. What it closes is the state where a model is LISTED but refusing work, which
/// is precisely the state the arena would otherwise lease seats into: a cap for a model that is
/// present-but-refusing is a seat that can be leased and never served, and the mismatch check would
/// call it served because the id was in the list.
///
/// The rules, and why each is shaped the way it is:
///   * `status`, when present and a string, must equal `"servable"` — exact and case-sensitive,
///     because a status we do not recognise is not a promise. `"SERVABLE"` is not a claim we can
///     honour; `"loading"`, `"draining"`, `"queued"` and anything else are refusals.
///   * `admissible` and `resident`, when present and booleans, must each be `true`.
///   * a field that is ABSENT, `null`, or of an unexpected type is NOT a claim, and so is not a
///     reason to exclude the model. An unknown or missing field is read as **silence**, never as a
///     refusal — excluding a model on a field we do not understand would invent an outage. In
///     particular an OLDER provider that reports none of these fields keeps working exactly as it
///     does today: an entry making no readiness claim at all is servable.
pub fn model_is_servable(entry: &serde_json::Value) -> bool {
    if let Some(status) = entry.get("status").and_then(|value| value.as_str()) {
        if status != "servable" {
            return false;
        }
    }
    for flag in ["admissible", "resident"] {
        if let Some(claim) = entry.get(flag).and_then(|value| value.as_bool()) {
            if !claim {
                return false;
            }
        }
    }
    true
}

/// The model ids in a `/v1/models` body, read from `data[].id`, of the models the provider says it
/// will serve.
///
/// Returns None when the body cannot be parsed or has no `data` array — an unreadable answer is NOT
/// an empty fleet, and the difference decides whether the mismatch check runs at all. Collapsing
/// "unreadable" into `vec![]` would let the caller either report every capped family as absent
/// (a fleet-wide alarm from one bad HTTP response) or report nothing by accident rather than by
/// design. `None` makes the skip an explicit decision at the call site.
///
/// Ids that are not strings are skipped rather than fatal, since a provider adding a field shape
/// is not a reason to stop checking the ids it did send.
///
/// An entry is dropped when [`model_is_servable`] says it is not servable: being listed while
/// refusing work is not being available, and a model in that state must be REPORTED as absent by
/// [`mismatch_report`] rather than counted as a family the arena can lease seats on.
pub fn provider_ids_from_models_json(body: &str) -> Option<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let data = value.get("data")?.as_array()?;
    Some(
        data.iter()
            .filter(|entry| model_is_servable(entry))
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

/// Each alias in the profile's `models` block paired with the model id it
/// resolves to. A STRING value is the target (`"next": "qwen3.8-flash-next"`);
/// an OBJECT value is per-model config, not a redirect, so the alias key is
/// itself the model id (`"next": {"contextLength": 262144}`). Any other value
/// type is skipped rather than guessed at.
///
/// Read from the SAME named provider block the endpoint and the key come from, so an alias is
/// judged against the fleet it will actually be sent to. Sorted by BYTE ORDER OF THE ALIAS.
///
/// Byte order by alias is the ONLY stable order available once the profile is parsed: `serde_json`'s
/// default `Map` is a `BTreeMap`, so the order the operator typed the keys in is not recoverable
/// from the parsed map at all, and no ordering could claim to reproduce it. That is exactly why
/// `glimmer` precedes `glm` here — "i" (0x69) < "m" (0x6d).
///
/// Comparing the ALIAS ALONE rather than the whole pair still matters for that reason: the pair's
/// second element is a MODEL ID the operator never typed, so letting it participate in the order
/// would sort the operator's own names by a string of theirs that does not exist — repointing one
/// alias's target would move that alias relative to the others, changing the order of the operator's
/// names with no edit to any name of theirs. An alias is a JSON object key and therefore UNIQUE, so
/// the alias-only comparison is a total order on everything this reader can produce.
///
/// An empty vec for an unparseable profile, a provider the profile has no block for, or a block
/// with no `models` — never a panic, and never a guess. Those three are "the profile made no alias
/// claim", which is the same silence as a provider that reports no readiness fields: absence is not
/// a claim about a model, so it must not become a warning about one.
pub fn alias_targets_from_profile(profile_json: &str, provider: &str) -> Vec<(String, String)> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(profile_json) else {
        return Vec::new();
    };
    let Some(models) = value
        .get("providers")
        .and_then(|providers| providers.get(provider))
        .and_then(|block| block.get("models"))
        .and_then(|models| models.as_object())
    else {
        return Vec::new();
    };
    let mut pairs = models
        .iter()
        .filter_map(|(alias, target)| {
            let target = match target {
                // A string value IS the redirect: the alias names the model the provider is asked
                // for, and that id is what has to exist in the fleet.
                serde_json::Value::String(target) => target.as_str(),
                // An object value is per-model CONFIG (`{"contextLength": 262144}`), not a
                // redirect — kloo resolves such an alias to a model of the same name, so the KEY
                // is the model id. Reading the object as "no target" would leave the most common
                // profile shape unchecked, which is the hole this function exists to close.
                serde_json::Value::Object(_) => alias.as_str(),
                // A number, array, bool or null is not a shape this module claims to understand,
                // so it is skipped rather than guessed at.
                _ => return None,
            };
            let target = target.trim();
            (!target.is_empty()).then(|| (alias.clone(), target.to_string()))
        })
        .collect::<Vec<_>>();
    // Sorted by ALIAS — the operator's own name — so the report is the same line every boot rather
    // than whatever order the map's keys happened to come out in. serde_json's `Map` is feature-gated:
    // WITHOUT `preserve_order` it is a `BTreeMap`, whose walk is already in alias byte order, so today
    // this sort is a no-op; WITH `preserve_order` it becomes an insertion-ordered `IndexMap`, and an
    // unsorted walk would then return the aliases in whatever order the profile file happens to spell
    // them — which changes the shape of the warning line with no code change only WHEN that spelling
    // order is not already byte order. A profile whose operator happened to type the aliases in byte
    // order would emit a byte-identical line either way, so the exposure is a property of the profile
    // file, not of every profile file; it is exactly the profiles that spell their aliases any other
    // way that would start warning in a different order the moment the feature is unified on. A Cargo feature can be switched
    // on by ANY crate in the dependency tree through feature unification without touching this crate's
    // own Cargo.toml, so the explicit sort is what makes the order a property of THIS function rather
    // than a property of a dependency's feature flags. BYTE ORDER BY ALIAS is the only stable order
    // available once the map is parsed: the order the operator typed the keys in is not recoverable
    // from a parsed `Map` at all, so no ordering could claim to reproduce it. That is exactly why
    // `glimmer` precedes `glm` here — "i" < "m" — and not because the profile lists glimmer first.
    // `sort_by` on the alias ALONE, NOT a plain `sort` on the pair, still matters for that reason:
    // the pair's second element is a MODEL ID the operator never typed, so letting it participate in
    // the order would sort the operator's own names by a string of theirs that does not exist —
    // repointing the target under one alias would move that alias relative to the others, changing
    // the order of the operator's names with no edit to any name of theirs. An alias is a JSON object
    // key and therefore UNIQUE, so the alias-only comparison is a total order on everything this
    // reader can produce, and `sort_by` (stable) still gives byte-identical output across boots.
    pairs.sort_by(|left, right| left.0.cmp(&right.0));
    pairs
}

/// The aliases whose target the provider does not serve, as ONE line for the log, or `None` when
/// every alias resolves to something served. `None` also for an empty provider list, exactly as
/// [`families_absent_from_provider`] does: an unreadable fleet is unknown, not empty.
///
/// The line names BOTH the alias and its target, because the fix differs depending on which of the
/// two is wrong: the alias is the user's own name for a model, while the target is what the
/// provider was actually asked for. `"glimmer"` alone would send the operator to the provider's
/// console; `"muse-glimmer-30b"` alone would send them to their own profile. Several mismatches are
/// joined with `; ` into one line, so one boot produces one warning to read rather than one per
/// alias.
///
/// **This must NOT be wired to the lease pool.** An alias is not a capped family: there is no seat
/// count to refuse, and `acquire` is never asked for an alias — it is called with a family from the
/// caps table. So unlike [`mismatch_report`], whose conclusive answer feeds
/// [`LeasePool::mark_unserved`], this comparison is LOG-ONLY and there is nothing for
/// `mark_unserved` to do with its result. Do not add it to `unserved_from_models_body`, and do not
/// feed it to `mark_unserved`: refusing an alias would be refusing a name the pool has never heard,
/// which either no-ops or — worse, if a family ever shares a spelling with an alias — narrows the
/// arena on the strength of a profile typo.
///
/// [`LeasePool::mark_unserved`]: leases::LeasePool::mark_unserved
pub fn alias_mismatch_report(
    aliases: &[(String, String)],
    provider_ids: &[String],
) -> Option<String> {
    // An empty fleet is "we could not read the provider", not "the provider serves nothing" — the
    // leases module's rule, inherited rather than restated. Reporting here would turn one bad HTTP
    // response into an alarm about every alias in the profile.
    if provider_ids.is_empty() {
        return None;
    }
    let problems = aliases
        .iter()
        .filter(|(_, target)| !provider_ids.iter().any(|id| id == target))
        .map(|(alias, target)| {
            format!("alias \"{alias}\" points at \"{target}\", which this provider does not serve")
        })
        .collect::<Vec<_>>();
    (!problems.is_empty()).then(|| problems.join("; "))
}

/// The families to refuse, decided from one model-list body. `None` means
/// INCONCLUSIVE (unparseable body, or a parseable body that named no ids) and
/// the caller must leave the pool untouched. `Some(list)` is conclusive, and an
/// empty list is a real answer: every capped family is served, so nothing is refused.
///
/// The distinction is the whole content of this function. `None` and `Some(vec![])` look alike to
/// a caller that only checks `is_empty()`, but they mean opposite things to the pool: `None` is
/// "we could not ask the provider", and marking on it would refuse the entire fleet because one
/// HTTP response failed or a laptop booted offline; `Some(vec![])` is "the provider answered and
/// serves everything", which must CLEAR a refusal left by the last outage. The two inconclusive
/// shapes are therefore both `None` here — an unparseable body (`provider_ids_from_models_json`
/// answers None) and a parseable body that named no ids at all (an empty vec) — and the conclusive
/// list is the leases module's own [`families_absent_from_provider`], not a second copy of the
/// comparison, so exact-not-prefix matching stays defined in exactly one place.
pub fn unserved_from_models_body(caps: &SeatCaps, body: &str) -> Option<Vec<String>> {
    let ids = provider_ids_from_models_json(body)?;
    if ids.is_empty() {
        return None;
    }
    Some(families_absent_from_provider(caps, &ids))
}

/// Why this read is being skipped, as the operator will read it: the fixed sentence plus the cause
/// the transport gave, for the three outcomes that are a skip. `None` for `Body`, which is not a
/// skip at all.
///
/// The cause is the entire content of the skip line — DNS, a refused connection and a timeout are
/// three different failures with three different fixes, and collapsing them into one sentence is
/// what made the line useless. This is the ONE place those sentences are assembled: the skip arms of
/// [`run_once_with`] log this value rather than building their own text, so the words a test reads
/// are the words the operator reads.
pub fn skip_reason(outcome: &ModelListFetch) -> Option<String> {
    match outcome {
        ModelListFetch::Unreachable(cause) => {
            Some(format!("model list unreachable: {cause}"))
        }
        ModelListFetch::Unreadable(cause) => {
            Some(format!("model list body unreadable: {cause}"))
        }
        ModelListFetch::NotSuccess(status) => {
            Some(format!("model list request was not 2xx: {status}"))
        }
        ModelListFetch::Body(_) => None,
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

/// What one model-list read came back as. The preflight's safety property is
/// entirely about which of these leave the pool untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelListFetch {
    /// The request never got an answer (DNS, refused, timeout) — with the transport error's own
    /// words, because which of those three it was IS the thing this log line exists to say.
    Unreachable(String),
    /// An answer that was not 2xx; the status is kept for the log line.
    NotSuccess(u16),
    /// A 2xx whose body could not be read — with the read error's own words, for the same reason.
    Unreadable(String),
    /// A 2xx body, parseable or not — `unserved_from_models_body` decides.
    Body(String),
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

/// Read the profile, fetch the provider's model list once, log the report, and TELL THE POOL.
///
/// Every failure is silent-but-logged at debug: a preflight that cannot reach the provider must not
/// block or alarm the host, and an empty/unreadable answer means "unknown", which reports nothing.
/// Nothing here returns a `Result` and nothing is `warn!` except the report itself — the host's
/// boot path must not be able to fail because of this.
///
/// The pool is passed instead of a caps table so the caps the report is judged against are the caps
/// the pool is actually enforcing (`pool.caps()`, built from `seat_caps_from_db` on this host)
/// rather than a freshly-built default table that may describe a different fleet — and so the one
/// conclusive answer can be acted on: [`LeasePool::mark_unserved`] refuses those families at
/// `acquire`. EVERY early return below leaves the pool untouched; a provider hiccup at boot must
/// not refuse the whole fleet.
///
/// [`LeasePool::mark_unserved`]: leases::LeasePool::mark_unserved
pub async fn run_once_with<F, Fut>(
    profile_path: &str,
    provider: &str,
    pool: &SharedLeasePool,
    fetch: F,
) where
    F: FnOnce(&PreflightRequest) -> Fut,
    Fut: std::future::Future<Output = ModelListFetch>,
{
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
    // The answer comes ONLY from `fetch` — this function never builds a request itself, which is
    // what makes the safety property below testable without a network. The plan is BORROWED into
    // the closure so a caller that never reaches `fetch` (no profile, no endpoint) can still be
    // proven not to have made a request, and `url` below is the same url that was handed over.
    let outcome = fetch(&plan).await;
    let url = &plan.url;
    // A skip logs [`skip_reason`]'s value — the fixed sentence AND the cause — rather than
    // assembling its own text here, so the words a test reads are the words the operator reads and
    // an arm cannot silently drop its cause.
    let body = match &outcome {
        ModelListFetch::Body(body) => body.clone(),
        other => {
            let reason = skip_reason(other).unwrap_or_default();
            tracing::debug!(%url, %reason, "provider preflight skipped");
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
    match mismatch_report(pool.caps(), &provider_ids) {
        Some(report) => tracing::warn!(%url, %report, "provider/seat-cap mismatch"),
        None => tracing::debug!(
            %url,
            ids = provider_ids.len(),
            "provider preflight: every capped family is served"
        ),
    }
    // The one conclusive answer this function can get — a 2xx body that parsed and named at least
    // one id — is now ACTED on rather than only logged: the pool is told which capped families the
    // provider will not serve, so `acquire` refuses them instead of leasing seats that can never
    // run work. Every early return above leaves the pool untouched, which is the whole safety
    // property here: a provider hiccup at boot must not refuse the whole fleet. `mark_unserved`
    // REPLACES the set, so a body naming every family clears a previous refusal — a provider that
    // came back is not still being refused for the last one's outage.
    // The aliases are checked against the SAME `provider_ids` already in hand — no second request,
    // and no second parse of a body that has already given up its ids. Deliberately placed BEFORE
    // the `mark_unserved` call and never feeding it: an alias is not a capped family, so there is
    // no seat count to refuse and `acquire` is never asked for an alias. This comparison ends at
    // the log.
    if let Some(report) = alias_mismatch_report(
        &alias_targets_from_profile(&profile_json, provider),
        &provider_ids,
    ) {
        tracing::warn!(%url, %report, "provider/alias mismatch");
    }
    if let Some(families) = unserved_from_models_body(pool.caps(), &body) {
        pool.mark_unserved(families);
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

/// Read the profile, fetch the provider's model list once, log the report, and TELL THE POOL.
///
/// Every failure is silent-but-logged at debug: a preflight that cannot reach the provider must not
/// block or alarm the host, and an empty/unreadable answer means "unknown", which reports nothing.
/// Nothing here returns a `Result` and nothing is `warn!` except the report itself — the host's
/// boot path must not be able to fail because of this.
///
/// The pool is passed instead of a caps table so the caps the report is judged against are the caps
/// the pool is actually enforcing (`pool.caps()`, built from `seat_caps_from_db` on this host)
/// rather than a freshly-built default table that may describe a different fleet — and so the one
/// conclusive answer can be acted on: [`LeasePool::mark_unserved`] refuses those families at
/// `acquire`. EVERY early return in [`run_once_with`] leaves the pool untouched; a provider hiccup
/// at boot must not refuse the whole fleet.
///
/// This is the real-IO wrapper: it exists ONLY to turn a [`PreflightRequest`] into a
/// [`ModelListFetch`] over reqwest. It decides nothing — every decision, including which outcomes
/// may touch the pool, lives in [`run_once_with`], which the tests drive with a closure.
///
/// [`LeasePool::mark_unserved`]: leases::LeasePool::mark_unserved
pub async fn run_once(profile_path: &str, provider: &str, pool: &SharedLeasePool) {
    run_once_with(profile_path, provider, pool, |plan| {
        let url = plan.url.clone();
        let token = plan.token.clone();
        async move {
            let client = reqwest::Client::new();
            // Authenticated with the token from THE SAME provider block the url came from — the model
            // list is not public on the provider the profile names, so an unauthenticated GET is
            // answered 401 and the check never runs. A profile with no usable key still sends the
            // request unauthenticated, so a public provider keeps working; a 401 is already handled as
            // "skip" by the caller, never as an empty fleet. The token itself is never logged and never
            // part of a message.
            let mut request = client
                .get(&url)
                .timeout(std::time::Duration::from_secs(PREFLIGHT_TIMEOUT_SECS));
            if let Some(token) = token {
                request = request.bearer_auth(token);
            }
            let response = match request.send().await {
                Ok(response) => response,
                Err(e) => return ModelListFetch::Unreachable(e.to_string()),
            };
            if !response.status().is_success() {
                return ModelListFetch::NotSuccess(response.status().as_u16());
            }
            match response.text().await {
                Ok(body) => ModelListFetch::Body(body),
                Err(e) => return ModelListFetch::Unreadable(e.to_string()),
            }
        }
    })
    .await;
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
///
/// Takes the [`SharedLeasePool`] itself rather than a caps table: the spawned task outlives `setup`
/// and cannot borrow a caller's table, and the pool is what has to be told — the caps come off it
/// (`pool.caps()`), so the comparison and the refusal are made against the same table the arena
/// leases against.
pub fn spawn_provider_preflight(pool: SharedLeasePool) {
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
        run_once(&profile_path, &provider, &pool).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::leases::{LeasePool, SeatError};

    /// The real profile shape at `~/etc/kloo.json`.
    const PROFILE: &str = r#"{"defaultProvider":"lokalai","providers":{"lokalai":{"endpoint":"https://lokalai.silverjrom.app/v1","apiKey":"sk-…","defaultModel":"next","models":{"next":{"contextLength":262144}}}}}"#;

    /// The real body of `GET https://lokalai.silverjrom.app/v1/models`.
    const MODELS: &str = r#"{"object":"list","data":[{"id":"glm-5.3-flash","object":"model","owned_by":"lokalai"},{"id":"qwen3.8-flash-next","object":"model","owned_by":"lokalai"},{"id":"next","object":"model","owned_by":"lokalai"}]}"#;

    fn ids(body: &str) -> Vec<String> {
        provider_ids_from_models_json(body).expect("parseable")
    }

    /// A profile naming the endpoint the preflight will be pointed at, written to a file of THIS
    /// test's own so concurrent tests in the same binary cannot read each other's profile.
    fn profile_named_after_the_test(test_name: &str) -> String {
        let path = std::env::temp_dir().join(format!("provider_preflight_{test_name}.json"));
        std::fs::write(
            &path,
            r#"{"defaultProvider":"lokalai","providers":{"lokalai":{"endpoint":"https://example.invalid/v1","apiKey":"sk-test"}}}"#,
        )
        .expect("the temp dir is writable");
        path.to_string_lossy().into_owned()
    }

    /// Drive [`run_once_with`] with a canned answer.
    ///
    /// Driven with `tokio::runtime::Builder::new_current_thread` rather than
    /// `tauri::async_runtime::block_on`: `block_on` panics when called from a thread that is
    /// ALREADY inside a runtime, and these are plain `#[test]`s — a fresh single-threaded runtime
    /// per drive is the one executor that cannot be re-entrant. It is the same reactor
    /// `#[tokio::test]` gives the rest of this crate's async tests.
    fn drive(profile_path: &str, pool: &SharedLeasePool, outcome: ModelListFetch) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime to drive the preflight on");
        let profile = profile_path.to_string();
        runtime.block_on(run_once_with(
            &profile,
            "lokalai",
            pool,
            move |_plan| async move { outcome },
        ));
    }

    /// Drive [`run_once_with`] with a caller-supplied closure, for the test that has to observe
    /// whether the closure was reached at all. Same runtime as [`drive`], for the same reason.
    fn drive_with<F, Fut>(profile_path: &str, pool: &SharedLeasePool, fetch: F)
    where
        F: FnOnce(&PreflightRequest) -> Fut,
        Fut: std::future::Future<Output = ModelListFetch>,
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime to drive the preflight on");
        runtime.block_on(run_once_with(profile_path, "lokalai", pool, fetch));
    }

    /// The (a) half of the safety property: a fresh pool is still refusing nothing.
    fn assert_fresh_pool_untouched(profile_path: &str, outcome: ModelListFetch) {
        let pool = LeasePool::with_default_caps();
        assert!(pool.unserved().is_empty(), "a fresh pool refuses nothing");
        drive(profile_path, &pool, outcome);
        assert!(
            pool.unserved().is_empty(),
            "an inconclusive read must not refuse any family, let alone the whole fleet"
        );
    }

    /// The (b) half: a refusal made before the read is STILL there after it. A hiccup must not
    /// clear a refusal any more than it may create one — `mark_unserved` REPLACES the set, so a
    /// path that reached it with an empty list would silently un-refuse a family the provider is
    /// still known not to serve.
    fn assert_previous_refusal_survives(profile_path: &str, outcome: ModelListFetch) {
        let pool = LeasePool::with_default_caps();
        pool.mark_unserved(["deepseek-v4.1-flash".to_string()]);
        drive(profile_path, &pool, outcome);
        assert!(
            pool.unserved()
                .contains("deepseek-v4.1-flash"),
            "an inconclusive read must leave an existing refusal in place"
        );
    }

    /// Both halves at once, which is the point of every `paths_*` test here.
    fn assert_pool_untouched_both_ways(profile_path: &str, outcome: ModelListFetch) {
        assert_fresh_pool_untouched(profile_path, outcome.clone());
        assert_previous_refusal_survives(profile_path, outcome);
    }

    #[test]
    fn paths_an_unreachable_provider_leaves_the_pool_untouched() {
        // DNS failed, the connection was refused, the 5s timeout expired — the provider never
        // answered, so nothing about the fleet is known and the pool keeps leasing exactly as it
        // did. Marking here would refuse the whole arena because a laptop booted offline.
        let profile = profile_named_after_the_test(
            "an_unreachable_provider_leaves_the_pool_untouched",
        );
        assert_pool_untouched_both_ways(
            &profile,
            ModelListFetch::Unreachable("dns error".to_string()),
        );
    }

    #[test]
    fn paths_a_non_2xx_leaves_the_pool_untouched() {
        // A 401 (the failure this module has already shipped three times over) or a 502 gateway
        // page is an ANSWER ABOUT THE TRANSPORT, not about which models exist. The status is kept
        // for the log line and decides nothing else.
        let profile = profile_named_after_the_test("a_non_2xx_leaves_the_pool_untouched");
        assert_pool_untouched_both_ways(&profile, ModelListFetch::NotSuccess(401));
        assert_pool_untouched_both_ways(&profile, ModelListFetch::NotSuccess(502));
    }

    #[test]
    fn paths_an_unreadable_body_leaves_the_pool_untouched() {
        // 2xx all the way, then the stream dies before the body arrives. "We could not read it" is
        // not "it serves nothing", and an empty list handed to `mark_unserved` here would clear a
        // refusal the last conclusive read earned.
        let profile = profile_named_after_the_test("an_unreadable_body_leaves_the_pool_untouched");
        assert_pool_untouched_both_ways(
            &profile,
            ModelListFetch::Unreadable("body stream terminated".to_string()),
        );
    }

    #[test]
    fn paths_an_unparseable_body_leaves_the_pool_untouched() {
        // A 2xx whose body is HTML or truncated JSON. `unserved_from_models_body` answers None for
        // this, and the caller must honour that rather than defaulting to an empty list — which is
        // exactly the mutation this test exists to catch, since an `unwrap_or_default()` here would
        // refuse the whole fleet and every other test in the crate would stay green.
        let profile = profile_named_after_the_test("an_unparseable_body_leaves_the_pool_untouched");
        assert_pool_untouched_both_ways(&profile, ModelListFetch::Body("{".to_string()));
        assert_pool_untouched_both_ways(
            &profile,
            ModelListFetch::Body("<html>502 Bad Gateway</html>".to_string()),
        );
    }

    #[test]
    fn paths_an_empty_model_list_leaves_the_pool_untouched() {
        // A parseable `{"data":[]}` named NO ids, so the provider made no claim about the fleet at
        // all. This is the shape most likely to be mistaken for conclusive, because the body parsed
        // fine — but "said nothing" is not "serves nothing", and marking from it would refuse every
        // capped family on the strength of one empty response.
        let profile = profile_named_after_the_test("an_empty_model_list_leaves_the_pool_untouched");
        assert_pool_untouched_both_ways(
            &profile,
            ModelListFetch::Body(r#"{"object":"list","data":[]}"#.to_string()),
        );
    }

    #[test]
    fn paths_a_conclusive_body_marks_the_pool() {
        // The ONE path allowed to touch the pool: a 2xx body that parsed and named at least one id.
        // `glm-5.3-flash` and `qwen3.8-flash-next` are served, `deepseek-v4.1-flash` is not, so the
        // pool ends up refusing exactly that one family — no more (that would narrow the arena for
        // no reason the provider gave) and no less (that would lease seats that can never run).
        let profile = profile_named_after_the_test("a_conclusive_body_marks_the_pool");
        let pool = LeasePool::with_default_caps();
        drive(
            &profile,
            &pool,
            ModelListFetch::Body(
                r#"{"object":"list","data":[
                    {"id":"glm-5.3-flash","status":"servable"},
                    {"id":"qwen3.8-flash-next","status":"servable"}
                ]}"#
                .to_string(),
            ),
        );
        assert_eq!(
            pool.unserved(),
            ["deepseek-v4.1-flash".to_string()].into_iter().collect(),
            "exactly the family the provider does not serve is refused"
        );
        // And the refusal is real at the point of use, not just a set nobody consults.
        assert!(
            matches!(
                pool.acquire("deepseek-v4.1-flash", 1),
                Err(SeatError::ProviderWillNotServe { .. })
            ),
            "the refused family is refused on provider grounds"
        );
    }

    #[test]
    fn paths_a_fully_served_body_clears_a_previous_refusal() {
        // The other half of acting on a conclusive answer: `mark_unserved` REPLACES, so a body
        // naming every capped family clears the refusal the last outage left. Refusing to clear
        // would make one bad boot permanent — a provider that came back must not still be refused.
        let profile = profile_named_after_the_test("a_fully_served_body_clears_a_previous_refusal");
        let pool = LeasePool::with_default_caps();
        pool.mark_unserved(["deepseek-v4.1-flash".to_string()]);
        let served = format!(
            r#"{{"object":"list","data":[{}]}}"#,
            pool.caps()
                .families()
                .map(|family| format!(r#"{{"id":"{family}","status":"servable"}}"#))
                .collect::<Vec<_>>()
                .join(",")
        );
        drive(&profile, &pool, ModelListFetch::Body(served));
        assert!(pool.unserved().is_empty(), "the refusal was cleared");
        let lease = pool
            .acquire("deepseek-v4.1-flash", 1)
            .expect("the family leases again once the provider serves it");
        assert_eq!(lease.family(), "deepseek-v4.1-flash");
    }

    #[test]
    fn paths_a_missing_profile_never_calls_fetch() {
        // No profile means nothing to ask: no endpoint, no provider block, no url. The closure is
        // never invoked — a fetch here would be a request to a guessed endpoint, which is the very
        // first failure this module shipped with — and the pool is untouched in both directions.
        let profile = std::env::temp_dir().join(
            "provider_preflight_a_missing_profile_never_calls_fetch-does-not-exist.json",
        );
        assert!(
            !profile.exists(),
            "the test's own path must not exist: {profile:?}"
        );
        let pool = LeasePool::with_default_caps();
        pool.mark_unserved(["deepseek-v4.1-flash".to_string()]);
        let asked = std::sync::atomic::AtomicBool::new(false);
        drive_with(
            profile.to_string_lossy().as_ref(),
            &pool,
            |_plan| {
                asked.store(true, std::sync::atomic::Ordering::SeqCst);
                async move { ModelListFetch::Unreachable("dns error".to_string()) }
            },
        );
        assert!(
            !asked.load(std::sync::atomic::Ordering::SeqCst),
            "with no profile to read there is no request to make"
        );
        assert!(
            pool.unserved()
                .contains("deepseek-v4.1-flash"),
            "a missing profile leaves the pool exactly as it was"
        );
    }

    #[test]
    fn reason_an_unreachable_read_names_the_transport_error() {
        // The line's whole purpose is naming WHY the read failed — DNS, a refused connection and a
        // timeout are three different failures with three different fixes — so the reason the
        // operator reads carries the transport error's own words, not just a fixed sentence.
        let reason = skip_reason(&ModelListFetch::Unreachable(
            "dns error: no record".to_string(),
        ))
        .expect("an unreachable read is a skip and has a reason");
        assert!(
            reason.contains("model list unreachable"),
            "the reason names the outcome the operator sees: {reason}"
        );
        assert!(
            reason.contains("dns error: no record"),
            "the reason carries the cause the transport gave: {reason}"
        );
    }

    #[test]
    fn reason_an_unreadable_body_names_its_error() {
        // Same shape for a 2xx whose body could not be read: the read error's own words belong in
        // the line, and the sentence is the unreadable one, not the unreachable one.
        let reason = skip_reason(&ModelListFetch::Unreadable(
            "body read failed: connection reset".to_string(),
        ))
        .expect("an unreadable body is a skip and has a reason");
        assert!(
            reason.contains("model list body unreadable"),
            "the reason names the outcome the operator sees: {reason}"
        );
        assert!(
            reason.contains("body read failed: connection reset"),
            "the reason carries the cause the read gave: {reason}"
        );
    }

    #[test]
    fn reason_a_non_2xx_names_the_status() {
        // A server that ANSWERED is not a server that could not be reached: the status is the whole
        // claim here, and the sentence must not borrow the unreachable one.
        let reason = skip_reason(&ModelListFetch::NotSuccess(503))
            .expect("a non-2xx answer is a skip and has a reason");
        assert!(
            reason.contains("503"),
            "the reason names the status the provider gave: {reason}"
        );
        assert!(
            !reason.contains("unreachable"),
            "a server that answered is not a server that could not be reached: {reason}"
        );
    }

    #[test]
    fn reason_a_body_is_not_a_skip() {
        // A body — parseable or not — is not a skip: `unserved_from_models_body` decides what it
        // means, and a function that invented a reason here would be claiming a failure that did
        // not happen.
        assert_eq!(
            skip_reason(&ModelListFetch::Body("{\"object\":\"list\",\"data\":[]}".to_string())),
            None,
            "a body is not a skip and so has no skip reason"
        );
    }

    #[test]
    fn reason_every_skip_names_a_cause() {
        // The one that fails if any arm drops its cause again: each skip outcome is built with a
        // distinctive string, and that exact string must survive into the reason the arm logs.
        for (outcome, cause) in [
            (
                ModelListFetch::Unreachable("transport said: no route to host".to_string()),
                "transport said: no route to host",
            ),
            (
                ModelListFetch::Unreadable("read said: unexpected eof".to_string()),
                "read said: unexpected eof",
            ),
            (
                ModelListFetch::NotSuccess(429),
                // The status IS the cause for this outcome, so it is the string that must appear.
                "429",
            ),
        ] {
            let reason = skip_reason(&outcome)
                .unwrap_or_else(|| panic!("every skip outcome has a reason: {outcome:?}"));
            assert!(
                reason.contains(cause),
                "the reason for {outcome:?} must contain its cause {cause:?}: {reason}"
            );
        }
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
        let caps = SeatCaps::default_caps();
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
        let caps = SeatCaps::default_caps();
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
    fn preflight_a_listed_but_unservable_model_does_not_count_as_served() {
        // "The id appears in `data`" and "the provider will serve it" are DIFFERENT claims. Every
        // entry of the real payload carries readiness fields (measured 2026-10-19:
        // `status="servable"`, `resident=true`, `admissible=true`), so a model can be listed while
        // refusing work — and that is the state the arena would otherwise lease seats into.
        let body = r#"{"object":"list","data":[
            {"id":"glm-5.3-flash","status":"servable"},
            {"id":"deepseek-v4.1-flash","status":"loading"}
        ]}"#;
        let served = ids(body);
        assert!(
            served.iter().any(|id| id == "glm-5.3-flash"),
            "an entry whose status is servable is served: {served:?}"
        );
        assert!(
            !served.iter().any(|id| id == "deepseek-v4.1-flash"),
            "a model that is listed but not servable is NOT served: {served:?}"
        );

        // The same exclusion for each boolean claim, on its own.
        let inadmissible = r#"{"data":[
            {"id":"glm-5.3-flash","status":"servable"},
            {"id":"deepseek-v4.1-flash","status":"servable","admissible":false}
        ]}"#;
        let served = ids(inadmissible);
        assert!(served.iter().any(|id| id == "glm-5.3-flash"));
        assert!(
            !served.iter().any(|id| id == "deepseek-v4.1-flash"),
            "admissible:false is a refusal: {served:?}"
        );
        let unresident = r#"{"data":[
            {"id":"glm-5.3-flash","status":"servable"},
            {"id":"deepseek-v4.1-flash","status":"servable","resident":false}
        ]}"#;
        let served = ids(unresident);
        assert!(served.iter().any(|id| id == "glm-5.3-flash"));
        assert!(
            !served.iter().any(|id| id == "deepseek-v4.1-flash"),
            "resident:false is a refusal: {served:?}"
        );
        // Positive claims are honoured too — all three true is today's real payload.
        assert!(model_is_servable(&serde_json::json!({
            "id": "glm-5.3-flash",
            "status": "servable",
            "resident": true,
            "admissible": true,
            "configured": true
        })));

        // Being listed did not save it: the excluded model is REPORTED as absent by the mismatch
        // check, which is the whole point — a seat leased on a refusing model is never served.
        let caps = SeatCaps::default_caps();
        let served = ids(body);
        let report = mismatch_report(&caps, &served).expect("the excluded family is now absent");
        assert!(
            report.contains("deepseek-v4.1-flash"),
            "the report has to name the family the provider refused to serve: {report}"
        );
    }

    #[test]
    fn preflight_a_model_that_claims_nothing_about_readiness_is_taken_as_servable() {
        // An OLDER provider that reports no readiness fields at all must keep working exactly as it
        // does today: no claim is not a refusal.
        let body = r#"{"object":"list","data":[
            {"id":"glm-5.3-flash","object":"model","owned_by":"lokalai"},
            {"id":"qwen3.8-flash-next","object":"model","owned_by":"lokalai"},
            {"id":"next","object":"model","owned_by":"lokalai"}
        ]}"#;
        assert_eq!(
            ids(body),
            vec![
                "glm-5.3-flash".to_string(),
                "qwen3.8-flash-next".to_string(),
                "next".to_string()
            ]
        );
        // …which includes the real MODELS body this module was written against.
        assert_eq!(ids(MODELS).len(), 3);

        // Silence in each of its shapes — absent, null, wrong type — is NOT a refusal.
        assert_eq!(
            ids(r#"{"data":[{"id":"glm-5.3-flash","status":null,"admissible":null},
                            {"id":"qwen3.8-flash-next","resident":null,"status":42}]}"#),
            vec![
                "glm-5.3-flash".to_string(),
                "qwen3.8-flash-next".to_string()
            ],
            "a null or wrongly-typed field made no claim, so it cannot exclude a model"
        );
        assert!(model_is_servable(&serde_json::json!({"id": "x", "status": 42})));
        assert!(model_is_servable(&serde_json::json!({"id": "x", "admissible": "no"})));
        assert!(model_is_servable(&serde_json::json!({"id": "x"})));

        // An UNRECOGNISED status is not a promise, including the right word in the wrong case —
        // exact, case-sensitive match, because guessing at a status would promise a model the
        // provider never offered.
        assert!(!model_is_servable(&serde_json::json!({"id": "x", "status": "SERVABLE"})));
        assert!(!model_is_servable(&serde_json::json!({"id": "x", "status": "Servable"})));
        assert!(!model_is_servable(&serde_json::json!({"id": "x", "status": "ready"})));
        assert_eq!(
            ids(r#"{"data":[{"id":"glm-5.3-flash","status":"SERVABLE"}]}"#),
            Vec::<String>::new(),
            "a status we do not recognise is not a promise"
        );

        // And the unreadable-answer rule is untouched by all of this: None, never Some(vec![]).
        assert_eq!(provider_ids_from_models_json("<html>502 Bad Gateway</html>"), None);
        assert_eq!(provider_ids_from_models_json(""), None);
        assert_eq!(provider_ids_from_models_json(r#"{"object":"list"}"#), None);
    }

    #[test]
    fn wire_an_unparseable_body_is_inconclusive() {
        // A body we could not read is INCONCLUSIVE, and the caller must leave the pool untouched.
        // Marking on this would refuse the whole fleet because one HTTP response came back as HTML,
        // a truncated stream, or a 2xx gateway page — a provider hiccup at boot must not do that.
        let caps = SeatCaps::default_caps();
        assert_eq!(unserved_from_models_body(&caps, "{"), None);
        // The other inconclusive shapes answer the same way, for the same reason.
        assert_eq!(unserved_from_models_body(&caps, ""), None);
        assert_eq!(unserved_from_models_body(&caps, "<html>502 Bad Gateway</html>"), None);
        assert_eq!(unserved_from_models_body(&caps, r#"{"object":"list"}"#), None);
    }

    #[test]
    fn wire_an_empty_model_list_is_inconclusive() {
        // A parseable `{"data":[]}` named NO ids, so the provider made no claim about the fleet at
        // all — which is "we do not know", NOT "it serves nothing". Marking every capped family
        // from an empty list is the exact fleet-wide refusal this distinction exists to prevent, so
        // this is None and the pool keeps leasing exactly as it did before the call.
        let caps = SeatCaps::default_caps();
        assert_eq!(
            unserved_from_models_body(&caps, r#"{"object":"list","data":[]}"#),
            None
        );
        // A `data` array whose entries all name nothing usable is the same silence.
        assert_eq!(unserved_from_models_body(&caps, r#"{"data":[{},{}]}"#), None);
    }

    #[test]
    fn wire_a_missing_family_is_named() {
        // A body that parsed and named at least one id IS conclusive, and the family it does not
        // name is refused: `deepseek-v4.1-flash` is genuinely absent from this fleet, so leasing
        // its 2 seats would hold seats that can never run work.
        let caps = SeatCaps::default_caps();
        let body = r#"{"object":"list","data":[
            {"id":"glm-5.3-flash","status":"servable"},
            {"id":"qwen3.8-flash-next","status":"servable"}
        ]}"#;
        let refused = unserved_from_models_body(&caps, body).expect("a conclusive answer");
        assert_eq!(
            refused,
            vec!["deepseek-v4.1-flash".to_string()],
            "the absent family is named so the pool can refuse it"
        );
        // The two served families are NOT in the list — refusing them would narrow the arena for
        // no reason the provider gave.
        assert!(!refused.iter().any(|family| family == "glm-5.3-flash"));
        assert!(!refused.iter().any(|family| family == "qwen3.8-flash-next"));
    }

    #[test]
    fn wire_a_fully_served_fleet_refuses_nothing() {
        // Some(vec![]) is a REAL answer, not "unknown": the provider answered and serves every
        // capped family, so nothing is refused AND a previous refusal must be cleared. None here
        // would strand a fleet that came back, refusing families on the strength of an outage that
        // has ended — which is why this is deliberately Some(empty) and not None.
        let caps = SeatCaps::default_caps();
        let body = format!(
            r#"{{"object":"list","data":[{}]}}"#,
            caps
                .families()
                .map(|family| format!(r#"{{"id":"{family}","status":"servable"}}"#))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert_eq!(unserved_from_models_body(&caps, &body), Some(Vec::new()));
    }

    #[test]
    fn wire_marking_from_a_body_makes_the_pool_refuse() {
        // The end-to-end claim: the preflight's conclusive answer reaches the pool, and `acquire`
        // refuses the family on provider grounds rather than leasing seats that can never run.
        let pool = LeasePool::with_default_caps();
        let body = r#"{"object":"list","data":[
            {"id":"glm-5.3-flash","status":"servable"},
            {"id":"qwen3.8-flash-next","status":"servable"}
        ]}"#;
        let refused = unserved_from_models_body(pool.caps(), body).expect("a conclusive answer");
        pool.mark_unserved(refused);
        assert_eq!(
            pool.unserved(),
            ["deepseek-v4.1-flash".to_string()].into_iter().collect(),
            "the pool was told exactly the family the provider does not serve"
        );
        assert!(
            matches!(
                pool.acquire("deepseek-v4.1-flash", 1),
                Err(SeatError::ProviderWillNotServe { .. })
            ),
            "the absent family is refused on provider grounds"
        );
        // A served family still leases — the refusal is per family, not a fleet-wide shutdown.
        let lease = pool
            .acquire("glm-5.3-flash", 1)
            .expect("a served family still leases");
        assert_eq!(lease.family(), "glm-5.3-flash");
    }

    #[test]
    fn wire_a_served_fleet_clears_a_previous_refusal() {
        // `mark_unserved` REPLACES the set, and that is what makes a recovery possible: the family
        // the last outage refused is leasable again once a body names every capped family. Adding
        // instead of replacing would make the first bad boot permanent.
        let pool = LeasePool::with_default_caps();
        let missing = r#"{"object":"list","data":[
            {"id":"glm-5.3-flash","status":"servable"},
            {"id":"qwen3.8-flash-next","status":"servable"}
        ]}"#;
        pool.mark_unserved(
            unserved_from_models_body(pool.caps(), missing).expect("a conclusive answer"),
        );
        assert!(matches!(
            pool.acquire("deepseek-v4.1-flash", 1),
            Err(SeatError::ProviderWillNotServe { .. })
        ));
        // The provider came back and now names everything.
        let served = format!(
            r#"{{"object":"list","data":[{}]}}"#,
            pool.caps()
                .families()
                .map(|family| format!(r#"{{"id":"{family}","status":"servable"}}"#))
                .collect::<Vec<_>>()
                .join(",")
        );
        pool.mark_unserved(
            unserved_from_models_body(pool.caps(), &served)
                .expect("a fully-served body is conclusive"),
        );
        assert!(pool.unserved().is_empty(), "the refusal was cleared");
        let lease = pool
            .acquire("deepseek-v4.1-flash", 1)
            .expect("the family leases again once the provider serves it");
        assert_eq!(lease.family(), "deepseek-v4.1-flash");
    }

    #[test]
    fn alias_a_string_value_is_the_target() {
        // The redirect shape: `"next": "qwen3.8-flash-next"` says WHICH model the provider is
        // asked for, so the string is the target that has to exist in the fleet.
        assert_eq!(
            alias_targets_from_profile(
                r#"{"providers":{"lokalai":{"models":{"next":"qwen3.8-flash-next"}}}}"#,
                "lokalai"
            ),
            vec![("next".to_string(), "qwen3.8-flash-next".to_string())]
        );
    }

    #[test]
    fn alias_an_object_value_means_the_key_is_the_model() {
        // The per-model-config shape: `"next": {"contextLength": 262144}` makes no redirect, so
        // the alias KEY is itself the model id. Reading an object as "no target here" would leave
        // the shape this repo's own PROFILE constant uses — and the shape most profiles use —
        // completely unchecked.
        assert_eq!(
            alias_targets_from_profile(
                r#"{"providers":{"lokalai":{"models":{"next":{"contextLength":262144}}}}}"#,
                "lokalai"
            ),
            vec![("next".to_string(), "next".to_string())]
        );
        // …and the same for the real PROFILE constant in this module.
        assert_eq!(
            alias_targets_from_profile(PROFILE, "lokalai"),
            vec![("next".to_string(), "next".to_string())]
        );
    }

    #[test]
    fn alias_a_broken_profile_yields_nothing() {
        // All three silences answer with an empty vec rather than panicking or guessing: an
        // unparseable profile, a provider the profile has no block for (lokalai's aliases must not
        // be judged against openrouter's fleet, nor leak into a claim about it), and a block with
        // no `models` at all.
        assert_eq!(alias_targets_from_profile("not json at all", "lokalai"), Vec::new());
        assert_eq!(alias_targets_from_profile("", "lokalai"), Vec::new());
        assert_eq!(alias_targets_from_profile("{}", "lokalai"), Vec::new());
        assert_eq!(alias_targets_from_profile(PROFILE, "openrouter"), Vec::new());
        assert_eq!(
            alias_targets_from_profile(
                r#"{"providers":{"lokalai":{"endpoint":"https://lokalai.silverjrom.app/v1"}}}"#,
                "lokalai"
            ),
            Vec::new()
        );
        // A `models` block that is not an object is the same silence, and a value type this module
        // does not understand is skipped rather than guessed at — the alias key is NOT silently
        // treated as a model id for a number/array/bool/null, because those are not the two shapes
        // kloo defines.
        assert_eq!(
            alias_targets_from_profile(r#"{"providers":{"lokalai":{"models":[1,2]}}}"#, "lokalai"),
            Vec::new()
        );
        assert_eq!(
            alias_targets_from_profile(
                r#"{"providers":{"lokalai":{"models":{"next":42,"glimmer":["a"],"glm":null,"k":true}}}}"#,
                "lokalai"
            ),
            Vec::new()
        );
        // Sorted by BYTE ORDER OF THE ALIAS — the only stable order available once the profile has
        // been parsed. The profile spells its keys next, glimmer, glm, but that order is gone the
        // moment the JSON becomes a `Map` (serde_json's default `Map` is a `BTreeMap`), so no
        // ordering at any cost could claim to reproduce the order the operator typed the keys in.
        // Hence `glimmer` BEFORE `glm` — "i" (0x69) < "m" (0x6d) — regardless of how the profile
        // spells its aliases, and the pair's model-id half never participates in that decision
        // because it is an id the operator never typed.
        assert_eq!(
            alias_targets_from_profile(
                r#"{"providers":{"lokalai":{"models":{
                    "next":"qwen3.8-flash-next",
                    "glimmer":"muse-glimmer-30b",
                    "glm":"glm-5.3-flash"
                }}}}"#,
                "lokalai"
            ),
            vec![
                ("glimmer".to_string(), "muse-glimmer-30b".to_string()),
                ("glm".to_string(), "glm-5.3-flash".to_string()),
                ("next".to_string(), "qwen3.8-flash-next".to_string()),
            ]
        );
    }

    #[test]
    fn alias_an_unserved_target_is_named_with_its_alias() {
        // The real profile at `~/etc/kloo.json` against the real fleet measured today:
        // `muse-glimmer-30b` is NOT among the servable ids, and NOTHING noticed, because the
        // seat-cap check has no cap for `glimmer` to compare. The line has to name both the alias
        // (the user's own name, fixed in the profile) and the target (what the provider was asked
        // for, fixed at the provider), because the fix differs per side.
        let aliases = alias_targets_from_profile(
            r#"{"providers":{"lokalai":{"models":{
                "next":"qwen3.8-flash-next",
                "glimmer":"muse-glimmer-30b",
                "glm":"glm-5.3-flash"
            }}}}"#,
            "lokalai",
        );
        let provider = vec!["glm-5.3-flash".to_string(), "qwen3.8-flash-next".to_string()];
        let report = alias_mismatch_report(&aliases, &provider).expect("glimmer is not served");
        assert!(
            report.contains("glimmer"),
            "the report names the alias the run would ask for: {report}"
        );
        assert!(
            report.contains("muse-glimmer-30b"),
            "the report names the target the provider was asked for: {report}"
        );
        assert!(
            report.contains("does not serve"),
            "the report says what is wrong: {report}"
        );
        // The two aliases that DO resolve to served models are not mentioned — naming them would
        // send the operator hunting for a problem that is not there. Assert the absence of the ALIAS
        // in the phrase the report formats it in (`alias "X" points at "Y"`), NOT of the bare word
        // `next`: the report's wording may legitimately contain "next" one day ("next boot", "the
        // next alias") and a substring match on the word would fail on correct code.
        assert!(
            !report.contains(&format!("alias \"{}\" points at", "next")),
            "a served alias must not appear in the line: {report}"
        );
        assert!(
            !report.contains(&format!("alias \"{}\" points at", "glm")),
            "a served alias must not appear in the line: {report}"
        );
        // Exact matching, never prefix, exactly like the family check: `muse-glimmer-30b-fast` is
        // not `muse-glimmer-30b`.
        let near_miss = vec![
            "glm-5.3-flash".to_string(),
            "qwen3.8-flash-next".to_string(),
            "muse-glimmer-30b-fast".to_string(),
        ];
        let report = alias_mismatch_report(&aliases, &near_miss).expect("a variant is not the model");
        assert!(report.contains("glimmer"), "{report}");
        // The mirror-image fleet: `muse-glimmer-30b` is back and `qwen3.8-flash-next` is the id that
        // vanished instead. To report `next` the served list must OMIT `qwen3.8-flash-next` while
        // INCLUDING `muse-glimmer-30b` — with all three ids served every alias resolves, the answer
        // is correctly `None`, and a list naming all three could never produce this line.
        let glimmer_back = vec![
            "glm-5.3-flash".to_string(),
            "muse-glimmer-30b".to_string(),
        ];
        let report = alias_mismatch_report(&aliases, &glimmer_back)
            .expect("qwen3.8-flash-next is the id that vanished this time");
        assert!(
            report.contains("next"),
            "the line names the alias the run would ask for: {report}"
        );
        assert!(
            report.contains("qwen3.8-flash-next"),
            "and the target the provider was asked for: {report}"
        );
        assert!(
            !report.contains("glimmer"),
            "the alias now served is not mentioned: {report}"
        );
        let two_problems = alias_mismatch_report(
            &aliases,
            &["glm-5.3-flash".to_string()],
        )
        .expect("two aliases are unserved");
        assert!(
            two_problems.contains("; "),
            "several mismatches are one line joined with `; `: {two_problems}"
        );
        assert!(two_problems.contains("glimmer"), "{two_problems}");
        assert!(two_problems.contains("next"), "{two_problems}");
    }

    #[test]
    fn alias_every_alias_served_reports_nothing() {
        // Every alias resolves to an id the provider serves — including the object-value shape,
        // where the alias key names the model. Silence here is the point: a profile that agrees
        // with the fleet must not produce a warning on every boot.
        let aliases = vec![
            ("glm".to_string(), "glm-5.3-flash".to_string()),
            ("glimmer".to_string(), "muse-glimmer-30b".to_string()),
            ("next".to_string(), "qwen3.8-flash-next".to_string()),
        ];
        let provider = vec![
            "glm-5.3-flash".to_string(),
            "muse-glimmer-30b".to_string(),
            "qwen3.8-flash-next".to_string(),
        ];
        assert_eq!(alias_mismatch_report(&aliases, &provider), None);
        // An empty alias list is the same silence: no claim, nothing to say.
        assert_eq!(alias_mismatch_report(&[], &provider), None);
    }

    #[test]
    fn alias_an_empty_provider_list_reports_nothing() {
        // An EMPTY provider list means "we could not read the fleet", not "the fleet is empty" —
        // unknown is not empty. Reporting against it would turn one bad HTTP response, or one
        // laptop that booted offline, into an alarm about every alias in the profile. This is the
        // leases module's rule for the seat-cap comparison, inherited here rather than restated.
        let aliases = vec![("glimmer".to_string(), "muse-glimmer-30b".to_string())];
        assert_eq!(alias_mismatch_report(&aliases, &[]), None);
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
