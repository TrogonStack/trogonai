import type { CustodyTier } from '@/features/custody/domain/custody';

import type { HourlyDeliveries } from './delivery';
import { sourceKindLabels, type SourceKind } from './source-kind';
import type { IntegrationStatus, SourceView } from './view';
import { webhookSubject, type WebhookEndpoint } from './webhook-endpoint';

export type SourceListKind = SourceKind | 'webhook';
export const sourceListKindLabels: Record<SourceListKind, string> = { ...sourceKindLabels, webhook: 'Generic webhook' };

interface SourceListFields {
  readonly displayName: string;
  readonly status: IntegrationStatus;
  readonly subjectPrefix: string;
  readonly credentialSummary: string;
  readonly credentialDetail: string;
  readonly tier: CustodyTier;
  readonly lastDeliveryAt: Date | undefined;
  readonly deliveries24h: number;
  readonly rejected24h: number;
  readonly hourly: readonly HourlyDeliveries[];
}

export type SourceListRow = SourceListFields &
  (
    | { readonly family: 'provider'; readonly kind: SourceKind; readonly id: SourceView['id'] }
    | { readonly family: 'webhook'; readonly kind: 'webhook'; readonly id: WebhookEndpoint['id'] }
  );

export function providerSourceRow(source: SourceView): SourceListRow {
  return {
    ...source,
    family: 'provider',
    credentialDetail: `${source.slots.length} ${source.slots.length === 1 ? 'slot' : 'slots'}`,
  };
}

export function webhookSourceRow(endpoint: WebhookEndpoint, now: Date): SourceListRow {
  const hour = 60 * 60 * 1_000;
  const cutoff = now.getTime() - 24 * hour;
  const attempts = endpoint.activity.filter((entry) => entry.receivedAt.getTime() > cutoff && entry.receivedAt <= now);
  const hourly: { accepted: number; rejected: number }[] = Array.from({ length: 24 }, () => ({
    accepted: 0,
    rejected: 0,
  }));
  for (const attempt of attempts) {
    const index = Math.min(23, Math.floor((attempt.receivedAt.getTime() - cutoff) / hour));
    const bucket = hourly[index];
    if (bucket) bucket[attempt.accepted ? 'accepted' : 'rejected'] += attempt.count;
  }
  return {
    family: 'webhook',
    kind: 'webhook',
    id: endpoint.id,
    displayName: endpoint.displayName,
    status: !endpoint.enabled ? 'paused' : endpoint.credential.status === 'revoked' ? 'misconfigured' : 'receiving',
    subjectPrefix: webhookSubject(endpoint),
    tier: 'managed',
    credentialSummary: endpoint.credential.status === 'active' ? '1 runtime' : 'Revoked',
    credentialDetail: `Webhook secret · v${endpoint.credential.version}`,
    lastDeliveryAt: endpoint.recent[0]?.receivedAt,
    deliveries24h: attempts.reduce((sum, attempt) => sum + attempt.count, 0),
    rejected24h: attempts.reduce((sum, attempt) => sum + (attempt.accepted ? 0 : attempt.count), 0),
    hourly,
  };
}
