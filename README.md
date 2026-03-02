# JohnnyOne

> "Number 5 is alive!" -- Short Circuit (1986)

Personal AI agent platform. Your desktop is the always-on compute node (Tauri + Ionic/Angular), your phone is the remote control (Ionic/Angular + Capacitor), and Cloudflare Workers orchestrate everything in between. Multi-provider LLM support with Claude, OpenAI, and Ollama for fully offline operation.

## Architecture

```
                          +-------------------+
                          |   LLM Providers   |
                          | Claude | OpenAI   |
                          +--------+----------+
                                   |
  +----------+           +---------+---------+           +-----------------+
  |  Mobile  | <-------> |   CF Worker (Hub) | <-------> | Desktop Agent   |
  |  App     |  GraphQL  |                   |    WSS    | (Tauri)         |
  |          |  + WS     | +---------------+ |           |                 |
  +----------+           | | D1 (SQLite)   | |           | +-------------+ |
                         | | R2 (Storage)  | |           | | Tool Exec   | |
  +----------+           | | Durable Objs  | |           | | Shell       | |
  |  Web Hub | <-------> | | (Agent Loop)  | |           | | Filesystem  | |
  |          |  GraphQL  | +---------------+ |           | | Process     | |
  +----------+           +-------------------+           | | Ollama      | |
                                                         | +-------------+ |
                                                         +-----------------+
```

## Project Structure

```
johnnyone/
  desktop/              Ionic/Angular desktop frontend + Tauri backend
    src/                  Angular app (chat, sessions, tools, settings pages)
    src-tauri/            Rust backend (WS client, tool executors, Ollama)
  mobile/               Ionic/Angular + Capacitor mobile app (Android)
    src/                  Angular app (chat, sessions, settings pages)
  worker/               Cloudflare Worker -- API hub + agent orchestration
    d1/                   D1 migrations and seed data
    resolvers/            GraphQL resolvers (ai/, channels/)
    schema/               GraphQL schema definitions
  ui/                   Shared UI component library
    src/components/       Reusable Angular components (8 components)
    src/services/         GraphQL client, JohnnyAPI, AI chat state
    src/models/           TypeScript models and interfaces
  shared/               Cross-platform shared code
    src/types/            WebSocket protocol types, tool definitions
    src/utils/            ID generation, date formatting, helpers
  package.json          Root monorepo config (Nx 20, Angular 19)
  nx.json               Nx workspace configuration
  lokal.yaml            Lokal deployment configuration
  tsconfig.base.json    Shared TypeScript config
```

## Tech Stack

| Layer         | Technology                              |
|---------------|-----------------------------------------|
| Frontend      | Ionic 8 / Angular 19                    |
| Desktop       | Tauri 2 (Rust)                          |
| Mobile        | Capacitor 6 (Android)                   |
| Backend       | Cloudflare Workers                      |
| Database      | D1 (SQLite at the edge)                 |
| Real-time     | Durable Objects + WebSockets            |
| LLM Providers | Claude (Anthropic), OpenAI, Ollama      |
| Monorepo      | Nx 20                                   |
| GraphQL       | Custom resolver layer + graphql-ws      |

## Getting Started

### Prerequisites

- Node.js 20+
- Rust / Cargo (for Tauri desktop builds)
- Nx CLI (`npm install -g nx`)
- Wrangler CLI (bundled via devDependencies)
- Lokal CLI (for worker simulation and D1 migrations)

### Install

```bash
npm install
```

### Development

```bash
# Start the desktop Angular dev server (port 4200)
nx serve desktop

# Start the Tauri desktop app (in a separate terminal)
cd desktop/src-tauri && cargo tauri dev

# Start the worker API (port 7714)
lokal cf worker sim

# Apply D1 database migrations (local)
lokal cf db migrate --local

# Start the mobile Angular dev server (port 4201)
nx serve mobile
```

### Build

```bash
npm run build:desktop    # Build desktop Angular app
npm run build:mobile     # Build mobile Angular app
npm run build:worker     # Build Cloudflare Worker
```

## Key Features

- **Multi-provider LLM** -- Claude (Anthropic Messages API), OpenAI (Chat Completions), and Ollama for local/offline inference
- **Desktop tool execution** -- Shell commands (with timeout and blocked-command safety), filesystem operations (with path sandboxing), process listing (via sysinfo)
- **Real-time streaming** -- Token-by-token response streaming via Durable Objects and WebSockets
- **Tool approval flow** -- Tools that require approval are held in `pending` state until the user approves, rejects, or cancels
- **Offline mode** -- Ollama integration on the desktop node means the agent works without internet
- **Mobile remote control** -- Send messages, approve tool calls, and monitor sessions from your phone
- **Channel adapters** -- Telegram, Discord, WhatsApp adapter stubs ready for Phase 2

## Data Flow

1. Mobile (or Web Hub) sends a message via GraphQL `sendAgentMessage` mutation
2. Worker creates the message in D1 and routes the request to the `AgentSessionDO` Durable Object
3. The DO assembles conversation context and calls the configured LLM provider (streaming)
4. If the LLM returns a `tool_use` block, the DO creates a `ToolExecution` record and dispatches it to the Desktop Agent via the WebSocket connection
5. Desktop Agent executes the tool (shell, filesystem, process) and returns the result over WebSocket
6. The DO feeds the tool result back into the LLM as a `tool` role message
7. The LLM produces a final text response, streamed token-by-token to all connected clients via `onAgentMessageDelta` subscription

## Ports

| Service           | Port |
|-------------------|------|
| Worker API        | 7714 |
| Desktop frontend  | 4200 |
| Mobile frontend   | 4201 |

## GraphQL API

The worker exposes a GraphQL API with:

- **6 queries** -- `getAiSession`, `listAiSessions`, `listAiMessages`, `listToolDefinitions`, `listDesktopNodes`, `getAiUsageSummary`
- **11 mutations** -- Session CRUD, `sendAgentMessage`, tool approval/rejection/cancellation, provider config management, desktop node registration
- **4 subscriptions** -- `onAgentMessage`, `onAgentMessageDelta` (streaming), `onToolExecution`, `onDesktopNodeStatus`

Schema: [`worker/schema/johnnyone-ai.graphql`](worker/schema/johnnyone-ai.graphql)

## Roadmap

| Phase  | Description                                         | Status      |
|--------|-----------------------------------------------------|-------------|
| 0      | Nx monorepo scaffold, project structure             | Done        |
| 1A     | Foundation + basic AI chat                          | In progress |
| 1B     | Streaming responses + AgentSessionDO                | Planned     |
| 1C     | Tool system + desktop agent execution               | Planned     |
| 1D     | Multi-provider support + node management            | Planned     |
| 2      | Channel adapters (Telegram, Discord, WhatsApp)      | Planned     |
| 3      | Browser automation, cron scheduling, voice input    | Planned     |

## License

Private / Proprietary. All rights reserved.
