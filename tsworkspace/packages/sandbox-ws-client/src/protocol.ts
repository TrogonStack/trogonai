import type { WireErrorExtras } from './errors.ts';

export interface WireErrorBody extends WireErrorExtras {
  code: string;
  message: string;
}

export type WsFrame =
  | { kind: 'request'; id: string; tool: string; payload: unknown }
  | { kind: 'response'; id: string; result?: unknown; error?: WireErrorBody }
  | { kind: 'event'; stream_id: string; event: unknown };

export interface CreateSessionResponse {
  token: string;
  expires_at: number;
}

export interface CreateSessionErrorBody {
  error: string;
}
