import type { DispatchOptions, SourcesClient } from '../client';
import type { SourceCommand } from '../domain/commands';
import { SourceFault } from '../domain/faults';
import { decideSource, settleSource, type IntegrationState } from '../domain/integration';
import { parseIntegrationId } from '../domain/integration-id';
import type { WebhookDraft, WebhookEndpoint, WebhookTarget, WebhookTestRequest } from '../domain/webhook-endpoint';
import { InMemoryWebhookEndpoints } from './webhook-endpoints';

export const DEMO_PROJECT = 'proj-demo';
const SETTLE_LATENCY_FACTOR = 6;

export interface InMemoryClientOptions {
  readonly seeded?: boolean;
  readonly latencyMs?: number;
  readonly settleMs?: number;
  readonly clock?: () => Date;
  readonly projectId?: string;
}

function sleep(milliseconds: number): Promise<void> {
  return milliseconds > 0 ? new Promise((resolve) => setTimeout(resolve, milliseconds)) : Promise.resolve();
}

export class InMemorySourcesClient implements SourcesClient {
  private readonly endpoints: InMemoryWebhookEndpoints;
  private readonly integrations = new Map<string, IntegrationState>();
  private readonly latencyMs: number;
  private readonly settleMs: number;
  private readonly clock: () => Date;
  private seed: () => readonly IntegrationState[] = () => [];
  private ready: Promise<void> | undefined;

  constructor(options: InMemoryClientOptions = {}) {
    this.endpoints = new InMemoryWebhookEndpoints(options);
    this.latencyMs = options.latencyMs ?? 0;
    this.settleMs = options.settleMs ?? this.latencyMs * SETTLE_LATENCY_FACTOR;
    this.clock = options.clock ?? (() => new Date());
  }

  listWebhookEndpoints() {
    return this.endpoints.listWebhookEndpoints();
  }
  createWebhookEndpoint(draft: WebhookDraft) {
    return this.endpoints.createWebhookEndpoint(draft);
  }
  updateWebhookEndpoint(target: WebhookTarget, draft: WebhookDraft) {
    return this.endpoints.updateWebhookEndpoint(target, draft);
  }
  setWebhookEnabled(target: WebhookTarget, enabled: boolean) {
    return this.endpoints.setWebhookEnabled(target, enabled);
  }
  rotateWebhookCredential(target: WebhookTarget) {
    return this.endpoints.rotateWebhookCredential(target);
  }
  revokeWebhookCredential(target: WebhookTarget) {
    return this.endpoints.revokeWebhookCredential(target);
  }
  deleteWebhookEndpoint(target: WebhookTarget) {
    return this.endpoints.deleteWebhookEndpoint(target);
  }
  simulateWebhookDelivery(id: WebhookEndpoint['id'], request: WebhookTestRequest) {
    return this.endpoints.simulateWebhookDelivery(id, request);
  }

  seedWith(populate: () => readonly IntegrationState[]): void {
    this.seed = populate;
  }

  private async pause(): Promise<void> {
    this.ready ??= Promise.resolve().then(() => {
      for (const state of this.seed()) this.integrations.set(state.id, state);
    });
    await this.ready;
    await sleep(this.latencyMs);
    this.settleDue();
  }

  private settleDue(): void {
    const now = this.clock();
    for (const [id, state] of this.integrations) {
      if (state.settlement && state.settlement.at <= now) this.integrations.set(id, settleSource(state));
    }
  }

  async list(): Promise<readonly IntegrationState[]> {
    await this.pause();
    return [...this.integrations.values()];
  }

  async get(integrationId: string): Promise<IntegrationState | undefined> {
    await this.pause();
    return this.integrations.get(integrationId);
  }

  async dispatch(command: SourceCommand, options: DispatchOptions = {}): Promise<IntegrationState> {
    await this.pause();
    if (!parseIntegrationId(command.integrationId)) throw new SourceFault('COMMAND_INVALID');
    const state = this.integrations.get(command.integrationId);
    if (!state) throw new SourceFault('INTEGRATION_NOT_FOUND');
    if (options.expectedRevision === undefined || options.expectedRevision < 1) {
      throw new SourceFault('COMMAND_INVALID', 'Commands on an existing integration require an expected revision.');
    }
    if (options.expectedRevision !== state.revision) {
      throw new SourceFault(
        'REVISION_CONFLICT',
        `The integration is at revision ${state.revision}, not ${options.expectedRevision}.`,
      );
    }
    const settleAt = new Date(this.clock().getTime() + this.settleMs);
    const next = decideSource(state, command, settleAt);
    this.integrations.set(next.id, next);
    return next;
  }
}
