/** Message envelope sent over WebSocket between Worker and Desktop Agent */
export type WsMessage =
  | WsToolCallMessage
  | WsToolResultMessage
  | WsHeartbeatMessage
  | WsHeartbeatAckMessage
  | WsSessionStartMessage
  | WsSessionEndMessage
  | WsErrorMessage;

export interface WsEnvelope {
  id: string;
  timestamp: string;
}

export interface WsToolCallMessage extends WsEnvelope {
  type: 'tool_call';
  payload: {
    sessionId: string;
    messageId: string;
    executionId: string;
    tool: string;
    input: Record<string, unknown>;
    timeout?: number;
  };
}

export interface WsToolResultMessage extends WsEnvelope {
  type: 'tool_result';
  payload: {
    executionId: string;
    status: 'completed' | 'failed';
    output?: string;
    error?: string;
    durationMs: number;
  };
}

export interface WsHeartbeatMessage extends WsEnvelope {
  type: 'heartbeat';
  payload: {
    nodeId: string;
    hostname: string;
    os: string;
    cpuUsage: number;
    memoryUsage: number;
    activeExecutions: number;
  };
}

export interface WsHeartbeatAckMessage extends WsEnvelope {
  type: 'heartbeat_ack';
  payload: {
    serverTime: string;
  };
}

export interface WsSessionStartMessage extends WsEnvelope {
  type: 'session_start';
  payload: {
    sessionId: string;
    userId: string;
  };
}

export interface WsSessionEndMessage extends WsEnvelope {
  type: 'session_end';
  payload: {
    sessionId: string;
    reason: 'completed' | 'archived' | 'timeout';
  };
}

export interface WsErrorMessage extends WsEnvelope {
  type: 'error';
  payload: {
    code: string;
    message: string;
    executionId?: string;
  };
}
