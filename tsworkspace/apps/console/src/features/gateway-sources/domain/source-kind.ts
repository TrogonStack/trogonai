export const sourceKinds = [
  'github',
  'gitlab',
  'slack',
  'discord',
  'telegram',
  'linear',
  'notion',
  'sentry',
  'datadog',
  'incidentio',
  'microsoft_graph',
  'google_pubsub',
  'twitter',
  'zoom',
] as const;

export type SourceKind = (typeof sourceKinds)[number];

export type SlotName =
  | 'webhook_secret'
  | 'signing_secret'
  | 'signing_token'
  | 'bot_token'
  | 'public_key'
  | 'client_secret'
  | 'verification_token';

export const sourceKindLabels: Record<SourceKind, string> = {
  github: 'GitHub',
  gitlab: 'GitLab',
  slack: 'Slack',
  discord: 'Discord',
  telegram: 'Telegram',
  linear: 'Linear',
  notion: 'Notion',
  sentry: 'Sentry',
  datadog: 'Datadog',
  incidentio: 'incident.io',
  microsoft_graph: 'Microsoft Graph',
  google_pubsub: 'Google Pub/Sub',
  twitter: 'Twitter',
  zoom: 'Zoom',
};

export const slotLabels: Record<SlotName, string> = {
  webhook_secret: 'Webhook secret',
  signing_secret: 'Signing secret',
  signing_token: 'Signing token',
  bot_token: 'Bot token',
  public_key: 'Public key',
  client_secret: 'Client secret',
  verification_token: 'Verification token',
};

export const kindSlots: Record<SourceKind, readonly SlotName[]> = {
  github: ['webhook_secret'],
  gitlab: ['signing_token'],
  slack: ['signing_secret', 'bot_token'],
  discord: ['bot_token', 'public_key'],
  telegram: ['webhook_secret', 'bot_token'],
  linear: ['webhook_secret'],
  notion: ['verification_token'],
  sentry: ['client_secret'],
  datadog: ['webhook_secret'],
  incidentio: ['signing_secret'],
  microsoft_graph: ['client_secret'],
  google_pubsub: ['verification_token'],
  twitter: ['client_secret'],
  zoom: ['verification_token'],
};

export const kindEvents: Record<SourceKind, readonly string[]> = {
  github: ['push', 'pull_request', 'issues', 'check_run'],
  gitlab: ['push', 'merge_request', 'pipeline'],
  slack: ['message', 'app_mention', 'reaction_added'],
  discord: ['interaction', 'message_create'],
  telegram: ['message', 'callback_query'],
  linear: ['issue.update', 'comment.create'],
  notion: ['page.updated', 'comment.created'],
  sentry: ['issue.created', 'alert.triggered'],
  datadog: ['monitor.alert', 'monitor.recovered'],
  incidentio: ['incident.created', 'incident.updated'],
  microsoft_graph: ['mail.received', 'calendar.updated'],
  google_pubsub: ['message.published'],
  twitter: ['tweet_create', 'direct_message'],
  zoom: ['meeting.started', 'meeting.ended'],
};

export function isSourceKind(value: string): value is SourceKind {
  return (sourceKinds as readonly string[]).includes(value);
}
