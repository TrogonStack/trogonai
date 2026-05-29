import { InvalidAgentId } from "./errors.js";

export class AgentId {
  readonly #tenant: string;
  readonly #name: string;

  private constructor(tenant: string, name: string) {
    this.#tenant = tenant;
    this.#name = name;
  }

  static parse(value: string): AgentId {
    const slash = value.indexOf("/");
    if (slash < 0) {
      throw new InvalidAgentId("agent id must be {tenant}/{name}");
    }
    const tenant = value.slice(0, slash);
    const name = value.slice(slash + 1);
    if (tenant.length === 0 || name.length === 0) {
      throw new InvalidAgentId("agent id segments must be non-empty");
    }
    if (name.includes("/")) {
      throw new InvalidAgentId("agent id must be {tenant}/{name}");
    }
    return new AgentId(tenant, name);
  }

  get tenant(): string {
    return this.#tenant;
  }

  get name(): string {
    return this.#name;
  }

  toString(): string {
    return `${this.#tenant}/${this.#name}`;
  }

  equals(other: AgentId): boolean {
    return this.#tenant === other.#tenant && this.#name === other.#name;
  }
}

export class Audience {
  readonly #value: string;

  private constructor(value: string) {
    this.#value = value;
  }

  static of(value: string): Audience {
    return new Audience(value);
  }

  static forAgent(agent: AgentId): Audience {
    return new Audience(`urn:trogon:a2a:agent:${agent.tenant}:${agent.name}`);
  }

  toString(): string {
    return this.#value;
  }

  equals(other: Audience): boolean {
    return this.#value === other.#value;
  }
}

export class Purpose {
  readonly #value: string;

  constructor(value: string) {
    this.#value = value;
  }

  toString(): string {
    return this.#value;
  }
}

export interface ActChainEntry {
  sub: string;
  wkl?: string;
  aud?: string;
  iat?: number;
  agent_id?: string;
  purpose?: string;
}

export interface Caller {
  originator: ActChainEntry;
  chain: ActChainEntry[];
  direct: ActChainEntry;
  purpose?: Purpose;
  sessionId?: string;
}

export function callerChainDepth(caller: Caller): number {
  return caller.chain.length;
}

export interface AgentRecord {
  allowedAudiences: string[];
  allowedPurposes: string[];
  meshTokenTtlS?: number;
}

export interface ExchangeRequest {
  subjectToken: string;
  actorToken: string;
  audience: string;
  scope?: string;
  purpose?: string;
  agentId?: string;
}

export interface ExchangeResponse {
  accessToken: string;
  expiresIn?: number;
  tokenType?: string;
}
