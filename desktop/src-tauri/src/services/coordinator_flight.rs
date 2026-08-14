//! Per-plan single-flight for `coordinator_loop`.
//!
//! At most one coordinator task runs per plan id. A second `spawn_coordinator_loop`
//! for a live plan is a no-op (Amendment 1 §C). The slot is released when the
//! spawned task returns *or* panics, so stop/done/error/unwind can re-enter.

use std::collections::HashSet;
use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

pub type CoordinatorSlots = Arc<Mutex<HashSet<String>>>;

/// Insert `plan_id`. Returns `true` if this caller now owns the slot.
pub fn try_claim(slots: &CoordinatorSlots, plan_id: &str) -> bool {
    match slots.lock() {
        Ok(mut set) => set.insert(plan_id.to_string()),
        Err(poisoned) => poisoned.into_inner().insert(plan_id.to_string()),
    }
}

/// Drop guard: removes `plan_id` from the set when the spawned task ends.
pub struct SlotGuard {
    slots: CoordinatorSlots,
    plan_id: String,
}

impl SlotGuard {
    pub fn new(slots: CoordinatorSlots, plan_id: String) -> Self {
        Self { slots, plan_id }
    }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        let mut set = match self.slots.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        set.remove(&self.plan_id);
    }
}

/// Claim the slot and spawn `task`. Returns `None` if a loop is already live
/// for `plan_id` (no second join handle is created).
pub fn spawn_if_idle<F, Fut>(
    slots: &CoordinatorSlots,
    plan_id: String,
    task: F,
) -> Option<JoinHandle<()>>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    if !try_claim(slots, &plan_id) {
        tracing::info!(
            plan_id = %plan_id,
            "coordinator already running; skipping spawn"
        );
        return None;
    }
    let slots = slots.clone();
    Some(tokio::spawn(async move {
        let _guard = SlotGuard::new(slots, plan_id);
        task().await;
    }))
}

#[cfg(test)]
mod single_flight {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn single_flight_second_spawn_does_not_start_second_loop() {
        let slots: CoordinatorSlots = Arc::new(Mutex::new(HashSet::new()));
        let started = Arc::new(AtomicUsize::new(0));
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();

        let started_first = started.clone();
        let first = spawn_if_idle(&slots, "plan-a".to_string(), move || {
            let started = started_first;
            async move {
                started.fetch_add(1, Ordering::SeqCst);
                let _ = release_rx.await;
            }
        });
        assert!(first.is_some(), "first spawn must create a join handle");

        tokio::time::timeout(Duration::from_secs(1), async {
            while started.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first loop should start");

        let started_second = started.clone();
        let second = spawn_if_idle(&slots, "plan-a".to_string(), move || {
            let started = started_second;
            async move {
                started.fetch_add(1, Ordering::SeqCst);
            }
        });
        assert!(
            second.is_none(),
            "second spawn for a live plan must not create a join handle"
        );
        assert_eq!(
            started.load(Ordering::SeqCst),
            1,
            "second spawn must not increment the loop counter"
        );

        let _ = release_tx.send(());
        first.unwrap().await.unwrap();
    }

    #[tokio::test]
    async fn single_flight_different_plan_ids_run_concurrently() {
        let slots: CoordinatorSlots = Arc::new(Mutex::new(HashSet::new()));
        let started = Arc::new(AtomicUsize::new(0));
        let (gate_a, wait_a) = tokio::sync::oneshot::channel::<()>();
        let (gate_b, wait_b) = tokio::sync::oneshot::channel::<()>();

        let started_a = started.clone();
        let handle_a = spawn_if_idle(&slots, "plan-a".to_string(), move || {
            let started = started_a;
            async move {
                started.fetch_add(1, Ordering::SeqCst);
                let _ = wait_a.await;
            }
        });
        let started_b = started.clone();
        let handle_b = spawn_if_idle(&slots, "plan-b".to_string(), move || {
            let started = started_b;
            async move {
                started.fetch_add(1, Ordering::SeqCst);
                let _ = wait_b.await;
            }
        });
        assert!(handle_a.is_some() && handle_b.is_some());

        tokio::time::timeout(Duration::from_secs(1), async {
            while started.load(Ordering::SeqCst) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both plan ids must get a live loop");

        assert_eq!(slots.lock().unwrap().len(), 2);

        let _ = gate_a.send(());
        let _ = gate_b.send(());
        handle_a.unwrap().await.unwrap();
        handle_b.unwrap().await.unwrap();
    }

    #[tokio::test]
    async fn single_flight_slot_released_after_loop_exits() {
        let slots: CoordinatorSlots = Arc::new(Mutex::new(HashSet::new()));
        let first = spawn_if_idle(&slots, "plan-a".to_string(), || async {});
        first.expect("first spawn").await.unwrap();
        assert!(
            slots.lock().unwrap().is_empty(),
            "slot must be released when the loop returns"
        );

        let started = Arc::new(AtomicUsize::new(0));
        let started_again = started.clone();
        let second = spawn_if_idle(&slots, "plan-a".to_string(), move || {
            let started = started_again;
            async move {
                started.fetch_add(1, Ordering::SeqCst);
            }
        });
        assert!(
            second.is_some(),
            "a later spawn for the same id must be allowed after release"
        );
        second.unwrap().await.unwrap();
        assert_eq!(started.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn single_flight_app_state_set_is_the_guard() {
        let (state, _root) = crate::test_support::test_state();
        let started = Arc::new(AtomicUsize::new(0));
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let started_first = started.clone();
        let first = spawn_if_idle(
            &state.coordinator_loops,
            "plan-app".to_string(),
            move || {
                let started = started_first;
                async move {
                    started.fetch_add(1, Ordering::SeqCst);
                    let _ = release_rx.await;
                }
            },
        );
        assert!(first.is_some());
        tokio::time::timeout(Duration::from_secs(1), async {
            while started.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            spawn_if_idle(&state.coordinator_loops, "plan-app".to_string(), || async {})
                .is_none()
        );
        let _ = release_tx.send(());
        first.unwrap().await.unwrap();
    }
}
