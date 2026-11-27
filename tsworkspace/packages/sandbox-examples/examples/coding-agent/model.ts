import { createOpenAICompatible } from '@ai-sdk/openai-compatible';
import type { LanguageModel } from 'ai';
import { MockLanguageModelV3 } from 'ai/test';
import { z } from 'zod';

/** The tool calls the keyless model replays, in order, before it answers. */
const SCRIPT: Array<{ toolName: string; input: Record<string, unknown> }> = [
  { toolName: 'find_files', input: { path: '.' } },
  { toolName: 'read_file', input: { path: 'greeting.txt' } },
  { toolName: 'edit_file', input: { path: 'greeting.txt', old_text: 'Hello, there!', new_text: 'Hello, world!' } },
  { toolName: 'run_command', input: { command: 'cat greeting.txt' } },
];

const usage = {
  inputTokens: { total: 0, noCache: 0, cacheRead: 0, cacheWrite: 0 },
  outputTokens: { total: 0, text: 0, reasoning: 0 },
};

/** A model that ignores the prompt and replays `SCRIPT`, one tool call per step. */
function scriptedModel(): LanguageModel {
  let step = 0;
  return new MockLanguageModelV3({
    provider: 'scripted',
    modelId: 'scripted',
    doGenerate: async () => {
      const next = SCRIPT[step++];
      return {
        content:
          next === undefined
            ? [{ type: 'text', text: 'Changed the greeting in greeting.txt. Scripted run, no model was called.' }]
            : [
                {
                  type: 'tool-call',
                  toolCallId: `call-${step}`,
                  toolName: next.toolName,
                  input: JSON.stringify(next.input),
                },
              ],
        finishReason: { unified: next === undefined ? 'stop' : 'tool-calls', raw: undefined },
        usage,
        warnings: [],
      };
    },
  });
}

const blankAsUnset = (value: unknown) => (typeof value === 'string' && value.trim() === '' ? undefined : value);

const modelSelectionSchema = z.object({
  SANDBOX_EXAMPLE_MODEL: z.preprocess(
    blankAsUnset,
    z.string({ error: 'set SANDBOX_EXAMPLE_MODEL to a model id, or to "scripted" for the keyless replay' }).trim(),
  ),
});

const endpointSchema = z.object({
  SANDBOX_EXAMPLE_MODEL_BASE_URL: z.preprocess(
    blankAsUnset,
    z.url({ error: 'set SANDBOX_EXAMPLE_MODEL_BASE_URL to an OpenAI-compatible API base URL' }),
  ),
  SANDBOX_EXAMPLE_MODEL_API_KEY: z.preprocess(blankAsUnset, z.string().optional()),
});

/**
 * `SANDBOX_EXAMPLE_MODEL=scripted` needs no key or endpoint. Any other value is a model id served by the
 * OpenAI-compatible endpoint at `SANDBOX_EXAMPLE_MODEL_BASE_URL`, authenticated with
 * `SANDBOX_EXAMPLE_MODEL_API_KEY` when the endpoint needs a key. There is no default model.
 */
export function pickModel(env: NodeJS.ProcessEnv = process.env): LanguageModel {
  const { SANDBOX_EXAMPLE_MODEL: id } = modelSelectionSchema.parse(env);
  if (id === 'scripted') {
    return scriptedModel();
  }
  const endpoint = endpointSchema.parse(env);
  return createOpenAICompatible({
    name: 'sandbox-example',
    baseURL: endpoint.SANDBOX_EXAMPLE_MODEL_BASE_URL,
    apiKey: endpoint.SANDBOX_EXAMPLE_MODEL_API_KEY,
  })(id);
}
