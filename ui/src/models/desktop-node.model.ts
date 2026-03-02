export interface DesktopNode {
  id: string;
  hostname: string;
  os: string;
  arch: string;
  version: string;
  status: 'online' | 'offline' | 'busy';
  capabilities: string[];
  lastHeartbeatAt?: string;
  createdAt: string;
}

export interface ChannelBinding {
  id: string;
  channelType: 'telegram' | 'discord' | 'whatsapp';
  channelIdentifier: string;
  isActive: boolean;
}
