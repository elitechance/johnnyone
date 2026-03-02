import type { NormalizedMessage, NormalizedAttachment } from './normalize';

/**
 * Discord webhook/interaction payload types (subset).
 */
interface DiscordWebhookPayload {
  type: number; // 1 = PING, 2 = APPLICATION_COMMAND, etc.
  id: string;
  token: string;
  channel_id?: string;
  guild_id?: string;
  member?: {
    user: DiscordUser;
    nick?: string;
  };
  user?: DiscordUser;
  data?: {
    id: string;
    name: string;
    options?: Array<{
      name: string;
      value: string | number | boolean;
      type: number;
    }>;
  };
  // For message-based interactions
  message?: DiscordMessage;
}

interface DiscordMessage {
  id: string;
  channel_id: string;
  author: DiscordUser;
  content: string;
  timestamp: string;
  attachments?: DiscordAttachment[];
  embeds?: Array<{
    title?: string;
    description?: string;
    url?: string;
  }>;
}

interface DiscordUser {
  id: string;
  username: string;
  discriminator: string;
  global_name?: string;
  avatar?: string;
}

interface DiscordAttachment {
  id: string;
  filename: string;
  content_type?: string;
  size: number;
  url: string;
  proxy_url: string;
}

/**
 * Normalize a Discord webhook payload into a NormalizedMessage.
 * Supports both interaction-based and message-based payloads.
 */
export function normalizeDiscordMessage(payload: string): NormalizedMessage {
  const data: DiscordWebhookPayload = JSON.parse(payload);

  // Handle direct message payloads
  if (data.message) {
    const msg = data.message;
    const attachments: NormalizedAttachment[] = (msg.attachments ?? []).map((att) => ({
      type: inferAttachmentType(att.content_type),
      url: att.url,
      filename: att.filename,
      mimeType: att.content_type,
      size: att.size,
    }));

    return {
      text: msg.content,
      channelIdentifier: msg.channel_id,
      senderId: msg.author.id,
      senderName: msg.author.global_name ?? msg.author.username,
      timestamp: msg.timestamp,
      attachments: attachments.length > 0 ? attachments : undefined,
      externalMessageId: msg.id,
      rawPayload: data,
    };
  }

  // Handle slash command interactions
  if (data.data) {
    const user = data.member?.user ?? data.user;
    const optionsText = (data.data.options ?? [])
      .map((opt) => `${opt.name}: ${opt.value}`)
      .join(', ');

    const text = optionsText || `/${data.data.name}`;

    return {
      text,
      channelIdentifier: data.channel_id ?? data.guild_id ?? '',
      senderId: user?.id ?? '',
      senderName: user?.global_name ?? user?.username,
      externalMessageId: data.id,
      rawPayload: data,
    };
  }

  throw new Error('Unsupported Discord payload type');
}

function inferAttachmentType(
  contentType?: string,
): 'image' | 'file' | 'audio' | 'video' | 'sticker' {
  if (!contentType) return 'file';
  if (contentType.startsWith('image/')) return 'image';
  if (contentType.startsWith('audio/')) return 'audio';
  if (contentType.startsWith('video/')) return 'video';
  return 'file';
}
