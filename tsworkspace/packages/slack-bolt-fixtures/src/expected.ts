import { createHash } from 'node:crypto';

import type { DispatchRecord, ExpectedNats, PayloadKind, Transport } from './fixture.ts';
import type { JsonObject } from './wire.ts';

export interface ExpectedNatsInput {
  kind: Extract<PayloadKind, 'event' | 'interaction' | 'command'>;
  transport: Transport;
  body: JsonObject;
  wirePayload: string;
  dispatch: DispatchRecord;
  requestTimestamp?: string;
  envelopeId?: string;
}

function text(value: unknown): string | null {
  return typeof value === 'string' ? value : null;
}

function record(value: unknown): JsonObject {
  return typeof value === 'object' && value !== null ? (value as JsonObject) : {};
}

function interactionDedupId(body: JsonObject, wirePayload: string): string {
  const triggerId = text(body.trigger_id);
  if (triggerId !== null && triggerId !== '') {
    return triggerId;
  }
  const viewId = text(record(body.view).id);
  if (body.type === 'view_closed' && viewId !== null && viewId !== '') {
    return `view_closed-${viewId}`;
  }
  return `interaction-${createHash('sha256').update(wirePayload).digest('hex')}`;
}

function transportHeader(transport: Transport): string {
  switch (transport) {
    case 'http':
      return 'webhook';
    case 'socket_mode':
      return 'socket_mode';
  }
}

function contentType(kind: ExpectedNatsInput['kind'], transport: Transport): string {
  return kind === 'command' && transport === 'http' ? 'application/x-www-form-urlencoded' : 'application/json';
}

export function expectedNats(input: ExpectedNatsInput): ExpectedNats {
  const { kind, transport, body, dispatch } = input;
  const event = record(body.event);
  const headers: Record<string, string> = {};

  let subjectSuffix: string;
  let dedupId: string | null;
  let eventType: string;
  switch (kind) {
    case 'event':
      eventType = text(event.type) ?? 'unknown';
      subjectSuffix = `event.${eventType}`;
      dedupId = text(body.event_id);
      break;
    case 'interaction':
      eventType = text(body.type) ?? 'unknown';
      subjectSuffix = `interaction.${eventType}`;
      dedupId = interactionDedupId(body, input.wirePayload);
      break;
    case 'command':
      eventType = text(body.command) ?? 'unknown';
      subjectSuffix = `command.${eventType.replace(/^\/+/, '')}`;
      dedupId = text(body.trigger_id);
      break;
  }

  headers['X-Slack-Event-Type'] = eventType;
  if (kind === 'event' && dedupId !== null) {
    headers['X-Slack-Event-Id'] = dedupId;
  }
  headers['X-Slack-Team-Id'] = dispatch.team_id ?? 'unknown';
  headers['X-Slack-Payload-Kind'] = kind;
  headers['Content-Type'] = contentType(kind, transport);

  if (dispatch.retry_num !== null) {
    headers['X-Slack-Retry-Num'] = String(dispatch.retry_num);
  }
  if (dispatch.retry_reason !== null) {
    headers['X-Slack-Retry-Reason'] = dispatch.retry_reason;
  }
  if (dispatch.enterprise_id !== null) {
    headers['X-Slack-Enterprise-Id'] = dispatch.enterprise_id;
  }
  headers['X-Slack-Is-Enterprise-Install'] = String(dispatch.is_enterprise_install);
  const apiAppId = text(body.api_app_id);
  if (apiAppId !== null) {
    headers['X-Slack-Api-App-Id'] = apiAppId;
  }
  const subtype = text(event.subtype);
  if (kind === 'event' && subtype !== null) {
    headers['X-Slack-Event-Subtype'] = subtype;
  }
  const channelType = text(event.channel_type);
  if (kind === 'event' && channelType !== null) {
    headers['X-Slack-Channel-Type'] = channelType;
  }
  headers['X-Slack-Transport'] = transportHeader(transport);
  if (input.envelopeId !== undefined) {
    headers['X-Slack-Envelope-Id'] = input.envelopeId;
  }
  if (input.requestTimestamp !== undefined) {
    headers['X-Slack-Request-Timestamp'] = input.requestTimestamp;
  }

  return { subject_suffix: subjectSuffix, dedup_id: dedupId, payload: input.wirePayload, payload_fill: null, headers };
}
