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
  eventType: string;
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

@Injectable({ providedIn: 'root' })
export class JohnnyApiService {
  private readonly gql = inject(GraphQLClient);
  private readonly agentPlanRunFields = `
    plan {
      id runType title workspacePath planPath status workerSessionId reviewerSessionId
      workerProvider reviewerProvider currentPhaseId currentPhaseIndex error
      brief appScope docsScope referencePaths
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
      id planId phaseId eventType payloadJson createdAt
    }
  `;

  // ── Sessions ──────────────────────────────────────────────────────────

  listSessions(status?: string): Observable<AiSession[]> {
    return this.gql
      .query<{ listAiSessions: AiSession[] }>(
        `query ListSessions($status: String) {
          listAiSessions(status: $status) {
            id title provider model workingDirectory status
            totalInputTokens totalOutputTokens totalCostCents
            createdAt updatedAt
          }
        }`,
        { status }
      )
      .pipe(map((data) => data.listAiSessions));
  }

  getSession(id: string): Observable<AiSession> {
    return this.gql
      .query<{ getAiSession: AiSession }>(
        `query GetSession($id: ID!) {
          getAiSession(id: $id) {
            id title provider model workingDirectory status
            totalInputTokens totalOutputTokens totalCostCents
            createdAt updatedAt
          }
        }`,
        { id }
      )
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
    return this.gql
      .query<{ listAiMessages: AiMessage[] }>(
        `query ListMessages($sessionId: ID!, $limit: Int, $offset: Int) {
          listAiMessages(sessionId: $sessionId, limit: $limit, offset: $offset) {
            id sessionId role content toolCalls
            finishReason inputTokens outputTokens costCents createdAt
          }
        }`,
        { sessionId, limit, offset }
      )
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

  sendAiChatMessage(input: { sessionId: string; content: string }): Observable<AiChatRunResult> {
    return this.gql
      .mutate<{ sendAiChatMessage: AiChatRunResult }>(
        `mutation SendAiChatMessage($input: SendAiChatMessageInput!) {
          sendAiChatMessage(input: $input) {
            userMessage {
              id sessionId role content toolCalls
              finishReason inputTokens outputTokens costCents createdAt
            }
            assistantMessage {
              id sessionId role content toolCalls
              finishReason inputTokens outputTokens costCents createdAt
            }
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.sendAiChatMessage));
  }

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

  onAiChatDelta(sessionId: string): Observable<AiMessageDelta> {
    return this.gql
      .subscribe<{ onAiChatDelta: AiMessageDelta & { chunkType: string; isFinal: boolean } }>(
        `subscription OnAiChatDelta($sessionId: String!) {
          onAiChatDelta(sessionId: $sessionId) {
            sessionId
            messageId
            delta
            chunkType
            isFinal
          }
        }`,
        { sessionId }
      )
      .pipe(
        map((data) => ({
          sessionId: data.onAiChatDelta.sessionId,
          messageId: data.onAiChatDelta.messageId,
          delta: data.onAiChatDelta.delta,
          chunkType: data.onAiChatDelta.chunkType,
          isFinal: data.onAiChatDelta.isFinal,
          finishReason: data.onAiChatDelta.isFinal ? 'stop' : undefined,
        }))
      );
  }

  onAiChatComplete(sessionId: string): Observable<AiChatComplete> {
    return this.gql
      .subscribe<{ onAiChatComplete: AiChatComplete }>(
        `subscription OnAiChatComplete($sessionId: String!) {
          onAiChatComplete(sessionId: $sessionId) {
            sessionId
            messageId
          }
        }`,
        { sessionId }
      )
      .pipe(map((data) => data.onAiChatComplete));
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
    return this.gql
      .query<{ listDetectedCliTools: DetectedCliTool[] }>(
        `query ListDetectedCliTools {
          listDetectedCliTools {
            provider command found path
          }
        }`
      )
      .pipe(map((data) => data.listDetectedCliTools));
  }

  getSetting(key: string): Observable<string> {
    return this.gql
      .query<{ getSetting: string }>(
        `query GetSetting($key: String!) {
          getSetting(key: $key)
        }`,
        { key }
      )
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

  listAgentPlans(status?: string, runType?: 'planning' | 'development'): Observable<AgentPlan[]> {
    return this.gql
      .query<{ listAgentPlans: AgentPlan[] }>(
        `query ListAgentPlans($status: String, $runType: String) {
          listAgentPlans(status: $status, runType: $runType) {
            id runType title workspacePath planPath status workerSessionId reviewerSessionId
            workerProvider reviewerProvider currentPhaseId currentPhaseIndex error
            brief appScope docsScope referencePaths
            createdAt updatedAt
          }
        }`,
        { status, runType }
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

  startAgentPlan(id: string, phaseId?: string): Observable<AgentPlanRun> {
    return this.gql
      .mutate<{ startAgentPlan: AgentPlanRun }>(
        `mutation StartAgentPlan($id: ID!, $phaseId: String) {
          startAgentPlan(id: $id, phaseId: $phaseId) {
            ${this.agentPlanRunFields}
          }
        }`,
        { id, phaseId: phaseId || null }
      )
      .pipe(map((data) => data.startAgentPlan));
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

  listWorkspaceFiles(planId: string, mode: 'changed' | 'all'): Observable<HostFileEntry[]> {
    return this.gql
      .query<{ listWorkspaceFiles: HostFileEntry[] }>(
        `query ListWorkspaceFiles($planId: ID!, $mode: String!) {
          listWorkspaceFiles(planId: $planId, mode: $mode) {
            path name kind status size
          }
        }`,
        { planId, mode }
      )
      .pipe(map((data) => data.listWorkspaceFiles));
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
