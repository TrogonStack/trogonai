import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { test } from 'node:test';
import { promisify } from 'node:util';

import {
  CommandFinished,
  PreviewNotReadyError,
  Sandbox,
  SandboxWsClient,
  SandboxWsError,
  leaseId,
} from '@trogonai/sandbox-ws-client';
import type { LeaseId } from '@trogonai/sandbox-ws-client';
import { z } from 'zod';

import { POST } from './app/api/chat/route.ts';
import { GET as preview } from './app/api/preview/route.ts';
import { previewStateSchema } from './contracts.ts';
import { WORKDIR, sandboxForSession } from './server/sandbox.ts';

const eventSchema = z
  .object({
    type: z.string(),
    toolName: z.string().optional(),
    output: z.unknown().optional(),
  })
  .passthrough();

function events(text: string): z.infer<typeof eventSchema>[] {
  return text
    .split('\n')
    .filter((line) => line.startsWith('data: ') && line !== 'data: [DONE]')
    .map((line) => eventSchema.parse(JSON.parse(line.slice(6))));
}

test(
  'app-builder serves a signed preview, reuses its server and reattaches from a fresh host process',
  { timeout: 180_000 },
  async (context) => {
    z.object({
      SANDBOX_PROVIDER: z.literal('local_docker').default('local_docker'),
      SANDBOX_EXAMPLE_MODEL: z.literal('scripted'),
      SANDBOX_GATEWAY_HTTP_URL: z.string().url(),
      SANDBOX_GATEWAY_WS_URL: z.string().url(),
      SANDBOX_API_KEY: z.string().min(1),
      SANDBOX_TENANT: z.string().min(1),
    }).parse(process.env);
    const runCommand = Sandbox.prototype.runCommand;
    context.mock.method(
      Sandbox.prototype,
      'runCommand',
      async function (this: Sandbox, ...args: Parameters<typeof runCommand>) {
        const result = await runCommand.apply(this, args);
        const input = args[0];
        if (typeof input === 'object' && input.cmd === 'node' && result instanceof CommandFinished) {
          const text = result.stdout().trim();
          context.diagnostic(
            JSON.stringify({
              phase: 'guest-preview-probe',
              exit: result.exitCode,
              timedOut: result.timedOut,
              stdoutBytes: text.length,
              stderrBytes: result.stderr().length,
              errorCode: /^[A-Z_]+$/.test(text) ? text : undefined,
            }),
          );
        }
        return result;
      },
    );
    const resolvePreview = Sandbox.prototype.preview;
    context.mock.method(
      Sandbox.prototype,
      'preview',
      async function (this: Sandbox, ...args: Parameters<typeof resolvePreview>) {
        try {
          const result = await resolvePreview.apply(this, args);
          context.diagnostic(
            JSON.stringify({ phase: 'signed-preview', waitMs: args[1]?.waitMs ?? 0, outcome: 'available' }),
          );
          return result;
        } catch (error) {
          context.diagnostic(
            JSON.stringify({
              phase: 'signed-preview',
              waitMs: args[1]?.waitMs ?? 0,
              outcome: error instanceof PreviewNotReadyError ? 'not_ready' : 'failed',
              code: error instanceof SandboxWsError ? error.code : error instanceof Error ? error.name : 'unknown',
              variant: error instanceof SandboxWsError ? error.variant : undefined,
            }),
          );
          throw error;
        }
      },
    );
    const owned = new Set<LeaseId>();
    const lease = SandboxWsClient.prototype.lease;
    const clients = new Set<SandboxWsClient>();
    context.mock.method(
      SandboxWsClient.prototype,
      'lease',
      async function (this: SandboxWsClient, ...args: Parameters<typeof lease>) {
        const grant = await lease.apply(this, args);
        owned.add(grant.lease_id);
        clients.add(this);
        return grant;
      },
    );
    context.after(async () => {
      const [client] = clients;
      if (client === undefined) return;
      try {
        const present = (await client.listLeases()).leases.filter((row) => owned.has(leaseId(row.lease_id)));
        const cleaned = await Promise.allSettled(present.map((row) => client.deleteLease(leaseId(row.lease_id))));
        assert.ok(
          cleaned.every((result) => result.status === 'fulfilled'),
          'clean up every observed app lease',
        );
        assert.equal((await client.listLeases()).leases.filter((row) => owned.has(leaseId(row.lease_id))).length, 0);
      } finally {
        client.close();
      }
    });
    const first = await POST(
      new Request('http://example.invalid/api/chat', {
        method: 'POST',
        body: JSON.stringify({
          messages: [{ id: 'u1', role: 'user', parts: [{ type: 'text', text: 'Build a hello page' }] }],
        }),
      }),
    );
    assert.equal(first.status, 200);
    const cookie = first.headers.get('set-cookie');
    assert.ok(cookie);
    const firstEvents = events(await first.text());
    assert.ok(!firstEvents.some((event) => event.type === 'error'));
    const firstCalls = firstEvents
      .filter((event) => event.type === 'tool-input-available')
      .map((event) => event.toolName);
    assert.ok(firstCalls.includes('start_server'));
    const outputs = firstEvents.filter((event) => event.type === 'tool-output-available');
    context.diagnostic(
      JSON.stringify({
        phase: 'chat-tools',
        calls: firstCalls,
        outputKinds: outputs.map((event) =>
          typeof event.output === 'string'
            ? event.output.startsWith('Verdict: ok')
              ? 'ok'
              : 'non-ok'
            : (previewStateSchema.safeParse(event.output).data?.status ?? 'invalid'),
        ),
        toolErrors: firstEvents.filter((event) => event.type === 'tool-output-error').length,
      }),
    );
    assert.ok(
      outputs.every(
        (event) => typeof event.output !== 'string' || !/^Verdict: (error|failed|refused)/.test(event.output),
      ),
    );
    const sessionId = cookie.split(';')[0]!.split('=')[1]!;
    const session = await sandboxForSession(sessionId);
    assert.equal(session.sandbox.sandboxId, [...owned][0]);
    assert.match((await session.sandbox.readFile({ path: WORKDIR + '/server.mjs' })) ?? '', /listen\(3000\)/);
    const state = previewStateSchema.parse(
      await (await preview(new Request('http://example.invalid/api/preview', { headers: { cookie } }))).json(),
    );
    assert.equal(state.status, 'ready', state.status === 'unavailable' ? state.reason : 'preview has not answered');
    if (state.status !== 'ready') throw new Error('preview was not ready');
    const page = await fetch(state.url, { signal: AbortSignal.timeout(15_000) });
    assert.equal(page.status, 200);
    assert.match(await page.text(), /Built by the scripted agent/);

    const second = await POST(
      new Request('http://example.invalid/api/chat', {
        method: 'POST',
        headers: { cookie },
        body: JSON.stringify({
          messages: [{ id: 'u2', role: 'user', parts: [{ type: 'text', text: 'Update the page' }] }],
        }),
      }),
    );
    assert.equal(second.status, 200);
    const secondEvents = events(await second.text());
    assert.ok(!secondEvents.some((event) => event.type === 'error'));
    assert.ok(secondEvents.some((event) => event.type === 'tool-input-available' && event.toolName === 'get_preview'));
    assert.ok(
      !secondEvents.some((event) => event.type === 'tool-input-available' && event.toolName === 'start_server'),
    );
    assert.equal(owned.size, 1);

    const child = [
      "import assert from 'node:assert/strict';",
      "import { sandboxForSession, APP_PORT } from './server/sandbox.ts';",
      'const session = await sandboxForSession(process.argv[1]);',
      'try {',
      'assert.equal(session.sandbox.created, false);',
      'assert.equal(session.sandbox.sandboxId, process.argv[2]);',
      'const probe = await session.sandbox.runCommand({cmd: "node", args: ["--input-type=module", "-e", `const r = await fetch("http://127.0.0.1:${APP_PORT}"); if (!r.ok) process.exitCode=1;`]});',
      'assert.equal(probe.exitCode, 0);',
      'const preview = await session.sandbox.preview(APP_PORT, { waitMs: 2000 });',
      'const page = await fetch(preview.url, { signal: AbortSignal.timeout(15000) });',
      'assert.equal(page.status, 200);',
      '} finally { session.client?.close(); }',
    ].join('\n');
    const flags = process.allowedNodeEnvironmentFlags.has('--experimental-transform-types')
      ? ['--experimental-transform-types']
      : [];
    await promisify(execFile)(
      process.execPath,
      [...flags, '--input-type=module', '-e', child, sessionId, session.sandbox.sandboxId],
      {
        cwd: new URL('.', import.meta.url),
        timeout: 45_000,
        maxBuffer: 1024 * 1024,
      },
    );
  },
);
