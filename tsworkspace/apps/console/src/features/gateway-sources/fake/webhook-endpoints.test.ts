import { describe, expect, it } from 'vitest';

import { createFakeBackend } from '@/fake-backend';
import { mintProjectId } from '@/features/projects/domain/project-id';

import {
  PREVIEW_DUPLICATE_WINDOW_MS,
  webhookDraft,
  webhookSubject,
  type WebhookEndpoint,
} from '../domain/webhook-endpoint';
import { createFakeSourcesClient } from './index';
import { InMemoryWebhookEndpoints } from './webhook-endpoints';

const draft = { ...webhookDraft(), displayName: 'Orders' };
const target = (endpoint: WebhookEndpoint) => ({ id: endpoint.id, expectedRevision: endpoint.revision });

describe('generic webhook endpoint preview', () => {
  it('creates validated project-owned endpoints and refuses invalid limits at the client boundary', async () => {
    const client = new InMemoryWebhookEndpoints({ projectId: 'orders', seeded: false });
    const endpoint = await client.createWebhookEndpoint(draft);
    expect(endpoint).toMatchObject({
      projectId: 'orders',
      displayName: 'Orders',
      maxBodyBytes: 262144,
      authentication: { mode: 'hmac', timestampToleranceSeconds: 300 },
      credential: { status: 'active', version: 1 },
    });
    expect(webhookSubject(endpoint)).toBe(`webhooks.orders.${endpoint.id}.received`);
    for (const maxBodyBytes of ['0', '-1', '1.5', '1048577', '']) {
      await expect(client.createWebhookEndpoint({ ...draft, maxBodyBytes })).rejects.toThrow();
    }
    await expect(client.createWebhookEndpoint({ ...draft, timestampToleranceSeconds: '3601' })).rejects.toThrow();
    await expect(client.createWebhookEndpoint({ ...draft, displayName: '  ' })).rejects.toThrow();
    expect(await client.listWebhookEndpoints()).toHaveLength(1);
  });

  it('keeps endpoints and mutations isolated between project clients', async () => {
    const backend = createFakeBackend({ seeded: false });
    const orders = backend.gatewaySources.forProject(mintProjectId('orders'));
    const support = backend.gatewaySources.forProject(mintProjectId('support'));
    const endpoint = await orders.createWebhookEndpoint(draft);
    expect(await support.listWebhookEndpoints()).toHaveLength(0);
    await expect(support.setWebhookEnabled(target(endpoint), false)).rejects.toMatchObject({ reason: 'NOT_FOUND' });
    await expect(
      support.simulateWebhookDelivery(endpoint.id, { deliveryId: 'order-one', scenario: 'valid' }),
    ).rejects.toMatchObject({ reason: 'NOT_FOUND' });
    expect((await orders.listWebhookEndpoints())[0]?.enabled).toBe(true);
  });

  it('deduplicates only successfully admitted delivery identities within the bounded window', async () => {
    let now = new Date('2026-11-01T00:00:00Z');
    const client = new InMemoryWebhookEndpoints({ seeded: false, clock: () => now });
    const endpoint = await client.createWebhookEndpoint(draft);
    const request = { deliveryId: 'order-1042-created', scenario: 'valid' as const };
    expect(
      (await client.simulateWebhookDelivery(endpoint.id, { ...request, scenario: 'invalid_credential' })).outcome
        .status,
    ).toBe(401);
    expect((await client.simulateWebhookDelivery(endpoint.id, request)).outcome).toEqual({
      status: 202,
      kind: 'stored',
    });
    expect((await client.simulateWebhookDelivery(endpoint.id, request)).outcome).toEqual({
      status: 202,
      kind: 'duplicate',
    });
    expect(
      (await client.simulateWebhookDelivery(endpoint.id, { ...request, scenario: 'invalid_credential' })).outcome
        .status,
    ).toBe(401);
    now = new Date(now.getTime() + PREVIEW_DUPLICATE_WINDOW_MS);
    expect((await client.simulateWebhookDelivery(endpoint.id, request)).outcome.kind).toBe('stored');
  });

  it('models malformed IDs, signed freshness, body limits, pause and credential revocation refusals', async () => {
    const client = new InMemoryWebhookEndpoints({ seeded: false });
    let endpoint = await client.createWebhookEndpoint(draft);
    const send = (deliveryId: string, scenario: 'valid' | 'stale_timestamp' | 'oversized_body' = 'valid') =>
      client.simulateWebhookDelivery(endpoint.id, { deliveryId, scenario });
    for (const id of ['bad.id', '', 'space id', 'é', 'x'.repeat(129)])
      expect((await send(id)).outcome.status).toBe(400);
    expect((await send('stale', 'stale_timestamp')).outcome).toEqual({ status: 401, kind: 'stale_timestamp' });
    expect((await send('big', 'oversized_body')).outcome.status).toBe(413);
    endpoint = await client.setWebhookEnabled(target(endpoint), false);
    expect((await send('paused')).outcome.status).toBe(404);
    endpoint = await client.setWebhookEnabled(target(endpoint), true);
    endpoint = await client.revokeWebhookCredential(target(endpoint));
    expect((await send('revoked')).outcome).toEqual({ status: 401, kind: 'credential_revoked' });
    await expect(client.rotateWebhookCredential(target(endpoint))).rejects.toMatchObject({
      reason: 'CREDENTIAL_REVOKED',
    });
    expect((await client.listWebhookEndpoints())[0]?.recent).toHaveLength(9);
  });

  it('uses registry revisions for receipt metadata, independently of credential changes', async () => {
    const client = new InMemoryWebhookEndpoints({ seeded: false });
    let endpoint = await client.createWebhookEndpoint(draft);
    const initial = target(endpoint);
    endpoint = await client.rotateWebhookCredential(initial);
    expect(endpoint).toMatchObject({ revision: 2, configurationRevision: 1, credential: { version: 2 } });
    expect(
      (await client.simulateWebhookDelivery(endpoint.id, { deliveryId: 'first', scenario: 'valid' }))
        .configurationRevision,
    ).toBe(1);
    await expect(client.setWebhookEnabled(initial, false)).rejects.toMatchObject({ reason: 'REVISION_CONFLICT' });
    endpoint = await client.updateWebhookEndpoint(target(endpoint), { ...draft, maxBodyBytes: '1024' });
    expect(endpoint).toMatchObject({ revision: 3, configurationRevision: 2, maxBodyBytes: 1024 });
    expect(
      (await client.simulateWebhookDelivery(endpoint.id, { deliveryId: 'second', scenario: 'valid' }))
        .configurationRevision,
    ).toBe(2);
    endpoint = await client.revokeWebhookCredential(target(endpoint));
    expect(endpoint.configurationRevision).toBe(2);
    await client.deleteWebhookEndpoint(target(endpoint));
    expect(await client.listWebhookEndpoints()).toHaveLength(0);
  });

  it('applies the configured body cap to the example request, not only the oversize scenario', async () => {
    const client = new InMemoryWebhookEndpoints({ seeded: false });
    const endpoint = await client.createWebhookEndpoint({ ...draft, maxBodyBytes: '1' });
    expect(
      (await client.simulateWebhookDelivery(endpoint.id, { deliveryId: 'bounded', scenario: 'valid' })).outcome,
    ).toEqual({ status: 413, kind: 'body_too_large' });
  });

  it('supports bearer endpoints without signed timestamp tolerance and honors unseeded clients', async () => {
    const client = createFakeSourcesClient({ seeded: false });
    expect(await client.listWebhookEndpoints()).toHaveLength(0);
    const endpoint = await client.createWebhookEndpoint({
      ...draft,
      authentication: 'bearer',
      timestampToleranceSeconds: '',
    });
    expect(endpoint.authentication).toEqual({ mode: 'bearer' });
    expect(
      (await client.simulateWebhookDelivery(endpoint.id, { deliveryId: 'bearer-one', scenario: 'valid' })).outcome
        .status,
    ).toBe(202);
  });
});
