import type { ToolName } from '../schemas.js';
import { toolList } from '../tools.js';
import type { SandboxTool, SandboxToolSet } from '../tools.js';

/** Fields `tool(...)` from `@openai/agents` takes for a function tool. */
export interface OpenAiAgentsToolDefinition {
  name: ToolName;
  description: string;
  parameters: SandboxTool['inputSchema'];
  execute: (input: unknown) => Promise<string>;
}

/**
 * One definition per tool; hand each to `tool(...)` and pass them as the agent's `tools`.
 *
 * Final text only. A function tool in `@openai/agents` resolves to one value and the SDK has
 * no channel for partial tool output, so there is nothing for a sandbox tool's `stream` to
 * feed; the model sees the result once the command finishes.
 */
export function toOpenAiAgentsTools(set: SandboxToolSet): OpenAiAgentsToolDefinition[] {
  return toolList(set).map((t) => ({
    name: t.name,
    description: t.description,
    parameters: t.inputSchema,
    execute: (input) => t.execute(input),
  }));
}
