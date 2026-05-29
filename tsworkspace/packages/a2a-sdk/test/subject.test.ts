import { describe, expect, it } from "vitest";

import {
  AgentId,
  agentQueueGroup,
  agentRequestSubject,
} from "../src/index.js";

describe("subject helpers", () => {
  it("formats the agent request subject", () => {
    const id = AgentId.parse("acme/planner");
    expect(agentRequestSubject(id)).toBe("mcp.agent.acme.planner.request");
  });

  it("formats the queue group", () => {
    const id = AgentId.parse("acme/planner");
    expect(agentQueueGroup(id)).toBe("agent-acme-planner");
  });
});
