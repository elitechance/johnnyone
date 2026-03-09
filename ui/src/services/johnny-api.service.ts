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

@Injectable({ providedIn: 'root' })
export class JohnnyApiService {
  private readonly gql = inject(GraphQLClient);

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
}
