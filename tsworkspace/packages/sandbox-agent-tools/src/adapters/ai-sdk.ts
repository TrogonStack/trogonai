import type { ToolName } from '../schemas.js';
import { toolList } from '../tools.js';
import type { SandboxTool, SandboxToolSet } from '../tools.js';

/**
 * Shape the Vercel AI SDK accepts as a tool: pass each value to `tool(...)` or use the record as-is.
 * With `streamOutput`, `run_command` returns an async iterable: each value is a preliminary
 * result the UI can show, and the last one is the final result the model sees.
 */
export interface AiSdkToolDefinition {
  description: string;
  inputSchema: SandboxTool['inputSchema'];
  execute: (input: unknown) => Promise<string> | AsyncIterable<string>;
}

export interface AiSdkToolsOptions {
  /**
   * Stream `run_command` progress as preliminary tool results. Needs a stack that keeps
   * durable command logs; otherwise the tool still returns its one final result. Defaults to false.
   */
  streamOutput?: boolean;
}

/** Record keyed by tool name, ready for `generateText({ tools })` or `streamText({ tools })`. */
export function toAiSdkTools(
  set: SandboxToolSet,
  options: AiSdkToolsOptions = {},
): Partial<Record<ToolName, AiSdkToolDefinition>> {
  const out: Partial<Record<ToolName, AiSdkToolDefinition>> = {};
  for (const t of toolList(set)) {
    const stream = options.streamOutput === true ? t.stream : undefined;
    out[t.name] = {
      description: t.description,
      inputSchema: t.inputSchema,
      execute: stream === undefined ? (input) => t.execute(input) : (input) => stream.call(t, input),
    };
  }
  return out;
}
