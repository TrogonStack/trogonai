import { describe, expect, it } from "vitest";

import { AgentId, InvalidAgentId } from "../src/index.js";

describe("AgentId.parse", () => {
  it("splits {tenant}/{name}", () => {
    const id = AgentId.parse("acme/oncall");
    expect(id.tenant).toBe("acme");
    expect(id.name).toBe("oncall");
    expect(id.toString()).toBe("acme/oncall");
  });

  it("rejects values without a slash", () => {
    expect(() => AgentId.parse("acme-oncall")).toThrow(InvalidAgentId);
  });

  it("rejects empty tenant", () => {
    expect(() => AgentId.parse("/oncall")).toThrow(InvalidAgentId);
  });

  it("rejects empty name", () => {
    expect(() => AgentId.parse("acme/")).toThrow(InvalidAgentId);
  });

  it("rejects multi-slash values", () => {
    expect(() => AgentId.parse("a/b/c")).toThrow(InvalidAgentId);
  });

  it("compares by tenant and name", () => {
    const a = AgentId.parse("acme/x");
    const b = AgentId.parse("acme/x");
    const c = AgentId.parse("acme/y");
    expect(a.equals(b)).toBe(true);
    expect(a.equals(c)).toBe(false);
  });
});
