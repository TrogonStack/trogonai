import type {
  AgentId,
  AgentRecord,
  ExchangeRequest,
  ExchangeResponse,
} from "./types.js";

export interface SvidSource {
  current(): Promise<string>;
}

export interface SubjectTokenSource {
  current(): Promise<string>;
}

export interface Sts {
  exchange(req: ExchangeRequest): Promise<ExchangeResponse>;
}

export interface Registry {
  lookup(agentId: AgentId): Promise<AgentRecord>;
}

export interface MessageTransport {
  request(
    subject: string,
    payload: Uint8Array,
    headers: Record<string, string>,
  ): Promise<Uint8Array>;
}

export interface JwksKey {
  kid?: string;
  key: string;
}

export interface Jwks {
  decodingKey(kid?: string): Promise<JwksKey>;
}
