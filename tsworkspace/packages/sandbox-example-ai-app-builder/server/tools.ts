import {
  createSandboxTools,
  nativeFileOps,
  TOOL_NAMES,
  toolDescriptions,
  toolSchemas,
} from '@trogonai/sandbox-agent-tools';
import type { ToolName } from '@trogonai/sandbox-agent-tools';
import { toAiSdkTools } from '@trogonai/sandbox-agent-tools/ai-sdk';
import { tool } from 'ai';
import type { Tool, ToolSet } from 'ai';
import { z } from 'zod';

import { previewStateSchema } from '../contracts.ts';
import { previewForSandbox } from './preview.ts';
import { WORKDIR } from './sandbox.ts';
import type { SessionSandbox } from './sandbox.ts';

function selectedTools(memory: boolean): readonly ToolName[] {
  return memory ? ['read_file', 'write_file', 'edit_file'] : TOOL_NAMES;
}

export function historyTools(memory: boolean): Record<string, Tool<unknown, unknown>> {
  return {
    ...Object.fromEntries(
      selectedTools(memory).map((name) => [
        name,
        tool<unknown, unknown>({
          description: toolDescriptions[name],
          inputSchema: toolSchemas[name],
          outputSchema: z.string(),
        }),
      ]),
    ),
    get_preview: tool<unknown, unknown>({
      description:
        'Check the actual guest HTTP service and return a signed preview. Start a server only for not_ready; unavailable is an uncertain check and must not trigger another server.',
      inputSchema: z.object({}),
      outputSchema: previewStateSchema,
    }),
  };
}

export function toolsForSession(session: SessionSandbox): ToolSet {
  if (session.tools !== undefined) return session.tools;
  const memory = session.client === undefined;
  const contracts = historyTools(memory);
  const set = createSandboxTools(session.sandbox, {
    cwd: WORKDIR,
    allow: selectedTools(memory),
    nativeFileOps: session.client === undefined ? undefined : nativeFileOps(session.client, session.sandbox.sandboxId),
  });
  const adapted = toAiSdkTools(set, { streamOutput: !memory });
  session.tools = {
    ...Object.fromEntries(
      Object.entries(adapted).map(([name, definition]) => [
        name,
        tool({
          ...contracts[name]!,
          execute: definition!.execute,
        }),
      ]),
    ),
    get_preview: tool({ ...contracts.get_preview!, execute: () => previewForSandbox(session.sandbox, memory) }),
  };
  return session.tools;
}
