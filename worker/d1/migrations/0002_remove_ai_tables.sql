-- Remove AI tables that are now managed locally on desktop
-- These tables are no longer needed since the desktop app owns all AI data

DROP TABLE IF EXISTS ai_usage_log;
DROP TABLE IF EXISTS ai_tool_executions;
DROP TABLE IF EXISTS ai_tool_definitions;
DROP TABLE IF EXISTS ai_messages;
DROP TABLE IF EXISTS ai_sessions;
DROP TABLE IF EXISTS ai_provider_configs;
