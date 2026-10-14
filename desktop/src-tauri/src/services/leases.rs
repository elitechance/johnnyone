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
    pub fn from_json(raw: &str) -> Result<Self, String> {
        let parsed: BTreeMap<String, u32> = serde_json::from_str(raw.trim())
            .map_err(|e| format!("seat caps must be a JSON object of family -> seats: {}", e))?;
        if parsed.is_empty() {
            return Err("seat caps object is empty".to_string());
        }
        Ok(Self::from_pairs(parsed))
    }

    /// This family's cap, or `None` when the family is not in the table.
    pub fn cap(&self, family: &str) -> Option<u32> {
        self.caps.get(family).copied()
    }

    /// Every capped family, ascending. Stable order so a `fleet_busy` snapshot is comparable.
    pub fn families(&self) -> impl Iterator<Item = &str> {
        self.caps.keys().map(String::as_str)
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
    match SeatCaps::from_json(&raw) {
        Ok(caps) => caps,
        Err(reason) => {
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
        // A zero cap is "cannot be leased", which is the same as absent.
        assert_eq!(SeatCaps::from_json(r#"{"x": 0}"#).unwrap().cap("x"), None);
        // Bad configuration is an error, not a silent fallback to different numbers.
        assert!(SeatCaps::from_json("not json").is_err());
        assert!(SeatCaps::from_json("{}").is_err());
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
    #[test]
    fn concurrent_families_do_not_bleed_into_each_other() {
        let pool = LeasePool::with_measured_caps();
        let families = ["glm-5.3-flash", "deepseek-v4.1-flash", "qwen3.8-flash-next"];
        let start = Arc::new(Barrier::new(30));
        let gate = Arc::new(Barrier::new(30));
        let handles: Vec<_> = (0..30)
            .map(|i| {
                let pool = Arc::clone(&pool);
                let start = Arc::clone(&start);
                let gate = Arc::clone(&gate);
                let family = families[i % families.len()];
                std::thread::spawn(move || {
                    start.wait();
                    let lease = pool.acquire(family, 1);
                    if lease.is_ok() {
                        let cap = pool.cap_for(family).unwrap();
                        assert!(
                            pool.busy_for(family) <= cap,
                            "{family} went over its cap of {cap}"
                        );
                    }
                    gate.wait();
                    drop(lease);
                })
            })
            .collect();
        for h in handles {
            h.join().expect("no worker may panic");
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
