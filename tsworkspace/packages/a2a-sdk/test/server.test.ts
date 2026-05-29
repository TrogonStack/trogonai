import { describe, expect, it } from "vitest";

import {
  AgentId,
  Audience,
  ChainLoop,
  ChainTooDeep,
  Forbidden,
  Server,
  encodeJwtForTesting,
  type ActChainEntry,
  type Caller,
  type Handler,
} from "../src/index.js";
import { NoopJwks } from "../src/test-doubles.js";

class CapturingHandler implements Handler {
  caller?: Caller;
  payload?: Uint8Array;

  async handle(caller: Caller, payload: Uint8Array): Promise<Uint8Array> {
    this.caller = caller;
    this.payload = payload;
    return new TextEncoder().encode("ok");
  }
}

function entry(agentId: string, wkl: string, aud: string): ActChainEntry {
  return { sub: agentId, agent_id: agentId, wkl, aud, iat: 0 };
}

const self = AgentId.parse("acme/oncall");
const expectedAud = Audience.forAgent(self).toString();

describe("Server.dispatch", () => {
  it("delivers caller chain to the handler on the happy path", async () => {
    const handler = new CapturingHandler();
    const server = Server.build({ agentId: self, jwks: new NoopJwks(), handler });

    const token = encodeJwtForTesting({
      aud: expectedAud,
      act_chain: [
        entry("acme/source", "spiffe://source", expectedAud),
        entry("acme/middle", "spiffe://middle", expectedAud),
      ],
      purpose: "handoff",
      session_id: "sess-1",
    });

    const reply = await server.dispatch(token, new TextEncoder().encode("hi"));
    expect(new TextDecoder().decode(reply)).toBe("ok");
    expect(handler.caller).toBeDefined();
    expect(handler.caller!.chain).toHaveLength(2);
    expect(handler.caller!.originator.agent_id).toBe("acme/source");
    expect(handler.caller!.direct.agent_id).toBe("acme/middle");
    expect(handler.caller!.purpose?.toString()).toBe("handoff");
    expect(handler.caller!.sessionId).toBe("sess-1");
  });

  it("rejects an aud mismatch with Forbidden", async () => {
    const server = Server.build({
      agentId: self,
      jwks: new NoopJwks(),
      handler: new CapturingHandler(),
    });
    const token = encodeJwtForTesting({
      aud: "urn:trogon:a2a:agent:acme:other",
      act_chain: [entry("acme/source", "spiffe://source", "x")],
    });
    await expect(server.dispatch(token, new Uint8Array())).rejects.toBeInstanceOf(
      Forbidden,
    );
  });

  it("rejects chains deeper than max", async () => {
    const server = Server.build({
      agentId: self,
      jwks: new NoopJwks(),
      handler: new CapturingHandler(),
      maxChainDepth: 2,
    });
    const token = encodeJwtForTesting({
      aud: expectedAud,
      act_chain: [
        entry("acme/a", "spiffe://a", expectedAud),
        entry("acme/b", "spiffe://b", expectedAud),
        entry("acme/c", "spiffe://c", expectedAud),
      ],
    });
    await expect(server.dispatch(token, new Uint8Array())).rejects.toBeInstanceOf(
      ChainTooDeep,
    );
  });

  it("rejects duplicate (agent_id, wkl) in chain with ChainLoop", async () => {
    const server = Server.build({
      agentId: self,
      jwks: new NoopJwks(),
      handler: new CapturingHandler(),
    });
    const token = encodeJwtForTesting({
      aud: expectedAud,
      act_chain: [
        entry("acme/a", "spiffe://a", expectedAud),
        entry("acme/b", "spiffe://b", expectedAud),
        entry("acme/a", "spiffe://a", expectedAud),
      ],
    });
    await expect(server.dispatch(token, new Uint8Array())).rejects.toBeInstanceOf(
      ChainLoop,
    );
  });
});
