import type { Headers, JsonObject } from './wire.ts';

export type Transport = 'http' | 'socket_mode';

export type PayloadKind = 'event' | 'interaction' | 'command' | 'ssl_check' | 'url_verification' | 'malformed';

export type ExpectedOutcome = 'publish' | 'request' | 'inline' | 'reject' | 'ignore';

export interface BodyFill {
  prefix: string;
  filler: string;
  count: number;
  suffix: string;
}

export interface Provenance {
  bolt_version: string;
  bolt_commit: string;
  node_slack_sdk_commit: string | null;
  payload_source: string;
  wire_source: string | null;
  ack_source: string | null;
}

export type HttpInput = {
  headers: Headers;
} & ({ body: string } | { body_fill: BodyFill });

export interface SocketInput {
  frame: string;
}

export interface DispatchRecord {
  incoming_type: string;
  conversation_id: string | null;
  retry_num: number | null;
  retry_reason: string | null;
  is_enterprise_install: boolean;
  team_id: string | null;
  enterprise_id: string | null;
  user_id: string | null;
}

export interface BoltHttpVerdict {
  outcome: 'dispatched' | 'inline' | 'rejected';
  status: number;
  body: string | null;
  content_type: string | null;
  reason: string | null;
  dispatch: DispatchRecord | null;
}

export interface SocketAckFrame {
  envelope_id: string;
  payload: JsonObject;
}

export interface BoltSocketVerdict {
  outcome: 'dispatched' | 'ignored';
  ack: SocketAckFrame | null;
  dispatch: DispatchRecord | null;
}

export interface ExpectedNats {
  subject_suffix: string;
  dedup_id: string | null;
  payload: string;
  payload_fill: BodyFill | null;
  headers: Record<string, string>;
}

export interface ExpectedHttp {
  status: number;
  body: string | null;
  content_type: string | null;
  challenge: string | null;
}

export interface ExpectedReply {
  content_type: string | null;
  body: string;
}

export interface ExpectedRequest {
  subject_suffix: string;
  headers: Record<string, string>;
  payload: string;
  reply: ExpectedReply;
}

export interface ExpectedSocketAck {
  envelope_id: string;
  payload: JsonObject | null;
}

export interface Expected {
  outcome: ExpectedOutcome;
  nats: ExpectedNats | null;
  request: ExpectedRequest | null;
  http: ExpectedHttp | null;
  socket_ack: ExpectedSocketAck | null;
}

interface FixtureBase {
  schema_version: 1;
  id: string;
  kind: PayloadKind;
  provenance: Provenance;
  not_representable: string | null;
  expected: Expected;
}

export interface HttpFixture extends FixtureBase {
  transport: 'http';
  input: HttpInput;
  bolt: BoltHttpVerdict;
}

export interface SocketFixture extends FixtureBase {
  transport: 'socket_mode';
  input: SocketInput;
  bolt: BoltSocketVerdict;
}

export type Fixture = HttpFixture | SocketFixture;
