# small-mode-usable

Workspace-only fixture for phase 06. The fake planner (see
`tests/small_mode_acceptance.rs`) writes `task.yml` at test time.

There is **no** plan store and **no** `task.yml` in this directory.
The workspace crate is reused from `../small-mode-spine/workspace` at
runtime — do not check in a plan here.
