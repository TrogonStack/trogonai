import { createOpenAICompatible } from '@ai-sdk/openai-compatible';
import { simulateReadableStream } from 'ai';
import type { LanguageModel } from 'ai';
import { MockLanguageModelV3 } from 'ai/test';
import { z } from 'zod';

import { previewStateSchema } from '../contracts.ts';
import { HTTP_CHECK_SCRIPT } from './preview.ts';
import { APP_PORT, WORKDIR, isMemoryMode } from './sandbox.ts';

export const SYSTEM_PROMPT = [
  'You build small web apps inside a sandbox, using the tools provided.',
  `Work in ${WORKDIR}. Write the files, then call get_preview to check the current service.`,
  `For not_ready, use start_server on port ${APP_PORT} with no path probe (the Node image has no curl or wget).`,
  'For ready, reuse the running server. For unavailable, report the problem without starting another server.',
  'Verify HTTP with Node built-in fetch through run_command before claiming the app is serving.',
  'When start_server is unavailable, only write files and explain that Memory mode runs no server.',
  'Finish with one short sentence. Never treat a failed tool call as a successful build.',
].join(' ');

const INDEX_HTML = '<!doctype html><title>Hello</title><h1>Built by the scripted agent</h1>';
const SERVER_JS = [
  "import { createServer } from 'node:http';",
  "import { readFileSync } from 'node:fs';",
  `createServer((_req, res) => res.setHeader('content-type', 'text/html').end(readFileSync('${WORKDIR}/index.html'))).listen(${APP_PORT});`,
].join('\n');

const FILES = [
  { toolName: 'write_file', input: { path: `${WORKDIR}/index.html`, content: INDEX_HTML } },
  { toolName: 'write_file', input: { path: `${WORKDIR}/server.mjs`, content: SERVER_JS } },
];

function shellQuote(value: string): string {
  return "'" + value.replaceAll("'", "'\"'\"'") + "'";
}

type StreamPart =
  Awaited<ReturnType<MockLanguageModelV3['doStream']>>['stream'] extends ReadableStream<infer P> ? P : never;

const usage = {
  inputTokens: { total: 0, noCache: 0, cacheRead: 0, cacheWrite: 0 },
  outputTokens: { total: 0, text: 0, reasoning: 0 },
};

function scriptedModel(memory: boolean): LanguageModel {
  return new MockLanguageModelV3({
    modelId: 'scripted',
    doStream: async ({ prompt }) => {
      const lastUser = prompt.findLastIndex((message) => message.role === 'user');
      const results = prompt
        .slice(lastUser + 1)
        .flatMap((message) =>
          message.role === 'tool' ? message.content.filter((part) => part.type === 'tool-result') : [],
        );
      const failed = results.some(
        (result) =>
          result.output.type.startsWith('error-') ||
          (result.output.type === 'text' && /^Verdict: (error|failed|refused|timed out)/.test(result.output.value)),
      );
      let next: { toolName: string; input: Record<string, unknown> } | undefined;
      let reply = memory
        ? 'Files are ready. Memory mode runs no server; use a gateway for a live preview.'
        : `Done. The app is serving on port ${APP_PORT}.`;
      if (failed) {
        reply = 'The build could not finish because a sandbox tool failed.';
      } else if (results.length < FILES.length) {
        next = FILES[results.length];
      } else if (!memory) {
        const preview = results.find((result) => result.toolName === 'get_preview');
        if (preview === undefined) {
          next = { toolName: 'get_preview', input: {} };
        } else {
          const state = previewStateSchema.safeParse(preview.output.type === 'json' ? preview.output.value : undefined);
          if (!state.success || state.data.status === 'unavailable') {
            reply = 'The preview is unavailable. No additional server was started.';
          } else if (
            state.data.status === 'not_ready' &&
            !results.some((result) => result.toolName === 'start_server')
          ) {
            next = {
              toolName: 'start_server',
              input: { name: 'web', command: 'node server.mjs', cwd: WORKDIR, port: APP_PORT },
            };
          } else if (!results.some((result) => result.toolName === 'run_command')) {
            next = {
              toolName: 'run_command',
              input: { command: 'node --input-type=module -e ' + shellQuote(HTTP_CHECK_SCRIPT), cwd: WORKDIR },
            };
          }
        }
      }
      const chunks: StreamPart[] =
        next === undefined
          ? [
              { type: 'text-start', id: 't' },
              { type: 'text-delta', id: 't', delta: reply },
              { type: 'text-end', id: 't' },
              { type: 'finish', finishReason: { unified: 'stop', raw: undefined }, usage },
            ]
          : [
              {
                type: 'tool-call',
                toolCallId: `call-${results.length}`,
                toolName: next.toolName,
                input: JSON.stringify(next.input),
              },
              { type: 'finish', finishReason: { unified: 'tool-calls', raw: undefined }, usage },
            ];
      return { stream: simulateReadableStream({ chunks }) };
    },
  });
}

const blankAsUnset = (value: unknown) => (typeof value === 'string' && value.trim() === '' ? undefined : value);

const modelSelectionSchema = z.object({
  SANDBOX_EXAMPLE_MODEL: z.preprocess(
    blankAsUnset,
    z.string({ error: 'set SANDBOX_EXAMPLE_MODEL to a model id, or to "scripted" for the keyless build' }).trim(),
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
 * SANDBOX_EXAMPLE_MODEL=scripted needs no endpoint or key. Any other value is a model id served by the
 * OpenAI-compatible API at SANDBOX_EXAMPLE_MODEL_BASE_URL. There is no default model.
 */
export function chatModel(env: Readonly<Record<string, string | undefined>> = process.env): LanguageModel {
  const { SANDBOX_EXAMPLE_MODEL: id } = modelSelectionSchema.parse(env);
  if (id === 'scripted') return scriptedModel(isMemoryMode());
  const endpoint = endpointSchema.parse(env);
  return createOpenAICompatible({
    name: 'sandbox-example',
    baseURL: endpoint.SANDBOX_EXAMPLE_MODEL_BASE_URL,
    apiKey: endpoint.SANDBOX_EXAMPLE_MODEL_API_KEY,
  })(id);
}
