-- 015_add_reviewer_setup_commands.sql
-- For a "shell" reviewer provider: commands run in each spawned reviewer shell
-- on first launch. Read when the (lazily-created) reviewer session spawns.
ALTER TABLE agent_plans ADD COLUMN reviewer_setup_commands TEXT;
