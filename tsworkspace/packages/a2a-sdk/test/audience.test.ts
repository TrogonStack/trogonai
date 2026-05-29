import { describe, expect, it } from "vitest";

import { AgentId, Audience } from "../src/index.js";

describe("Audience.forAgent", () => {
  it("produces the urn:trogon:a2a:agent shape", () => {
    const aud = Audience.forAgent(AgentId.parse("acme/planner"));
    expect(aud.toString()).toBe("urn:trogon:a2a:agent:acme:planner");
  });
});
