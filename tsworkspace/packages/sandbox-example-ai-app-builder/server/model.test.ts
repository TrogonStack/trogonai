import assert from 'node:assert/strict';
import { test } from 'node:test';

import { createSandboxTools, toolSchemas } from '@trogonai/sandbox-agent-tools';
import { toAiSdkTools } from '@trogonai/sandbox-agent-tools/ai-sdk';
import { MemorySandboxClient, Sandbox, tenantId } from '@trogonai/sandbox-ws-client';
import { stepCountIs, streamText, tool } from 'ai';
import { z } from 'zod';

import { previewStateSchema } from '../contracts.ts';
import type { PreviewState } from '../contracts.ts';
import { chatModel } from './model.ts';

for (const scenario of [
  { provider: 'local_docker', preview: { status: 'not_ready' } },
  { provider: 'local_docker', preview: { status: 'ready', url: 'https://preview.invalid/app/' } },
  { provider: 'local_docker', preview: { status: 'unavailable', reason: 'readiness check failed' } },
  { provider: 'memory', preview: { status: 'unavailable', reason: 'Memory mode runs no server' } },
] as const) {
  test(`scripted model respects ${scenario.provider} preview state ${scenario.preview.status}`, async (context) => {
    const previous = new Map(['SANDBOX_PROVIDER', 'SANDBOX_EXAMPLE_MODEL'].map((name) => [name, process.env[name]]));
    process.env.SANDBOX_PROVIDER = scenario.provider;
    process.env.SANDBOX_EXAMPLE_MODEL = 'scripted';
    context.after(() => {
      for (const [name, value] of previous) {
        if (value === undefined) delete process.env[name];
        else process.env[name] = value;
      }
    });
    const sandbox = await Sandbox.create(new MemorySandboxClient(), { tenant: tenantId('model-tests') });
    context.after(() => sandbox.delete());
    const calls: { name: string; input: unknown }[] = [];
    const set = createSandboxTools(sandbox, {
      allow: scenario.provider === 'memory' ? ['read_file', 'write_file', 'edit_file'] : undefined,
    });
    const adapted = toAiSdkTools(set);
    const tools = {
      ...Object.fromEntries(
        Object.entries(adapted).map(([name, definition]) => [
          name,
          tool({
            ...definition!,
            outputSchema: z.string(),
            execute: async (input: unknown) => {
              calls.push({ name, input });
              return 'Verdict: ok';
            },
          }),
        ]),
      ),
      get_preview: tool({
        inputSchema: z.object({}),
        outputSchema: previewStateSchema,
        execute: async (): Promise<PreviewState> => {
          calls.push({ name: 'get_preview', input: {} });
          return scenario.preview;
        },
      }),
    };
    const result = streamText({
      model: chatModel(),
      messages: [{ role: 'user', content: 'Build a hello page' }],
      tools,
      stopWhen: stepCountIs(10),
    });
    const reply = await result.text;
    const names = calls.map((call) => call.name);
    assert.equal(names.filter((name) => name === 'write_file').length, 2);
    assert.ok(!names.includes('background_process'), 'scripted builds use managed servers');
    if (scenario.provider === 'memory') {
      assert.deepEqual(names, ['write_file', 'write_file']);
      assert.match(reply, /memory|no.*server|gateway/i);
    } else {
      assert.ok(names.includes('get_preview'), 'inspect readiness before starting a server');
      assert.equal(
        names.filter((name) => name === 'start_server').length,
        scenario.preview.status === 'not_ready' ? 1 : 0,
      );
      if (scenario.preview.status === 'unavailable') {
        assert.ok(!names.includes('run_command'));
        assert.match(reply, /unavailable|could not|cannot|failed/i);
      } else {
        const probe = calls.find((call) => call.name === 'run_command');
        assert.ok(probe);
        const input = toolSchemas.run_command.parse(probe.input);
        assert.match(input.command, /^node /);
        assert.match(input.command, /fetch\(/);
        if (scenario.preview.status === 'ready')
          assert.ok(!names.includes('start_server'), 'a later turn reuses the running server');
      }
    }
  });
}
