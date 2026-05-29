import { describe, expect, it } from "vitest";

import {
  AgentId,
  CALLER_JWT_HEADER,
  Client,
  Forbidden,
  LookupFailed,
  Purpose,
  PurposeRequired,
} from "../src/index.js";
import {
  InMemoryRegistry,
  InMemorySts,
  NoopJwks,
  RecordingTransport,
  StaticSubjectTokenSource,
  StaticSvidSource,
} from "../src/test-doubles.js";

function buildClient(opts: {
  self: AgentId;
  registry: InMemoryRegistry;
  sts?: InMemorySts;
  transport?: RecordingTransport;
}) {
  const sts = opts.sts ?? new InMemorySts();
  const transport = opts.transport ?? new RecordingTransport('{"ok":true}');
  const client = Client.build({
    agentId: opts.self,
    svidSource: new StaticSvidSource("svid-jwt"),
    subjectTokenSource: new StaticSubjectTokenSource("bootstrap-jwt"),
    sts,
    registry: opts.registry,
    transport,
    jwks: new NoopJwks(),
  });
  return { client, sts, transport };
}

describe("Client.call", () => {
  const self = AgentId.parse("acme/oncall");
  const target = AgentId.parse("acme/planner");

  it("runs lookup -> exchange -> publish -> decode", async () => {
    const registry = new InMemoryRegistry()
      .set(self, { allowedAudiences: [], allowedPurposes: ["handoff"] })
      .set(target, {
        allowedAudiences: ["urn:trogon:a2a:agent:acme:planner"],
        allowedPurposes: [],
      });
    const { client, sts, transport } = buildClient({ self, registry });

    const reply = await client.call<{ q: string }, { ok: boolean }>(
      target,
      { q: "hello" },
      new Purpose("handoff"),
    );

    expect(reply).toEqual({ ok: true });
    expect(sts.calls).toHaveLength(1);
    expect(sts.calls[0]!.request.audience).toBe(
      "urn:trogon:a2a:agent:acme:planner",
    );
    expect(sts.calls[0]!.request.agentId).toBe("acme/oncall");
    expect(sts.calls[0]!.request.purpose).toBe("handoff");

    expect(transport.calls).toHaveLength(1);
    expect(transport.calls[0]!.subject).toBe("mcp.agent.acme.planner.request");
    expect(transport.calls[0]!.headers[CALLER_JWT_HEADER]).toBeDefined();
  });

  it("auto-fills purpose when caller has a single allowed purpose", async () => {
    const registry = new InMemoryRegistry()
      .set(self, { allowedAudiences: [], allowedPurposes: ["handoff"] })
      .set(target, { allowedAudiences: [], allowedPurposes: [] });
    const { client, sts } = buildClient({ self, registry });

    await client.call(target, { q: "hi" }, null);
    expect(sts.calls[0]!.request.purpose).toBe("handoff");
  });

  it("throws PurposeRequired when caller has multiple purposes and none passed", async () => {
    const registry = new InMemoryRegistry()
      .set(self, { allowedAudiences: [], allowedPurposes: ["a", "b"] })
      .set(target, { allowedAudiences: [], allowedPurposes: [] });
    const { client } = buildClient({ self, registry });

    await expect(client.call(target, { q: "hi" }, null)).rejects.toBeInstanceOf(
      PurposeRequired,
    );
  });

  it("rejects self-calls with Forbidden", async () => {
    const registry = new InMemoryRegistry().set(self, {
      allowedAudiences: [],
      allowedPurposes: [],
    });
    const { client } = buildClient({ self, registry });

    await expect(client.call(self, {}, null)).rejects.toBeInstanceOf(Forbidden);
  });

  it("surfaces a missing registry record as LookupFailed", async () => {
    const registry = new InMemoryRegistry().set(self, {
      allowedAudiences: [],
      allowedPurposes: [],
    });
    const { client } = buildClient({ self, registry });

    await expect(
      client.call(AgentId.parse("acme/unknown"), {}, new Purpose("handoff")),
    ).rejects.toBeInstanceOf(LookupFailed);
  });
});
