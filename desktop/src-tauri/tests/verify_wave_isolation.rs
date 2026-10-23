//! THE SPECIFICATION for one invariant:
//!
//!   **No two verifies may run at the same time in the same directory.**
//!
//! Why it exists. `plan_check` runs verifies four at a time (`run_waves(…, 4,
//! …)`, and `execute_budget_ms`/`warm_budget_ms` both size the budget as
//! `n.div_ceil(4)`), and it does not consult `depends_on`. The kart visual
//! identity plan has eight tasks whose verify is `npx ng build` in
//! `frontend/kart`. Four of them run concurrently, and an Angular build wipes
//! and rewrites `www/` — so the builds delete each other's output mid-flight.
//!
//! Reproduced by the T1 planner before this file existed: four concurrent
//! builds, three exited 0 and one died with `ENOENT … unlink
//! www/svg/subway.svg`, exit 127.
//!
//! The damage runs both ways, which is why this is not merely flaky: a correct
//! task can fail, and a build that LOST the race can leave output that the next
//! task's verify reads as a pass.
//!
//! This file is the gate, not application code: it is outside the run's edit
//! scope, and changing it to make a run pass is a failed task.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use johnnyone_desktop_lib::services::plan_check::run_waves;

/// A job is just its working directory for the purposes of this invariant.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Job {
    id: usize,
    cwd: String,
}

/// Runs `jobs` through `run_waves` with a worker that holds its slot open long
/// enough for an overlap to be observable, and records the high-water mark of
/// concurrent workers PER DIRECTORY.
fn peak_concurrency_per_dir(jobs: &[Job], width: usize) -> HashMap<String, usize> {
    let live: Arc<Mutex<HashMap<String, Arc<AtomicUsize>>>> = Arc::new(Mutex::new(HashMap::new()));
    let peak: Arc<Mutex<HashMap<String, usize>>> = Arc::new(Mutex::new(HashMap::new()));

    let live_c = Arc::clone(&live);
    let peak_c = Arc::clone(&peak);

    let _ = run_waves(
        jobs,
        width,
        600_000,
        |job: &Job| job.cwd.clone(),
        move |job: &Job, started: bool| {
            if !started {
                return Default::default();
            }
            let counter = {
                let mut map = live_c.lock().unwrap();
                Arc::clone(
                    map.entry(job.cwd.clone())
                        .or_insert_with(|| Arc::new(AtomicUsize::new(0))),
                )
            };
            let now = counter.fetch_add(1, Ordering::SeqCst) + 1;
            {
                let mut p = peak_c.lock().unwrap();
                let slot = p.entry(job.cwd.clone()).or_insert(0);
                if now > *slot {
                    *slot = now;
                }
            }
            // Wide enough that a genuine overlap cannot be missed.
            std::thread::sleep(std::time::Duration::from_millis(60));
            counter.fetch_sub(1, Ordering::SeqCst);
            Default::default()
        },
    );

    let peak = peak.lock().unwrap().clone();
    peak
}

#[test]
fn w1_eight_builds_in_one_directory_never_overlap() {
    // The kart shape, exactly: eight `npx ng build` tasks in frontend/kart.
    let jobs: Vec<Job> = (0..8)
        .map(|id| Job {
            id,
            cwd: "frontend/kart".to_string(),
        })
        .collect();

    let peak = peak_concurrency_per_dir(&jobs, 4);
    assert_eq!(
        peak.get("frontend/kart").copied(),
        Some(1),
        "two verifies ran at once in frontend/kart — an Angular build wipes www/, \
         so they delete each other's output"
    );
}

#[test]
fn w2_distinct_directories_still_use_the_full_width() {
    // The invariant must not cost throughput where there is no conflict.
    let jobs: Vec<Job> = (0..8)
        .map(|id| Job {
            id,
            cwd: format!("app-{id}"),
        })
        .collect();

    let started = std::time::Instant::now();
    let peak = peak_concurrency_per_dir(&jobs, 4);
    let elapsed = started.elapsed();

    for (dir, n) in &peak {
        assert_eq!(*n, 1, "{dir} somehow saw {n} at once with one job each");
    }
    assert!(
        elapsed < std::time::Duration::from_millis(400),
        "eight independent 60ms jobs at width 4 took {elapsed:?} — they were \
         serialised although no two share a directory"
    );
}

#[test]
fn w3_every_job_runs_exactly_once() {
    let mut jobs: Vec<Job> = (0..5)
        .map(|id| Job {
            id,
            cwd: "same".to_string(),
        })
        .collect();
    jobs.push(Job {
        id: 99,
        cwd: "other".to_string(),
    });

    let seen: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_c = Arc::clone(&seen);
    let _ = run_waves(
        &jobs,
        4,
        600_000,
        |job: &Job| job.cwd.clone(),
        move |job: &Job, started: bool| {
            if started {
                seen_c.lock().unwrap().push(job.id);
            }
            Default::default()
        },
    );

    let mut ids = seen.lock().unwrap().clone();
    ids.sort_unstable();
    assert_eq!(
        ids,
        vec![0, 1, 2, 3, 4, 99],
        "packing by directory must not drop or duplicate a job"
    );
}

#[test]
fn w4_the_budget_is_sized_for_the_waves_actually_needed() {
    use johnnyone_desktop_lib::services::plan_check::{execute_budget_ms, wave_count};

    // Eight jobs in one directory cannot be four-wide: they are eight waves.
    let same = vec!["frontend/kart".to_string(); 8];
    assert_eq!(wave_count(&same, 4), 8);

    // Eight jobs in eight directories are two waves, as before.
    let distinct: Vec<String> = (0..8).map(|i| format!("app-{i}")).collect();
    assert_eq!(wave_count(&distinct, 4), 2);

    // The budget must follow the real wave count, or the serialised tail is
    // cut off as `never_started` and reported as if it had never been eligible.
    assert!(
        execute_budget_ms(&same) > execute_budget_ms(&distinct),
        "eight serialised builds were given the same budget as two parallel waves"
    );
}
