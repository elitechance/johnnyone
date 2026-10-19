//! Per-family seat accounting for the shared model fleet — the lease pool.
//!
//! **Why this has to exist at all.** The fleet exposes no way to ask how busy it is:
//! `/v1/models` returns no provider count, no queue depth and no active-user field, and going
//! over a model's cap raises **no error** — requests are batched rather than rejected. There is
//! no 429 to back off on, so contention shows up only as latency drifting off a baseline. Two
//! concurrent arena runs therefore starve each other silently, and the slower one looks like the
//! worse workflow: the first thing a busy fleet does is promote a champion because the other box
//! happened to be quiet. Counting seats here is the only place the count can exist.
//!
//! **Seats are per family, never pooled.** `family` is part of a workflow's identity, not its
//! environment — a Hop tuned around one model's 262k context does not transfer to another's 132k.
//! So a request for `glm-5.3-flash` is refused when `glm-5.3-flash` is full even if
//! `deepseek-v4.1-flash` has seats free. Pooling would hand a run seats on a family its prompt
//! was never searched on, which is how you promote a champion that is worse everywhere it is
//! actually used.
//!
//! **No lock is ever held across an `await`.** Every method here is synchronous and the guard is
//! dropped before returning, so there is no suspension point for it to span. That is deliberate:
//! the earlier shape of this feature held a lock across an await and deadlocked. An async caller
//! acquires, awaits its work, and lets the [`SeatLease`] drop — it does not hold the pool's lock
//! while it waits.
//!
//! **Release is a `Drop`.** A lease released only on the happy path is a leaked seat on every
//! panic, and a leaked seat is permanent: nothing reconciles against the fleet, because the fleet
//! cannot be asked.
//!
//! ## The caller this is for
//!
//! One, today: the arena's run starter. It holds the lease for the life of the run and records
//! two columns on the `workflow_runs` row, which together are what makes a comparison between
//! two runs honest — a preference recorded across mismatched seat budgets is measuring
//! scheduling luck, not prompt quality.
//!
//! ### The order is load-bearing, and it is three steps, not two
//!
//! ```text
//! 1. fleet_busy  = pool.fleet_busy_json();                 // BEFORE acquiring
//! 2. let lease   = pool.acquire(family, fan_out)?;          // then take the seats
//! 3. seats_held  = lease.seats_held_json();                 // from the returned lease
//! ```
//!
//! **Why:** `fleet_busy` is meant to record the contention this run started against — *another*
//! run's seats belong in it, this run's own do not. Snapshot it after `acquire` and the run
//! counts its own `fan_out` as foreign contention.
//!
//! **The failure mode of getting it wrong**, which is why this is spelled out rather than left
//! to the reader: every run then reports inflated contention, no two runs ever look like they
//! ran under comparable budgets, `workflow_prefs.comparable` is 0 for every pair, and **no
//! champion can ever be promoted.** Not a crash and not a wrong number anybody would notice —
//! an arena that simply never concludes anything.
//!
//! **And it cannot be collapsed into two steps.** `seats_held_json` is a method on the lease, so
//! it exists only after `acquire`, while `fleet_busy` has to be read before it. The two columns
//! cannot be taken at one moment; the order above is the only one that gives both the right
//! value.
//!
//! An earlier version of this header prescribed acquire-then-snapshot. Dev A's branch had
//! independently written down the same wrong sequence — two branches that cannot see each other
//! arriving at one mistake — and QA caught it by reading both. Recorded rather than quietly
//! corrected, because the next person to wire this up will follow the doc.
//!
//! There is deliberately **no GraphQL surface here**: the arena API is owned elsewhere, and this
//! is an in-process API for it to call.
//!
//! ## Two things this pool does NOT do, which that caller must
//!
//! Both are stated here because "the lease pool is done" must not be read as "the mock's seat
//! behaviour is implemented". QA pinned each as a tripwire
//! (`qa_documents_there_is_no_queue_only_an_immediate_refusal`,
//! `qa_documents_a_two_family_request_is_not_atomic_across_families`).
//!
//! 1. **There is no queue, and no fairness.** `mock.html` step 3 says a run that cannot get its
//!    seats *queues*, and never proceeds at a reduced width. Only the second clause lives here:
//!    [`LeasePool::acquire`] refuses immediately and never blocks. So a wide request can starve
//!    behind a stream of narrow ones, indefinitely, and nothing in this module will notice. The
//!    arena's run starter owns the queue — including its ordering policy, because the fair
//!    answer ("the run that asked first") is not derivable from anything the pool knows.
//! 2. **Atomicity is per lease, not per run.** A run needing seats on two families takes two
//!    [`SeatLease`]es, and there is no all-or-nothing primitive across them. So the caller can
//!    hold one family's seats while being refused the other's, and must unwind the first itself;
//!    two such callers taking the families in opposite orders deadlock-by-starvation with no
//!    queue to break the tie. `seats_held_json` can likewise only ever describe one family.
//!    This is not an oversight to fix here: a cross-family primitive needs a wait policy, which
//!    is item 1, which belongs to the caller.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

/// The seat caps as MEASURED against the live fleet on 2026-10-12, recorded in
/// `personal/plans/johnnyone/mass/mock.html` step 3.
///
/// Data, not logic: the numbers live in this one table, nothing branches on a family name, and
/// a deployment with a different fleet overrides them via [`SeatCaps::from_json`] rather than by
/// editing code. A family that is not in the table has no cap and cannot be leased at all —
/// refusing an unknown family is the honest answer, because a cap guessed from nothing is worse
/// than no pool.
///
/// The context lengths in the per-entry comments below were read from the provider's own
/// `/v1/models` on 2026-10-19, and the figure these comments used to carry for
/// `qwen3.8-flash-next` was never its context length — the provider reports 135168, which the
/// comment below now records as 132K.
pub const MEASURED_SEAT_CAPS: &[(&str, u32)] = &[
    // 262K · reasoning · executes the phase
    ("glm-5.3-flash", 4),
    // 262K · proposes the next prompt
    ("deepseek-v4.1-flash", 2),
    // 132K · the busiest model — left alone by the arena, but counted
    ("qwen3.8-flash-next", 4),
];

/// The setting key a host may use to override [`MEASURED_SEAT_CAPS`], as a JSON object
/// (`{"glm-5.3-flash": 4, …}`). Absent or unparseable → the measured table, with a warning.
pub const SEAT_CAPS_SETTING_KEY: &str = "arena_seat_caps";

/// How many seats each family has. Immutable once built, so a cap cannot drift mid-run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatCaps {
    caps: BTreeMap<String, u32>,
}

impl SeatCaps {
    /// The measured 2026-10-12 caps.
    pub fn measured() -> Self {
        Self::from_pairs(MEASURED_SEAT_CAPS.iter().map(|(f, n)| ((*f).to_string(), *n)))
    }

    /// Build from any `(family, seats)` source. A family with zero seats is dropped rather than
    /// stored: "configured to zero" and "cannot be leased" are the same state, and keeping both
    /// spellings means two code paths for one condition.
    pub fn from_pairs<I: IntoIterator<Item = (String, u32)>>(pairs: I) -> Self {
        Self {
            caps: pairs.into_iter().filter(|(_, n)| *n > 0).collect(),
        }
    }

    /// Parse a `{"family": seats}` object. Errors are returned, never defaulted away — a host
    /// that set this meant something by it, and silently running on different caps than the
    /// operator configured is exactly the invisible-contention failure this module exists for.
    ///
    /// **Emptiness is checked AFTER normalisation, and that ordering is the fix for a real
    /// bypass.** The check used to run on the parsed map, before [`from_pairs`](Self::from_pairs)
    /// drops zero-seat families — so `{}` was refused while
    /// `{"glm-5.3-flash":0,"deepseek-v4.1-flash":0,"qwen3.8-flash-next":0}` was accepted and
    /// produced the identical end state the guard exists to prevent: a caps table with no
    /// families. The host then refused every request as `UnknownFamily`, whose message tells the
    /// operator to add the family to the table when they are the one who zeroed it.
    pub fn from_json(raw: &str) -> Result<Self, String> {
        let parsed = Self::parse_override_map(raw)?;
        let zeroed: Vec<&str> = parsed
            .iter()
            .filter(|(_, n)| **n == 0)
            .map(|(f, _)| f.as_str())
            .collect();
        let caps = Self::from_pairs(parsed.clone());
        if caps.caps.is_empty() {
            // Two spellings of one state, so they get one answer. The `{}` wording is unchanged
            // because it is the honest description of the empty object.
            if parsed.is_empty() {
                return Err("seat caps object is empty".to_string());
            }
            // Why an all-zero override is refused when a partial one is merely noted, since
            // under merge semantics both end at the measured caps for every family:
            //
            // This is an API-CONTRACT argument, not a live-caller one, and the distinction
            // matters because "there is a caller" is the kind of sentence that gets trusted
            // later: `from_json` has NO production caller today either — QA checked, and its
            // callers are tests plus the merge path below.
            //
            // It is a PUBLIC constructor, and its contract is "produce a usable caps table or
            // fail". An all-zero table is not usable: a pool built from it refuses every request
            // as `UnknownFamily`, whose message tells the operator to add a family they
            // themselves zeroed. Honouring that contract at the constructor is worth more than
            // the symmetry of accepting it, because the next caller to arrive will be written
            // against the signature, not against this comment.
            //
            // What WAS inconsistent is the diagnostic, and that is fixed here: both routes now
            // state the same rule in the same words, so an operator learns it either way.
            return Err(format!(
                "every family in the seat caps object is set to zero ({}), so no family could be \
                 leased at all; remove the key to use the measured caps instead. Zero does NOT \
                 disable a family — there is deliberately no disable switch",
                zeroed_list(&zeroed)
            ));
        }
        Ok(caps)
    }

    /// The raw `{"family": seats}` object, with nothing dropped. One parser, two callers.
    fn parse_override_map(raw: &str) -> Result<BTreeMap<String, u32>, String> {
        serde_json::from_str(raw.trim())
            .map_err(|e| format!("seat caps must be a JSON object of family -> seats: {}", e))
    }

    /// Apply a host override ON TOP OF the measured table, and report everything questionable
    /// about it.
    ///
    /// **A partial override MERGES; it does not replace.** It used to replace, and that was the
    /// bug: `{"glm-5.3-flash": 4}` left the other two measured families with no cap at all, so
    /// they could not be leased — two thirds of the harm the all-zero guard was added to
    /// prevent, with no error and no warning. It also shrank `fleet_busy` to one row, which is
    /// precisely the "a quiet fleet versus a snapshot that never recorded that family"
    /// ambiguity [`LeasePool::busy`]'s own doc comment says it exists to avoid — and that column
    /// is what decides whether two arena runs are comparable at all.
    ///
    /// So an omitted family keeps its measured cap, and that is now the documented default
    /// rather than an implied disable. **There is deliberately no way to disable a family here.**
    /// A zero entry does not do it (it is reported and ignored, see [`CapNote::ZeroIgnored`]),
    /// because "disable" has to be explicit and warned rather than inferred from an absence, and
    /// nothing today needs it. When something does, it should arrive as its own key with its own
    /// warning — not as a second meaning for `0`.
    ///
    /// Returns the merged table plus every [`CapNote`]; `seat_caps_from_db` is what logs them.
    pub fn from_override_json(raw: &str) -> Result<(Self, Vec<CapNote>), String> {
        let parsed = Self::parse_override_map(raw)?;
        // The all-zero / empty refusal still runs on the override AS WRITTEN, so its message
        // keeps naming what the operator typed. Merging would hide it: an all-zero override
        // merges to exactly the measured table, so it would look like success.
        let _ = Self::from_json(raw)?;

        let mut notes: Vec<CapNote> = Vec::new();
        let mut merged = Self::measured().caps;
        for (family, seats) in &parsed {
            let measured = MEASURED_SEAT_CAPS
                .iter()
                .find(|(m, _)| m == family)
                .map(|(_, n)| *n);

            // The key diagnostics run BEFORE the zero check, not after it. They used to be
            // after a `continue`, so `{"local-7b":0}` skipped them and the operator's ONLY note
            // was `ZeroIgnored` claiming "the measured cap is kept" — for a family that has no
            // measured cap to keep. The one note they got was the one that misinformed them.
            if measured.is_none() {
                // A different fleet may legitimately have a family nobody here measured. But a
                // typo of a measured name is the likelier cause and is otherwise invisible: the
                // intended family keeps its old cap while a new one appears beside it. Matching
                // stays EXACT — lowering or trimming the key would be a guess about a name the
                // fleet owns — so a near miss is reported, never silently corrected.
                notes.push(match near_miss(family) {
                    Some(NearMiss::Invisible {
                        measured,
                        characters,
                        case_too,
                    }) => CapNote::InvisibleCharacter {
                        configured: family.clone(),
                        measured,
                        characters,
                        case_too,
                    },
                    Some(NearMiss::Whitespace { measured, case_too }) => {
                        CapNote::WhitespaceMismatch {
                            configured: family.clone(),
                            measured,
                            case_too,
                        }
                    }
                    Some(NearMiss::Case { measured }) => CapNote::CaseMismatch {
                        configured: family.clone(),
                        measured,
                    },
                    None => CapNote::UnmeasuredFamily {
                        family: family.clone(),
                    },
                });
            }
            if *seats == 0 {
                // `measured` rides along so the message can only say "the measured cap is kept"
                // when there is one.
                notes.push(CapNote::ZeroIgnored {
                    family: family.clone(),
                    measured,
                });
                continue;
            }
            merged.insert(family.clone(), *seats);
        }
        let caps = Self { caps: merged };
        // A family the override REPLACED still gets the over-cap warning it always got.
        for (family, configured, measured) in caps.exceedances_vs_measured() {
            notes.push(CapNote::AboveMeasured {
                family,
                configured,
                measured,
            });
        }
        Ok((caps, notes))
    }

    /// This family's cap, or `None` when the family is not in the table.
    pub fn cap(&self, family: &str) -> Option<u32> {
        self.caps.get(family).copied()
    }

    /// Every capped family, ascending. Stable order so a `fleet_busy` snapshot is comparable.
    pub fn families(&self) -> impl Iterator<Item = &str> {
        self.caps.keys().map(String::as_str)
    }

    /// Families whose cap here is HIGHER than the measured one, as
    /// `(family, configured, measured)`, **in [`MEASURED_SEAT_CAPS`] order** — which is the
    /// fleet's own ordering (glm, deepseek, qwen), not alphabetical and not this table's
    /// `BTreeMap` order. The doc comment said "ascending", which was wrong and QA pinned it in
    /// `qa_the_exceedance_order_is_the_measured_tables_not_alphabetical`; the order is stable,
    /// which is all a log line needs, so the comment is what was fixed.
    ///
    /// A different fleet legitimately needs a different number, so this is **not** a refusal and
    /// not a clamp — it is the thing that makes the difference visible. Without it a typo of `40`
    /// for `4` was accepted in full and the pool handed out ten times the fleet's measured
    /// capacity with no log line: precisely the invisible over-subscription this module's own doc
    /// comment says it exists to prevent. Returned as DATA rather than logged in place so it can
    /// be asserted on; [`seat_caps_from_db`] is what turns each row into a warning.
    pub fn exceedances_vs_measured(&self) -> Vec<(String, u32, u32)> {
        MEASURED_SEAT_CAPS
            .iter()
            .filter_map(|(family, measured)| {
                let configured = self.cap(family)?;
                (configured > *measured).then(|| ((*family).to_string(), configured, *measured))
            })
            .collect()
    }
}

/// Render a family list for the all-zero error message.
fn zeroed_list(families: &[&str]) -> String {
    families.join(", ")
}

/// Zero-width and byte-order format characters that are not `char::is_whitespace`.
///
/// `str::trim` recognises `char::is_whitespace`, and U+200B and friends are **format** characters
/// (category Cf), so they are not whitespace and `"glm-5.3-flash\u{200b}".trim()` is itself.
/// That left the most invisible of the invisible characters as the one that got no help at all —
/// the operator saw only "nobody measured this family" — which is the opposite of the reason
/// [`CapNote::WhitespaceMismatch`] exists. Each is listed with its name so a note can say WHICH
/// one to delete; "there is an invisible character somewhere" is not an actionable message.
const INVISIBLE_CHARS: &[(char, &str)] = &[
    ('\u{200b}', "U+200B ZERO WIDTH SPACE"),
    ('\u{200c}', "U+200C ZERO WIDTH NON-JOINER"),
    ('\u{200d}', "U+200D ZERO WIDTH JOINER"),
    ('\u{2060}', "U+2060 WORD JOINER"),
    ('\u{feff}', "U+FEFF ZERO WIDTH NO-BREAK SPACE (BOM)"),
];

/// How `key` nearly named a measured family.
#[derive(Debug)]
enum NearMiss {
    /// Differs only in case.
    Case { measured: String },
    /// Differs by surrounding whitespace `str::trim` can see, and possibly case.
    Whitespace { measured: String, case_too: bool },
    /// Differs by one or more zero-width format characters, and possibly whitespace/case.
    Invisible {
        measured: String,
        characters: String,
        case_too: bool,
    },
}

/// Did `key` nearly name a measured family?
///
/// Compares with surrounding whitespace removed, the zero-width format characters of
/// [`INVISIBLE_CHARS`] removed, and case folded — because all three are what hand-edited JSON
/// actually gets wrong, and each used to earn only the generic "nobody measured this family"
/// note with nothing pointing at the family the operator meant.
///
/// **Reporting, never correcting.** The key itself is never trimmed or rewritten: matching stays
/// exact, so the note tells the operator what to fix rather than silently accepting a different
/// key than was written. See the loop in `from_override_json`.
///
/// Case folding is `eq_ignore_ascii_case`, so a NON-ASCII case variant would not match. Every
/// measured family name is ASCII today; if one ever is not, this needs a Unicode case fold, and
/// the symptom would be a near miss falling through to `UnmeasuredFamily`.
fn near_miss(key: &str) -> Option<NearMiss> {
    let found: Vec<&str> = INVISIBLE_CHARS
        .iter()
        .filter(|(c, _)| key.contains(*c))
        .map(|(_, name)| *name)
        .collect();
    let stripped: String = key
        .chars()
        .filter(|c| !INVISIBLE_CHARS.iter().any(|(bad, _)| bad == c))
        .collect();
    let trimmed = stripped.trim();
    let (measured, _) = MEASURED_SEAT_CAPS
        .iter()
        .find(|(m, _)| m.eq_ignore_ascii_case(trimmed))?;
    let measured = (*measured).to_string();
    let case_too = measured != trimmed;
    if !found.is_empty() {
        return Some(NearMiss::Invisible {
            measured,
            characters: found.join(", "),
            case_too,
        });
    }
    if trimmed != stripped.as_str() {
        return Some(NearMiss::Whitespace { measured, case_too });
    }
    Some(NearMiss::Case { measured })
}

/// Something questionable about a host seat-cap override. Never fatal — every one of these is a
/// configuration a different fleet might legitimately want — but never silent either, which is
/// the whole point of this module.
///
/// Returned as DATA rather than logged where it is found, so it can be asserted on: "it warns"
/// is not a testable claim about a `tracing::warn!`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapNote {
    /// The configured cap is above the measured capacity for that family.
    AboveMeasured {
        family: String,
        configured: u32,
        measured: u32,
    },
    /// The key differs from a measured family only by case, so it did not apply to it.
    CaseMismatch { configured: String, measured: String },
    /// The key differs from a measured family by surrounding whitespace (and possibly case too),
    /// so it did not apply to it. Its own variant rather than a `CaseMismatch`, because the
    /// operator has to be told WHICH invisible character to delete.
    WhitespaceMismatch {
        configured: String,
        measured: String,
        case_too: bool,
    },
    /// The key differs from a measured family by one or more ZERO-WIDTH format characters, which
    /// `str::trim` cannot see at all because they are not `char::is_whitespace`.
    ///
    /// Its own variant rather than a `WhitespaceMismatch` because the message has to name the
    /// character: an operator cannot find a U+200B by looking, and "there is whitespace in your
    /// key" would send them hunting for a space that is not there.
    InvisibleCharacter {
        configured: String,
        measured: String,
        /// Human-readable names of the characters found, e.g. `U+200B ZERO WIDTH SPACE`.
        characters: String,
        case_too: bool,
    },
    /// The key matches no measured family at all, not even stripped, trimmed and case-folded.
    UnmeasuredFamily { family: String },
    /// A zero entry. Zero is not a way to disable a family.
    ///
    /// `measured` is the family's measured cap, if it has one. It is carried so the message
    /// cannot claim "the measured cap is kept" about a family that has no measured cap — the
    /// false note an unmeasured, zeroed key used to produce as the operator's ONLY diagnostic.
    ZeroIgnored {
        family: String,
        measured: Option<u32>,
    },
}

impl fmt::Display for CapNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CapNote::AboveMeasured {
                family,
                configured,
                measured,
            } => write!(
                f,
                "configured seat cap for {:?} is {}, ABOVE the measured capacity of {}; the \
                 fleet reports no error when over-subscribed, so this will show up only as \
                 latency",
                family, configured, measured
            ),
            CapNote::CaseMismatch {
                configured,
                measured,
            } => write!(
                f,
                "seat-cap key {:?} differs only in case from the measured family {:?}, so it did \
                 NOT change that family's cap — {:?} keeps its measured value and {:?} was added \
                 beside it. Family names are matched exactly; fix the case if this was a typo",
                configured, measured, measured, configured
            ),
            CapNote::WhitespaceMismatch {
                configured,
                measured,
                case_too,
            } => write!(
                f,
                "seat-cap key {:?} differs from the measured family {:?} only by surrounding \
                 whitespace{}, so it did NOT change that family's cap — {:?} keeps its measured \
                 value and {:?} was added beside it as a family of its own. Family names are \
                 matched exactly and the key is NOT trimmed; delete the whitespace if this was \
                 a typo",
                configured,
                measured,
                if *case_too { " and case" } else { "" },
                measured,
                configured
            ),
            CapNote::InvisibleCharacter {
                configured,
                measured,
                characters,
                case_too,
            } => write!(
                f,
                "seat-cap key {:?} contains {} and so does NOT name the measured family {:?}{} \
                 — {:?} keeps its measured value and the key was added beside it as a family of \
                 its own. The character is invisible in an editor; delete it. Family names are \
                 matched exactly and the key is NOT trimmed or rewritten",
                configured,
                characters,
                measured,
                if *case_too { " (its case differs too)" } else { "" },
                measured
            ),
            CapNote::UnmeasuredFamily { family } => write!(
                f,
                "seat-cap key {:?} matches no measured family; it is accepted as a new family, \
                 but nothing here has measured its real capacity",
                family
            ),
            CapNote::ZeroIgnored {
                family,
                measured: Some(measured),
            } => write!(
                f,
                "seat-cap entry {:?} is 0, which does NOT disable the family — its measured cap \
                 of {} is kept. Remove the key instead; there is deliberately no disable switch",
                family, measured
            ),
            CapNote::ZeroIgnored {
                family,
                measured: None,
            } => write!(
                f,
                "seat-cap entry {:?} is 0 and that family has no measured cap either, so the \
                 entry does nothing at all — it neither grants seats nor disables anything. \
                 Remove it, or give it the number of seats the family really has",
                family
            ),
        }
    }
}

/// Why a seat request was refused. Every variant names the numbers, because the point of this
/// module is that over-subscription is otherwise invisible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeatError {
    /// No cap is recorded for this family, so nothing here can say how many seats it has.
    UnknownFamily { family: String },
    /// A lease of nothing is a bug in the caller, not a free lease.
    ZeroSeats { family: String },
    /// The request does not fit. Nothing was taken.
    OverCap {
        family: String,
        requested: u32,
        free: u32,
        cap: u32,
    },
}

impl fmt::Display for SeatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SeatError::UnknownFamily { family } => write!(
                f,
                "no measured seat cap for model family {:?}; add it to the seat-cap table before \
                 leasing seats on it",
                family
            ),
            SeatError::ZeroSeats { family } => write!(
                f,
                "refusing a zero-seat lease on {:?}: a run holds seats or it does not",
                family
            ),
            SeatError::OverCap {
                family,
                requested,
                free,
                cap,
            } => write!(
                f,
                "{} seats requested on {:?} but only {} of {} are free; the fleet reports no \
                 error when it is over-subscribed, so this request is refused rather than \
                 partially granted",
                requested, family, free, cap
            ),
        }
    }
}

impl std::error::Error for SeatError {}

/// Seats in use per family. The shared handle, so [`SeatLease`] can release without borrowing.
pub type SharedLeasePool = Arc<LeasePool>;

/// Counts seats in use per family, against a fixed set of caps.
#[derive(Debug)]
pub struct LeasePool {
    caps: SeatCaps,
    /// Seats currently leased, per family. A `std::sync::Mutex` on purpose: it is never held
    /// across an `await`, and `Drop` can take it (a `tokio::sync::Mutex` cannot be locked from
    /// `Drop`, which is what makes release-on-panic possible here).
    held: Mutex<BTreeMap<String, u32>>,
}

/// Lock the counts, recovering a poisoned lock rather than panicking — the same choice
/// `coordinator_flight` makes. A poisoned lock here means some other thread panicked; the counts
/// it leaves behind are still integers, and a pool that panics on read turns one panic into all
/// of them.
fn counts(held: &Mutex<BTreeMap<String, u32>>) -> MutexGuard<'_, BTreeMap<String, u32>> {
    match held.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl LeasePool {
    /// A pool over the measured 2026-10-12 caps.
    pub fn with_measured_caps() -> SharedLeasePool {
        Arc::new(Self::new(SeatCaps::measured()))
    }

    pub fn new(caps: SeatCaps) -> Self {
        Self {
            caps,
            held: Mutex::new(BTreeMap::new()),
        }
    }

    /// Take `seats` on `family`, all or nothing.
    ///
    /// The check and the take happen under ONE lock acquisition, so two threads cannot both
    /// observe the same free seats and both take them. On refusal the counts are not touched:
    /// there is no partial acquire, because a run at a reduced width is not running the same
    /// experiment and its result is not comparable to the other side's.
    pub fn acquire(self: &Arc<Self>, family: &str, seats: u32) -> Result<SeatLease, SeatError> {
        let cap = self.caps.cap(family).ok_or_else(|| SeatError::UnknownFamily {
            family: family.to_string(),
        })?;
        if seats == 0 {
            return Err(SeatError::ZeroSeats {
                family: family.to_string(),
            });
        }
        let mut held = counts(&self.held);
        let in_use = held.get(family).copied().unwrap_or(0);
        let free = cap.saturating_sub(in_use);
        if seats > free {
            // Returns while still holding the guard; the guard drops on the way out and the map
            // was never mutated, so a refused request leaves no trace.
            return Err(SeatError::OverCap {
                family: family.to_string(),
                requested: seats,
                free,
                cap,
            });
        }
        held.insert(family.to_string(), in_use + seats);
        drop(held);
        Ok(SeatLease {
            pool: Arc::clone(self),
            family: family.to_string(),
            seats,
        })
    }

    /// Seats in use, per family, for every capped family — including the idle ones at `0`.
    ///
    /// Idle families are present on purpose: this snapshot is the `workflow_runs.fleet_busy`
    /// column, and "0 busy" and "not in the snapshot" have to be distinguishable later or the
    /// comparability check cannot tell a quiet fleet from an older snapshot that did not record
    /// that family at all.
    pub fn busy(&self) -> BTreeMap<String, u32> {
        let held = counts(&self.held);
        self.caps
            .families()
            .map(|f| (f.to_string(), held.get(f).copied().unwrap_or(0)))
            .collect()
    }

    /// Seats in use on one family.
    pub fn busy_for(&self, family: &str) -> u32 {
        counts(&self.held).get(family).copied().unwrap_or(0)
    }

    /// Seats still free on `family`, or `None` when the family has no cap.
    pub fn free_for(&self, family: &str) -> Option<u32> {
        let cap = self.caps.cap(family)?;
        Some(cap.saturating_sub(self.busy_for(family)))
    }

    /// This family's cap, or `None` when it is not in the table.
    pub fn cap_for(&self, family: &str) -> Option<u32> {
        self.caps.cap(family)
    }

    /// [`busy`](Self::busy) as the JSON that goes into `workflow_runs.fleet_busy`.
    pub fn fleet_busy_json(&self) -> String {
        // A BTreeMap<String, u32> cannot fail to serialise; the fallback keeps the column
        // writable rather than failing a run over a snapshot.
        serde_json::to_string(&self.busy()).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Seats held by one run. Releasing them is this value's `Drop`, so a panic, an early return or
/// a cancelled future all give the seats back.
#[derive(Debug)]
pub struct SeatLease {
    pool: SharedLeasePool,
    family: String,
    seats: u32,
}

impl SeatLease {
    /// The family these seats are on. A lease is single-family by construction — that is how
    /// "one family per run" is enforced structurally rather than by a check someone can forget.
    pub fn family(&self) -> &str {
        &self.family
    }

    pub fn seats(&self) -> u32 {
        self.seats
    }

    /// This lease as the JSON that goes into `workflow_runs.seats_held`.
    pub fn seats_held_json(&self) -> String {
        let mut map = BTreeMap::new();
        map.insert(self.family.clone(), self.seats);
        serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string())
    }
}

impl Drop for SeatLease {
    fn drop(&mut self) {
        let mut held = counts(&self.pool.held);
        let in_use = held.get(&self.family).copied().unwrap_or(0);
        // `saturating_sub` rather than an assert: a double-release would be a bug in this module,
        // and turning it into a panic inside `Drop` during an unwind aborts the process.
        let left = in_use.saturating_sub(self.seats);
        if left == 0 {
            held.remove(&self.family);
        } else {
            held.insert(self.family.clone(), left);
        }
    }
}

/// The caps this host runs on: the [`SEAT_CAPS_SETTING_KEY`] override when it is set and parses,
/// otherwise the measured table. The one impure entry point, kept thin so everything above it is
/// testable without a database.
///
/// Takes `&Database` rather than `&AppState` because `AppState::new` builds the pool, so this has
/// to run before an `AppState` exists — the same reason `terminal`'s startup reconciler does.
pub fn seat_caps_from_db(db: &crate::db::Database) -> SeatCaps {
    let raw = db
        .with_conn(|conn| {
            Ok(conn
                .query_row(
                    "SELECT value FROM settings WHERE key = ?1",
                    rusqlite::params![SEAT_CAPS_SETTING_KEY],
                    |row| row.get::<_, String>(0),
                )
                .unwrap_or_default())
        })
        .unwrap_or_default();
    if raw.trim().is_empty() {
        return SeatCaps::measured();
    }
    match SeatCaps::from_override_json(&raw) {
        Ok((caps, notes)) => {
            // Nothing here is fatal — a different fleet needs different numbers — but nothing
            // here is silent either. One line per note, each naming the family and the numbers,
            // so "we are running at 40 on a fleet measured at 4" or "your key was a case typo"
            // is readable in a log rather than inferred from latency months later.
            for note in notes {
                tracing::warn!(key = SEAT_CAPS_SETTING_KEY, note = %note, "seat-cap override");
            }
            caps
        }
        Err(reason) => {
            // Covers the all-zero override too, which used to slip past `from_json` and leave
            // the host with no capped families, no fallback and no log line.
            tracing::warn!(
                key = SEAT_CAPS_SETTING_KEY,
                %reason,
                "ignoring the configured seat caps; falling back to the measured table"
            );
            SeatCaps::measured()
        }
    }
}

/// [`seat_caps_from_db`] for a caller that already holds the state.
pub fn load_seat_caps(state: &crate::state::app_state::AppState) -> SeatCaps {
    seat_caps_from_db(&state.db)
}

/// The capped families the provider does not list. The seat table is the operator's
/// intent; the provider's /v1/models is what will actually answer. A family in the
/// table and absent from the provider is a pool that can be leased and never used.
///
/// Returned in the caps table's own (sorted) order. Pure: the caller fetches the
/// provider list, this decides, so the claim is testable without a network.
///
/// An EMPTY provider list means "we could not read the provider", NOT "the provider
/// serves nothing": returning every family there would turn one failed HTTP call into
/// a fleet-wide alarm. So an empty list reports nothing.
///
/// Matching is EXACT string equality on the family id — no prefix or substring
/// matching, because `qwen3.8-flash-next` and `qwen3.8-flash-next-132k-fast` are
/// different models and a prefix match would silently call the second one a match for
/// the first.
pub fn families_absent_from_provider(caps: &SeatCaps, provider_ids: &[String]) -> Vec<String> {
    if provider_ids.is_empty() {
        return Vec::new();
    }
    caps.families()
        .filter(|family| !provider_ids.iter().any(|id| id == family))
        .map(String::from)
        .collect()
}

/// Whether the provider serves NONE of the capped families — a different condition
/// from "one is missing", because the arena then cannot run at all rather than
/// running narrower. False for an empty provider list, for the same reason as above.
pub fn provider_serves_no_capped_family(caps: &SeatCaps, provider_ids: &[String]) -> bool {
    if provider_ids.is_empty() {
        return false;
    }
    caps.families()
        .all(|family| !provider_ids.iter().any(|id| id == family))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Barrier;

    #[test]
    fn the_measured_caps_are_the_ones_recorded_in_the_mock() {
        // Measured 2026-10-12, mock.html step 3 ("Measured caps · 2026-10-12").
        let caps = SeatCaps::measured();
        assert_eq!(caps.cap("glm-5.3-flash"), Some(4));
        assert_eq!(caps.cap("deepseek-v4.1-flash"), Some(2));
        assert_eq!(caps.cap("qwen3.8-flash-next"), Some(4));
        assert_eq!(caps.cap("gpt-nonexistent"), None);
        assert_eq!(
            caps.families().collect::<Vec<_>>(),
            vec!["deepseek-v4.1-flash", "glm-5.3-flash", "qwen3.8-flash-next"]
        );
    }

    #[test]
    fn caps_are_data_a_host_can_replace() {
        let caps = SeatCaps::from_json(r#"{"local-7b": 8, "glm-5.3-flash": 1}"#).unwrap();
        assert_eq!(caps.cap("local-7b"), Some(8));
        assert_eq!(caps.cap("glm-5.3-flash"), Some(1));
        // A zero cap is "cannot be leased", so it is dropped from the table...
        assert_eq!(
            SeatCaps::from_json(r#"{"local-7b": 8, "x": 0}"#).unwrap().cap("x"),
            None
        );
        // ...but an override in which EVERY family is zero is refused, not accepted as a table
        // with no families. This assertion is the inverse of the one that was here in round 1:
        // I had written `{"x": 0}` -> Ok as if it were correct, which encoded the bypass QA
        // then found. The guard has to fire on the state, not on the spelling.
        let err = SeatCaps::from_json(r#"{"x": 0}"#).expect_err("an all-zero table is refused");
        assert!(err.contains("set to zero"), "{err}");
        assert!(err.contains("x"), "{err}");
        // Bad configuration is an error, not a silent fallback to different numbers.
        assert!(SeatCaps::from_json("not json").is_err());
        assert_eq!(
            SeatCaps::from_json("{}").unwrap_err(),
            "seat caps object is empty"
        );
        assert!(SeatCaps::from_json(r#"{"x": -1}"#).is_err());
    }

    #[test]
    fn acquire_then_release_restores_the_previous_count() {
        let pool = LeasePool::with_measured_caps();
        assert_eq!(pool.busy_for("glm-5.3-flash"), 0);

        let first = pool.acquire("glm-5.3-flash", 1).unwrap();
        assert_eq!(pool.busy_for("glm-5.3-flash"), 1);
        assert_eq!(pool.free_for("glm-5.3-flash"), Some(3));

        {
            let second = pool.acquire("glm-5.3-flash", 2).unwrap();
            assert_eq!(pool.busy_for("glm-5.3-flash"), 3);
            assert_eq!(second.seats(), 2);
            assert_eq!(second.family(), "glm-5.3-flash");
            assert_eq!(second.seats_held_json(), r#"{"glm-5.3-flash":2}"#);
        }
        // Releasing the inner lease restores the count to what it was before it, not to zero.
        assert_eq!(pool.busy_for("glm-5.3-flash"), 1);

        drop(first);
        assert_eq!(pool.busy_for("glm-5.3-flash"), 0);
        assert_eq!(pool.free_for("glm-5.3-flash"), Some(4));
    }

    #[test]
    fn over_acquire_is_refused_whole_and_takes_nothing() {
        let pool = LeasePool::with_measured_caps();
        let _held = pool.acquire("glm-5.3-flash", 3).unwrap();

        let err = pool.acquire("glm-5.3-flash", 2).expect_err("2 must not fit in 1 free seat");
        assert_eq!(
            err,
            SeatError::OverCap {
                family: "glm-5.3-flash".to_string(),
                requested: 2,
                free: 1,
                cap: 4,
            }
        );
        // Loud: the message names the numbers, so the refusal is readable in a log.
        let shown = err.to_string();
        assert!(shown.contains("2 seats requested"), "{shown}");
        assert!(shown.contains("only 1 of 4 are free"), "{shown}");

        // Atomic: NOT ONE of the two seats was taken.
        assert_eq!(pool.busy_for("glm-5.3-flash"), 3);
        assert_eq!(pool.free_for("glm-5.3-flash"), Some(1));

        // The one that does fit still fits afterwards.
        let _last = pool.acquire("glm-5.3-flash", 1).unwrap();
        assert_eq!(pool.busy_for("glm-5.3-flash"), 4);

        // A request bigger than the whole cap is the same refusal, not a special case.
        let whole = LeasePool::with_measured_caps();
        assert!(matches!(
            whole.acquire("deepseek-v4.1-flash", 3),
            Err(SeatError::OverCap { free: 2, cap: 2, .. })
        ));
        assert_eq!(whole.busy_for("deepseek-v4.1-flash"), 0);
    }

    #[test]
    fn a_full_family_blocks_even_with_other_families_idle() {
        let pool = LeasePool::with_measured_caps();
        let _full = pool.acquire("deepseek-v4.1-flash", 2).unwrap();

        // 8 seats are free across glm + qwen, and none of them may satisfy this.
        assert_eq!(pool.free_for("glm-5.3-flash"), Some(4));
        assert_eq!(pool.free_for("qwen3.8-flash-next"), Some(4));
        assert!(matches!(
            pool.acquire("deepseek-v4.1-flash", 1),
            Err(SeatError::OverCap { free: 0, .. })
        ));

        // And taking seats on another family does not move the full one's count.
        let _other = pool.acquire("glm-5.3-flash", 4).unwrap();
        assert_eq!(pool.busy_for("deepseek-v4.1-flash"), 2);
        assert_eq!(
            pool.busy(),
            BTreeMap::from([
                ("deepseek-v4.1-flash".to_string(), 2),
                ("glm-5.3-flash".to_string(), 4),
                ("qwen3.8-flash-next".to_string(), 0),
            ])
        );
    }

    #[test]
    fn an_uncapped_family_cannot_be_leased() {
        let pool = LeasePool::with_measured_caps();
        let err = pool.acquire("some-model-nobody-measured", 1).unwrap_err();
        assert_eq!(
            err,
            SeatError::UnknownFamily {
                family: "some-model-nobody-measured".to_string()
            }
        );
        assert!(err.to_string().contains("no measured seat cap"), "{err}");
        assert_eq!(pool.cap_for("some-model-nobody-measured"), None);
        assert_eq!(pool.free_for("some-model-nobody-measured"), None);
        // Zero seats is a caller bug, and is refused before anything is counted.
        assert!(matches!(
            pool.acquire("glm-5.3-flash", 0),
            Err(SeatError::ZeroSeats { .. })
        ));
        assert_eq!(pool.busy_for("glm-5.3-flash"), 0);
    }

    /// A lease released only on the happy path is a leaked seat on every panic, and a leaked seat
    /// is permanent: nothing can reconcile against a fleet that cannot be asked how busy it is.
    #[test]
    fn seats_come_back_when_the_holder_panics() {
        let pool = LeasePool::with_measured_caps();
        let inner = Arc::clone(&pool);
        let joined = std::thread::spawn(move || {
            let _lease = inner.acquire("glm-5.3-flash", 3).unwrap();
            assert_eq!(inner.busy_for("glm-5.3-flash"), 3);
            panic!("the phase worker died holding three seats");
        })
        .join();
        assert!(joined.is_err(), "the thread must have panicked");
        assert_eq!(
            pool.busy_for("glm-5.3-flash"),
            0,
            "the panicking holder's seats were never returned"
        );
        // And the pool is still usable afterwards — a poisoned lock must not take it down.
        let _after = pool.acquire("glm-5.3-flash", 4).unwrap();
        assert_eq!(pool.busy_for("glm-5.3-flash"), 4);
    }

    /// Under contention the cap must hold EXACTLY, not approximately.
    ///
    /// 32 threads race for the 4 glm seats, 50 rounds, with a barrier IMMEDIATELY BEFORE
    /// `acquire` so that all 32 calls are in flight together, and a second barrier after it so
    /// nobody releases until everybody has tried. Exactly 4 may win each round.
    ///
    /// The pre-acquire barrier is the whole test. Written without it — threads merely spawned and
    /// left to race — this passed against a deliberately broken `acquire` that read the free
    /// count under one lock, yielded, and took seats under another: `cargo test` sequenced the
    /// spawns so completely that the window never opened. That version measured nothing. With
    /// the barrier, the same mutation over-admits.
    #[test]
    fn the_cap_holds_exactly_under_contention() {
        const THREADS: usize = 32;
        const ROUNDS: usize = 50;
        const CAP: u32 = 4;
        let pool = LeasePool::with_measured_caps();
        let start = Arc::new(Barrier::new(THREADS));
        let mid = Arc::new(Barrier::new(THREADS));
        let won = Arc::new(AtomicU32::new(0));
        let peak = Arc::new(AtomicU32::new(0));

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let pool = Arc::clone(&pool);
                let start = Arc::clone(&start);
                let mid = Arc::clone(&mid);
                let won = Arc::clone(&won);
                let peak = Arc::clone(&peak);
                std::thread::spawn(move || {
                    for _ in 0..ROUNDS {
                        // Every thread is released into `acquire` at the same instant.
                        start.wait();
                        let lease = pool.acquire("glm-5.3-flash", 1);
                        if lease.is_ok() {
                            won.fetch_add(1, Ordering::SeqCst);
                        }
                        peak.fetch_max(pool.busy_for("glm-5.3-flash"), Ordering::SeqCst);
                        // Nobody releases until everybody has tried, so no seat is recycled
                        // mid-round and "exactly 4 winners" is an exact claim, not a likely one.
                        mid.wait();
                        drop(lease);
                        // The next round's `start` barrier cannot release until every thread has
                        // reached it, i.e. after every drop above.
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("no worker may panic");
        }

        assert_eq!(
            won.load(Ordering::SeqCst),
            CAP * ROUNDS as u32,
            "exactly {} of {} threads may win each of {} rounds",
            CAP,
            THREADS,
            ROUNDS
        );
        assert!(
            peak.load(Ordering::SeqCst) <= CAP,
            "seats in use went above the cap of {}: peak {}",
            CAP,
            peak.load(Ordering::SeqCst)
        );
        assert_eq!(
            pool.busy_for("glm-5.3-flash"),
            0,
            "every lease must be released when its holder returns"
        );
    }

    /// The same race across families: the per-family totals must stay independent.
    ///
    /// **No thread in here may panic, and that is a property of the test, not an accident.**
    /// The first version asserted the cap INSIDE the worker thread, before `gate.wait()`. When
    /// the assertion tripped, that thread unwound without reaching the barrier, the other 29
    /// blocked on `Barrier::new(30)` forever and the main thread blocked in `join()` — so the
    /// test HUNG instead of failing, and non-deterministically: it reported correctly 2 runs in
    /// 4, so on a lucky CI run it looked like a working test. Workers therefore only RECORD, and
    /// every assertion happens after `join`.
    ///
    /// **And it must assert that seats were GRANTED, not only that none were over-granted.** The
    /// rewrite checked `peak <= cap` and nothing else, which is true of a pool that refuses
    /// everything: QA measured that `if true || seats > free` left this test green while 17 of
    /// the 24 lease tests went red around it. So the strong form is `won == cap == peak`, per
    /// family: 10 threads race for each family's 4/2/4 seats and exactly that family's cap is
    /// won.
    ///
    /// **What `won == cap` does and does not isolate** — corrected, because the first version of
    /// this comment got it wrong and a wrong reason in a test is worse than none:
    ///
    /// - It does NOT rule out pooling. A pooled implementation is already caught by the
    ///   pre-existing `peak <= cap`, because `busy_for` then reports the shared total and the
    ///   peak reaches 10 against a cap of 4 — the `won` assertion is never even reached.
    /// - It DOES rule out always-refuse (above), and — the clean isolation — **a cap enforced one
    ///   seat too tightly**. With `cap - 1` the peak is 3 against a cap of 4, so every peak
    ///   assertion still passes and only the granted count fails. That matters beyond tidiness:
    ///   an under-granting pool makes the arena silently run narrower than it was configured to,
    ///   and a comparison between two runs at an unintended width is the exact thing this module
    ///   exists to make valid.
    /// - It does NOT cover **starvation**. Both cross-family tests hold a barrier and request
    ///   equal widths, so a wide request refused fifty times behind narrow ones is invisible
    ///   here. That is consistent with there being no queue — documented in the module header as
    ///   the caller's job — and QA pinned it in
    ///   `qa_documents_the_cap_tests_cannot_see_starvation` so that `won == cap` is not read as
    ///   covering fairness.
    #[test]
    fn concurrent_families_do_not_bleed_into_each_other() {
        const FAMILIES: [&str; 3] = ["glm-5.3-flash", "deepseek-v4.1-flash", "qwen3.8-flash-next"];
        const THREADS: usize = 30;
        let pool = LeasePool::with_measured_caps();
        let start = Arc::new(Barrier::new(THREADS));
        let gate = Arc::new(Barrier::new(THREADS));
        // Per-family, by the same index as FAMILIES: peak seats observed in use, and how many
        // threads were granted a lease.
        let peaks: Arc<[AtomicU32; 3]> =
            Arc::new([AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)]);
        let wins: Arc<[AtomicU32; 3]> =
            Arc::new([AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)]);

        let handles: Vec<_> = (0..THREADS)
            .map(|i| {
                let pool = Arc::clone(&pool);
                let start = Arc::clone(&start);
                let gate = Arc::clone(&gate);
                let peaks = Arc::clone(&peaks);
                let wins = Arc::clone(&wins);
                let slot = i % FAMILIES.len();
                let family = FAMILIES[slot];
                std::thread::spawn(move || {
                    start.wait();
                    let lease = pool.acquire(family, 1);
                    if lease.is_ok() {
                        wins[slot].fetch_add(1, Ordering::SeqCst);
                    }
                    // Observe the count whether or not this thread won, so an over-admission
                    // caused by someone else is still seen.
                    peaks[slot].fetch_max(pool.busy_for(family), Ordering::SeqCst);
                    gate.wait();
                    drop(lease);
                })
            })
            .collect();
        for h in handles {
            h.join().expect("no worker may panic");
        }

        // Every judgement is made here, on the main thread, where a failure is a RED TEST.
        for (slot, family) in FAMILIES.iter().enumerate() {
            let cap = pool.cap_for(family).expect("every family in FAMILIES is capped");
            let peak = peaks[slot].load(Ordering::SeqCst);
            let won = wins[slot].load(Ordering::SeqCst);
            assert!(
                peak <= cap,
                "{family} went over its cap of {cap}: peak {peak} seats in use"
            );
            assert_eq!(
                won, cap,
                "{family}: {won} of {cap} seats granted — 10 threads raced for them, so anything \
                 less means this race measured nothing, and a pool that refuses everything would \
                 satisfy the cap assertion above on its own"
            );
            assert_eq!(
                peak, cap,
                "{family}: peak {peak} must REACH the cap {cap}, or no seat was ever really held"
            );
        }
        // All three families back to idle — not just the one, so a release that credited the
        // wrong family would show here.
        assert_eq!(
            pool.busy(),
            BTreeMap::from([
                ("deepseek-v4.1-flash".to_string(), 0),
                ("glm-5.3-flash".to_string(), 0),
                ("qwen3.8-flash-next".to_string(), 0),
            ])
        );
    }

    /// The ORDER the module header prescribes, measured against the order it used to prescribe.
    ///
    /// There is no code here to get wrong — the sequence lives in the caller, which does not
    /// exist yet — so this test exists to make the cost of the wrong order a number rather than
    /// a paragraph. It is the only guard the doc has until Dev A writes those three lines.
    #[test]
    fn the_snapshot_order_decides_whether_two_runs_are_ever_comparable() {
        let pool = LeasePool::with_measured_caps();
        // A rival run already holds 1 glm seat: that is the contention this run starts against,
        // and it is what `fleet_busy` is for.
        let _rival = pool.acquire("glm-5.3-flash", 1).unwrap();

        // RIGHT: snapshot, then acquire, then take seats_held from the lease.
        let fleet_busy_right = pool.fleet_busy_json();
        let lease = pool.acquire("glm-5.3-flash", 2).unwrap();
        let seats_held = lease.seats_held_json();

        // WRONG: the order the header used to prescribe, and that Dev A's branch had too.
        let fleet_busy_wrong = pool.fleet_busy_json();

        assert!(
            fleet_busy_right.contains(r#""glm-5.3-flash":1"#),
            "the rival's 1 seat is the contention this run started against: {fleet_busy_right}"
        );
        assert!(
            fleet_busy_wrong.contains(r#""glm-5.3-flash":3"#),
            "snapshotting after acquire counts this run's own 2 seats as foreign contention: \
             {fleet_busy_wrong}"
        );
        assert_ne!(
            fleet_busy_right, fleet_busy_wrong,
            "the two orders are not interchangeable"
        );
        // And this run's OWN seats are the other column, which is why nothing is lost by
        // excluding them from the first one.
        assert_eq!(seats_held, r#"{"glm-5.3-flash":2}"#);

        // The failure mode, concretely. A run ALONE on an idle fleet, under the wrong order,
        // reports contention that does not exist — its own seats.
        //
        // (A correction to this test, kept because it is the same mistake in miniature: the
        // first version asserted the quiet run's snapshot was IDENTICAL to the contended one's.
        // It is not — 2 against 3 — because the wrong order adds a per-run offset rather than
        // erasing the difference. The real cost is that every run's snapshot is inflated by its
        // own `fan_out`, so a budget comparison between two runs of different widths is wrong by
        // the difference of their widths, which is exactly the quantity it is trying to control
        // for.)
        let quiet = LeasePool::with_measured_caps();
        let quiet_lease = quiet.acquire("glm-5.3-flash", 2).unwrap();
        let quiet_wrong = quiet.fleet_busy_json();
        assert!(
            quiet_wrong.contains(r#""glm-5.3-flash":2"#),
            "a run alone on an idle fleet reports 2 seats of contention that do not exist: \
             {quiet_wrong}"
        );
        drop(quiet_lease);
        // Under the RIGHT order the same run would have recorded an idle fleet.
        let quiet = LeasePool::with_measured_caps();
        let quiet_right = quiet.fleet_busy_json();
        let _quiet_lease = quiet.acquire("glm-5.3-flash", 2).unwrap();
        assert!(
            quiet_right.contains(r#""glm-5.3-flash":0"#),
            "the truth is that the fleet was idle: {quiet_right}"
        );
        assert_ne!(quiet_right, quiet_wrong, "the two orders disagree about an idle fleet");
    }

    /// The two JSON columns a run snapshots at start.
    #[test]
    fn the_snapshot_columns_are_json_a_run_can_store() {
        let pool = LeasePool::with_measured_caps();
        assert_eq!(
            pool.fleet_busy_json(),
            r#"{"deepseek-v4.1-flash":0,"glm-5.3-flash":0,"qwen3.8-flash-next":0}"#
        );
        let lease = pool.acquire("glm-5.3-flash", 2).unwrap();
        assert_eq!(lease.seats_held_json(), r#"{"glm-5.3-flash":2}"#);
        assert_eq!(
            pool.fleet_busy_json(),
            r#"{"deepseek-v4.1-flash":0,"glm-5.3-flash":2,"qwen3.8-flash-next":0}"#
        );
        // An idle family stays in the snapshot at 0, so "quiet" is distinguishable from "not
        // recorded" when a later comparability check reads the column back.
        assert!(pool.fleet_busy_json().contains(r#""qwen3.8-flash-next":0"#));
    }

    #[test]
    fn the_host_pool_is_on_app_state_and_starts_idle() {
        let (state, _root) = crate::test_support::test_state();
        assert_eq!(
            state.lease_pool.busy(),
            BTreeMap::from([
                ("deepseek-v4.1-flash".to_string(), 0),
                ("glm-5.3-flash".to_string(), 0),
                ("qwen3.8-flash-next".to_string(), 0),
            ])
        );
        // Unset setting → the measured table.
        assert_eq!(load_seat_caps(&state), SeatCaps::measured());
        crate::services::settings::set_setting(
            &state,
            SEAT_CAPS_SETTING_KEY.to_string(),
            r#"{"glm-5.3-flash": 1}"#.to_string(),
        )
        .unwrap();
        assert_eq!(load_seat_caps(&state).cap("glm-5.3-flash"), Some(1));
        // A broken value falls back rather than leaving the host with no caps at all.
        crate::services::settings::set_setting(
            &state,
            SEAT_CAPS_SETTING_KEY.to_string(),
            "{oops".to_string(),
        )
        .unwrap();
        assert_eq!(load_seat_caps(&state), SeatCaps::measured());
    }

    /// A provider id list, spelled once so each test reads as its own claim.
    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn seatcaps_a_capped_family_the_provider_does_not_list_is_reported() {
        // The real case, read off the provider's /v1/models on 2026-10-12: it serves glm and
        // the qwen family (plus qwen variants) but NOT deepseek, so the table caps a pool that
        // can be leased and never used.
        let caps = SeatCaps::measured();
        let provider = ids(&[
            "glm-5.3-flash",
            "qwen3.8-flash-next",
            "qwen3.8-flash-next-132k-fast",
            "qwen3.8-flash-next-160k-validator",
        ]);
        assert_eq!(
            families_absent_from_provider(&caps, &provider),
            vec!["deepseek-v4.1-flash".to_string()]
        );
    }

    #[test]
    fn seatcaps_a_family_the_provider_lists_is_not_reported() {
        let caps = SeatCaps::measured();
        let provider = ids(&["glm-5.3-flash", "deepseek-v4.1-flash", "qwen3.8-flash-next"]);
        assert_eq!(families_absent_from_provider(&caps, &provider), Vec::<String>::new());

        // The prefix-match trap: `qwen3.8-flash-next-132k-fast` is a DIFFERENT model, so it
        // does not satisfy `qwen3.8-flash-next`. A startswith/contains comparison would call
        // this a match and the gap would go unreported.
        let variants_only = ids(&["qwen3.8-flash-next-132k-fast"]);
        assert_eq!(
            families_absent_from_provider(&caps, &variants_only),
            vec![
                "deepseek-v4.1-flash".to_string(),
                "glm-5.3-flash".to_string(),
                "qwen3.8-flash-next".to_string(),
            ]
        );
    }

    #[test]
    fn seatcaps_an_empty_provider_list_is_not_treated_as_every_family_missing() {
        // An empty list means "we could not read the provider", not "the provider serves
        // nothing" — one failed HTTP call must not become a fleet-wide alarm.
        let caps = SeatCaps::measured();
        assert_eq!(families_absent_from_provider(&caps, &[]), Vec::<String>::new());
        assert!(!provider_serves_no_capped_family(&caps, &[]));
    }

    #[test]
    fn seatcaps_provider_serving_none_of_the_capped_families_is_its_own_condition() {
        // A non-empty list that shares nothing with the table: the arena cannot run at all,
        // which is a louder state than "one family is missing".
        let caps = SeatCaps::measured();
        let none = ids(&["dgx3-qwen", "boxb-qwen"]);
        assert_eq!(
            families_absent_from_provider(&caps, &none),
            vec![
                "deepseek-v4.1-flash".to_string(),
                "glm-5.3-flash".to_string(),
                "qwen3.8-flash-next".to_string(),
            ]
        );
        assert!(provider_serves_no_capped_family(&caps, &none));

        // One family present and the arena can run, narrowly — so this condition is false even
        // though two of the three are still absent.
        let one = ids(&["glm-5.3-flash"]);
        assert!(!provider_serves_no_capped_family(&caps, &one));
        assert_eq!(
            families_absent_from_provider(&caps, &one),
            vec![
                "deepseek-v4.1-flash".to_string(),
                "qwen3.8-flash-next".to_string(),
            ]
        );
    }
}

/// QA B — adversarial tests written against the claims in this module's own doc comment.
///
/// These exist to try to BREAK the lease pool, not to re-state it. Where a test pins behaviour
/// that QA believes is a GAP rather than a guarantee, its name says `documents_` and its doc
/// comment says what a reviewer should conclude. Those are tripwires: if someone later closes
/// the gap, the test fails and has to be updated deliberately rather than the gap closing
/// silently in one direction or the other.
#[cfg(test)]
mod qa_b_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Barrier;
    use std::time::{Duration, Instant};

    // ── Claim 1: "no lock is held across an await" — and its CONSEQUENCE ────────────────────────

    /// The module's claim is about its own internals. The consequence for a CALLER is the thing
    /// that matters, and it rests on a property the doc comment never states: a `SeatLease` is
    /// `Send`, so an async caller really can hold one across an `.await`. If it were not `Send`,
    /// the arena's run starter — which holds the lease "for the life of the run" — could not be
    /// written as an async task at all, and the design note about the earlier deadlock would be
    /// describing an API nobody can use correctly.
    #[test]
    fn qa_a_lease_is_send_and_sync_so_an_async_run_starter_can_hold_one() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<SeatLease>();
        assert_sync::<SeatLease>();
        assert_send::<SharedLeasePool>();
        assert_sync::<SharedLeasePool>();
    }

    /// A lease held across an `.await` whose future is CANCELLED must give the seats back.
    ///
    /// This is the real-world leak: an arena run is a future, a future gets dropped (a timeout, a
    /// `select!` losing a race, `stop_plan`), and the lease's `Drop` is the only thing that can
    /// notice. The future below is polled once — so the seats are really taken — and then dropped
    /// while suspended inside the `await`, not allowed to run to completion.
    #[tokio::test]
    async fn qa_a_future_cancelled_mid_await_returns_its_seats() {
        let pool = LeasePool::with_measured_caps();
        let inner = Arc::clone(&pool);
        let mut held = Box::pin(async move {
            let lease = inner.acquire("glm-5.3-flash", 3).unwrap();
            // Suspension point WITH the lease alive across it.
            tokio::time::sleep(Duration::from_secs(3600)).await;
            drop(lease);
        });

        // Poll it exactly far enough to reach the await.
        tokio::select! {
            _ = &mut held => unreachable!("the hour-long sleep cannot have finished"),
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
        assert_eq!(
            pool.busy_for("glm-5.3-flash"),
            3,
            "the future must really be suspended while holding the seats"
        );

        // Cancel it: dropping a suspended future runs the destructors of everything it owns.
        drop(held);
        assert_eq!(
            pool.busy_for("glm-5.3-flash"),
            0,
            "a cancelled run leaked its seats; nothing can reconcile them back"
        );
        // The freed seats are really re-leasable, not just missing from the count.
        let _after = pool.acquire("glm-5.3-flash", 4).unwrap();
    }

    /// The same thing through `JoinHandle::abort`, which is how a real task is cancelled.
    #[tokio::test]
    async fn qa_an_aborted_task_returns_its_seats() {
        let pool = LeasePool::with_measured_caps();
        let inner = Arc::clone(&pool);
        let task = tokio::spawn(async move {
            let _lease = inner.acquire("deepseek-v4.1-flash", 2).unwrap();
            tokio::time::sleep(Duration::from_secs(3600)).await;
        });
        // Wait for the seats to appear rather than assuming the task was scheduled.
        let deadline = Instant::now() + Duration::from_secs(5);
        while pool.busy_for("deepseek-v4.1-flash") != 2 {
            assert!(Instant::now() < deadline, "the task never took its seats");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        task.abort();
        let _ = task.await;
        assert_eq!(
            pool.busy_for("deepseek-v4.1-flash"),
            0,
            "an aborted task leaked its seats"
        );
    }

    // ── Claim 3: a refused acquire mutates nothing, and its message is not a lie ───────────────

    /// Many refusals at once, all reporting the same `free`, and then exactly that many seats
    /// really are takeable — no more.
    ///
    /// Attacks two things at once: that racing refusals leave no trace (claim 3), and that
    /// `SeatError::OverCap`'s `free` cannot name seats that do not exist. If the message said
    /// `free: 1` and a 1-seat acquire then failed, the message would be a lie; if TWO 1-seat
    /// acquires succeeded, the cap would be a lie.
    #[test]
    fn qa_racing_refusals_take_nothing_and_report_a_free_count_that_is_real() {
        const N: usize = 24;
        let pool = LeasePool::with_measured_caps();
        let _held = pool.acquire("glm-5.3-flash", 3).unwrap(); // exactly 1 seat free

        let start = Arc::new(Barrier::new(N));
        let handles: Vec<_> = (0..N)
            .map(|_| {
                let pool = Arc::clone(&pool);
                let start = Arc::clone(&start);
                std::thread::spawn(move || {
                    start.wait();
                    // 2 cannot fit in 1 — every one of these must be refused.
                    pool.acquire("glm-5.3-flash", 2).map(|l| l.seats())
                })
            })
            .collect();
        for h in handles {
            let outcome = h.join().expect("no racer may panic");
            assert_eq!(
                outcome.unwrap_err(),
                SeatError::OverCap {
                    family: "glm-5.3-flash".to_string(),
                    requested: 2,
                    free: 1,
                    cap: 4,
                },
                "a refusal under contention must report the real numbers and take nothing"
            );
        }
        assert_eq!(
            pool.busy_for("glm-5.3-flash"),
            3,
            "{N} refusals must not have moved the count by a single seat"
        );

        // The `free: 1` every refusal named is real...
        let last = pool.acquire("glm-5.3-flash", 1).expect("the reported free seat must exist");
        // ...and there was exactly one of it.
        assert!(matches!(
            pool.acquire("glm-5.3-flash", 1),
            Err(SeatError::OverCap { free: 0, cap: 4, .. })
        ));
        drop(last);
        assert_eq!(pool.busy_for("glm-5.3-flash"), 3);
    }

    /// `free` must equal `cap - in_use` for every reachable `(in_use, requested)` pair, and the
    /// exact `free` must always fit. Exhaustive over the whole 4-seat family rather than sampled.
    #[test]
    fn qa_over_cap_free_is_exact_for_every_reachable_state() {
        for in_use in 0..=4u32 {
            for requested in (5 - in_use)..=12u32 {
                let pool = LeasePool::with_measured_caps();
                let _pre = (in_use > 0).then(|| pool.acquire("glm-5.3-flash", in_use).unwrap());
                let free = 4 - in_use;
                assert_eq!(
                    pool.acquire("glm-5.3-flash", requested).unwrap_err(),
                    SeatError::OverCap {
                        family: "glm-5.3-flash".to_string(),
                        requested,
                        free,
                        cap: 4,
                    },
                    "in_use={in_use} requested={requested}"
                );
                assert_eq!(pool.busy_for("glm-5.3-flash"), in_use, "refusal mutated the count");
                if free > 0 {
                    // The refusal's own number is actionable: asking for exactly `free` works.
                    let fits = pool.acquire("glm-5.3-flash", free).expect("free seats must fit");
                    assert_eq!(fits.seats(), free);
                    assert_eq!(pool.busy_for("glm-5.3-flash"), 4);
                }
            }
        }
    }

    // ── Claim 2, the other direction: an interleaving the committed test cannot see ────────────

    /// The committed concurrency test holds a barrier between `acquire` and `drop`, so no seat is
    /// ever recycled while another thread is acquiring. This one deliberately removes that
    /// barrier and mixes request widths, so `acquire` races `release` and a 3-seat request races
    /// a 1-seat one. The pool's OWN count is sampled (not an independent mirror, which could
    /// legitimately skew past the cap and make this flaky in the wrong direction), so a sample
    /// above the cap can only mean the pool granted more than it has.
    #[test]
    fn qa_the_cap_holds_when_acquire_races_release_at_mixed_widths() {
        const THREADS: usize = 24;
        const ROUNDS: usize = 400;
        const CAP: u32 = 4;
        let pool = LeasePool::with_measured_caps();
        let start = Arc::new(Barrier::new(THREADS));
        let peak = Arc::new(AtomicU32::new(0));
        let grants = Arc::new(AtomicU32::new(0));

        let handles: Vec<_> = (0..THREADS)
            .map(|t| {
                let pool = Arc::clone(&pool);
                let start = Arc::clone(&start);
                let peak = Arc::clone(&peak);
                let grants = Arc::clone(&grants);
                std::thread::spawn(move || {
                    // Widths 1, 2 and 3 all in flight at once.
                    let width = (t % 3) as u32 + 1;
                    start.wait();
                    for _ in 0..ROUNDS {
                        // No barrier anywhere in here: some threads are dropping leases while
                        // others are acquiring.
                        if let Ok(lease) = pool.acquire("glm-5.3-flash", width) {
                            grants.fetch_add(1, Ordering::SeqCst);
                            let seen = pool.busy_for("glm-5.3-flash");
                            peak.fetch_max(seen, Ordering::SeqCst);
                            assert!(
                                seen <= CAP,
                                "the pool itself reports {seen} seats in use against a cap of {CAP}"
                            );
                            assert_eq!(lease.seats(), width);
                            drop(lease);
                        }
                        std::thread::yield_now();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("no worker may panic");
        }
        assert!(
            peak.load(Ordering::SeqCst) <= CAP,
            "seats in use peaked at {} against a cap of {CAP}",
            peak.load(Ordering::SeqCst)
        );
        // Not vacuous: the race really did happen, i.e. seats were granted many times over.
        assert!(
            grants.load(Ordering::SeqCst) > (THREADS as u32),
            "only {} grants — the race never got going, so this test measured nothing",
            grants.load(Ordering::SeqCst)
        );
        assert_eq!(
            pool.busy_for("glm-5.3-flash"),
            0,
            "every lease must be released when its holder returns"
        );
    }

    // ── Claim 8: the DB override path ─────────────────────────────────────────────────────────

    /// FIXED (was `qa_documents_an_all_zero_override_bypasses_the_empty_object_guard`, QA round
    /// 2). An all-zero override is refused exactly like an empty object, because it is the same
    /// state.
    ///
    /// The emptiness check used to run on the PARSED map, before `from_pairs` drops zero-seat
    /// families, so `{}` was refused while `{"glm":0,…}` was accepted — and produced the very
    /// state the guard exists to prevent: a caps table with no families, every request then
    /// refused as `UnknownFamily`, whose message tells the operator to add the family to the
    /// table when they are the one who zeroed it. The check now runs after normalisation.
    #[test]
    fn qa_an_all_zero_override_is_refused_like_an_empty_object() {
        // The empty object's wording is unchanged — it is the honest description of `{}`.
        assert_eq!(
            SeatCaps::from_json("{}").unwrap_err(),
            "seat caps object is empty"
        );
        // The same END STATE, now refused, with a message that names the real cause and the
        // families involved rather than misdirecting the operator.
        let err = SeatCaps::from_json(
            r#"{"glm-5.3-flash":0,"deepseek-v4.1-flash":0,"qwen3.8-flash-next":0}"#,
        )
        .expect_err("an override that zeroes every family must be refused");
        assert!(err.contains("set to zero"), "{err}");
        assert!(err.contains("glm-5.3-flash"), "{err}");
        assert!(err.contains("deepseek-v4.1-flash"), "{err}");
        assert!(err.contains("remove the key to use the measured caps"), "{err}");

        // A partial zero is NOT refused: the surviving families are a usable table.
        let partial = SeatCaps::from_json(r#"{"glm-5.3-flash":4,"qwen3.8-flash-next":0}"#)
            .expect("one real family is enough");
        assert_eq!(partial.cap("glm-5.3-flash"), Some(4));
        assert_eq!(partial.cap("qwen3.8-flash-next"), None);
    }

    /// FIXED (was `qa_documents_a_zeroed_setting_leaves_the_host_with_no_caps_and_no_fallback`,
    /// QA round 2). Through the real setting: a zeroed override now falls back to the measured
    /// table, exactly as a malformed one does, instead of leaving the host with no caps at all.
    #[test]
    fn qa_a_zeroed_setting_falls_back_to_the_measured_table() {
        let (state, _root) = crate::test_support::test_state();
        crate::services::settings::set_setting(
            &state,
            SEAT_CAPS_SETTING_KEY.to_string(),
            r#"{"glm-5.3-flash":0}"#.to_string(),
        )
        .unwrap();
        let caps = load_seat_caps(&state);
        assert_eq!(
            caps,
            SeatCaps::measured(),
            "a zeroed override must fall back to the measured table, like a malformed one"
        );
        assert_eq!(caps.families().count(), 3);
        // And the host is usable rather than refusing everything as UnknownFamily.
        let pool = Arc::new(LeasePool::new(caps));
        assert!(pool.acquire("glm-5.3-flash", 4).is_ok());
    }

    /// FIXED (was `qa_an_override_above_the_measured_cap_is_accepted_with_no_warning`, QA round
    /// 2). An override above the measured cap is still ACCEPTED — a different fleet legitimately
    /// needs a different number — but it is no longer silent.
    ///
    /// `exceedances_vs_measured` names the family, the configured value and the measured one, and
    /// `seat_caps_from_db` logs one warning per row. Returned as data precisely so this can be
    /// asserted: "it warns" is not a testable claim about a `tracing::warn!`.
    #[test]
    fn qa_an_override_above_the_measured_cap_is_accepted_but_reported() {
        let fat = SeatCaps::from_json(r#"{"glm-5.3-flash": 40}"#).unwrap();
        assert_eq!(
            fat.cap("glm-5.3-flash"),
            Some(40),
            "ten times the measured 4, still accepted verbatim — this is a warning, not a clamp"
        );
        assert_eq!(SeatCaps::measured().cap("glm-5.3-flash"), Some(4));

        // The difference is now reported, with all three numbers.
        assert_eq!(
            fat.exceedances_vs_measured(),
            vec![("glm-5.3-flash".to_string(), 40, 4)]
        );

        // Still granted in full, so the operator's intent is honoured.
        let pool = Arc::new(LeasePool::new(fat));
        let lease = pool
            .acquire("glm-5.3-flash", 40)
            .expect("40 seats are granted on an explicitly configured 40-seat cap");
        assert_eq!(lease.seats(), 40);
        assert_eq!(pool.busy_for("glm-5.3-flash"), 40);

        // A cap at or BELOW the measured one is not reported — only an excess is news.
        assert!(SeatCaps::measured().exceedances_vs_measured().is_empty());
        assert!(SeatCaps::from_json(r#"{"glm-5.3-flash": 1}"#)
            .unwrap()
            .exceedances_vs_measured()
            .is_empty());
        // A family nobody measured has nothing to exceed, so it is not reported either.
        assert!(SeatCaps::from_json(r#"{"local-7b": 99}"#)
            .unwrap()
            .exceedances_vs_measured()
            .is_empty());
        // u32::MAX is still accepted on the way in, and reported.
        let huge = SeatCaps::from_json(r#"{"glm-5.3-flash": 4294967295}"#).unwrap();
        assert_eq!(huge.cap("glm-5.3-flash"), Some(u32::MAX));
        assert_eq!(
            huge.exceedances_vs_measured(),
            vec![("glm-5.3-flash".to_string(), u32::MAX, 4)]
        );
        // Every measured family is checked, not just the first.
        let all = SeatCaps::from_json(
            r#"{"glm-5.3-flash":5,"deepseek-v4.1-flash":3,"qwen3.8-flash-next":5}"#,
        )
        .unwrap();
        assert_eq!(
            all.exceedances_vs_measured(),
            vec![
                ("glm-5.3-flash".to_string(), 5, 4),
                ("deepseek-v4.1-flash".to_string(), 3, 2),
                ("qwen3.8-flash-next".to_string(), 5, 4),
            ]
        );
    }

    /// The caps table is immutable, so no family can appear or change mid-run: the one
    /// interleaving the concurrency test could not cover is also the one that cannot happen.
    #[test]
    fn qa_a_family_cannot_be_added_or_resized_after_the_pool_is_built() {
        let pool = LeasePool::with_measured_caps();
        assert_eq!(pool.cap_for("late-arrival"), None);
        // There is no `&mut self` on LeasePool and no setter on SeatCaps; the only way to a
        // different cap is a different pool, which a running arena run cannot be moved onto.
        let other = Arc::new(LeasePool::new(
            SeatCaps::from_json(r#"{"late-arrival": 2}"#).unwrap(),
        ));
        let _on_other = other.acquire("late-arrival", 2).unwrap();
        assert_eq!(
            pool.acquire("late-arrival", 1).unwrap_err(),
            SeatError::UnknownFamily {
                family: "late-arrival".to_string()
            },
            "the original pool is unchanged by another pool's caps"
        );
        assert_eq!(pool.busy_for("late-arrival"), 0);
    }

    // ── Gaps a reviewer should be told about ──────────────────────────────────────────────────

    /// GAP. `mock.html` step 3 says "a run that cannot get its seats QUEUES; it never proceeds at
    /// a reduced width". The pool implements the second half and not the first: there is no
    /// blocking or waiting acquire, so a refused caller gets an immediate error and the queue —
    /// including its fairness — is left for the caller to build. Pinned so that "the lease pool
    /// is done" is not read as "the mock's seat behaviour is implemented".
    #[test]
    fn qa_documents_there_is_no_queue_only_an_immediate_refusal() {
        let pool = LeasePool::with_measured_caps();
        let _full = pool.acquire("glm-5.3-flash", 4).unwrap();
        let before = Instant::now();
        assert!(pool.acquire("glm-5.3-flash", 1).is_err());
        assert!(
            before.elapsed() < Duration::from_millis(100),
            "acquire returned after {:?} — it must refuse, not wait",
            before.elapsed()
        );
    }

    /// GAP. A run that needs seats on two families takes two leases, and there is no
    /// all-or-nothing primitive across them. So a caller can hold one family's seats while being
    /// refused the other's, and must unwind the first itself — and two such callers taking the
    /// families in opposite orders starve each other with no queue to break the tie. The
    /// module's atomicity guarantee is per lease, not per run.
    #[test]
    fn qa_documents_a_two_family_request_is_not_atomic_across_families() {
        let pool = LeasePool::with_measured_caps();
        let _rival = pool.acquire("deepseek-v4.1-flash", 2).unwrap();

        let half = pool
            .acquire("glm-5.3-flash", 2)
            .expect("the first half of the run's budget is granted");
        assert!(
            pool.acquire("deepseek-v4.1-flash", 1).is_err(),
            "the second half is refused"
        );
        // The granted half is STILL held: nothing rolled it back.
        assert_eq!(pool.busy_for("glm-5.3-flash"), 2);
        assert_eq!(half.seats(), 2);
        // `seats_held_json` can therefore only ever describe one family of a multi-family run.
        assert_eq!(half.seats_held_json(), r#"{"glm-5.3-flash":2}"#);
    }

    /// The measured table is transcribed from the mock. Re-pinned here against the mock's own
    /// wording so the two cannot drift silently in either direction.
    #[test]
    fn qa_the_measured_table_matches_the_mocks_three_rows() {
        assert_eq!(
            MEASURED_SEAT_CAPS,
            &[
                ("glm-5.3-flash", 4u32),
                ("deepseek-v4.1-flash", 2),
                ("qwen3.8-flash-next", 4),
            ][..],
            "mock.html step 3, table \"Measured caps · 2026-10-12\""
        );
        // Nothing branches on a family name: an unknown family is refused by the same code path
        // that would serve a known one, not by a special case.
        let pool = LeasePool::with_measured_caps();
        for (family, cap) in MEASURED_SEAT_CAPS {
            assert_eq!(pool.cap_for(family), Some(*cap));
            let lease = pool.acquire(family, *cap).unwrap();
            assert_eq!(pool.free_for(family), Some(0));
            drop(lease);
        }
    }
}

/// QA B round 2 — attacking the all-zero refusal and the exceedance report.
#[cfg(test)]
mod qa_b_r2_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Barrier;
    use std::sync::Arc;

    // ── `exceedances_vs_measured` — every shape ───────────────────────────────────────────────

    /// The report's contents for each shape an override can take.
    #[test]
    fn qa_exceedances_are_right_for_every_override_shape() {
        let caps = |s: &str| SeatCaps::from_json(s).unwrap();

        // Exactly equal to measured → nothing to report.
        assert!(caps(r#"{"glm-5.3-flash":4,"deepseek-v4.1-flash":2,"qwen3.8-flash-next":4}"#)
            .exceedances_vs_measured()
            .is_empty());
        // One over, by one.
        assert_eq!(
            caps(r#"{"glm-5.3-flash":5}"#).exceedances_vs_measured(),
            vec![("glm-5.3-flash".to_string(), 5, 4)]
        );
        // All three over.
        assert_eq!(
            caps(r#"{"glm-5.3-flash":9,"deepseek-v4.1-flash":9,"qwen3.8-flash-next":9}"#)
                .exceedances_vs_measured(),
            vec![
                ("glm-5.3-flash".to_string(), 9, 4),
                ("deepseek-v4.1-flash".to_string(), 9, 2),
                ("qwen3.8-flash-next".to_string(), 9, 4),
            ]
        );
        // Below measured → not news.
        assert!(caps(r#"{"glm-5.3-flash":1}"#).exceedances_vs_measured().is_empty());
        // A family not in the measured table has nothing to exceed.
        assert!(caps(r#"{"local-7b":999}"#).exceedances_vs_measured().is_empty());
        // A mixed override reports only the family that exceeds.
        assert_eq!(
            caps(r#"{"glm-5.3-flash":1,"deepseek-v4.1-flash":7,"local-7b":999}"#)
                .exceedances_vs_measured(),
            vec![("deepseek-v4.1-flash".to_string(), 7, 2)]
        );
        // And the measured table never reports against itself.
        assert!(SeatCaps::measured().exceedances_vs_measured().is_empty());
    }

    /// The report's ORDER is the measured table's order, not alphabetical.
    ///
    /// The doc comment says "ascending", which is what `families()` (a `BTreeMap`) gives and is
    /// NOT what this gives: it iterates `MEASURED_SEAT_CAPS`, so the order is glm, deepseek,
    /// qwen. Deterministic either way, and the order is what Dev B's own assertion depends on —
    /// pinned here because the doc and the code disagree about which guarantee a caller has.
    #[test]
    fn qa_the_exceedance_order_is_the_measured_tables_not_alphabetical() {
        let all = SeatCaps::from_json(
            r#"{"deepseek-v4.1-flash":9,"glm-5.3-flash":9,"qwen3.8-flash-next":9}"#,
        )
        .unwrap();
        let names: Vec<String> = all.exceedances_vs_measured().into_iter().map(|(f, _, _)| f).collect();
        assert_eq!(
            names,
            vec!["glm-5.3-flash", "deepseek-v4.1-flash", "qwen3.8-flash-next"],
            "the order follows MEASURED_SEAT_CAPS"
        );
        // Alphabetical would be a different list, so the two really are distinguishable.
        let mut alphabetical = names.clone();
        alphabetical.sort();
        assert_ne!(names, alphabetical, "if these matched, the test would prove nothing");
        // `families()` IS ascending, which is where the doc's wording came from.
        assert_eq!(
            all.families().collect::<Vec<_>>(),
            vec!["deepseek-v4.1-flash", "glm-5.3-flash", "qwen3.8-flash-next"]
        );
    }

    // ── other doors to the all-zero state ─────────────────────────────────────────────────────

    /// The all-zero refusal fires on the STATE, so every spelling of it is refused.
    #[test]
    fn qa_every_spelling_of_an_all_zero_override_is_refused() {
        for raw in [
            r#"{"x":0}"#,
            r#"{"glm-5.3-flash":0}"#,
            r#"{"glm-5.3-flash":0,"deepseek-v4.1-flash":0,"qwen3.8-flash-next":0}"#,
            // Whitespace-only and odd keys are still keys, and still zero.
            r#"{" ":0}"#,
            r#"{"":0}"#,
            // Padded JSON (from_json trims the input).
            "   {\"glm-5.3-flash\":0}   ",
            // A DUPLICATE key: serde takes the last value, so a trailing zero silently wins and
            // the whole table is zeroed. Worth pinning — the first value looks authoritative.
            r#"{"glm-5.3-flash":4,"glm-5.3-flash":0}"#,
        ] {
            let err = SeatCaps::from_json(raw).expect_err(raw);
            assert!(
                err.contains("set to zero") || err == "seat caps object is empty",
                "{raw} -> {err}"
            );
        }
        // The duplicate-key case specifically: last value wins, so this is NOT a 4-seat table.
        assert!(
            SeatCaps::from_json(r#"{"glm-5.3-flash":4,"glm-5.3-flash":0}"#).is_err(),
            "a duplicate key whose last value is 0 must not be read as the first value"
        );
        // And the reverse order is accepted at the LAST value, not the first.
        assert_eq!(
            SeatCaps::from_json(r#"{"glm-5.3-flash":0,"glm-5.3-flash":4}"#)
                .unwrap()
                .cap("glm-5.3-flash"),
            Some(4)
        );
    }

    /// FIXED (was `qa_documents_an_override_that_omits_a_measured_family_is_silent`, QA round 3).
    /// A partial override now MERGES over the measured table, so an omitted family keeps its
    /// measured cap instead of becoming unleasable.
    ///
    /// QA's finding, verbatim: omitting a key had the same effect as zeroing it, and the all-zero
    /// refusal only fired when EVERY key was zero — so `{"glm-5.3-flash": 4}` silently took two
    /// of the three measured families out of service, two thirds of the harm the all-zero guard
    /// was added to prevent. It also shrank `fleet_busy` to one row, the exact ambiguity
    /// `busy()`'s doc comment says it exists to avoid, in the column that decides comparability.
    ///
    /// Same fixture, inverted. The coordinator ruled merge-over-measured; I agree, and the
    /// alternative (omission disables) is argued against in `from_override_json`'s doc comment:
    /// a disable has to be its own explicit, warned key, not an absence.
    #[test]
    fn qa_an_override_that_omits_a_measured_family_keeps_its_measured_cap() {
        let (subset, notes) = SeatCaps::from_override_json(r#"{"glm-5.3-flash": 4}"#).unwrap();
        // The omitted families are still leasable, at their measured caps.
        assert_eq!(subset.cap("glm-5.3-flash"), Some(4));
        assert_eq!(subset.cap("deepseek-v4.1-flash"), Some(2));
        assert_eq!(subset.cap("qwen3.8-flash-next"), Some(4));
        // 4 is exactly the measured cap, so there is nothing to report about this override.
        assert!(notes.is_empty(), "{notes:?}");
        assert!(subset.exceedances_vs_measured().is_empty());

        let pool = Arc::new(LeasePool::new(subset));
        let lease = pool
            .acquire("deepseek-v4.1-flash", 2)
            .expect("an omitted family must still be leasable");
        assert_eq!(lease.seats(), 2);

        // And the comparability snapshot keeps all three rows, so a run recorded under this
        // override is comparable with one recorded under the measured table.
        drop(lease);
        assert_eq!(
            pool.fleet_busy_json(),
            r#"{"deepseek-v4.1-flash":0,"glm-5.3-flash":0,"qwen3.8-flash-next":0}"#
        );
        assert_eq!(
            pool.fleet_busy_json(),
            Arc::new(LeasePool::new(SeatCaps::measured())).fleet_busy_json(),
            "a partial override must not change the SHAPE of the snapshot"
        );

        // A partial override that really does change a cap still changes only that one, and is
        // still reported when it exceeds the measured value.
        let (raised, notes) = SeatCaps::from_override_json(r#"{"glm-5.3-flash": 8}"#).unwrap();
        assert_eq!(raised.cap("glm-5.3-flash"), Some(8));
        assert_eq!(raised.cap("deepseek-v4.1-flash"), Some(2));
        assert_eq!(
            notes,
            vec![CapNote::AboveMeasured {
                family: "glm-5.3-flash".to_string(),
                configured: 8,
                measured: 4,
            }]
        );
        // A cap LOWERED below the measured one is a legitimate, unremarkable override.
        let (lowered, notes) = SeatCaps::from_override_json(r#"{"glm-5.3-flash": 1}"#).unwrap();
        assert_eq!(lowered.cap("glm-5.3-flash"), Some(1));
        assert!(notes.is_empty(), "{notes:?}");
    }

    /// FIXED (was `qa_documents_a_case_typo_in_a_family_name_is_not_reported`, QA round 3). A key
    /// that differs from a measured family only in case is now reported.
    ///
    /// Matching stays exact and case-sensitive — lowering the key would be a guess about a name
    /// the fleet owns — so `{"GLM-5.3-Flash": 40}` still does not raise glm's cap. What changed
    /// is that it no longer does so silently: under merge the operator's intended family keeps
    /// its measured 4, a new family appears beside it, and the note says exactly that.
    #[test]
    fn qa_a_case_typo_in_a_family_name_is_reported() {
        let (typo, notes) = SeatCaps::from_override_json(r#"{"GLM-5.3-Flash": 40}"#).unwrap();
        assert_eq!(
            notes,
            vec![CapNote::CaseMismatch {
                configured: "GLM-5.3-Flash".to_string(),
                measured: "glm-5.3-flash".to_string(),
            }]
        );
        let shown = notes[0].to_string();
        assert!(shown.contains("differs only in case"), "{shown}");
        assert!(shown.contains("glm-5.3-flash"), "{shown}");
        assert!(shown.contains("GLM-5.3-Flash"), "{shown}");

        // The intended family is unharmed — which is the point of merging — and the typo'd one
        // exists beside it rather than replacing it.
        assert_eq!(typo.cap("glm-5.3-flash"), Some(4));
        assert_eq!(typo.cap("GLM-5.3-Flash"), Some(40));
        let pool = Arc::new(LeasePool::new(typo));
        assert!(
            pool.acquire("glm-5.3-flash", 4).is_ok(),
            "the real family must still be leasable at its measured cap"
        );

        // A family that matches nothing even ignoring case is a different note: it may be a
        // genuine new family on another fleet, so it is accepted and merely flagged.
        let (other, notes) = SeatCaps::from_override_json(r#"{"local-7b": 8}"#).unwrap();
        assert_eq!(
            notes,
            vec![CapNote::UnmeasuredFamily {
                family: "local-7b".to_string()
            }]
        );
        assert_eq!(other.cap("local-7b"), Some(8));
        assert_eq!(other.cap("glm-5.3-flash"), Some(4), "merge still applies");

        // And a zero entry is reported rather than quietly dropping or disabling a family.
        let (zeroed, notes) =
            SeatCaps::from_override_json(r#"{"glm-5.3-flash": 0, "deepseek-v4.1-flash": 1}"#)
                .unwrap();
        assert!(notes.contains(&CapNote::ZeroIgnored {
            family: "glm-5.3-flash".to_string(),
            // The measured cap rides along, so the message can only promise to keep one when
            // there IS one. QA found the unmeasured case claiming otherwise.
            measured: Some(4),
        }));
        assert!(notes[0].to_string().contains("does NOT disable"), "{:?}", notes[0]);
        assert_eq!(
            zeroed.cap("glm-5.3-flash"),
            Some(4),
            "a zero entry keeps the MEASURED cap; zero is not a disable switch"
        );
        assert_eq!(zeroed.cap("deepseek-v4.1-flash"), Some(1));
    }

    // ── the vacuity hole in `concurrent_families_do_not_bleed_into_each_other` ─────────────────

    /// The companion assertion that test is missing: the race must actually GRANT seats.
    ///
    /// `concurrent_families_do_not_bleed_into_each_other` checks only that no per-family peak
    /// exceeded its cap, and never that any seat was taken — so it passes against an `acquire`
    /// that refuses everything. QA measured exactly that: with `acquire` hard-wired to return
    /// `OverCap`, that test stays GREEN while both cap tests go red.
    ///
    /// Written as a separate test rather than an edit to Dev B's, so their shape is untouched.
    /// With the gate barrier holding every thread before any release, the peak is not merely
    /// positive — it is EXACTLY the cap, because there are more threads per family (10) than
    /// seats (4, 2, 4). Asserting equality is what makes this a non-vacuity guard rather than
    /// another way to pass.
    #[test]
    fn qa_the_cross_family_race_really_grants_every_seat() {
        const FAMILIES: [&str; 3] = ["glm-5.3-flash", "deepseek-v4.1-flash", "qwen3.8-flash-next"];
        const THREADS: usize = 30;
        let pool = LeasePool::with_measured_caps();
        let start = Arc::new(Barrier::new(THREADS));
        let gate = Arc::new(Barrier::new(THREADS));
        let peaks: Arc<[AtomicU32; 3]> =
            Arc::new([AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)]);
        let wins: Arc<[AtomicU32; 3]> =
            Arc::new([AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)]);

        let handles: Vec<_> = (0..THREADS)
            .map(|i| {
                let pool = Arc::clone(&pool);
                let start = Arc::clone(&start);
                let gate = Arc::clone(&gate);
                let peaks = Arc::clone(&peaks);
                let wins = Arc::clone(&wins);
                let slot = i % FAMILIES.len();
                std::thread::spawn(move || {
                    // Record only; nothing on the path to either barrier can panic.
                    start.wait();
                    let lease = pool.acquire(FAMILIES[slot], 1);
                    if lease.is_ok() {
                        wins[slot].fetch_add(1, Ordering::SeqCst);
                    }
                    peaks[slot].fetch_max(pool.busy_for(FAMILIES[slot]), Ordering::SeqCst);
                    gate.wait();
                    drop(lease);
                })
            })
            .collect();
        for h in handles {
            h.join().expect("no worker may panic");
        }

        for (slot, family) in FAMILIES.iter().enumerate() {
            let cap = pool.cap_for(family).expect("capped");
            let peak = peaks[slot].load(Ordering::SeqCst);
            let won = wins[slot].load(Ordering::SeqCst);
            assert!(peak <= cap, "{family} peaked at {peak} over a cap of {cap}");
            // THE non-vacuity half: every seat of every family really was handed out.
            assert_eq!(
                won, cap,
                "{family}: {won} of {cap} seats granted — 10 threads raced for them, so anything \
                 less means the race measured nothing"
            );
            assert_eq!(peak, cap, "{family}: peak {peak} should reach the cap {cap}");
        }
        assert_eq!(pool.busy_for("glm-5.3-flash"), 0);
    }
}

/// QA B round 3 — attacking the merge semantics, the note taxonomy, and the strong test shape.
#[cfg(test)]
mod qa_b_r3_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Barrier;

    fn notes(raw: &str) -> Vec<CapNote> {
        SeatCaps::from_override_json(raw).expect(raw).1
    }
    fn caps_of(raw: &str) -> SeatCaps {
        SeatCaps::from_override_json(raw).expect(raw).0
    }

    // ── the merge, and the cost that was ruled acceptable ─────────────────────────────────────

    /// A partial override MERGES over the measured table, and the stated cost is real: the two
    /// families the operator did not mention keep their measured caps and stay leasable.
    ///
    /// This is the round-2 finding's agreed resolution, pinned so the cost is visible in the
    /// suite rather than only in a commit message: a fleet that genuinely lacks `deepseek` will
    /// be told it has two seats there.
    #[test]
    fn qa_a_partial_override_merges_and_keeps_the_unmentioned_families() {
        let caps = caps_of(r#"{"glm-5.3-flash": 6}"#);
        assert_eq!(caps.cap("glm-5.3-flash"), Some(6), "the override wins where it speaks");
        assert_eq!(caps.cap("deepseek-v4.1-flash"), Some(2), "measured, kept — the stated cost");
        assert_eq!(caps.cap("qwen3.8-flash-next"), Some(4));
        assert_eq!(caps.families().count(), 3);

        // The round-2 finding is closed: the snapshot keeps all three rows.
        let pool = Arc::new(LeasePool::new(caps));
        assert_eq!(
            pool.fleet_busy_json(),
            r#"{"deepseek-v4.1-flash":0,"glm-5.3-flash":0,"qwen3.8-flash-next":0}"#
        );
        assert!(pool.acquire("deepseek-v4.1-flash", 2).is_ok());
    }

    /// The all-zero refusal runs on the override AS WRITTEN, before the merge.
    ///
    /// This is the ordering that matters: under merge semantics an all-zero override would merge
    /// to exactly the measured table and look like success, so the guard has to see the raw
    /// object. Checked through `from_override_json`, not only through `from_json`.
    #[test]
    fn qa_the_all_zero_refusal_runs_before_the_merge() {
        for raw in [
            r#"{"glm-5.3-flash":0,"deepseek-v4.1-flash":0,"qwen3.8-flash-next":0}"#,
            r#"{"glm-5.3-flash":0}"#,
            r#"{"local-7b":0}"#,
            "{}",
        ] {
            let err = SeatCaps::from_override_json(raw).expect_err(raw);
            assert!(
                err.contains("set to zero") || err == "seat caps object is empty",
                "{raw} -> {err}"
            );
        }
        // And it is not merely `from_json` being called first by accident: the merged result for
        // an all-zero override WOULD have been the measured table, i.e. indistinguishable from
        // success. This is what the ordering prevents.
        assert!(SeatCaps::from_override_json(
            r#"{"glm-5.3-flash":0,"deepseek-v4.1-flash":0,"qwen3.8-flash-next":0}"#
        )
        .is_err());
    }

    /// FIXED-AS-DIAGNOSTIC (was `qa_documents_zero_is_refused_alone_but_noted_in_company`, QA
    /// round 4). The two routes for a zero now state the SAME rule in the same words. The
    /// behaviours stay different, and the reason is recorded in `from_json`.
    ///
    /// QA's analysis was right about the merge path: an all-zero override merges to exactly the
    /// measured table, so on that path the refusal buys nothing. It is kept because `from_json`
    /// is a public constructor with its own contract — produce a usable caps table or fail — and
    /// has a caller shape that never merges, `LeasePool::new(SeatCaps::from_json(..)?)`. For that
    /// caller an all-zero table IS the empty-table state the guard exists to prevent: every
    /// request refused as `UnknownFamily`, whose message tells the operator to add a family they
    /// zeroed themselves. One of two callers cannot be the only one considered.
    ///
    /// So what was actually inconsistent — the diagnostic, where only one route taught the rule
    /// — is what changed. Both now say "zero does NOT disable a family" and both point at
    /// removing the key.
    #[test]
    fn qa_both_routes_for_a_zero_state_the_same_rule() {
        // Alone: still refused, and the error now teaches the rule.
        let err = SeatCaps::from_override_json(r#"{"glm-5.3-flash":0}"#)
            .expect_err("an all-zero override is refused; see from_json's doc comment for why");
        assert!(err.contains("set to zero"), "{err}");
        assert!(err.contains("remove the key to use the measured caps"), "{err}");
        assert!(err.contains("does NOT disable a family"), "{err}");
        assert!(err.contains("no disable switch"), "{err}");

        // In company: still accepted, glm at its MEASURED value, with a note that says the same.
        let (caps, ns) =
            SeatCaps::from_override_json(r#"{"glm-5.3-flash":0,"deepseek-v4.1-flash":2}"#)
                .expect("accepted once another key is present");
        assert_eq!(caps.cap("glm-5.3-flash"), Some(4), "zero did not disable it");
        let note = ns
            .iter()
            .find(|n| matches!(n, CapNote::ZeroIgnored { family, .. } if family == "glm-5.3-flash"))
            .expect("a ZeroIgnored note is emitted for it");
        let shown = note.to_string();
        assert!(shown.contains("does NOT disable"), "{shown}");
        assert!(shown.contains("no disable switch"), "{shown}");

        // THE point: an operator who hits either route is taught the same rule.
        for phrase in ["does NOT disable", "disable switch"] {
            assert!(err.contains(phrase), "refusal is missing {phrase:?}");
            assert!(shown.contains(phrase), "note is missing {phrase:?}");
        }
    }

    /// FIXED (was `qa_documents_zero_ignored_misinforms_for_an_unmeasured_family`, QA round 4).
    /// A zeroed key for a family nobody measured now gets a true note AND the note that was
    /// being suppressed.
    ///
    /// The zero branch used to `continue` before the unmeasured-family check, so such a key got
    /// no `UnmeasuredFamily` note at all — and `ZeroIgnored` claimed "the measured cap is kept"
    /// about a family with no measured cap. The single note the operator received was the one
    /// that misinformed them. The key diagnostics now run first, and `ZeroIgnored` carries the
    /// measured value so its wording cannot promise something that does not exist.
    #[test]
    fn qa_a_zeroed_unmeasured_family_gets_a_true_note_and_the_suppressed_one() {
        let (caps, ns) = SeatCaps::from_override_json(r#"{"local-7b":0,"glm-5.3-flash":4}"#)
            .expect("accepted");
        // Nothing is "kept": the family simply is not there.
        assert_eq!(caps.cap("local-7b"), None);

        // The note that was suppressed now fires.
        assert!(
            ns.iter().any(|n| matches!(n, CapNote::UnmeasuredFamily { family } if family == "local-7b")),
            "the UnmeasuredFamily note must no longer be swallowed by the zero branch: {ns:?}"
        );
        // And the zero note is TRUE for this family.
        let zero = ns
            .iter()
            .find(|n| matches!(n, CapNote::ZeroIgnored { family, .. } if family == "local-7b"))
            .expect("a ZeroIgnored note is emitted for it");
        assert_eq!(
            zero,
            &CapNote::ZeroIgnored {
                family: "local-7b".to_string(),
                measured: None
            }
        );
        let shown = zero.to_string();
        assert!(
            !shown.contains("measured cap of"),
            "it must not promise to keep a cap that does not exist: {shown}"
        );
        assert!(shown.contains("no measured cap either"), "{shown}");
        assert!(shown.contains("does nothing at all"), "{shown}");

        // The MEASURED case still says the true thing for itself, naming the number.
        let (_, ns) = SeatCaps::from_override_json(r#"{"glm-5.3-flash":0,"local-7b":8}"#).unwrap();
        let zero = ns
            .iter()
            .find(|n| matches!(n, CapNote::ZeroIgnored { family, .. } if family == "glm-5.3-flash"))
            .unwrap();
        assert!(zero.to_string().contains("measured cap of 4 is kept"), "{zero}");
    }

    /// The note taxonomy, over every shape QA could construct.
    #[test]
    fn qa_the_note_taxonomy_is_complete_for_every_override_shape() {
        // Equal to measured: nothing to say.
        assert!(notes(r#"{"glm-5.3-flash":4}"#).is_empty());
        // Below measured: the operator's explicit intent, nothing to say.
        assert!(notes(r#"{"glm-5.3-flash":1}"#).is_empty());
        // Above measured: one note, all three numbers.
        assert_eq!(
            notes(r#"{"glm-5.3-flash":40}"#),
            vec![CapNote::AboveMeasured {
                family: "glm-5.3-flash".to_string(),
                configured: 40,
                measured: 4
            }]
        );
        // A new family nobody measured: accepted, and said so.
        assert_eq!(
            notes(r#"{"local-7b":8,"glm-5.3-flash":4}"#),
            vec![CapNote::UnmeasuredFamily { family: "local-7b".to_string() }]
        );
        // A case typo: named as such, and the note says what actually happened.
        let case = notes(r#"{"GLM-5.3-Flash":40,"glm-5.3-flash":4}"#);
        assert!(case.contains(&CapNote::CaseMismatch {
            configured: "GLM-5.3-Flash".to_string(),
            measured: "glm-5.3-flash".to_string()
        }));
        let shown = case.iter().find(|n| matches!(n, CapNote::CaseMismatch { .. })).unwrap().to_string();
        assert!(shown.contains("did NOT change that family's cap"), "{shown}");
        // The case-variant key really is a FOURTH family, as the note says.
        let c = caps_of(r#"{"GLM-5.3-Flash":40,"glm-5.3-flash":4}"#);
        assert_eq!(c.cap("GLM-5.3-Flash"), Some(40));
        assert_eq!(c.cap("glm-5.3-flash"), Some(4));
        assert_eq!(c.families().count(), 4);
        // Every note's Display is non-empty and names its family.
        for raw in [
            r#"{"glm-5.3-flash":40}"#,
            r#"{"local-7b":8,"glm-5.3-flash":4}"#,
            r#"{"GLM-5.3-Flash":40,"glm-5.3-flash":4}"#,
            r#"{"glm-5.3-flash":0,"deepseek-v4.1-flash":2}"#,
        ] {
            for n in notes(raw) {
                let s = n.to_string();
                assert!(s.len() > 40, "a note must explain itself: {s}");
            }
        }
    }

    /// FIXED (was `qa_documents_a_whitespace_typo_gets_only_the_generic_note`, QA round 4). A key
    /// that differs from a measured family by surrounding whitespace now gets a note naming the
    /// family it was meant to be, the same quality a case typo gets.
    ///
    /// A leading space in hand-edited JSON is at least as likely as a capital, and it used to
    /// earn only "nobody measured this family" — with nothing pointing at the family the
    /// operator obviously meant. Its own variant rather than a `CaseMismatch` because the
    /// operator has to be told WHICH invisible character to delete.
    ///
    /// The key is still **not trimmed**: matching stays exact, so the note reports rather than
    /// silently accepting a different key than was written. Same fixture as QA's, inverted.
    #[test]
    fn qa_a_whitespace_typo_names_the_family_it_was_meant_to_be() {
        let (caps, ns) =
            SeatCaps::from_override_json(r#"{" glm-5.3-flash":40,"qwen3.8-flash-next":4}"#)
                .expect("accepted");
        // Unchanged, and deliberately so: the key is reported, not corrected.
        assert_eq!(caps.cap(" glm-5.3-flash"), Some(40), "a fourth family, with a space");
        assert_eq!(caps.cap("glm-5.3-flash"), Some(4), "the real one is untouched");

        assert_eq!(
            ns,
            vec![CapNote::WhitespaceMismatch {
                configured: " glm-5.3-flash".to_string(),
                measured: "glm-5.3-flash".to_string(),
                case_too: false,
            }],
            "the note must point at the family it was obviously meant to be"
        );
        let shown = ns[0].to_string();
        assert!(shown.contains("surrounding whitespace"), "{shown}");
        assert!(shown.contains("did NOT change that family's cap"), "{shown}");
        assert!(shown.contains("NOT trimmed"), "the note must say the key was not corrected: {shown}");

        // A trailing newline behaves the same way.
        assert_eq!(
            notes("{\"glm-5.3-flash\\n\":40,\"qwen3.8-flash-next\":4}"),
            vec![CapNote::WhitespaceMismatch {
                configured: "glm-5.3-flash\n".to_string(),
                measured: "glm-5.3-flash".to_string(),
                case_too: false,
            }]
        );
        // Whitespace AND case together is called out as both, so the operator fixes both.
        let both = notes(r#"{" GLM-5.3-Flash":40,"qwen3.8-flash-next":4}"#);
        assert_eq!(
            both,
            vec![CapNote::WhitespaceMismatch {
                configured: " GLM-5.3-Flash".to_string(),
                measured: "glm-5.3-flash".to_string(),
                case_too: true,
            }]
        );
        assert!(both[0].to_string().contains("whitespace and case"), "{}", both[0]);
        // A key that is near nothing still gets the generic note, unchanged.
        assert_eq!(
            notes(r#"{"local-7b":8,"glm-5.3-flash":4}"#),
            vec![CapNote::UnmeasuredFamily { family: "local-7b".to_string() }]
        );
    }

    // ── item 5: does the strong shape really rule out pooling? ───────────────────────────────

    /// The third failure shape QA could find that BOTH cap tests miss: more than one lease held
    /// per thread, at different widths, concurrently.
    ///
    /// Both existing tests take exactly one 1-seat lease per thread, so a `Drop` that decrements
    /// by the wrong amount when two leases of different sizes coexist on one family is invisible
    /// to them — single-threaded `acquire_then_release_restores_the_previous_count` is the only
    /// cover, and it is not concurrent. Here each thread holds a 1-seat AND a 2-seat lease on the
    /// same family, so the per-family total must be exact at every moment and zero at the end.
    #[test]
    fn qa_two_leases_of_different_widths_per_thread_stay_exact() {
        const THREADS: usize = 16;
        const ROUNDS: usize = 300;
        let pool = Arc::new(LeasePool::new(
            SeatCaps::from_json(r#"{"glm-5.3-flash": 9}"#).unwrap(),
        ));
        let start = Arc::new(Barrier::new(THREADS));
        let over = Arc::new(AtomicU32::new(0));
        let both = Arc::new(AtomicU32::new(0));

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let pool = Arc::clone(&pool);
                let start = Arc::clone(&start);
                let over = Arc::clone(&over);
                let both = Arc::clone(&both);
                std::thread::spawn(move || {
                    start.wait();
                    for _ in 0..ROUNDS {
                        // Record only; nothing here panics.
                        let a = pool.acquire("glm-5.3-flash", 1);
                        let b = pool.acquire("glm-5.3-flash", 2);
                        if a.is_ok() && b.is_ok() {
                            both.fetch_add(1, Ordering::SeqCst);
                        }
                        over.fetch_max(pool.busy_for("glm-5.3-flash"), Ordering::SeqCst);
                        // Dropped in the opposite order to acquisition, which is where a
                        // width-confused release would show up.
                        drop(a);
                        drop(b);
                        std::thread::yield_now();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("no worker may panic");
        }
        assert!(
            over.load(Ordering::SeqCst) <= 9,
            "seats in use reached {} against a cap of 9",
            over.load(Ordering::SeqCst)
        );
        // Non-vacuity: threads really did hold two leases at once, many times.
        assert!(
            both.load(Ordering::SeqCst) > THREADS as u32,
            "only {} rounds held both leases — the test measured nothing",
            both.load(Ordering::SeqCst)
        );
        assert_eq!(
            pool.busy_for("glm-5.3-flash"),
            0,
            "a release of the wrong width leaves the count non-zero"
        );
        // And the cap is still exactly enforceable afterwards.
        let all = pool.acquire("glm-5.3-flash", 9).expect("the whole cap must be free again");
        assert_eq!(all.seats(), 9);
        assert!(pool.acquire("glm-5.3-flash", 1).is_err());
    }

    /// Neither cap test can see starvation, and that is the honest limit of the shape.
    ///
    /// Both hold a barrier and request equal widths, so "who got the seats" is never asked. With
    /// no queue in the pool (documented), a wide request can be refused indefinitely while narrow
    /// ones succeed — pinned here as a property of the design rather than a bug, so that
    /// `won == cap` is not read as covering fairness.
    #[test]
    fn qa_documents_the_cap_tests_cannot_see_starvation() {
        let pool = LeasePool::with_measured_caps();
        let _narrow: Vec<_> = (0..4).map(|_| pool.acquire("glm-5.3-flash", 1).unwrap()).collect();
        // A 4-seat request can never be satisfied while four 1-seat holders cycle, and nothing
        // in the pool records that it was asked, let alone orders it first.
        for _ in 0..50 {
            assert!(matches!(
                pool.acquire("glm-5.3-flash", 4),
                Err(SeatError::OverCap { free: 0, .. })
            ));
        }
        assert_eq!(pool.busy_for("glm-5.3-flash"), 4);
    }
}

/// QA B round 4 — attacking `WhitespaceMismatch` with the invisible characters it is for.
#[cfg(test)]
mod qa_b_r4_tests {
    use super::*;

    /// Build the override JSON with serde, not `format!("{:?}")`.
    ///
    /// QA's first version used Rust's `Debug` for the key, which emits `\u{a0}` — valid Rust,
    /// invalid JSON — so every non-ASCII case failed to parse and the test was measuring the
    /// parser rather than the notes.
    fn note_for(key: &str) -> CapNote {
        let mut map: BTreeMap<String, u32> = BTreeMap::new();
        map.insert(key.to_string(), 40);
        map.insert("qwen3.8-flash-next".to_string(), 4);
        let raw = serde_json::to_string(&map).expect("serialise");
        let (_, ns) = SeatCaps::from_override_json(&raw).expect(&raw);
        ns.into_iter()
            .find(|n| !matches!(n, CapNote::AboveMeasured { .. }))
            .unwrap_or_else(|| panic!("no key note for {key:?}"))
    }

    /// Every whitespace character `str::trim` recognises gets the specific note, naming the
    /// family it was meant to be and saying the key was NOT trimmed.
    #[test]
    fn qa_every_trimmable_invisible_gets_the_specific_note() {
        for (label, key) in [
            ("leading space", " glm-5.3-flash"),
            ("trailing space", "glm-5.3-flash "),
            ("tab", "\tglm-5.3-flash"),
            ("trailing newline", "glm-5.3-flash\n"),
            ("CRLF", "glm-5.3-flash\r\n"),
            ("non-breaking space", "\u{00a0}glm-5.3-flash"),
            ("ideographic space", "\u{3000}glm-5.3-flash"),
            ("vertical tab", "\u{000b}glm-5.3-flash"),
            ("both sides", "  glm-5.3-flash  "),
        ] {
            match note_for(key) {
                CapNote::WhitespaceMismatch {
                    configured,
                    measured,
                    case_too,
                } => {
                    assert_eq!(configured, key, "{label}: the note must quote the key verbatim");
                    assert_eq!(measured, "glm-5.3-flash", "{label}");
                    assert!(!case_too, "{label}: only whitespace is wrong here");
                    let shown = CapNote::WhitespaceMismatch {
                        configured,
                        measured,
                        case_too,
                    }
                    .to_string();
                    assert!(shown.contains("whitespace"), "{label}: {shown}");
                    assert!(
                        shown.contains("glm-5.3-flash"),
                        "{label}: the note must name the intended family: {shown}"
                    );
                    assert!(
                        shown.to_lowercase().contains("not trimmed")
                            || shown.contains("matched exactly")
                            || shown.contains("NOT change"),
                        "{label}: the note must say the key was not corrected: {shown}"
                    );
                }
                other => panic!("{label} ({key:?}) got {other:?}"),
            }
        }
    }

    /// Whitespace AND case wrong together says so, rather than picking one.
    #[test]
    fn qa_whitespace_and_case_together_are_both_reported() {
        match note_for(" GLM-5.3-Flash ") {
            CapNote::WhitespaceMismatch {
                configured,
                measured,
                case_too,
            } => {
                assert_eq!(configured, " GLM-5.3-Flash ");
                assert_eq!(measured, "glm-5.3-flash");
                assert!(case_too, "both are wrong, and the note must carry that");
                let shown = CapNote::WhitespaceMismatch {
                    configured,
                    measured,
                    case_too,
                }
                .to_string();
                assert!(shown.contains("whitespace"), "{shown}");
                assert!(shown.to_lowercase().contains("case"), "{shown}");
            }
            other => panic!("got {other:?}"),
        }
        // Case alone stays a CaseMismatch, so the two variants are not interchangeable.
        assert!(matches!(note_for("GLM-5.3-Flash"), CapNote::CaseMismatch { .. }));
    }

    /// FIXED (was `qa_documents_a_zero_width_space_gets_only_the_generic_note`, QA round 5). A
    /// zero-width format character now gets a note that NAMES the character and the family the
    /// key was meant to be.
    ///
    /// QA's finding: `str::trim` recognises `char::is_whitespace`, and U+200B and friends are
    /// format characters (category Cf), so `"glm-5.3-flash\u{200b}".trim()` is itself, the
    /// near-miss check failed, and the most invisible of the invisible characters was the one
    /// that got no help at all — the opposite of the reason `WhitespaceMismatch` exists.
    ///
    /// Same fixture, inverted. The key is still **not** trimmed or rewritten, for the reason
    /// already documented on `near_miss`: matching stays exact, so the note tells the operator
    /// what to delete instead of silently accepting a different key than was written.
    #[test]
    fn qa_a_zero_width_character_names_itself_and_the_family_it_was_meant_to_be() {
        for (label, key, expected_char) in [
            ("zero-width space", "glm-5.3-flash\u{200b}", "U+200B ZERO WIDTH SPACE"),
            (
                "zero-width non-joiner",
                "\u{200c}glm-5.3-flash",
                "U+200C ZERO WIDTH NON-JOINER",
            ),
            (
                "BOM / zero-width no-break space",
                "\u{feff}glm-5.3-flash",
                "U+FEFF ZERO WIDTH NO-BREAK SPACE (BOM)",
            ),
            ("word joiner", "glm-5.3-flash\u{2060}", "U+2060 WORD JOINER"),
            ("zero-width joiner", "glm-\u{200d}5.3-flash", "U+200D ZERO WIDTH JOINER"),
        ] {
            // QA's observation, kept: trim really cannot see these.
            assert_eq!(key.trim(), key, "{label}: trim leaves it");
            match note_for(key) {
                CapNote::InvisibleCharacter {
                    configured,
                    measured,
                    characters,
                    case_too,
                } => {
                    assert_eq!(configured, key, "{label}");
                    assert_eq!(measured, "glm-5.3-flash", "{label}");
                    assert_eq!(characters, expected_char, "{label}");
                    assert!(!case_too, "{label}");
                }
                other => panic!("{label}: expected InvisibleCharacter, got {other:?}"),
            }
            // The note has to name the character, or it is not actionable: nobody finds a
            // U+200B by looking at it.
            let shown = note_for(key).to_string();
            assert!(shown.contains(expected_char), "{label}: {shown}");
            assert!(shown.contains("invisible in an editor"), "{label}: {shown}");
            assert!(shown.contains("NOT trimmed"), "{label}: {shown}");
        }

        // Several at once are all named, and a case difference is called out as well.
        match note_for("\u{feff}GLM-5.3-Flash\u{200b}") {
            CapNote::InvisibleCharacter {
                characters,
                case_too,
                ..
            } => {
                assert!(characters.contains("U+200B"), "{characters}");
                assert!(characters.contains("U+FEFF"), "{characters}");
                assert!(case_too, "the case difference must be reported too");
            }
            other => panic!("expected InvisibleCharacter, got {other:?}"),
        }
        // Whitespace AND a zero-width character: reported as the invisible one, because that is
        // the half the operator cannot see.
        assert!(matches!(
            note_for(" glm-5.3-flash\u{200b}"),
            CapNote::InvisibleCharacter { .. }
        ));
        // A key near nothing still gets the generic note.
        assert!(matches!(
            note_for("local-7b"),
            CapNote::UnmeasuredFamily { .. }
        ));

        // QA's second half, unchanged: the real family keeps its measured cap and the odd key is
        // a family of its own — the note describes what happened, it does not change it.
        let mut map: BTreeMap<String, u32> = BTreeMap::new();
        map.insert("glm-5.3-flash\u{200b}".to_string(), 40);
        map.insert("qwen3.8-flash-next".to_string(), 4);
        let caps = SeatCaps::from_override_json(&serde_json::to_string(&map).unwrap())
            .unwrap()
            .0;
        assert_eq!(caps.cap("glm-5.3-flash"), Some(4), "the intended family is untouched");
        assert_eq!(caps.cap("glm-5.3-flash\u{200b}"), Some(40), "a fourth family, invisibly");
    }

    /// A whitespace-ONLY key trims to nothing, matches no family, and is reported generically.
    #[test]
    fn qa_a_whitespace_only_key_is_a_family_of_its_own() {
        for key in [" ", "\t", "   ", "\n"] {
            match note_for(key) {
                CapNote::UnmeasuredFamily { family } => assert_eq!(family, key),
                other => panic!("{key:?} got {other:?}"),
            }
        }
        // It really becomes a family, with a cap, which nothing can ever ask for by name.
        let caps = SeatCaps::from_override_json(r#"{" ": 3, "glm-5.3-flash": 4}"#).unwrap().0;
        assert_eq!(caps.cap(" "), Some(3));
        assert_eq!(caps.families().count(), 4);
    }

    /// The zero-state symmetry must need BOTH routes' wording, not one.
    ///
    /// Dev B's `qa_both_routes_for_a_zero_state_the_same_rule` ends in a loop that checks each
    /// phrase against each route, so dropping the wording from either side fails it. Asserted
    /// here independently on the two messages themselves, so the symmetry is pinned by a second
    /// test that does not share the first's structure.
    #[test]
    fn qa_the_zero_rule_is_taught_identically_by_both_routes() {
        let refusal = SeatCaps::from_override_json(r#"{"glm-5.3-flash":0}"#)
            .expect_err("all-zero is refused");
        let note = SeatCaps::from_override_json(r#"{"glm-5.3-flash":0,"deepseek-v4.1-flash":2}"#)
            .expect("accepted in company")
            .1
            .into_iter()
            .find(|n| matches!(n, CapNote::ZeroIgnored { .. }))
            .expect("a ZeroIgnored note")
            .to_string();

        for phrase in ["does NOT disable", "disable switch"] {
            assert!(refusal.contains(phrase), "the refusal is missing {phrase:?}: {refusal}");
            assert!(note.contains(phrase), "the note is missing {phrase:?}: {note}");
        }
        // And the two are genuinely different messages otherwise, so this is a symmetry of the
        // RULE rather than one string used twice.
        assert_ne!(refusal, note);
    }

    /// `ZeroIgnored` can no longer claim a measured cap that does not exist — the round-3 finding.
    #[test]
    fn qa_zero_ignored_tells_the_truth_for_both_measured_and_unmeasured() {
        let (_, ns) = SeatCaps::from_override_json(r#"{"local-7b":0,"glm-5.3-flash":0,"qwen3.8-flash-next":4}"#)
            .expect("accepted");
        let for_family = |f: &str| {
            ns.iter()
                .find(|n| matches!(n, CapNote::ZeroIgnored { family, .. } if family == f))
                .unwrap_or_else(|| panic!("no ZeroIgnored for {f}"))
                .to_string()
        };
        // Measured: it is honest to say the cap is kept, and the message names the NUMBER —
        // better than QA assumed, so the assertion pins the real wording.
        let measured = for_family("glm-5.3-flash");
        assert!(
            measured.contains("its measured cap of 4 is kept"),
            "the measured case must name the cap it keeps: {measured}"
        );
        // Unmeasured: it must not claim any kept cap at all.
        let unmeasured = for_family("local-7b");
        assert!(
            !unmeasured.contains("is kept"),
            "an unmeasured family has no measured cap to keep: {unmeasured}"
        );
        assert!(!unmeasured.contains("measured cap of"), "{unmeasured}");
        // And the unmeasured, zeroed key now ALSO gets the note that is true about it, which the
        // round-3 version suppressed.
        assert!(
            ns.iter().any(|n| matches!(n, CapNote::UnmeasuredFamily { family } if family == "local-7b")),
            "the key diagnostics must run before the zero check: {ns:?}"
        );
    }
}
