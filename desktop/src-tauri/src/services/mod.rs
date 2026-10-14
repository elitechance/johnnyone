pub mod agent_plans;
pub mod atomic_fs;
pub mod chat_host;
pub mod coordinator_flight;
pub mod git_history;
pub mod host_files;
pub mod leases;
pub mod plan_artifacts;
pub mod plan_check;
pub mod planner_prompts;
pub mod prompt_library;
pub mod providers;
pub mod relay;
pub mod sessions;
pub mod settings;
pub mod task_check;
pub mod task_loop;
pub mod task_replan;
pub mod task_spec;
pub mod task_state;
pub mod verify_policy;
pub mod workflows;
pub mod workspace_git;
#[cfg(test)]
mod fixture_spine;
/// QA A's adversarial tests against the workflow arena (migration 026).
#[cfg(test)]
mod workflows_qa_attacks;
/// QA A round 2: the fixes for round 1, and the surfaces the fixes added.
#[cfg(test)]
mod workflows_qa_attacks_r2;
/// QA A round 3: the executor, the identity key, the SQL guard, the cross-branch contract.
#[cfg(test)]
mod workflows_qa_attacks_r3;
/// QA A round 4: the final verification pass.
#[cfg(test)]
mod workflows_qa_attacks_r4;
/// QA A round 5: the fourth audit of the invisibility rule.
#[cfg(test)]
mod workflows_qa_attacks_r5;
