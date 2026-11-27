import assert from 'node:assert/strict';
import { test } from 'node:test';

import { MemorySandboxClient } from '@trogonai/sandbox-ws-client';
import type { LeaseRequest } from '@trogonai/sandbox-ws-client';
import { z } from 'zod';

import { POST } from './route.ts';

for (const body of [
  JSON.stringify({
    messages: [
      {
        id: 'a',
        role: 'assistant',
        parts: [
          {
            type: 'tool-write_file',
            state: 'output-available',
            toolCallId: 'bad-output',
            input: { path: '/tmp/page.html', content: 'hello' },
            output: { invalid: 'object' },
          },
        ],
      },
    ],
  }),
  JSON.stringify({
    messages: [
      {
        id: 'a',
        role: 'assistant',
        parts: [
          {
            type: 'tool-not_available',
            state: 'output-available',
            toolCallId: 'unknown',
            input: {},
            output: 'ok',
          },
        ],
      },
    ],
  }),
  JSON.stringify({
    messages: [
      {
        id: 'a',
        role: 'assistant',
        parts: [
          {
            type: 'tool-get_preview',
            state: 'output-available',
            toolCallId: 'bad-preview',
            input: {},
            output: { status: 'ready', url: 42 },
          },
        ],
      },
    ],
  }),
  JSON.stringify({
    messages: [
      {
        id: 'a',
        role: 'assistant',
        parts: [
          {
            type: 'tool-start_server',
            state: 'output-available',
            toolCallId: 'not-in-memory',
            input: { name: 'web', command: 'node server.mjs', port: 3000 },
            output: 'Verdict: ok',
          },
        ],
      },
    ],
  }),
  'not-json',
  JSON.stringify({ messages: [] }),
  JSON.stringify({ messages: 'not-an-array' }),
  JSON.stringify({ messages: [{ id: 'm1', role: 'user', parts: [{ type: 'text', text: 42 }] }] }),
  JSON.stringify({
    messages: [
      {
        id: 'm1',
        role: 'assistant',
        parts: [
          {
            type: 'tool-write_file',
            state: 'output-available',
            toolCallId: 'call-1',
            input: { path: 42, content: 'invalid path' },
            output: 'Verdict: ok',
          },
        ],
      },
    ],
  }),
]) {
  test('chat rejects malformed input before creating a sandbox', async (context) => {
    const previous = new Map(['SANDBOX_PROVIDER', 'SANDBOX_EXAMPLE_MODEL'].map((name) => [name, process.env[name]]));
    process.env.SANDBOX_PROVIDER = 'memory';
    process.env.SANDBOX_EXAMPLE_MODEL = 'scripted';
    context.after(() => {
      for (const [name, value] of previous) {
        if (value === undefined) delete process.env[name];
        else process.env[name] = value;
      }
    });
    let leases = 0;
    const lease = MemorySandboxClient.prototype.lease;
    context.mock.method(
      MemorySandboxClient.prototype,
      'lease',
      function (this: MemorySandboxClient, request: LeaseRequest) {
        leases++;
        return lease.call(this, request);
      },
    );
    const response = await POST(new Request('http://example.invalid/api/chat', { method: 'POST', body }));
    await response.text();
    assert.equal(response.status, 400);
    assert.equal(leases, 0);
  });
}

test('chat accepts a valid prior tool result on a later user turn', async (context) => {
  const previous = new Map(['SANDBOX_PROVIDER', 'SANDBOX_EXAMPLE_MODEL'].map((name) => [name, process.env[name]]));
  process.env.SANDBOX_PROVIDER = 'memory';
  process.env.SANDBOX_EXAMPLE_MODEL = 'scripted';
  context.after(() => {
    for (const [name, value] of previous) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
  });
  const messages = [
    { id: 'u1', role: 'user', parts: [{ type: 'text', text: 'Build a page' }] },
    {
      id: 'a1',
      role: 'assistant',
      parts: [
        {
          type: 'tool-write_file',
          state: 'output-available',
          toolCallId: 'prior-write',
          input: { path: '/workspace/index.html', content: 'prior page' },
          output: 'Verdict: ok',
        },
        { type: 'text', text: 'The prior turn completed.' },
      ],
    },
    { id: 'u2', role: 'user', parts: [{ type: 'text', text: 'Update the page' }] },
  ];
  const response = await POST(
    new Request('http://example.invalid/api/chat', {
      method: 'POST',
      body: JSON.stringify({ messages }),
    }),
  );
  assert.equal(response.status, 200);
  const chunks = (await response.text())
    .split('\n')
    .filter((line) => line.startsWith('data: ') && line !== 'data: [DONE]')
    .map((line) =>
      z
        .object({ type: z.string() })
        .passthrough()
        .parse(JSON.parse(line.slice(6))),
    );
  assert.ok(chunks.some((chunk) => chunk.type === 'text-delta'));
  assert.ok(chunks.every((chunk) => chunk.type !== 'error'));
});

test('chat accepts a prior custom preview object with its real output contract', async (context) => {
  const previous = new Map(['SANDBOX_PROVIDER', 'SANDBOX_EXAMPLE_MODEL'].map((name) => [name, process.env[name]]));
  process.env.SANDBOX_PROVIDER = 'memory';
  process.env.SANDBOX_EXAMPLE_MODEL = 'scripted';
  context.after(() => {
    for (const [name, value] of previous) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
  });
  const response = await POST(
    new Request('http://example.invalid/api/chat', {
      method: 'POST',
      body: JSON.stringify({
        messages: [
          { id: 'u1', role: 'user', parts: [{ type: 'text', text: 'Check preview' }] },
          {
            id: 'a1',
            role: 'assistant',
            parts: [
              {
                type: 'tool-get_preview',
                state: 'output-available',
                toolCallId: 'prior-preview',
                input: {},
                output: { status: 'unavailable', reason: 'Memory mode runs no server' },
              },
            ],
          },
          { id: 'u2', role: 'user', parts: [{ type: 'text', text: 'Write the page' }] },
        ],
      }),
    }),
  );
  assert.equal(response.status, 200);
  const stream = await response.text();
  assert.match(stream, /text-delta/);
  assert.doesNotMatch(stream, /"type":"error"/);
});
