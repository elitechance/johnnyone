/**
 * Message normalization interface for multi-channel support.
 *
 * Each channel adapter converts its native message format into
 * a NormalizedMessage that the agent system can process uniformly.
 */

export interface NormalizedMessage {
  /** The text content of the message */
  text: string;
  /** Channel-specific identifier for the conversation/chat */
  channelIdentifier: string;
  /** Channel-specific sender identifier */
  senderId: string;
  /** Sender display name, if available */
  senderName?: string;
  /** Message timestamp from the channel */
  timestamp?: string;
  /** Any attachments (URLs to files, images, etc.) */
  attachments?: NormalizedAttachment[];
  /** Channel-specific message ID for reply threading */
  externalMessageId?: string;
  /** Raw payload for debugging */
  rawPayload?: unknown;
}

export interface NormalizedAttachment {
  type: 'image' | 'file' | 'audio' | 'video' | 'sticker';
  url?: string;
  filename?: string;
  mimeType?: string;
  size?: number;
}

export type ChannelNormalizer = (payload: string) => NormalizedMessage;

const normalizers: Record<string, ChannelNormalizer> = {};

/**
 * Register a channel normalizer.
 */
export function registerNormalizer(channelType: string, normalizer: ChannelNormalizer): void {
  normalizers[channelType] = normalizer;
}

/**
 * Normalize an incoming channel message into a standard format.
 */
export function normalizeMessage(channelType: string, payload: string): NormalizedMessage {
  const normalizer = normalizers[channelType];
  if (!normalizer) {
    throw new Error(`No normalizer registered for channel type: ${channelType}`);
  }
  return normalizer(payload);
}

// Register built-in normalizers
import { normalizeTelegramMessage } from './telegram';
import { normalizeDiscordMessage } from './discord';
import { normalizeWhatsAppMessage } from './whatsapp';

registerNormalizer('telegram', normalizeTelegramMessage);
registerNormalizer('discord', normalizeDiscordMessage);
registerNormalizer('whatsapp', normalizeWhatsAppMessage);
