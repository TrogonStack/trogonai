import type { ToolName } from '../schemas.js';
import { toolList } from '../tools.js';
import type { SandboxTool, SandboxToolSet } from '../tools.js';

/** Fields `createTool(...)` from Mastra takes. */
export interface MastraToolDefinition {
  id: ToolName;
  description: string;
  inputSchema: SandboxTool['inputSchema'];
  execute: (args: unknown, context?: MastraExecuteContext) => Promise<string>;
}

/**
 * The part of Mastra's tool execution context used for streaming. Mastra hands `execute` a
 * `writer` whose chunks reach the client as the tool runs; older releases omit it.
 */
export interface MastraExecuteContext {
  writer?: { write(chunk: unknown): Promise<void> | void };
}

export interface MastraToolsOptions {
  /**
   * Write `run_command` progress snapshots to Mastra's tool `writer` while the command runs.
   * The returned text is still the final result the model sees. Without a `writer` on the
   * context the tool behaves as if this were off. Defaults to false.
   */
  streamOutput?: boolean;
}

/**
 * Mastra has passed tool input as `{ context }` in some releases and as the
 * bare input in others. None of the bundle's inputs has a `context` field, so
 * both are accepted.
 */
function unwrap(args: unknown): unknown {
  if (typeof args === 'object' && args !== null && 'context' in args) {
    return (args as { context: unknown }).context;
  }
  return args;
}

/** One definition per tool; hand each to `createTool(...)` and pass them to an `Agent`. */
export function toMastraTools(set: SandboxToolSet, options: MastraToolsOptions = {}): MastraToolDefinition[] {
  return toolList(set).map((t) => ({
    id: t.name,
    description: t.description,
    inputSchema: t.inputSchema,
    execute: async (args, context) => {
      const writer = context?.writer;
      const stream = options.streamOutput === true && writer !== undefined ? t.stream : undefined;
      if (stream === undefined || writer === undefined) {
        return t.execute(unwrap(args));
      }
      let last = '';
      for await (const snapshot of stream.call(t, unwrap(args))) {
        last = snapshot;
        await writer.write(snapshot);
      }
      return last;
    },
  }));
}
