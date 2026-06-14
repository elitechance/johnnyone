import { Injectable, inject } from '@angular/core';
import { Observable } from 'rxjs';
import { map } from 'rxjs/operators';
import { GraphQLClient } from './graphql-client';
import { AiSession } from '../models/ai-session.model';
import { AiMessage, AiMessageDelta } from '../models/ai-message.model';
import { ToolDefinition, ToolExecution } from '../models/tool.model';
import { ProviderConfig, AiUsageSummary } from '../models/provider.model';
import { DesktopNode } from '../models/desktop-node.model';

export interface CreateAiSessionInput {
  title?: string;
  provider?: string;
  model?: string;
  workingDirectory?: string;
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
  phaseRunMode: 'continue' | 'single' | string;
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

export interface WorkspaceFileDiff {
  path: string;
  diff: string;
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
      brief appScope docsScope referencePaths amendBrief phaseRunMode
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
      createdAt updatedAt
    `;
    const query = `query ListSessions($status: String) {
      listAiSessions(status: $status) { ${sessionFields} }
    }`;

    return this.gql
      .queryPreferLocalHost<{ listAiSessions: AiSession[] }>(query, query, { status })
      .pipe(map((data) => data.listAiSessions));
  }

  getSession(id: string): Observable<AiSession> {
    const sessionFields = `
      id title provider model workingDirectory status
      totalInputTokens totalOutputTokens totalCostCents
      createdAt updatedAt
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
            createdAt updatedAt
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
