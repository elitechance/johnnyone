import { Injectable, inject } from '@angular/core';
import { Observable } from 'rxjs';
import { map } from 'rxjs/operators';
import { GraphQLClient } from './graphql-client';
import { AiSession } from '../models/ai-session.model';
import { AiMessage, AiMessageDelta } from '../models/ai-message.model';
import { ToolDefinition, ToolExecution } from '../models/tool.model';
import { ProviderConfig, AiUsageSummary } from '../models/provider.model';
import { DesktopNode } from '../models/desktop-node.model';
import { TerminalScreen } from '../models/terminal.model';

export interface CreateAiSessionInput {
  title?: string;
  provider?: string;
  model?: string;
  workingDirectory?: string;
  /** When set, attach to this existing external tmux session instead of spawning a shell. */
  tmuxSessionName?: string;
}

/** An external tmux session a terminal can attach to. */
export interface TmuxSession {
  name: string;
  attached: boolean;
  windows: number;
}

export interface AiChatRunResult {
  userMessage: AiMessage;
  assistantMessage: AiMessage;
}

export interface AiChatComplete {
  sessionId: string;
  messageId: string;
}

export interface DetectedCliTool {
  provider: string;
  command: string;
  found: boolean;
  path?: string;
}

// ── M2M API keys (partner/developer credentials) ──────────────────────────
// Mirrors worker/schema/johnnyone-api-keys.graphql. The full secret (`jk_...`)
// is returned exactly once on create and is never re-readable.
export interface ApiKey {
  id: string;
  name: string;
  keyPrefix: string;
  scopes: string[];
  lastUsedAt?: string | null;
  expiresAt?: string | null;
  revokedAt?: string | null;
  createdAt: string;
}

export interface CreateApiKeyInput {
  name: string;
  scopes: string[];
  expiresAt?: string;
}

export interface CreateApiKeyResult {
  apiKey: ApiKey;
  secret: string;
}

export interface CreateAgentPlanInput {
  runType?: 'planning' | 'development';
  title?: string;
  workspacePath: string;
  planPath: string;
  workerProvider: string;
  reviewerProvider: string;
  brief?: string;
  appScope?: string;
  docsScope?: string;
  referencePaths?: string;
  /** When workerProvider is 'shell', commands run in the spawned shell on first launch. */
  workerSetupCommands?: string;
  /** When reviewerProvider is 'shell', commands run in each spawned reviewer shell. */
  reviewerSetupCommands?: string;
}

/** Create an Initiative in briefing (overhaul P4, D1). `brief` is the raw ask. */
export interface CreateBriefingInput {
  title?: string;
  workspacePath: string;
  brief: string;
  workerProvider: string;
  reviewerProvider: string;
  model?: string;
}

/** Accept a briefing brief (overhaul P4, D1/D4). `finalBrief` optionally overrides the stored draft. */
export interface AcceptBriefInput {
  initiativeId: string;
  finalBrief?: string;
}

export interface AgentPlan {
  id: string;
  runType: 'planning' | 'development' | string;
  title: string;
  workspacePath: string;
  planPath: string;
  status: string;
  workerSessionId?: string;
  reviewerSessionId?: string;
  workerProvider: string;
  reviewerProvider: string;
  currentPhaseId?: string;
  currentPhaseIndex: number;
  error?: string;
  brief?: string;
  appScope?: string;
  docsScope?: string;
  referencePaths?: string;
  /** Non-empty when an amendment cycle is in flight; cleared when T2 PASSes. */
  amendBrief?: string;
  /** Configurable validation lenses as a JSON string (overhaul P7, D1). Null/empty → the desktop
   * resolves the default product/qa/lead template. Nullable for deploy-skew safety. */
  validationConfig?: string | null;
  phaseRunMode: 'continue' | 'single' | string;
  /** Groups a planning stage-run + a development stage-run into one Initiative. */
  initiativeId: string;
  /** Lifecycle stage. */
  initiativeStatus: 'briefing' | 'planning' | 'development' | 'review' | 'done' | string;
  /** Condition axis, independent of stage. */
  health: 'in-progress' | 'needs-attention' | 'blocked' | string;
  /** Non-empty only on a briefing Initiative: its kind='user' chat session (overhaul P4, D3). */
  briefingSessionId?: string;
  createdAt: string;
  updatedAt: string;
}

export interface AgentPlanPhase {
  id: string;
  planId: string;
  phaseId: string;
  phaseTitle: string;
  phaseIndex: number;
  status: string;
  workerStartedAt?: string;
  workerIdleAt?: string;
  reviewerStartedAt?: string;
  reviewerIdleAt?: string;
  gateVerdict: string;
  clarificationAttempts: number;
  summary?: string;
  findingsJson: string;
  createdAt: string;
  updatedAt: string;
}

export interface AgentPlanTask {
  id: string;
  planId: string;
  phaseId: string;
  taskId: string;
  taskTitle: string;
  taskIndex: number;
  promptPath: string;
  statusPath?: string;
  decisionsPath?: string;
  status: string;
  createdAt: string;
  updatedAt: string;
}

export interface AgentPlanEvent {
  id: string;
  planId: string;
  phaseId?: string;
  phaseIndex?: number;
  phaseTitle?: string;
  eventType: string;
  actor: string;
  category: string;
  summary: string;
  statusBefore?: string;
  statusAfter?: string;
  reason?: string;
  verdict?: string;
  taskId?: string;
  clarificationAttempt?: number;
  payloadJson: string;
  createdAt: string;
}

export interface AgentPlanRun {
  plan: AgentPlan;
  phases: AgentPlanPhase[];
  tasks: AgentPlanTask[];
  events: AgentPlanEvent[];
}

export interface HostFileEntry {
  path: string;
  name: string;
  kind: string;
  status?: string;
  size?: number;
}

export interface HostFileContent {
  path: string;
  name: string;
  kind: string;
  contentType: string;
  encoding: string;
  content: string;
  size: number;
}

// File manager (overhaul P2, files_root-rooted). Mirrors the Phase-02 GraphQL types and the host
// serde (host_files.rs) — camelCase, separate from the plan-scoped HostFile* interfaces above.
export interface FileEntry {
  name: string;
  path: string; // relative to files_root
  kind: 'file' | 'dir' | string;
  size?: number;
  modified?: number; // unix millis
}
export interface DirListing {
  path: string;
  root: string;
  entries: FileEntry[];
}
/** A streaming delta of a relay chat response (worker `RelayChatDelta`). */
export interface RelayChatDeltaMsg {
  relayId: string;
  sessionId: string;
  delta: string;
  chunkType: string;
  isFinal: boolean;
}
/** A completed relay chat message (worker `RelayChatMessage`). */
export interface RelayChatMessageMsg {
  relayId: string;
  sessionId: string;
  role: string;
  content: string;
}
export interface FileContent {
  path: string;
  name: string;
  contentType: string;
  encoding: 'utf8' | 'base64' | string;
  content: string;
  size: number;
}
export interface FileOpResult {
  path: string;
  ok: boolean;
}
export interface UploadChunkResult {
  path: string;
  received: number;
  complete: boolean;
}

export interface WorkspaceFileDiff {
  path: string;
  diff: string;
}

/** One changed file in a whole-tree diff (overhaul P7, D7). Mirrors the Rust `GitDiffFile`. */
export interface GitDiffFile {
  path: string;
  oldPath?: string | null;
  additions: number;
  deletions: number;
  binary: boolean;
  diff: string;
}

/** Working-tree diff of a repo (overhaul P7, D7/D10/D11). Mirrors the Rust `GitDiffView`; a non-repo
 * or clean path yields `clean:true`, `files:[]`, `repoRoot:null` (benign empty state). */
export interface GitDiffView {
  repoRoot?: string | null;
  branch?: string | null;
  clean: boolean;
  files: GitDiffFile[];
}

export interface GitFilesView {
  path: string;
  repoRoot?: string;
  branch?: string;
  clean: boolean;
  entries: HostFileEntry[];
}

export interface GitActionResult {
  success: boolean;
  output: string;
}

export interface WorkspaceValidation {
  valid: boolean;
  workspacePath: string;
  planPath: string;
  title?: string;
  phaseCount: number;
  taskCount: number;
  error?: string;
}

export interface PlannerPromptSettings {
  schema?: string;
  development: {
    worker: string;
    reviewer: string;
  };
  planning: {
    planner: string;
    reviewer: string;
  };
}

export interface ChatAttachment {
  id: string;
  sessionId?: string;
  originalName: string;
  contentType: string;
  size: number;
  status: string;
  uploadedAt: string;
  localPath?: string;
}

@Injectable({ providedIn: 'root' })
export class JohnnyApiService {
  private readonly gql = inject(GraphQLClient);
  private readonly agentPlanRunFields = `
    plan {
      id runType title workspacePath planPath status workerSessionId reviewerSessionId
      workerProvider reviewerProvider currentPhaseId currentPhaseIndex error
      brief appScope docsScope referencePaths amendBrief validationConfig phaseRunMode
      initiativeId initiativeStatus health briefingSessionId
      createdAt updatedAt
    }
    phases {
      id planId phaseId phaseTitle phaseIndex status workerStartedAt workerIdleAt
      reviewerStartedAt reviewerIdleAt gateVerdict clarificationAttempts summary
      findingsJson createdAt updatedAt
    }
    tasks {
      id planId phaseId taskId taskTitle taskIndex promptPath statusPath
      decisionsPath status createdAt updatedAt
    }
    events {
      id planId phaseId phaseIndex phaseTitle eventType actor category summary
      statusBefore statusAfter reason verdict taskId clarificationAttempt payloadJson createdAt
    }
  `;

  // ── Sessions ──────────────────────────────────────────────────────────

  listSessions(status?: string): Observable<AiSession[]> {
    const sessionFields = `
      id title provider model workingDirectory status
      totalInputTokens totalOutputTokens totalCostCents
      createdAt updatedAt attachedTmux
    `;
    const query = `query ListSessions($status: String) {
      listAiSessions(status: $status) { ${sessionFields} }
    }`;

    return this.gql
      .queryPreferLocalHost<{ listAiSessions: AiSession[] }>(query, query, { status })
      .pipe(map((data) => data.listAiSessions));
  }

  /** External tmux sessions a new terminal can attach to (excludes johnnyone_<id> panes). */
  listTmuxSessions(): Observable<TmuxSession[]> {
    const query = `query ListTmuxSessions {
      listTmuxSessions { name attached windows }
    }`;
    return this.gql
      .queryPreferLocalHost<{ listTmuxSessions: TmuxSession[] }>(query, query, {})
      .pipe(map((data) => data.listTmuxSessions));
  }

  getSession(id: string): Observable<AiSession> {
    const sessionFields = `
      id title provider model workingDirectory status
      totalInputTokens totalOutputTokens totalCostCents
      createdAt updatedAt attachedTmux
    `;
    const query = `query GetSession($id: ID!) {
      getAiSession(id: $id) { ${sessionFields} }
    }`;

    return this.gql
      .queryPreferLocalHost<{ getAiSession: AiSession }>(query, query, { id })
      .pipe(map((data) => data.getAiSession));
  }

  deleteSession(id: string): Observable<boolean> {
    return this.gql
      .mutate<{ deleteAiSession: boolean }>(
        `mutation DeleteAiSession($id: ID!) {
          deleteAiSession(id: $id)
        }`,
        { id }
      )
      .pipe(map((data) => data.deleteAiSession));
  }

  createSession(input: CreateAiSessionInput): Observable<AiSession> {
    return this.gql
      .mutate<{ createAiSession: AiSession }>(
        `mutation CreateAiSession($input: CreateAiSessionInput!) {
          createAiSession(input: $input) {
            id title provider model workingDirectory status
            totalInputTokens totalOutputTokens totalCostCents
            createdAt updatedAt attachedTmux
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.createAiSession));
  }

  updateSessionTitle(id: string, title: string): Observable<AiSession> {
    return this.gql
      .mutate<{ updateAiSessionTitle: AiSession }>(
        `mutation UpdateAiSessionTitle($id: ID!, $title: String!) {
          updateAiSessionTitle(id: $id, title: $title) {
            id title provider model workingDirectory status
            totalInputTokens totalOutputTokens totalCostCents
            createdAt updatedAt
          }
        }`,
        { id, title }
      )
      .pipe(map((data) => data.updateAiSessionTitle));
  }

  updateSessionWorkingDirectory(id: string, workingDirectory: string): Observable<AiSession> {
    return this.gql
      .mutate<{ updateAiSessionWorkingDirectory: AiSession }>(
        `mutation UpdateAiSessionWorkingDirectory($id: ID!, $workingDirectory: String!) {
          updateAiSessionWorkingDirectory(id: $id, workingDirectory: $workingDirectory) {
            id title provider model workingDirectory status
            totalInputTokens totalOutputTokens totalCostCents
            createdAt updatedAt
          }
        }`,
        { id, workingDirectory }
      )
      .pipe(map((data) => data.updateAiSessionWorkingDirectory));
  }

  updateSessionProvider(id: string, provider: string): Observable<AiSession> {
    return this.gql
      .mutate<{ updateAiSessionProvider: AiSession }>(
        `mutation UpdateAiSessionProvider($id: ID!, $provider: String!) {
          updateAiSessionProvider(id: $id, provider: $provider) {
            id title provider model workingDirectory status
            totalInputTokens totalOutputTokens totalCostCents
            createdAt updatedAt
          }
        }`,
        { id, provider }
      )
      .pipe(map((data) => data.updateAiSessionProvider));
  }

  archiveSession(id: string): Observable<AiSession> {
    return this.gql
      .mutate<{ updateAiSessionArchived: AiSession }>(
        `mutation UpdateAiSessionArchived($id: ID!) {
          updateAiSessionArchived(id: $id) {
            id title provider model workingDirectory status
            totalInputTokens totalOutputTokens totalCostCents
            createdAt updatedAt
          }
        }`,
        { id }
      )
      .pipe(map((data) => data.updateAiSessionArchived));
  }

  // ── Messages ──────────────────────────────────────────────────────────

  listMessages(sessionId: string, limit?: number, offset?: number): Observable<AiMessage[]> {
    const query = `query ListMessages($sessionId: ID!, $limit: Int, $offset: Int) {
      listAiMessages(sessionId: $sessionId, limit: $limit, offset: $offset) {
        id sessionId role content toolCalls
        finishReason inputTokens outputTokens costCents createdAt
      }
    }`;

    return this.gql
      .queryPreferLocalHost<{ listAiMessages: AiMessage[] }>(query, query, { sessionId, limit, offset })
      .pipe(map((data) => data.listAiMessages));
  }

  sendRelayMessage(input: { sessionId: string; content: string; provider?: string; model?: string; workingDirectory?: string }): Observable<{ success: boolean; relayId: string; desktopNodeId: string }> {
    return this.gql
      .mutate<{ sendRelayChatMessage: { success: boolean; relayId: string; desktopNodeId: string } }>(
        `mutation SendRelayChatMessage($input: RelayChatMessageInput!) {
          sendRelayChatMessage(input: $input) {
            success relayId desktopNodeId
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.sendRelayChatMessage));
  }

  createChatAttachment(input: { sessionId?: string; originalName: string; contentType: string; dataBase64: string }): Observable<ChatAttachment> {
    return this.gql
      .mutate<{ createChatAttachment: ChatAttachment }>(
        `mutation CreateChatAttachment($input: ChatAttachmentInput!) {
          createChatAttachment(input: $input) {
            id sessionId originalName contentType size status uploadedAt localPath
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.createChatAttachment));
  }

  // sendAiChatMessage (forward path) removed by Phase 2 task 05 of multi-user-saas plan.
  // Use sendRelayChatMessage (above) — it routes through ChatRelayDO to the user's
  // registered host, which is multi-tenant safe.

  stopAiGeneration(sessionId: string): Observable<boolean> {
    return this.gql
      .mutate<{ cancelAiGeneration: boolean }>(
        `mutation CancelAiGeneration($sessionId: ID!) {
          cancelAiGeneration(sessionId: $sessionId)
        }`,
        { sessionId }
      )
      .pipe(map((data) => data.cancelAiGeneration));
  }

  // ── Message Subscriptions ─────────────────────────────────────────────

  onMessageDelta(sessionId: string): Observable<AiMessageDelta> {
    return this.gql
      .subscribe<{ aiMessageDelta: AiMessageDelta }>(
        `subscription OnMessageDelta($sessionId: ID!) {
          aiMessageDelta(sessionId: $sessionId) {
            sessionId messageId delta finishReason
          }
        }`,
        { sessionId }
      )
      .pipe(map((data) => data.aiMessageDelta));
  }

  onNewMessage(sessionId: string): Observable<AiMessage> {
    return this.gql
      .subscribe<{ aiMessageCreated: AiMessage }>(
        `subscription OnNewMessage($sessionId: ID!) {
          aiMessageCreated(sessionId: $sessionId) {
            id sessionId role content toolCalls
            finishReason inputTokens outputTokens costCents createdAt
          }
        }`,
        { sessionId }
      )
      .pipe(map((data) => data.aiMessageCreated));
  }

  // onAiChatDelta / onAiChatComplete (forward subscriptions) removed by Phase 2 task 05.
  // Use onRelayChatDelta + onRelayChatMessage (relay-WS via ChatRelayDO) for
  // streaming + completion.

  /** Live streaming deltas for ONE relay chat response, keyed by the `relayId` returned from
   *  `sendRelayMessage`. (The worker has no session-level delta subscription — deltas are per-request.) */
  onRelayChatDelta(relayId: string): Observable<RelayChatDeltaMsg> {
    return this.gql
      .subscribe<{ onRelayChatDelta: RelayChatDeltaMsg }>(
        `subscription OnRelayChatDelta($relayId: String!) {
          onRelayChatDelta(relayId: $relayId) {
            relayId sessionId delta chunkType isFinal
          }
        }`,
        { relayId }
      )
      .pipe(map((data) => data.onRelayChatDelta));
  }

  /** Completed relay chat messages for a session (user + assistant), as they are persisted. */
  onRelayChatMessage(sessionId: string): Observable<RelayChatMessageMsg> {
    return this.gql
      .subscribe<{ onRelayChatMessage: RelayChatMessageMsg }>(
        `subscription OnRelayChatMessage($sessionId: String!) {
          onRelayChatMessage(sessionId: $sessionId) {
            relayId sessionId role content
          }
        }`,
        { sessionId }
      )
      .pipe(map((data) => data.onRelayChatMessage));
  }

  // ── Tools ─────────────────────────────────────────────────────────────

  listToolDefinitions(category?: string): Observable<ToolDefinition[]> {
    return this.gql
      .query<{ toolDefinitions: ToolDefinition[] }>(
        `query ListToolDefinitions($category: String) {
          toolDefinitions(category: $category) {
            id slug name description category
            parametersSchema executor requiresApproval isEnabled
          }
        }`,
        { category }
      )
      .pipe(map((data) => data.toolDefinitions));
  }

  approveToolExecution(id: string): Observable<ToolExecution> {
    return this.gql
      .mutate<{ approveToolExecution: ToolExecution }>(
        `mutation ApproveToolExecution($id: ID!) {
          approveToolExecution(id: $id) {
            id status updatedAt
          }
        }`,
        { id }
      )
      .pipe(map((data) => data.approveToolExecution));
  }

  rejectToolExecution(id: string): Observable<ToolExecution> {
    return this.gql
      .mutate<{ rejectToolExecution: ToolExecution }>(
        `mutation RejectToolExecution($id: ID!) {
          rejectToolExecution(id: $id) {
            id status updatedAt
          }
        }`,
        { id }
      )
      .pipe(map((data) => data.rejectToolExecution));
  }

  cancelToolExecution(id: string): Observable<ToolExecution> {
    return this.gql
      .mutate<{ cancelToolExecution: ToolExecution }>(
        `mutation CancelToolExecution($id: ID!) {
          cancelToolExecution(id: $id) {
            id status updatedAt
          }
        }`,
        { id }
      )
      .pipe(map((data) => data.cancelToolExecution));
  }

  // ── Providers ─────────────────────────────────────────────────────────

  upsertProviderConfig(input: Partial<ProviderConfig>): Observable<ProviderConfig> {
    return this.gql
      .mutate<{ upsertProviderConfig: ProviderConfig }>(
        `mutation UpsertProviderConfig($input: UpsertProviderConfigInput!) {
          upsertProviderConfig(input: $input) {
            id provider cliPath apiKey defaultModel settings isAvailable updatedAt
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.upsertProviderConfig));
  }

  deleteProviderConfig(provider: string): Observable<boolean> {
    return this.gql
      .mutate<{ deleteProviderConfig: boolean }>(
        `mutation DeleteProviderConfig($provider: String!) {
          deleteProviderConfig(provider: $provider)
        }`,
        { provider }
      )
      .pipe(map((data) => data.deleteProviderConfig));
  }

  // ── Desktop Nodes ─────────────────────────────────────────────────────

  listDesktopNodes(): Observable<DesktopNode[]> {
    return this.gql
      .query<{ listDesktopNodes: DesktopNode[] }>(
        `query ListDesktopNodes {
          listDesktopNodes {
            id hostname os arch version status
            capabilities lastHeartbeatAt createdAt
          }
        }`
      )
      .pipe(map((data) => data.listDesktopNodes));
  }

  listProviderConfigs(): Observable<ProviderConfig[]> {
    return this.gql
      .query<{ listProviderConfigs: ProviderConfig[] }>(
        `query ListProviderConfigs {
          listProviderConfigs {
            id provider cliPath apiKey defaultModel settings isAvailable updatedAt
          }
        }`
      )
      .pipe(map((data) => data.listProviderConfigs));
  }

  detectCliTools(): Observable<DetectedCliTool[]> {
    const workerQuery = `query ListDetectedCliTools {
      listDetectedCliTools { provider command found path }
    }`;
    const hostQuery = `query DetectCliTools {
      detectCliTools { provider command found path }
    }`;

    return this.gql
      .queryPreferLocalHost<{
        listDetectedCliTools?: DetectedCliTool[];
        detectCliTools?: DetectedCliTool[];
      }>(workerQuery, hostQuery)
      .pipe(map((data) => data.detectCliTools ?? data.listDetectedCliTools ?? []));
  }

  getSetting(key: string): Observable<string> {
    const query = `query GetSetting($key: String!) {
      getSetting(key: $key)
    }`;

    return this.gql
      .queryPreferLocalHost<{ getSetting: string }>(query, query, { key })
      .pipe(map((data) => data.getSetting));
  }

  setSetting(key: string, value: string): Observable<boolean> {
    return this.gql
      .mutate<{ updateSetting: boolean }>(
        `mutation UpdateSetting($key: String!, $value: String!) {
          updateSetting(key: $key, value: $value)
        }`,
        { key, value }
      )
      .pipe(map((data) => data.updateSetting));
  }

  getPlannerPromptSettings(): Observable<PlannerPromptSettings> {
    return this.gql
      .query<{ getPlannerPromptSettings: PlannerPromptSettings }>(
        `query GetPlannerPromptSettings {
          getPlannerPromptSettings {
            schema
            development { worker reviewer }
            planning { planner reviewer }
          }
        }`
      )
      .pipe(map((data) => data.getPlannerPromptSettings));
  }

  updatePlannerPromptSettings(input: PlannerPromptSettings): Observable<PlannerPromptSettings> {
    const payload = {
      development: input.development,
      planning: input.planning,
    };
    return this.gql
      .mutate<{ updatePlannerPromptSettings: PlannerPromptSettings }>(
        `mutation UpdatePlannerPromptSettings($input: PlannerPromptSettingsInput!) {
          updatePlannerPromptSettings(input: $input) {
            schema
            development { worker reviewer }
            planning { planner reviewer }
          }
        }`,
        { input: payload }
      )
      .pipe(map((data) => data.updatePlannerPromptSettings));
  }

  // ── Agent Planner ────────────────────────────────────────────────────

  listAgentPlans(status?: string, runType?: 'planning' | 'development', onlyExisting = false): Observable<AgentPlan[]> {
    return this.gql
      .query<{ listAgentPlans: AgentPlan[] }>(
        `query ListAgentPlans($status: String, $runType: String, $onlyExisting: Boolean) {
          listAgentPlans(status: $status, runType: $runType, onlyExisting: $onlyExisting) {
            id runType title workspacePath planPath status workerSessionId reviewerSessionId
            workerProvider reviewerProvider currentPhaseId currentPhaseIndex error
            brief appScope docsScope referencePaths
            initiativeId initiativeStatus health validationConfig
            createdAt updatedAt
          }
        }`,
        { status, runType, onlyExisting }
      )
      .pipe(map((data) => data.listAgentPlans));
  }

  getAgentPlan(id: string): Observable<AgentPlanRun> {
    return this.gql
      .query<{ getAgentPlan: AgentPlanRun }>(
        `query GetAgentPlan($id: ID!) {
          getAgentPlan(id: $id) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id }
      )
      .pipe(map((data) => data.getAgentPlan));
  }

  createAgentPlan(input: CreateAgentPlanInput): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ createAgentPlan: AgentPlanRun }>(
        `mutation CreateAgentPlan($input: CreateAgentPlanInput!) {
          createAgentPlan(input: $input) {
            ${this.agentPlanRunFields}
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.createAgentPlan));
  }

  // ── Briefing loop (overhaul P4) ───────────────────────────────────────
  // Thin GraphQL clients over the Phase 03 relay mutations (each a desktopRpc pass-through to a
  // Phase 01/02 host method). Variable names match worker/schema/johnnyone-ai.graphql.

  /** Create an Initiative in briefing (host `create_briefing_run`). */
  createBriefingInitiative(input: CreateBriefingInput): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ createBriefingInitiative: AgentPlanRun }>(
        `mutation CreateBriefingInitiative($input: CreateBriefingInput!) {
          createBriefingInitiative(input: $input) {
            ${this.agentPlanRunFields}
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.createBriefingInitiative));
  }

  /** Accept the brief → flips the same Initiative briefing→planning (host `accept_brief`). */
  acceptInitiativeBrief(input: AcceptBriefInput): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ acceptInitiativeBrief: AgentPlanRun }>(
        `mutation AcceptInitiativeBrief($input: AcceptBriefInput!) {
          acceptInitiativeBrief(input: $input) {
            ${this.agentPlanRunFields}
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.acceptInitiativeBrief));
  }

  /** Record a ▤ Reference host path on a briefing Initiative (host `add_initiative_reference_path`). */
  addInitiativeReferencePath(initiativeId: string, path: string): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ addInitiativeReferencePath: AgentPlanRun }>(
        `mutation AddInitiativeReferencePath($initiativeId: ID!, $path: String!) {
          addInitiativeReferencePath(initiativeId: $initiativeId, path: $path) {
            ${this.agentPlanRunFields}
          }
        }`,
        { initiativeId, path }
      )
      .pipe(map((data) => data.addInitiativeReferencePath));
  }

  /** Upload a briefing attachment chunk into <id>/attachments/ (host `initiative_upload_chunk`). */
  initiativeUploadChunk(
    initiativeId: string,
    path: string,
    chunkIndex: number,
    totalChunks: number,
    dataBase64: string,
    done: boolean
  ): Observable<UploadChunkResult> {
    return this.gql
      .mutate<{ initiativeUploadChunk: UploadChunkResult }>(
        `query InitiativeUploadChunk($initiativeId: ID!, $path: String!, $chunkIndex: Int!, $totalChunks: Int!, $dataBase64: String!, $done: Boolean!) {
          initiativeUploadChunk(initiativeId: $initiativeId, path: $path, chunkIndex: $chunkIndex, totalChunks: $totalChunks, dataBase64: $dataBase64, done: $done) {
            path received complete
          }
        }`,
        { initiativeId, path, chunkIndex, totalChunks, dataBase64, done }
      )
      .pipe(map((data) => data.initiativeUploadChunk));
  }

  startAgentPlan(id: string, phaseId?: string, phaseRunMode?: 'continue' | 'single'): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ startAgentPlan: AgentPlanRun }>(
        `mutation StartAgentPlan($id: ID!, $phaseId: String, $phaseRunMode: String) {
          startAgentPlan(id: $id, phaseId: $phaseId, phaseRunMode: $phaseRunMode) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id, phaseId: phaseId || null, phaseRunMode: phaseRunMode || null }
      )
      .pipe(map((data) => data.startAgentPlan));
  }

  refreshAgentPlanPhases(id: string): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ updateAgentPlanPhasesRefresh: AgentPlanRun }>(
        `mutation UpdateAgentPlanPhasesRefresh($id: ID!) {
          updateAgentPlanPhasesRefresh(id: $id) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id }
      )
      .pipe(map((data) => data.updateAgentPlanPhasesRefresh));
  }

  updateAgentPlanTitle(id: string, title: string): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ updateAgentPlanTitle: AgentPlanRun }>(
        `mutation UpdateAgentPlanTitle($id: ID!, $title: String!) {
          updateAgentPlanTitle(id: $id, title: $title) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id, title }
      )
      .pipe(map((data) => data.updateAgentPlanTitle));
  }

  /** Set/clear the app-repo path (`app_scope`) the docs-commit agent uses. Works on
   * an in-flight run; an empty value clears it (docs commit then skips). */
  updateAgentPlanAppScope(id: string, appScope: string): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ updateAgentPlanAppScope: AgentPlanRun }>(
        `mutation UpdateAgentPlanAppScope($id: ID!, $appScope: String) {
          updateAgentPlanAppScope(id: $id, appScope: $appScope) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id, appScope }
      )
      .pipe(map((data) => data.updateAgentPlanAppScope));
  }

  /** Persist the Initiative's configurable validation lenses as a JSON string (overhaul P7, D1).
   * Pass `null` (or an empty string) to clear it — the desktop then resolves the default
   * product/qa/lead template. The desktop validates the JSON (parse + provider) before writing. */
  updateAgentPlanValidationConfig(
    id: string,
    config: string | null,
  ): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ updateAgentPlanValidationConfig: AgentPlanRun }>(
        `mutation UpdateAgentPlanValidationConfig($id: ID!, $config: String) {
          updateAgentPlanValidationConfig(id: $id, config: $config) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id, config }
      )
      .pipe(map((data) => data.updateAgentPlanValidationConfig));
  }

  /**
   * Amend an approved planning run. Stashes the brief on the plan, switches
   * T1 into "edit mode", and re-runs the T1→T2 planning cycle. On T2 PASS
   * the amended plan state is committed to the plan's per-plan git repo.
   *
   * The GraphQL name is `updateAgentPlanAmend` (uses an `update-` mutation
   * prefix lokal's validator recognizes); we expose it as `amendAgentPlan`
   * here so callers read naturally.
   */
  amendAgentPlan(id: string, brief: string): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ updateAgentPlanAmend: AgentPlanRun }>(
        `mutation UpdateAgentPlanAmend($id: ID!, $brief: String!) {
          updateAgentPlanAmend(id: $id, brief: $brief) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id, brief }
      )
      .pipe(map((data) => data.updateAgentPlanAmend));
  }

  stopAgentPlan(id: string): Observable<AgentPlanRun> {
    return this.agentPlanMutation('updateAgentPlanStopped', { id });
  }

  deleteAgentPlan(id: string): Observable<boolean> {
    return this.gql
      .mutate<{ deleteAgentPlan: boolean }>(
        `mutation DeleteAgentPlan($id: ID!) {
          deleteAgentPlan(id: $id)
        }`,
        { id }
      )
      .pipe(map((data) => data.deleteAgentPlan));
  }

  blockAgentPlan(id: string, reason: string): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ updateAgentPlanBlocked: AgentPlanRun }>(
        `mutation BlockAgentPlan($id: ID!, $reason: String!) {
          updateAgentPlanBlocked(id: $id, reason: $reason) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id, reason }
      )
      .pipe(map((data) => data.updateAgentPlanBlocked));
  }

  manualPassAgentPhase(id: string, phaseId: string): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ updateAgentPhaseManualPass: AgentPlanRun }>(
        `mutation ManualPassAgentPhase($id: ID!, $phaseId: String!) {
          updateAgentPhaseManualPass(id: $id, phaseId: $phaseId) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id, phaseId }
      )
      .pipe(map((data) => data.updateAgentPhaseManualPass));
  }

  sendAgentFeedbackToWorker(id: string): Observable<AgentPlanRun> {
    return this.agentPlanMutation('sendAgentFeedbackToWorker', { id });
  }

  rerunAgentReviewer(id: string): Observable<AgentPlanRun> {
    return this.agentPlanMutation('retryAgentReviewer', { id });
  }

  browseHostDirectory(path: string): Observable<HostFileEntry[]> {
    return this.gql
      .query<{ browseHostDirectory: HostFileEntry[] }>(
        `query BrowseHostDirectory($path: String!) {
          browseHostDirectory(path: $path) {
            path name kind status size
          }
        }`,
        { path }
      )
      .pipe(map((data) => data.browseHostDirectory));
  }

  validateWorkspacePlan(workspacePath: string, planPath: string): Observable<WorkspaceValidation> {
    return this.gql
      .query<{ validateWorkspacePlan: WorkspaceValidation }>(
        `query ValidateWorkspacePlan($workspacePath: String!, $planPath: String!) {
          validateWorkspacePlan(workspacePath: $workspacePath, planPath: $planPath) {
            valid workspacePath planPath title phaseCount taskCount error
          }
        }`,
        { workspacePath, planPath }
      )
      .pipe(map((data) => data.validateWorkspacePlan));
  }

  listWorkspaceFiles(planId: string, mode: 'changed' | 'all', path?: string): Observable<HostFileEntry[]> {
    return this.gql
      .query<{ listWorkspaceFiles: HostFileEntry[] }>(
        `query ListWorkspaceFiles($planId: ID!, $mode: String!, $path: String) {
          listWorkspaceFiles(planId: $planId, mode: $mode, path: $path) {
            path name kind status size
          }
        }`,
        { planId, mode, path }
      )
      .pipe(map((data) => data.listWorkspaceFiles));
  }

  gitFileView(planId: string, path?: string): Observable<GitFilesView> {
    return this.gql
      .query<{ gitFileView: GitFilesView }>(
        `query GitFileView($planId: ID!, $path: String) {
          gitFileView(planId: $planId, path: $path) {
            path repoRoot branch clean
            entries { path name kind status size }
          }
        }`,
        { planId, path }
      )
      .pipe(map((data) => data.gitFileView));
  }

  runGitAction(planId: string, input: { path?: string; action: 'fetch' | 'pull' | 'push' | 'commit'; message?: string }): Observable<GitActionResult> {
    return this.gql
      .mutate<{ updateGitAction: GitActionResult }>(
        `mutation RunGitAction($planId: ID!, $path: String, $action: String!, $message: String) {
          updateGitAction(planId: $planId, path: $path, action: $action, message: $message) {
            success output
          }
        }`,
        { planId, path: input.path || null, action: input.action, message: input.message || null }
      )
      .pipe(map((data) => data.updateGitAction));
  }

  readHostFile(planId: string, path: string): Observable<HostFileContent> {
    return this.gql
      .query<{ readHostFile: HostFileContent }>(
        `query ReadHostFile($planId: ID!, $path: String!) {
          readHostFile(planId: $planId, path: $path) {
            path name kind contentType encoding content size
          }
        }`,
        { planId, path }
      )
      .pipe(map((data) => data.readHostFile));
  }

  getWorkspaceFileDiff(planId: string, path: string): Observable<WorkspaceFileDiff> {
    return this.gql
      .query<{ getWorkspaceFileDiff: WorkspaceFileDiff }>(
        `query GetWorkspaceFileDiff($planId: ID!, $path: String!) {
          getWorkspaceFileDiff(planId: $planId, path: $path) {
            path diff
          }
        }`,
        { planId, path }
      )
      .pipe(map((data) => data.getWorkspaceFileDiff));
  }

  /** Whole-tree working-tree diff of the repo at `path` (a session's workingDirectory) — overhaul
   * P7, D7/D10. Scope-gated (`files:read`) + path-guarded on the worker/host; a non-repo/clean path
   * resolves to `{ clean:true, files:[] }` (no error). One round-trip, per-file +/- counts + hunks. */
  gitDiff(path: string): Observable<GitDiffView> {
    return this.gql
      .query<{ gitDiff: GitDiffView }>(
        `query GitDiff($path: String!) {
          gitDiff(path: $path) {
            repoRoot branch clean
            files { path oldPath additions deletions binary diff }
          }
        }`,
        { path }
      )
      .pipe(map((data) => data.gitDiff));
  }

  // ── File manager (overhaul P2) — relay-RPC via the Phase-02 worker GraphQL surface. ──────────
  // Auth (JWT Bearer + tenant/user headers) is attached centrally by graphql-client.ts; these
  // methods inherit it. `content`/`dataBase64` are forwarded, never logged.
  listDir(path: string): Observable<DirListing> {
    return this.gql
      .query<{ filesListDir: DirListing }>(
        `query FilesListDir($path: String!) {
          filesListDir(path: $path) {
            path root entries { name path kind size modified }
          }
        }`,
        { path }
      )
      .pipe(map((data) => data.filesListDir));
  }

  readFile(path: string): Observable<FileContent> {
    return this.gql
      .query<{ filesRead: FileContent }>(
        `query FilesRead($path: String!) {
          filesRead(path: $path) {
            path name contentType encoding content size
          }
        }`,
        { path }
      )
      .pipe(map((data) => data.filesRead));
  }

  writeFile(
    path: string,
    content: string,
    encoding: 'utf8' | 'base64' = 'utf8'
  ): Observable<FileOpResult> {
    return this.gql
      .mutate<{ filesWrite: FileOpResult }>(
        `query FilesWrite($path: String!, $content: String!, $encoding: String) {
          filesWrite(path: $path, content: $content, encoding: $encoding) {
            path ok
          }
        }`,
        { path, content, encoding }
      )
      .pipe(map((data) => data.filesWrite));
  }

  mkdir(path: string): Observable<FileOpResult> {
    return this.gql
      .mutate<{ filesMkdir: FileOpResult }>(
        `query FilesMkdir($path: String!) {
          filesMkdir(path: $path) {
            path ok
          }
        }`,
        { path }
      )
      .pipe(map((data) => data.filesMkdir));
  }

  rename(from: string, to: string): Observable<FileOpResult> {
    return this.gql
      .mutate<{ filesRename: FileOpResult }>(
        `query FilesRename($from: String!, $to: String!) {
          filesRename(from: $from, to: $to) {
            path ok
          }
        }`,
        { from, to }
      )
      .pipe(map((data) => data.filesRename));
  }

  deleteFile(path: string): Observable<FileOpResult> {
    return this.gql
      .mutate<{ filesDelete: FileOpResult }>(
        `query FilesDelete($path: String!) {
          filesDelete(path: $path) {
            path ok
          }
        }`,
        { path }
      )
      .pipe(map((data) => data.filesDelete));
  }

  uploadChunk(
    path: string,
    chunkIndex: number,
    totalChunks: number,
    dataBase64: string,
    done: boolean
  ): Observable<UploadChunkResult> {
    return this.gql
      .mutate<{ filesUploadChunk: UploadChunkResult }>(
        `query FilesUploadChunk($path: String!, $chunkIndex: Int!, $totalChunks: Int!, $dataBase64: String!, $done: Boolean!) {
          filesUploadChunk(path: $path, chunkIndex: $chunkIndex, totalChunks: $totalChunks, dataBase64: $dataBase64, done: $done) {
            path received complete
          }
        }`,
        { path, chunkIndex, totalChunks, dataBase64, done }
      )
      .pipe(map((data) => data.filesUploadChunk));
  }

  // Deterministic one-shot terminal capture (overhaul P2). The live stream still uses
  // relay-terminal.service.ts; this request/response read is for deterministic reads (tests/automation).
  captureTerminal(sessionId: string, historyLines?: number): Observable<TerminalScreen> {
    return this.gql
      .query<{ captureTerminal: TerminalScreen }>(
        `query CaptureTerminal($sessionId: ID!, $historyLines: Int) {
          captureTerminal(sessionId: $sessionId, historyLines: $historyLines) {
            sessionId tmuxSessionName paneId cursor content
            cursorX cursorY historyLines rows cols status
          }
        }`,
        { sessionId, historyLines }
      )
      .pipe(map((data) => data.captureTerminal));
  }

  registerDesktopNode(input: Partial<DesktopNode>): Observable<DesktopNode> {
    return this.gql
      .mutate<{ registerDesktopNode: DesktopNode }>(
        `mutation RegisterDesktopNode($input: RegisterDesktopNodeInput!) {
          registerDesktopNode(input: $input) {
            id hostname os arch version status
            capabilities lastHeartbeatAt createdAt
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.registerDesktopNode));
  }

  updateDesktopNodeStatus(id: string, status: DesktopNode['status']): Observable<DesktopNode> {
    return this.gql
      .mutate<{ updateDesktopNodeStatus: DesktopNode }>(
        `mutation UpdateDesktopNodeStatus($id: ID!, $status: String!) {
          updateDesktopNodeStatus(id: $id, status: $status) {
            id status lastHeartbeatAt
          }
        }`,
        { id, status }
      )
      .pipe(map((data) => data.updateDesktopNodeStatus));
  }

  // ── Usage ─────────────────────────────────────────────────────────────

  getUsageSummary(from?: string, to?: string): Observable<AiUsageSummary> {
    return this.gql
      .query<{ aiUsageSummary: AiUsageSummary }>(
        `query GetUsageSummary($from: String, $to: String) {
          aiUsageSummary(from: $from, to: $to) {
            totalInputTokens totalOutputTokens totalCost
            sessionCount messageCount
          }
        }`,
        { from, to }
      )
      .pipe(map((data) => data.aiUsageSummary));
  }

  // ── API keys (M2M) ────────────────────────────────────────────────────
  // JWT-only management surface against the worker. create returns the secret
  // exactly once; list/revoke return metadata only (no secret).

  private readonly apiKeyFields = `
    id name keyPrefix scopes lastUsedAt expiresAt revokedAt createdAt
  `;

  listApiKeys(): Observable<ApiKey[]> {
    return this.gql
      .query<{ listApiKeys: ApiKey[] }>(
        `query ListApiKeys { listApiKeys { ${this.apiKeyFields} } }`
      )
      .pipe(map((data) => data.listApiKeys));
  }

  createApiKey(input: CreateApiKeyInput): Observable<CreateApiKeyResult> {
    return this.gql
      .mutate<{ createApiKey: CreateApiKeyResult }>(
        `mutation CreateApiKey($input: CreateApiKeyInput!) {
          createApiKey(input: $input) {
            apiKey { ${this.apiKeyFields} }
            secret
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.createApiKey));
  }

  revokeApiKey(id: string): Observable<ApiKey> {
    return this.gql
      .mutate<{ revokeApiKey: ApiKey }>(
        `mutation RevokeApiKey($id: ID!) {
          revokeApiKey(id: $id) { ${this.apiKeyFields} }
        }`,
        { id }
      )
      .pipe(map((data) => data.revokeApiKey));
  }

  deleteApiKey(id: string): Observable<boolean> {
    return this.gql
      .mutate<{ deleteApiKey: boolean }>(
        `mutation DeleteApiKey($id: ID!) {
          deleteApiKey(id: $id)
        }`,
        { id }
      )
      .pipe(map((data) => data.deleteApiKey));
  }

  private agentPlanMutation(
    field: 'startAgentPlan' | 'updateAgentPlanStopped' | 'sendAgentFeedbackToWorker' | 'retryAgentReviewer',
    variables: { id: string }
  ): Observable<AgentPlanRun> {
    return this.gql
      .mutate<Record<typeof field, AgentPlanRun>>(
        `mutation AgentPlanMutation($id: ID!) {
          ${field}(id: $id) {
            ${this.agentPlanRunFields}
          }
        }`,
        variables
      )
      .pipe(map((data) => data[field]));
  }
}
