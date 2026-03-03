import { Injectable, inject } from '@angular/core';
import { Observable } from 'rxjs';
import { map } from 'rxjs/operators';
import { GraphQLClient } from './graphql-client';
import { AiSession } from '../models/ai-session.model';
import { AiMessage, AiMessageDelta } from '../models/ai-message.model';
import { ToolDefinition, ToolExecution } from '../models/tool.model';
import { ProviderConfig, AiUsageSummary } from '../models/provider.model';
import { DesktopNode } from '../models/desktop-node.model';

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
        `mutation UpsertProviderConfig($input: ProviderConfigInput!) {
          upsertProviderConfig(input: $input) {
            id provider model apiKeyRef baseUrl isDefault settings
          }
        }`,
        { input }
      )
      .pipe(map((data) => data.upsertProviderConfig));
  }

  deleteProviderConfig(id: string): Observable<boolean> {
    return this.gql
      .mutate<{ deleteProviderConfig: boolean }>(
        `mutation DeleteProviderConfig($id: ID!) {
          deleteProviderConfig(id: $id)
        }`,
        { id }
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
