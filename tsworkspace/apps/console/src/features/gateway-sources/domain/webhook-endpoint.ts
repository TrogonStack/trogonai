import { z } from 'zod';

import type { ProjectId } from '@/features/projects/domain/project-id';

declare const endpointIdBrand: unique symbol;
export type WebhookEndpointId = string & { readonly [endpointIdBrand]: true };

export const MAX_WEBHOOK_BODY_BYTES = 1_048_576;
export const PREVIEW_GATEWAY_ORIGIN = 'https://gateway.example.com';
export const WEBHOOK_EXAMPLE_BODY = '{"event":"order.created","order_id":"order-1042"}';
export const PREVIEW_DUPLICATE_WINDOW_MS = 7 * 24 * 60 * 60 * 1_000;

export const webhookDraftSchema = z
  .object({
    displayName: z.string().trim().min(1, 'Enter an endpoint name.').max(80, 'Use at most 80 characters.'),
    authentication: z.enum(['hmac', 'bearer']),
    maxBodyBytes: z.string().refine((value) => boundedInteger(value, MAX_WEBHOOK_BODY_BYTES), {
      message: 'Enter a whole number from 1 to 1048576 bytes.',
    }),
    timestampToleranceSeconds: z.string(),
  })
  .superRefine((draft, context) => {
    if (draft.authentication === 'hmac' && !boundedInteger(draft.timestampToleranceSeconds, 3_600)) {
      context.addIssue({
        code: 'custom',
        path: ['timestampToleranceSeconds'],
        message: 'Enter a whole number from 1 to 3600 seconds.',
      });
    }
  });

function boundedInteger(value: string, maximum: number): boolean {
  return /^\d+$/.test(value) && Number(value) >= 1 && Number(value) <= maximum;
}

export type WebhookDraft = z.input<typeof webhookDraftSchema>;
export type WebhookAuthentication =
  | { readonly mode: 'hmac'; readonly timestampToleranceSeconds: number }
  | { readonly mode: 'bearer' };

export interface WebhookConfiguration {
  readonly displayName: string;
  readonly authentication: WebhookAuthentication;
  readonly maxBodyBytes: number;
}

export type WebhookDeliveryOutcome =
  | { readonly status: 202; readonly kind: 'stored' | 'duplicate' }
  | { readonly status: 400; readonly kind: 'malformed_delivery_id' }
  | { readonly status: 401; readonly kind: 'invalid_credential' | 'stale_timestamp' | 'credential_revoked' }
  | { readonly status: 404; readonly kind: 'endpoint_paused' }
  | { readonly status: 413; readonly kind: 'body_too_large' };

export interface WebhookDelivery {
  readonly attemptId: string;
  readonly deliveryId: string;
  readonly receivedAt: Date;
  readonly configurationRevision: number;
  readonly outcome: WebhookDeliveryOutcome;
}

export interface WebhookEndpoint extends WebhookConfiguration {
  readonly id: WebhookEndpointId;
  readonly projectId: ProjectId;
  readonly revision: number;
  readonly configurationRevision: number;
  readonly enabled: boolean;
  readonly credential: { readonly status: 'active' | 'revoked'; readonly version: number };
  readonly createdAt: Date;
  readonly recent: readonly WebhookDelivery[];
  readonly activity: readonly { readonly receivedAt: Date; readonly accepted: boolean; readonly count: number }[];
}

export interface WebhookTarget {
  readonly id: WebhookEndpointId;
  readonly expectedRevision: number;
}

export type WebhookTestScenario = 'valid' | 'invalid_credential' | 'stale_timestamp' | 'oversized_body';
export interface WebhookTestRequest {
  readonly deliveryId: string;
  readonly scenario: WebhookTestScenario;
}

export type WebhookFaultReason = 'NOT_FOUND' | 'REVISION_CONFLICT' | 'AUTH_MODE_IMMUTABLE' | 'CREDENTIAL_REVOKED';
const messages: Record<WebhookFaultReason, string> = {
  NOT_FOUND: 'This webhook endpoint does not exist in the selected project.',
  REVISION_CONFLICT: 'This endpoint changed since it was loaded. Reload and try again.',
  AUTH_MODE_IMMUTABLE:
    'Authentication changes are not modeled in this preview. Live registry and credential management can replace the authentication configuration.',
  CREDENTIAL_REVOKED:
    'This preview cannot rotate a revoked credential. A live credential is replaced through credential provisioning.',
};

export class WebhookEndpointFault extends Error {
  constructor(readonly reason: WebhookFaultReason) {
    super(messages[reason]);
    this.name = 'WebhookEndpointFault';
  }
}

export function webhookConfiguration(draft: WebhookDraft): WebhookConfiguration {
  const valid = webhookDraftSchema.parse(draft);
  return {
    displayName: valid.displayName,
    maxBodyBytes: Number(valid.maxBodyBytes),
    authentication:
      valid.authentication === 'hmac'
        ? { mode: 'hmac', timestampToleranceSeconds: Number(valid.timestampToleranceSeconds) }
        : { mode: 'bearer' },
  };
}

export function endpointId(value: string): WebhookEndpointId {
  if (
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value) ||
    /^0{8}-0{4}-0{4}-0{4}-0{12}$/.test(value)
  ) {
    throw new Error('An endpoint id must be a lowercase, non-nil UUID.');
  }
  return value as WebhookEndpointId;
}

export function webhookUrl(endpoint: Pick<WebhookEndpoint, 'id'>): string {
  return `${PREVIEW_GATEWAY_ORIGIN}/webhooks/${endpoint.id}`;
}

export function webhookSubject(endpoint: Pick<WebhookEndpoint, 'id' | 'projectId'>): string {
  return `webhooks.${endpoint.projectId}.${endpoint.id}.received`;
}

export function webhookDraft(endpoint?: WebhookEndpoint): WebhookDraft {
  return {
    displayName: endpoint?.displayName ?? '',
    authentication: endpoint?.authentication.mode ?? 'hmac',
    maxBodyBytes: String(endpoint?.maxBodyBytes ?? 262_144),
    timestampToleranceSeconds: String(
      endpoint?.authentication.mode === 'hmac' ? endpoint.authentication.timestampToleranceSeconds : 300,
    ),
  };
}

export function isDeliveryId(value: string): boolean {
  return /^[\x21-\x2d\x2f-\x7e]{1,128}$/.test(value);
}

export const webhookOutcomeLabels: Record<WebhookDeliveryOutcome['kind'], string> = {
  stored: 'Durably received',
  duplicate: 'Duplicate, no new event',
  malformed_delivery_id: 'Invalid delivery ID',
  invalid_credential: 'Authentication failed',
  stale_timestamp: 'Timestamp outside tolerance',
  credential_revoked: 'Credential revoked',
  endpoint_paused: 'Endpoint paused',
  body_too_large: 'Body exceeds limit',
};
