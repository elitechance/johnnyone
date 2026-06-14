-- Planner path defaults (workspace-relative). Empty value in settings still
-- falls back to these in code when the key is missing.
INSERT OR IGNORE INTO settings (key, value) VALUES ('planner_methodology_path', 'lokal/agents/common/methodology.md');
INSERT OR IGNORE INTO settings (key, value) VALUES ('planner_conventions_path', 'lokal/agents/common/conventions');
INSERT OR IGNORE INTO settings (key, value) VALUES ('web_client_url', 'https://johnnyone.pages.dev/');

-- Upgrade dev installs that still have the original localhost worker default.
UPDATE settings
SET value = 'https://johnnyone-hub.ethan-353.workers.dev'
WHERE key = 'worker_url' AND value IN ('http://localhost:7714', 'https://johnnyone-dev-hub.ethan-353.workers.dev');