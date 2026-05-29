import type { AgentId } from "./types.js";

export function agentRequestSubject(agent: AgentId): string {
  return `mcp.agent.${agent.tenant}.${agent.name}.request`;
}

export function agentQueueGroup(agent: AgentId): string {
  return `agent-${agent.tenant}-${agent.name}`;
}

export const CALLER_JWT_HEADER = "A2a-Caller-Jwt";
export const DEFAULT_PURPOSE = "default";
export const MAX_ACT_CHAIN_DEPTH = 8;
