import { LookupFailed } from "./errors.js";
import type {
  Jwks,
  JwksKey,
  MessageTransport,
  Registry,
  Sts,
  SubjectTokenSource,
  SvidSource,
} from "./interfaces.js";
import { encodeJwtForTesting } from "./server.js";
import type {
  ActChainEntry,
  AgentId,
  AgentRecord,
  ExchangeRequest,
  ExchangeResponse,
} from "./types.js";
import { Audience } from "./types.js";

export class StaticSvidSource implements SvidSource {
  constructor(private readonly token: string) {}
  async current(): Promise<string> {
    return this.token;
  }
}

export class StaticSubjectTokenSource implements SubjectTokenSource {
  constructor(private readonly token: string) {}
  async current(): Promise<string> {
    return this.token;
  }
}

export class NoopJwks implements Jwks {
  async decodingKey(kid?: string): Promise<JwksKey> {
    return kid ? { kid, key: "noop" } : { key: "noop" };
  }
}

export class InMemoryRegistry implements Registry {
  readonly #records = new Map<string, AgentRecord>();

  set(agentId: AgentId, record: AgentRecord): this {
    this.#records.set(agentId.toString(), record);
    return this;
  }

  async lookup(agentId: AgentId): Promise<AgentRecord> {
    const found = this.#records.get(agentId.toString());
    if (!found) {
      throw new LookupFailed(`no record for ${agentId}`);
    }
    return found;
  }
}

export interface StsCall {
  request: ExchangeRequest;
  response: ExchangeResponse;
}

export class InMemorySts implements Sts {
  readonly calls: StsCall[] = [];
  #chain: ActChainEntry[] = [];

  withChain(chain: ActChainEntry[]): this {
    this.#chain = chain;
    return this;
  }

  async exchange(req: ExchangeRequest): Promise<ExchangeResponse> {
    const nextChain = [
      ...this.#chain,
      {
        sub: req.agentId ?? "unknown",
        wkl: `spiffe://test/${req.agentId ?? "unknown"}`,
        aud: req.audience,
        iat: Math.floor(Date.now() / 1000),
        agent_id: req.agentId,
        purpose: req.purpose,
      },
    ];
    const accessToken = encodeJwtForTesting({
      aud: req.audience,
      act_chain: nextChain,
      purpose: req.purpose,
    });
    const response: ExchangeResponse = {
      accessToken,
      expiresIn: 60,
      tokenType: "Bearer",
    };
    this.calls.push({ request: req, response });
    return response;
  }
}

export interface TransportCall {
  subject: string;
  payload: Uint8Array;
  headers: Record<string, string>;
}

export class RecordingTransport implements MessageTransport {
  readonly calls: TransportCall[] = [];
  #reply: Uint8Array;

  constructor(reply: Uint8Array | string) {
    this.#reply =
      typeof reply === "string" ? new TextEncoder().encode(reply) : reply;
  }

  setReply(reply: Uint8Array | string): void {
    this.#reply =
      typeof reply === "string" ? new TextEncoder().encode(reply) : reply;
  }

  async request(
    subject: string,
    payload: Uint8Array,
    headers: Record<string, string>,
  ): Promise<Uint8Array> {
    this.calls.push({ subject, payload, headers });
    return this.#reply;
  }
}

export function expectedAudienceFor(agent: AgentId): string {
  return Audience.forAgent(agent).toString();
}
