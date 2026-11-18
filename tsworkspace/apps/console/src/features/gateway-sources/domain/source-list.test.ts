import { describe, expect, it } from 'vitest';

import { createFakeSourcesClient } from '../fake';
import { providerSourceRow, webhookSourceRow } from './source-list';
import { summarizeSources, toSourceView } from './view';
import { webhookDraft } from './webhook-endpoint';

describe('unified source list projection', () => {
  it('combines provider and generic endpoint counts and status without merging their domain identities', async () => {
    const client = createFakeSourcesClient();
    const providers = (await client.list()).map(toSourceView);
    const endpoint = (await client.listWebhookEndpoints())[0]!;
    const rows = [...providers.map(providerSourceRow), webhookSourceRow(endpoint, new Date())];
    const before = summarizeSources(providers);
    expect(summarizeSources(rows)).toEqual({ ...before, total: before.total + 1, receiving: before.receiving + 1 });
    expect(rows.find((row) => row.family === 'webhook')).toMatchObject({
      kind: 'webhook',
      id: endpoint.id,
      status: 'receiving',
    });
    const revoked = await client.revokeWebhookCredential({ id: endpoint.id, expectedRevision: endpoint.revision });
    expect(webhookSourceRow(revoked, new Date()).status).toBe('misconfigured');
  });

  it('keeps rolling activity totals independent of the bounded inspection history and expires them', async () => {
    let now = new Date('2026-11-17T12:00:00Z');
    const client = createFakeSourcesClient({ seeded: false, clock: () => now });
    const endpoint = await client.createWebhookEndpoint({ ...webhookDraft(), displayName: 'Orders' });
    for (let index = 0; index < 26; index += 1) {
      await client.simulateWebhookDelivery(endpoint.id, {
        deliveryId: `order-${index}`,
        scenario: index % 2 ? 'invalid_credential' : 'valid',
      });
    }
    const current = (await client.listWebhookEndpoints())[0]!;
    expect(current.recent).toHaveLength(20);
    const row = webhookSourceRow(current, now);
    expect(row).toMatchObject({ deliveries24h: 26, rejected24h: 13, lastDeliveryAt: now });
    expect(row.hourly.reduce((total, hour) => total + hour.accepted + hour.rejected, 0)).toBe(26);
    now = new Date(now.getTime() + 24 * 60 * 60 * 1_000);
    expect(webhookSourceRow(current, now)).toMatchObject({
      deliveries24h: 0,
      rejected24h: 0,
      lastDeliveryAt: current.recent[0]?.receivedAt,
    });
  });
});
