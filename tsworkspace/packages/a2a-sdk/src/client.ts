import {
  ConfigError,
  Forbidden,
  LookupFailed,
  PurposeRequired,
  SdkError,
  SerializationError,
  TransportError,
} from "./errors.js";
import type {
  Jwks,
  MessageTransport,
  Registry,
  Sts,
  SubjectTokenSource,
  SvidSource,
} from "./interfaces.js";
import { CALLER_JWT_HEADER, DEFAULT_PURPOSE, agentRequestSubject } from "./subject.js";
import { AgentId, Audience, Purpose } from "./types.js";

export type AudiencePolicy = "strict" | "permissive";

export interface ClientConfig {
  agentId: AgentId;
  svidSource: SvidSource;
  subjectTokenSource: SubjectTokenSource;
  sts: Sts;
  registry: Registry;
  transport: MessageTransport;
  jwks?: Jwks;
  audiencePolicy?: AudiencePolicy;
  maxChainDepth?: number;
}

export class Client {
  readonly #cfg: Required<Omit<ClientConfig, "jwks">> & { jwks?: Jwks };

  private constructor(cfg: ClientConfig) {
    this.#cfg = {
      ...cfg,
      audiencePolicy: cfg.audiencePolicy ?? "strict",
      maxChainDepth: cfg.maxChainDepth ?? 8,
      jwks: cfg.jwks,
    };
  }

  static build(cfg: ClientConfig): Client {
    if (!cfg.agentId) throw new ConfigError("agentId is required");
    if (!cfg.svidSource) throw new ConfigError("svidSource is required");
    if (!cfg.subjectTokenSource) throw new ConfigError("subjectTokenSource is required");
    if (!cfg.sts) throw new ConfigError("sts is required");
    if (!cfg.registry) throw new ConfigError("registry is required");
    if (!cfg.transport) throw new ConfigError("transport is required");
    return new Client(cfg);
  }

  get agentId(): AgentId {
    return this.#cfg.agentId;
  }

  async call<TRequest, TResponse>(
    target: AgentId,
    payload: TRequest,
    purpose: Purpose | null,
  ): Promise<TResponse> {
    if (target.equals(this.#cfg.agentId)) {
      throw new Forbidden("self-call is not permitted");
    }

    let targetRecord;
    try {
      targetRecord = await this.#cfg.registry.lookup(target);
    } catch (e) {
      if (e instanceof SdkError) throw e;
      throw new LookupFailed(`registry lookup failed for ${target}`, { cause: e });
    }

    const resolvedPurpose = await this.#resolvePurpose(purpose);
    const audience =
      targetRecord.allowedAudiences[0] ?? Audience.forAgent(target).toString();

    const actorToken = await this.#cfg.svidSource.current();
    const subjectToken = await this.#cfg.subjectTokenSource.current();

    const exchange = await this.#cfg.sts.exchange({
      subjectToken,
      actorToken,
      audience,
      scope:
        targetRecord.allowedAudiences.length > 0
          ? targetRecord.allowedAudiences.join(" ")
          : undefined,
      purpose: resolvedPurpose.toString(),
      agentId: this.#cfg.agentId.toString(),
    });

    let body: Uint8Array;
    try {
      body = new TextEncoder().encode(JSON.stringify(payload));
    } catch (e) {
      throw new SerializationError("failed to encode request payload", { cause: e });
    }

    const subject = agentRequestSubject(target);
    let reply: Uint8Array;
    try {
      reply = await this.#cfg.transport.request(subject, body, {
        [CALLER_JWT_HEADER]: exchange.accessToken,
      });
    } catch (e) {
      if (e instanceof SdkError) throw e;
      throw new TransportError(`transport request failed: ${subject}`, { cause: e });
    }

    try {
      const text = new TextDecoder().decode(reply);
      return JSON.parse(text) as TResponse;
    } catch (e) {
      throw new SerializationError("failed to decode response payload", { cause: e });
    }
  }

  async #resolvePurpose(purpose: Purpose | null): Promise<Purpose> {
    if (purpose !== null) return purpose;
    const ownRecord = await this.#cfg.registry.lookup(this.#cfg.agentId);
    const allowed = ownRecord.allowedPurposes;
    if (allowed.length === 0) return new Purpose(DEFAULT_PURPOSE);
    if (allowed.length === 1) return new Purpose(allowed[0]!);
    throw new PurposeRequired();
  }
}
