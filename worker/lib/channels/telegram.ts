import type { NormalizedMessage, NormalizedAttachment } from './normalize';

/**
 * Telegram webhook payload types (subset of Telegram Bot API).
 */
interface TelegramUpdate {
  update_id: number;
  message?: TelegramMessage;
  edited_message?: TelegramMessage;
  callback_query?: {
    id: string;
    from: TelegramUser;
    message?: TelegramMessage;
    data?: string;
  };
}

interface TelegramMessage {
  message_id: number;
  from?: TelegramUser;
  chat: {
    id: number;
    type: string;
    title?: string;
    username?: string;
  };
  date: number;
  text?: string;
  caption?: string;
  photo?: Array<{
    file_id: string;
    file_unique_id: string;
    width: number;
    height: number;
    file_size?: number;
  }>;
  document?: {
    file_id: string;
    file_unique_id: string;
    file_name?: string;
    mime_type?: string;
    file_size?: number;
  };
  voice?: {
    file_id: string;
    duration: number;
    mime_type?: string;
    file_size?: number;
  };
  audio?: {
    file_id: string;
    duration: number;
    file_name?: string;
    mime_type?: string;
    file_size?: number;
  };
  sticker?: {
    file_id: string;
    type: string;
    emoji?: string;
  };
  reply_to_message?: TelegramMessage;
}

interface TelegramUser {
  id: number;
  is_bot: boolean;
  first_name: string;
  last_name?: string;
  username?: string;
}

/**
 * Normalize a Telegram webhook update into a NormalizedMessage.
 */
export function normalizeTelegramMessage(payload: string): NormalizedMessage {
  const update: TelegramUpdate = JSON.parse(payload);

  const message = update.message ?? update.edited_message;
  if (!message) {
    // Handle callback queries
    if (update.callback_query) {
      return {
        text: update.callback_query.data ?? '',
        channelIdentifier: String(update.callback_query.message?.chat.id ?? ''),
        senderId: String(update.callback_query.from.id),
        senderName: formatTelegramName(update.callback_query.from),
        externalMessageId: String(update.callback_query.message?.message_id ?? ''),
        rawPayload: update,
      };
    }
    throw new Error('Unsupported Telegram update type: no message or callback_query');
  }

  const text = message.text ?? message.caption ?? '';
  const attachments: NormalizedAttachment[] = [];

  // Handle photos (pick the largest resolution)
  if (message.photo && message.photo.length > 0) {
    const largest = message.photo[message.photo.length - 1];
    attachments.push({
      type: 'image',
      url: `telegram:file:${largest.file_id}`,
      size: largest.file_size,
    });
  }

  // Handle documents
  if (message.document) {
    attachments.push({
      type: 'file',
      url: `telegram:file:${message.document.file_id}`,
      filename: message.document.file_name,
      mimeType: message.document.mime_type,
      size: message.document.file_size,
    });
  }

  // Handle voice messages
  if (message.voice) {
    attachments.push({
      type: 'audio',
      url: `telegram:file:${message.voice.file_id}`,
      mimeType: message.voice.mime_type,
      size: message.voice.file_size,
    });
  }

  // Handle audio files
  if (message.audio) {
    attachments.push({
      type: 'audio',
      url: `telegram:file:${message.audio.file_id}`,
      filename: message.audio.file_name,
      mimeType: message.audio.mime_type,
      size: message.audio.file_size,
    });
  }

  // Handle stickers
  if (message.sticker) {
    attachments.push({
      type: 'sticker',
      url: `telegram:file:${message.sticker.file_id}`,
    });
  }

  return {
    text,
    channelIdentifier: String(message.chat.id),
    senderId: String(message.from?.id ?? 0),
    senderName: message.from ? formatTelegramName(message.from) : undefined,
    timestamp: new Date(message.date * 1000).toISOString(),
    attachments: attachments.length > 0 ? attachments : undefined,
    externalMessageId: String(message.message_id),
    rawPayload: update,
  };
}

function formatTelegramName(user: TelegramUser): string {
  const parts = [user.first_name];
  if (user.last_name) parts.push(user.last_name);
  return parts.join(' ');
}
