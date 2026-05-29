import {
  ChainLoop,
  ChainTooDeep,
  ConfigError,
  Forbidden,
  InvalidToken,
} from "./errors.js";
import type { Jwks } from "./interfaces.js";
import { MAX_ACT_CHAIN_DEPTH } from "./subject.js";
import { Audience, Purpose } from "./types.js";
import type { ActChainEntry, AgentId, Caller } from "./types.js";

export interface Handler {
  handle(caller: Caller, payload: Uint8Array): Promise<Uint8Array>;
}

export interface ServerConfig {
  agentId: AgentId;
  jwks: Jwks;
  handler: Handler;
  maxChainDepth?: number;
}

export class Server {
  readonly #agentId: AgentId;
  readonly #jwks: Jwks;
  readonly #handler: Handler;
  readonly #maxChainDepth: number;

  private constructor(cfg: Required<ServerConfig>) {
    this.#agentId = cfg.agentId;
    this.#jwks = cfg.jwks;
    this.#handler = cfg.handler;
    this.#maxChainDepth = cfg.maxChainDepth;
  }

  static build(cfg: ServerConfig): Server {
    if (!cfg.agentId) throw new ConfigError("agentId is required");
    if (!cfg.jwks) throw new ConfigError("jwks is required");
    if (!cfg.handler) throw new ConfigError("handler is required");
    return new Server({
      agentId: cfg.agentId,
      jwks: cfg.jwks,
      handler: cfg.handler,
      maxChainDepth: cfg.maxChainDepth ?? MAX_ACT_CHAIN_DEPTH,
    });
  }

  // STUB: skeleton dispatch — does NOT verify the JWT signature.
  // The real verify+subscribe path lands with the NATS/JOSE follow-up
  // (PENDING_TODO line 156 umbrella). For now the caller hands us a
  // pre-trusted token; we only enforce structural and aud / chain rules.
  async dispatch(rawJwt: string, payload: Uint8Array): Promise<Uint8Array> {
    const claims = decodeJwtClaims(rawJwt);
    const expectedAud = Audience.forAgent(this.#agentId).toString();
    if (claims.aud !== expectedAud) {
      throw new Forbidden(
        `aud_mismatch: expected ${expectedAud}, got ${claims.aud ?? "<missing>"}`,
      );
    }

    // Touch jwks so the skeleton still exercises the injection point.
    await this.#jwks.decodingKey(claims.kid);

    const chain = claims.act_chain ?? [];
    if (chain.length > this.#maxChainDepth) {
      throw new ChainTooDeep(this.#maxChainDepth, chain.length);
    }
    detectChainLoop(chain);
    if (chain.length === 0) {
      throw new InvalidToken("empty act_chain");
    }

    const originator = chain[0]!;
    const direct = chain[chain.length - 1]!;
    const caller: Caller = {
      originator,
      chain,
      direct,
      purpose: claims.purpose ? new Purpose(claims.purpose) : undefined,
      sessionId: claims.session_id,
    };

    return this.#handler.handle(caller, payload);
  }
}

interface MeshClaims {
  aud?: string;
  act_chain?: ActChainEntry[];
  purpose?: string;
  session_id?: string;
  kid?: string;
}

function decodeJwtClaims(token: string): MeshClaims {
  const parts = token.split(".");
  if (parts.length !== 3) {
    throw new InvalidToken("JWT must have three dot-separated segments");
  }
  const [headerSeg, payloadSeg] = parts;
  let header: { kid?: string } = {};
  try {
    header = JSON.parse(decodeBase64Url(headerSeg!));
  } catch (e) {
    throw new InvalidToken("failed to decode JWT header", { cause: e });
  }
  let payload: MeshClaims;
  try {
    payload = JSON.parse(decodeBase64Url(payloadSeg!));
  } catch (e) {
    throw new InvalidToken("failed to decode JWT payload", { cause: e });
  }
  if (header.kid !== undefined) payload.kid = header.kid;
  return payload;
}

function decodeBase64Url(segment: string): string {
  const padded = segment.replace(/-/g, "+").replace(/_/g, "/");
  const pad = padded.length % 4 === 0 ? "" : "=".repeat(4 - (padded.length % 4));
  return Buffer.from(padded + pad, "base64").toString("utf8");
}

function detectChainLoop(chain: ActChainEntry[]): void {
  const seen = new Set<string>();
  for (const entry of chain) {
    const agentId = entry.agent_id ?? "";
    const wkl = entry.wkl ?? "";
    if (agentId === "" && wkl === "") continue;
    const key = `${agentId}::${wkl}`;
    if (seen.has(key)) {
      throw new ChainLoop(agentId, wkl);
    }
    seen.add(key);
  }
}

export function encodeJwtForTesting(claims: MeshClaims, kid?: string): string {
  const header = kid ? { alg: "none", kid } : { alg: "none" };
  const enc = (obj: unknown) =>
    Buffer.from(JSON.stringify(obj), "utf8")
      .toString("base64")
      .replace(/\+/g, "-")
      .replace(/\//g, "_")
      .replace(/=+$/, "");
  return `${enc(header)}.${enc(claims)}.`;
}
