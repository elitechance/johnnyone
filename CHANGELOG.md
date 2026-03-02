# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- Nx monorepo scaffold with 7 project areas (163 files)
- Worker: D1 schema with 8 AI tables (ai_sessions, ai_messages, ai_tool_definitions, ai_tool_executions, ai_provider_configs, desktop_nodes, channel_bindings, ai_usage_log) + auth tables
- Worker: Full GraphQL schema -- 6 queries, 11 mutations, 4 subscriptions
- Worker: 21 resolver files covering session CRUD, message handling, tool approval, provider config, desktop node management
- Worker: LLM provider abstraction with Claude (Anthropic Messages API), OpenAI (Chat Completions), and Ollama implementations
- Worker: AgentSessionDO Durable Object -- agent loop, context assembly, tool dispatch, stream relay
- Worker: Tool schema with 7 built-in tools (shell_execute, file_read, file_write, file_list, file_search, process_list, system_info)
- Worker: Channel adapter placeholders for Telegram, Discord, WhatsApp
- Desktop: Ionic/Angular frontend with chat, sessions, tools, and settings pages
- Desktop: Tauri 2 Rust backend -- agent WebSocket client, reconnection logic, heartbeat
- Desktop: Tool executors -- shell (with timeout + blocked commands), filesystem (with path sandboxing), process (via sysinfo)
- Desktop: Ollama local provider for offline mode
- Desktop: Offline message queue for sync when back online
- Mobile: Ionic/Angular + Capacitor app (Android) with chat, sessions, settings pages
- Mobile: Push notification service
- UI: 8 reusable components -- chat-window, message-bubble, message-composer, tool-execution-card, session-list, provider-selector, node-status, streaming-text
- UI: GraphQL client service (Observable-based, fetch + graphql-ws)
- UI: JohnnyAPI service with full CRUD operations
- UI: AI chat state management service with signals
- Shared: WebSocket protocol types (tool_call, tool_result, heartbeat, session lifecycle)
- Shared: Tool definition constants and JSON parameter schemas
- Shared: Utility functions (ID generation, date formatting)
