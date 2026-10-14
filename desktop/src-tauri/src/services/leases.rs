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
//! environment — a Hop tuned around one model's 262k context does not transfer to another's 131k.
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
//! One, today: the arena's run starter. It will
//! [`acquire`](LeasePool::acquire) `fan_out` seats on the workflow's family, snapshot
//! [`fleet_busy_json`](LeasePool::fleet_busy_json) and [`SeatLease::seats_held_json`] into the
//! `workflow_runs` row's `fleet_busy` / `seats_held` columns at start, and hold the lease for the
//! life of the run. Those two columns are what makes a comparison between two runs honest — a
//! preference recorded across mismatched seat budgets is measuring scheduling luck. There is
//! deliberately **no GraphQL surface here**: the arena API is owned elsewhere, and this is an
//! in-process API for it to call.
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
pub const MEASURED_SEAT_CAPS: &[(&str, u32)] = &[
    // 262K · reasoning · executes the phase
    ("glm-5.3-flash", 4),
    // 262K · proposes the next prompt
    ("deepseek-v4.1-flash", 2),
    // 131K · the busiest model — left alone by the arena, but counted
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
        let parsed: BTreeMap<String, u32> = serde_json::from_str(raw.trim())
            .map_err(|e| format!("seat caps must be a JSON object of family -> seats: {}", e))?;
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
            return Err(format!(
                "every family in the seat caps object is set to zero ({}), so no family could be \
                 leased at all; remove the key to use the measured caps instead",
                zeroed_list(&zeroed)
            ));
        }
        Ok(caps)
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
    /// `(family, configured, measured)`, ascending.
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
    match SeatCaps::from_json(&raw) {
        Ok(caps) => {
            // An override ABOVE the measured cap is allowed — a different fleet needs a
            // different number — but it is never silent. One line per family, naming all three
            // numbers, so "we are running at 40 on a fleet measured at 4" is readable in a log
            // rather than being inferred from latency months later.
            for (family, configured, measured) in caps.exceedances_vs_measured() {
                tracing::warn!(
                    key = SEAT_CAPS_SETTING_KEY,
                    %family,
                    configured,
                    measured,
                    "configured seat cap is ABOVE the measured capacity for this family; the \
                     fleet reports no error when over-subscribed, so this will show up only as \
                     latency"
                );
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
    /// The first version of this test asserted the cap INSIDE the worker thread, before
    /// `gate.wait()`. When the assertion tripped, that thread unwound without ever reaching the
    /// barrier, the other 29 blocked on `Barrier::new(30)` forever and the main thread blocked
    /// in `join()` — so the test HUNG instead of failing. QA measured 5 timeouts in 6 runs
    /// against a deliberately raced `acquire`, one of them sitting for 10m34s. A test that
    /// cannot report the failure it detects is worse than no test: in CI it is a job timeout,
    /// not a red test, and the defect reads as infrastructure flake.
    ///
    /// So the workers only ever RECORD — `fetch_max` into a per-family peak — and every
    /// assertion happens after `join`. There is no `unwrap`, no `assert` and no indexing panic
    /// on the path to either barrier.
    #[test]
    fn concurrent_families_do_not_bleed_into_each_other() {
        const FAMILIES: [&str; 3] = ["glm-5.3-flash", "deepseek-v4.1-flash", "qwen3.8-flash-next"];
        const THREADS: usize = 30;
        let pool = LeasePool::with_measured_caps();
        let start = Arc::new(Barrier::new(THREADS));
        let gate = Arc::new(Barrier::new(THREADS));
        // Peak seats observed in use per family, by the same index as FAMILIES.
        let peaks: Arc<[AtomicU32; 3]> =
            Arc::new([AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)]);

        let handles: Vec<_> = (0..THREADS)
            .map(|i| {
                let pool = Arc::clone(&pool);
                let start = Arc::clone(&start);
                let gate = Arc::clone(&gate);
                let peaks = Arc::clone(&peaks);
                let slot = i % FAMILIES.len();
                let family = FAMILIES[slot];
                std::thread::spawn(move || {
                    start.wait();
                    let lease = pool.acquire(family, 1);
                    // Record only. Observe the count whether or not this thread won, so an
                    // over-admission caused by someone else is still seen.
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
            assert!(
                peak <= cap,
                "{family} went over its cap of {cap}: peak {peak} seats in use"
            );
        }
        assert_eq!(
            pool.busy(),
            BTreeMap::from([
                ("deepseek-v4.1-flash".to_string(), 0),
                ("glm-5.3-flash".to_string(), 0),
                ("qwen3.8-flash-next".to_string(), 0),
            ])
        );
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

    /// FINDING. The diagnostic is one-directional: a cap ABOVE the measured one is reported, and
    /// a measured family simply OMITTED is not reported at all.
    ///
    /// Omitting a key has the same effect as zeroing it — that family cannot be leased — and the
    /// all-zero refusal only fires when EVERY key is zero. So `{"glm-5.3-flash": 4}` silently
    /// takes two of the three measured families out of service, which is two thirds of the harm
    /// the all-zero guard was added to prevent, with no error and no warning. It also silently
    /// shrinks `fleet_busy`, the column whose whole job is making two runs comparable: a run
    /// recorded under this override has no row for the families it could not use, which is the
    /// "older snapshot that did not record that family" ambiguity the `busy()` doc comment says
    /// it is avoiding.
    ///
    /// What a reviewer should conclude: the override is all-or-nothing by family and nothing says
    /// so. Either a partial override should merge over the measured table, or omission should get
    /// the same one-line warning an excess gets. Both are the author's call.
    #[test]
    fn qa_documents_an_override_that_omits_a_measured_family_is_silent() {
        let subset = SeatCaps::from_json(r#"{"glm-5.3-flash": 4}"#).unwrap();
        // Accepted, with no exceedance to report...
        assert!(subset.exceedances_vs_measured().is_empty());
        // ...and two measured families are now unleasable.
        assert_eq!(subset.cap("deepseek-v4.1-flash"), None);
        assert_eq!(subset.cap("qwen3.8-flash-next"), None);
        let pool = Arc::new(LeasePool::new(subset));
        assert_eq!(
            pool.acquire("deepseek-v4.1-flash", 1).unwrap_err(),
            SeatError::UnknownFamily {
                family: "deepseek-v4.1-flash".to_string()
            },
            "an omitted family is indistinguishable from one nobody measured"
        );
        // And the comparability snapshot silently loses those rows.
        assert_eq!(pool.fleet_busy_json(), r#"{"glm-5.3-flash":0}"#);
        assert_eq!(
            Arc::new(LeasePool::new(SeatCaps::measured())).fleet_busy_json(),
            r#"{"deepseek-v4.1-flash":0,"glm-5.3-flash":0,"qwen3.8-flash-next":0}"#,
            "the measured table records all three, so the difference is real"
        );
    }

    /// FINDING (minor). Family matching is exact and case-sensitive, so a case typo is not an
    /// over-subscription warning — it is a silently disabled family.
    #[test]
    fn qa_documents_a_case_typo_in_a_family_name_is_not_reported() {
        let typo = SeatCaps::from_json(r#"{"GLM-5.3-Flash": 40}"#).unwrap();
        // 40 on a 4-seat fleet, and `exceedances_vs_measured` has nothing to say, because the
        // key does not match the measured family it was obviously meant to be.
        assert!(typo.exceedances_vs_measured().is_empty());
        assert_eq!(typo.cap("GLM-5.3-Flash"), Some(40));
        assert_eq!(typo.cap("glm-5.3-flash"), None);
        let pool = Arc::new(LeasePool::new(typo));
        assert!(matches!(
            pool.acquire("glm-5.3-flash", 1),
            Err(SeatError::UnknownFamily { .. })
        ));
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
