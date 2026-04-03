export type ChatChannelSupportLevel = 'Built-in' | 'Plugin' | 'Legacy';

export interface ChatChannelSupport {
  id: string;
  name: string;
  supportLevel: ChatChannelSupportLevel;
  summary: string;
  details?: string;
  recommended?: boolean;
}

export const CHAT_CHANNEL_SUPPORT: ChatChannelSupport[] = [
  {
    id: 'discord',
    name: 'Discord',
    supportLevel: 'Built-in',
    summary: 'Discord Bot API + Gateway for servers, channels, and direct messages.',
  },
  {
    id: 'irc',
    name: 'IRC',
    supportLevel: 'Built-in',
    summary: 'Classic IRC channels and DMs with pairing and allowlist controls.',
  },
  {
    id: 'slack',
    name: 'Slack',
    supportLevel: 'Built-in',
    summary: 'Slack workspace apps powered by Bolt SDK.',
  },
  {
    id: 'webchat',
    name: 'WebChat',
    supportLevel: 'Built-in',
    summary: 'Gateway WebChat UI over WebSocket for browser-based sessions.',
  },
  {
    id: 'notion',
    name: 'Notion',
    supportLevel: 'Built-in',
    summary: 'Polls a Notion database for pending tasks and writes results back via the Notion API.',
  },
];

export const CHAT_CHANNEL_NOTES: string[] = [
  'Channels can run simultaneously; configure multiple and ZeroClaw routes per chat.',
  'Group behavior varies by channel. See docs/channels-reference.md for policy details.',
  'DM pairing and allowlists are enforced for safety. See docs/security/README.md.',
  'Troubleshooting lives in docs/troubleshooting.md under channel guidance.',
  'Model providers are documented separately in docs/providers-reference.md.',
];
