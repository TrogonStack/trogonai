import { mintProjectId, type ProjectId } from '@/features/projects/domain/project-id';

import {
  PREVIEW_DUPLICATE_WINDOW_MS,
  WEBHOOK_EXAMPLE_BODY,
  WebhookEndpointFault,
  endpointId,
  isDeliveryId,
  webhookConfiguration,
  type WebhookDelivery,
  type WebhookDeliveryOutcome,
  type WebhookDraft,
  type WebhookEndpoint,
  type WebhookEndpointId,
  type WebhookTarget,
  type WebhookTestRequest,
} from '../domain/webhook-endpoint';
import type { WebhookEndpointsClient } from '../webhook-client';

export const PREVIEW_WEBHOOK_ENDPOINT_ID = endpointId('b6ea9b9d-6dbb-4c39-8d37-6ad87c522b01');

export class InMemoryWebhookEndpoints implements WebhookEndpointsClient {
  private readonly endpoints = new Map<WebhookEndpointId, WebhookEndpoint>();
  private readonly accepted = new Map<WebhookEndpointId, Map<string, number>>();
  private readonly projectId: ProjectId;
  private readonly clock: () => Date;

  constructor(options: { projectId?: string; seeded?: boolean; clock?: () => Date } = {}) {
    this.projectId = mintProjectId(options.projectId ?? 'proj-demo');
    this.clock = options.clock ?? (() => new Date());
    if (options.seeded ?? true) {
      this.endpoints.set(
        PREVIEW_WEBHOOK_ENDPOINT_ID,
        this.newEndpoint(PREVIEW_WEBHOOK_ENDPOINT_ID, {
          displayName: 'Orders service',
          authentication: 'hmac',
          maxBodyBytes: '262144',
          timestampToleranceSeconds: '300',
        }),
      );
    }
  }

  private newEndpoint(id: WebhookEndpointId, draft: WebhookDraft): WebhookEndpoint {
    return {
      ...webhookConfiguration(draft),
      id,
      projectId: this.projectId,
      revision: 1,
      configurationRevision: 1,
      enabled: true,
      credential: { status: 'active', version: 1 },
      createdAt: this.clock(),
      recent: [],
      activity: [],
    };
  }

  private existing(id: WebhookEndpointId): WebhookEndpoint {
    const endpoint = this.endpoints.get(id);
    if (!endpoint) throw new WebhookEndpointFault('NOT_FOUND');
    return endpoint;
  }

  private target(target: WebhookTarget): WebhookEndpoint {
    const endpoint = this.existing(target.id);
    if (endpoint.revision !== target.expectedRevision) throw new WebhookEndpointFault('REVISION_CONFLICT');
    return endpoint;
  }

  private save(endpoint: WebhookEndpoint, changes: Partial<WebhookEndpoint>): WebhookEndpoint {
    const next = { ...endpoint, ...changes, revision: endpoint.revision + 1 };
    this.endpoints.set(next.id, next);
    return next;
  }

  async listWebhookEndpoints(): Promise<readonly WebhookEndpoint[]> {
    return [...this.endpoints.values()];
  }

  async createWebhookEndpoint(draft: WebhookDraft): Promise<WebhookEndpoint> {
    const endpoint = this.newEndpoint(endpointId(crypto.randomUUID()), draft);
    this.endpoints.set(endpoint.id, endpoint);
    return endpoint;
  }

  async updateWebhookEndpoint(target: WebhookTarget, draft: WebhookDraft): Promise<WebhookEndpoint> {
    const endpoint = this.target(target);
    const configuration = webhookConfiguration(draft);
    if (configuration.authentication.mode !== endpoint.authentication.mode)
      throw new WebhookEndpointFault('AUTH_MODE_IMMUTABLE');
    return this.save(endpoint, { ...configuration, configurationRevision: endpoint.configurationRevision + 1 });
  }

  async setWebhookEnabled(target: WebhookTarget, enabled: boolean): Promise<WebhookEndpoint> {
    const endpoint = this.target(target);
    return this.save(endpoint, { enabled, configurationRevision: endpoint.configurationRevision + 1 });
  }

  async rotateWebhookCredential(target: WebhookTarget): Promise<WebhookEndpoint> {
    const endpoint = this.target(target);
    if (endpoint.credential.status === 'revoked') throw new WebhookEndpointFault('CREDENTIAL_REVOKED');
    return this.save(endpoint, { credential: { status: 'active', version: endpoint.credential.version + 1 } });
  }

  async revokeWebhookCredential(target: WebhookTarget): Promise<WebhookEndpoint> {
    const endpoint = this.target(target);
    return this.save(endpoint, { credential: { ...endpoint.credential, status: 'revoked' } });
  }

  async deleteWebhookEndpoint(target: WebhookTarget): Promise<void> {
    this.target(target);
    this.endpoints.delete(target.id);
    this.accepted.delete(target.id);
  }

  async simulateWebhookDelivery(id: WebhookEndpointId, request: WebhookTestRequest): Promise<WebhookDelivery> {
    const endpoint = this.existing(id);
    const receivedAt = this.clock();
    const identities = this.accepted.get(id) ?? new Map<string, number>();
    for (const [deliveryId, at] of identities) {
      if (receivedAt.getTime() - at >= PREVIEW_DUPLICATE_WINDOW_MS) identities.delete(deliveryId);
    }
    let outcome: WebhookDeliveryOutcome;
    if (!endpoint.enabled) outcome = { status: 404, kind: 'endpoint_paused' };
    else if (!isDeliveryId(request.deliveryId)) outcome = { status: 400, kind: 'malformed_delivery_id' };
    else if (endpoint.credential.status === 'revoked') outcome = { status: 401, kind: 'credential_revoked' };
    else if (
      request.scenario === 'oversized_body' ||
      new TextEncoder().encode(WEBHOOK_EXAMPLE_BODY).byteLength > endpoint.maxBodyBytes
    )
      outcome = { status: 413, kind: 'body_too_large' };
    else if (request.scenario === 'invalid_credential') outcome = { status: 401, kind: 'invalid_credential' };
    else if (request.scenario === 'stale_timestamp' && endpoint.authentication.mode === 'hmac')
      outcome = { status: 401, kind: 'stale_timestamp' };
    else if (identities.has(request.deliveryId)) outcome = { status: 202, kind: 'duplicate' };
    else {
      outcome = { status: 202, kind: 'stored' };
      identities.set(request.deliveryId, receivedAt.getTime());
    }
    this.accepted.set(id, identities);
    const delivery: WebhookDelivery = {
      attemptId: crypto.randomUUID(),
      deliveryId: request.deliveryId,
      receivedAt,
      configurationRevision: endpoint.configurationRevision,
      outcome,
    };
    const cutoff = receivedAt.getTime() - 24 * 60 * 60 * 1_000;
    const activity = endpoint.activity.filter((entry) => entry.receivedAt.getTime() > cutoff);
    const accepted = outcome.status === 202;
    const sameInstant = activity.findIndex(
      (entry) => entry.receivedAt.getTime() === receivedAt.getTime() && entry.accepted === accepted,
    );
    if (sameInstant >= 0) {
      const entry = activity[sameInstant]!;
      activity[sameInstant] = { ...entry, count: entry.count + 1 };
    } else activity.push({ receivedAt, accepted, count: 1 });
    this.endpoints.set(id, { ...endpoint, recent: [delivery, ...endpoint.recent].slice(0, 20), activity });
    return delivery;
  }
}
