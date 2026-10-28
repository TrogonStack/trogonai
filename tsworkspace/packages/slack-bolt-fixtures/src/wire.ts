import { createHmac } from 'node:crypto';

export const SIGNING_SECRET = 'bolt-conformance-signing-secret';
export const REQUEST_TIMESTAMP_SECONDS = 1_700_000_000;
export const ENVELOPE_ID = 'dbdd0ef3-1543-4f94-bfb4-133d0e6c1545';

export const CONTENT_TYPE_JSON = 'application/json';
export const CONTENT_TYPE_FORM = 'application/x-www-form-urlencoded';

export type JsonObject = Record<string, unknown>;

export type Headers = Record<string, string>;

export function signSlackRequest(signingSecret: string, timestampSeconds: number | string, body: string): string {
  const digest = createHmac('sha256', signingSecret).update(`v0:${timestampSeconds}:${body}`).digest('hex');
  return `v0=${digest}`;
}

export function eventBody(payload: JsonObject): string {
  return JSON.stringify(payload);
}

export function interactionPayloadText(payload: JsonObject): string {
  return JSON.stringify(payload);
}

export function interactionBody(payload: JsonObject): string {
  return new URLSearchParams({ payload: interactionPayloadText(payload) }).toString();
}

export function commandBody(payload: JsonObject): string {
  const fields = Object.entries(payload).map(([key, value]): [string, string] => [key, String(value)]);
  return new URLSearchParams(fields).toString();
}

export interface SignedHeadersOptions {
  contentType: string;
  body: string;
  timestampSeconds?: number | string;
  signingSecret?: string;
}

export function signedHeaders(options: SignedHeadersOptions): Headers {
  const timestamp = options.timestampSeconds ?? REQUEST_TIMESTAMP_SECONDS;
  return {
    'content-type': options.contentType,
    'x-slack-request-timestamp': String(timestamp),
    'x-slack-signature': signSlackRequest(options.signingSecret ?? SIGNING_SECRET, timestamp, options.body),
  };
}

export type SocketEnvelopeType = 'events_api' | 'interactive' | 'slash_commands';

export interface SocketEnvelopeOptions {
  type: SocketEnvelopeType;
  payload: JsonObject;
  acceptsResponsePayload: boolean;
  envelopeId?: string;
  retryAttempt?: number;
  retryReason?: string;
}

export function socketEnvelope(options: SocketEnvelopeOptions): string {
  const envelope: JsonObject = {
    payload: options.payload,
    envelope_id: options.envelopeId ?? ENVELOPE_ID,
    type: options.type,
    accepts_response_payload: options.acceptsResponsePayload,
  };
  if (options.retryAttempt !== undefined) {
    envelope.retry_attempt = options.retryAttempt;
  }
  if (options.retryReason !== undefined) {
    envelope.retry_reason = options.retryReason;
  }
  return JSON.stringify(envelope);
}
