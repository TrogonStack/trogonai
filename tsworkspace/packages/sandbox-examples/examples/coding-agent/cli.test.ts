import assert from 'node:assert/strict';
import { fork } from 'node:child_process';
import { test } from 'node:test';
import type { TestContext } from 'node:test';

import { z } from 'zod';

const eventSchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('connect') }),
  z.object({ type: z.literal('lease'), providers: z.array(z.string()) }),
  z.object({ type: z.literal('clone'), path: z.string() }),
  z.object({ type: z.literal('delete') }),
]);

async function runCli(
  context: TestContext,
  provider: string,
  fixture: 'writable-root' | 'no-connect',
  nodeEnvironment?: string,
) {
  const flags = process.allowedNodeEnvironmentFlags.has('--experimental-transform-types')
    ? ['--experimental-transform-types']
    : [];
  const env: NodeJS.ProcessEnv = {
    ...process.env,
    SANDBOX_PROVIDER: provider,
    SANDBOX_EXAMPLE_MODEL: 'scripted',
    CODING_CLI_FIXTURE: fixture,
    SANDBOX_GATEWAY_HTTP_URL: 'http://gateway.invalid',
    SANDBOX_GATEWAY_WS_URL: 'ws://gateway.invalid',
    SANDBOX_TENANT: 'cli-tests',
    SANDBOX_API_KEY: 'test-only-placeholder',
  };
  if (nodeEnvironment === undefined) delete env.NODE_ENV;
  else env.NODE_ENV = nodeEnvironment;
  delete env.GIT_TOKEN;
  delete env.SANDBOX_EXAMPLE_MODEL_API_KEY;
  const child = fork(
    new URL('./index.ts', import.meta.url),
    ['Greet the world', '--repo', 'https://example.invalid/demo.git'],
    {
      execArgv: [...flags, '--import', new URL('./test-support/gateway.ts', import.meta.url).href],
      env,
      stdio: ['ignore', 'pipe', 'pipe', 'ipc'],
    },
  );
  context.after(() => {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
  });
  const events: z.infer<typeof eventSchema>[] = [];
  let stdout = '';
  let stderr = '';
  child.stdout!.on('data', (chunk: Buffer) => {
    stdout += chunk.toString();
  });
  child.stderr!.on('data', (chunk: Buffer) => {
    stderr += chunk.toString();
  });
  child.on('message', (message) => {
    events.push(eventSchema.parse(message));
  });
  const result = await new Promise<{ code: number | null; signal: string | null }>((resolve, reject) => {
    child.once('error', reject);
    child.once('close', (code, signal) => resolve({ code, signal }));
  });
  return { ...result, stdout, stderr, events };
}

test(
  'coding CLI completes from a root image cwd using a writable non-root workspace',
  { timeout: 15_000 },
  async (context) => {
    const result = await runCli(context, 'local_docker', 'writable-root');
    assert.equal(result.signal, null);
    assert.equal(result.code, 0, result.stderr);
    const clone = result.events.find((event) => event.type === 'clone');
    assert.ok(clone?.type === 'clone');
    assert.match(clone.path, /^\/tmp\//);
    assert.deepEqual(
      result.events.filter((event) => event.type === 'lease').flatMap((event) => event.providers),
      ['local_docker'],
    );
    assert.equal(result.events.filter((event) => event.type === 'delete').length, 1);
    assert.match(result.stdout, /Hello, world!/);
    assert.match(result.stdout, /diff --git/);
  },
);

for (const provider of ['', 'not-a-provider', 'vercel']) {
  test(
    `coding CLI rejects incompatible provider ${JSON.stringify(provider)} before connecting`,
    { timeout: 15_000 },
    async (context) => {
      const result = await runCli(context, provider, 'no-connect');
      assert.notEqual(result.code, 0);
      assert.match(result.stderr, /SANDBOX_PROVIDER|unsupported provider/i);
      assert.ok(!result.events.some((event) => event.type === 'connect' || event.type === 'lease'));
    },
  );
}

test('coding CLI preserves its user report when NODE_ENV is production', { timeout: 15_000 }, async (context) => {
  const result = await runCli(context, 'local_docker', 'writable-root', 'production');
  assert.equal(result.signal, null);
  assert.equal(result.code, 0, result.stderr);
  assert.equal(result.events.filter((event) => event.type === 'delete').length, 1);
  assert.match(result.stdout, /diff --git a\/greeting.txt b\/greeting.txt/);
  assert.match(result.stdout, /\+Hello, world!/);
});
