// Drives the chat route handler in-process. Run with `pnpm smoke` (scripted model, memory sandbox).
import assert from 'node:assert/strict';

import { z } from 'zod';

import { POST } from './app/api/chat/route.ts';
import { GET as preview } from './app/api/preview/route.ts';
import { previewStateSchema } from './contracts.ts';
import { WORKDIR, sandboxForSession } from './server/sandbox.ts';

assert.equal(process.env.SANDBOX_EXAMPLE_MODEL, 'scripted', 'run through `pnpm smoke`');
assert.equal(process.env.SANDBOX_PROVIDER, 'memory', 'run through `pnpm smoke`');

const message = { id: 'm1', role: 'user', parts: [{ type: 'text', text: 'Build a hello page' }] };
const response = await POST(
  new Request('http://localhost/api/chat', { method: 'POST', body: JSON.stringify({ messages: [message] }) }),
);
assert.equal(response.status, 200);
const cookie = response.headers.get('set-cookie');
assert.ok(cookie, 'the first request mints a session cookie');

// The UI message stream is server-sent events; collect each event's JSON payload.
const events = (await response.text())
  .split('\n')
  .filter((line) => line.startsWith('data: ') && line !== 'data: [DONE]')
  .map((line) =>
    z
      .object({ type: z.string(), toolName: z.string().optional(), output: z.unknown().optional() })
      .passthrough()
      .parse(JSON.parse(line.slice('data: '.length))),
  );

const called = events.filter((e) => e.type === 'tool-input-available').map((e) => e.toolName);
assert.deepEqual(called, ['write_file', 'write_file']);
const outputs = events.filter((e) => e.type === 'tool-output-available');
assert.equal(outputs.length, 2, 'every tool call produced a result');
assert.ok(
  outputs.every((e) => !String(e.output).includes('Verdict: error')),
  `a tool reported an error: ${JSON.stringify(outputs)}`,
);
assert.ok(
  events.some((e) => e.type === 'text-delta'),
  'the agent closed with a text reply',
);

// The files landed in the sandbox bound to that session.
const sessionId = cookie.split(';')[0]!.split('=')[1]!;
const { sandbox } = await sandboxForSession(sessionId);
assert.match((await sandbox.readFile({ path: WORKDIR + '/server.mjs' })) ?? '', /listen\(3000\)/);

// A second request with the cookie reuses the same sandbox.
assert.equal((await sandboxForSession(sessionId)).sandbox.sandboxId, sandbox.sandboxId);

// Memory mode has no preview; the route says so rather than returning a fake URL.
const state = previewStateSchema.parse(
  await (await preview(new Request('http://localhost/api/preview', { headers: { cookie } }))).json(),
);
assert.equal(state.status, 'unavailable');

assert.match(
  events
    .filter((event) => event.type === 'text-delta')
    .map((event) => event.delta)
    .join(''),
  /Memory mode runs no server/,
);
