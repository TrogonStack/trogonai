import type {
  WebhookDelivery,
  WebhookDraft,
  WebhookEndpoint,
  WebhookTarget,
  WebhookTestRequest,
} from './domain/webhook-endpoint';

export interface WebhookEndpointsClient {
  listWebhookEndpoints(): Promise<readonly WebhookEndpoint[]>;
  createWebhookEndpoint(draft: WebhookDraft): Promise<WebhookEndpoint>;
  updateWebhookEndpoint(target: WebhookTarget, draft: WebhookDraft): Promise<WebhookEndpoint>;
  setWebhookEnabled(target: WebhookTarget, enabled: boolean): Promise<WebhookEndpoint>;
  rotateWebhookCredential(target: WebhookTarget): Promise<WebhookEndpoint>;
  revokeWebhookCredential(target: WebhookTarget): Promise<WebhookEndpoint>;
  deleteWebhookEndpoint(target: WebhookTarget): Promise<void>;
  simulateWebhookDelivery(id: WebhookEndpoint['id'], request: WebhookTestRequest): Promise<WebhookDelivery>;
}
