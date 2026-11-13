#!/usr/bin/env node
// Records what the TypeScript browser `query()` writes to its WebSocket transport and which errors it throws, by
// running the shipped `browser-sdk.js` against a scripted WebSocket, so the Rust `browser_query` is checked
// against the real entrypoint.
//
// usage: node browser_query_goldens.mjs <package-dir> <out-file>
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const [packageDir, out] = process.argv.slice(2);
if (!packageDir || !out) {
  process.stderr.write('usage: browser_query_goldens.mjs PACKAGE_DIR OUT_FILE\n');
  process.exit(2);
}

process.on('unhandledRejection', () => {});
const sdk = await import(pathToFileURL(path.join(packageDir, 'browser-sdk.js')).href);

let sockets = [];
class ScriptedSocket {
  static OPEN = 1;
  readyState = 1;
  sent = [];
  constructor(url) {
    this.url = url;
    sockets.push(this);
    queueMicrotask(() => this.onopen?.());
  }
  send(text) {
    this.sent.push(text);
    for (const line of text.split('\n').filter(Boolean)) {
      const frame = JSON.parse(line);
      if (frame.type === 'control_request') {
        const response = { subtype: 'success', request_id: frame.request_id, response: {} };
        setTimeout(() => this.onmessage?.({ data: `${JSON.stringify({ type: 'control_response', response })}\n` }), 1);
      }
    }
  }
  close() {}
}
globalThis.WebSocket = ScriptedSocket;
let fetches = 0;
globalThis.fetch = () => {
  fetches++;
  return new Promise(() => {});
};

const idle = {
  async *[Symbol.asyncIterator]() {
    await new Promise(() => {});
  },
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const websocket = { url: 'ws://127.0.0.1:1/' };
const sse = { streamUrl: 'http://127.0.0.1:1/stream', sendUrl: 'http://127.0.0.1:1/send', sessionId: 'session' };
const noop = async () => ({});
const calc = () => sdk.createSdkMcpServer({ name: 'calc', tools: [] });

const cases = [
  ['bare', () => ({ websocket })],
  ['prompt suggestions on', () => ({ websocket, promptSuggestions: true })],
  ['prompt suggestions off', () => ({ websocket, promptSuggestions: false })],
  ['dialog kinds', () => ({ websocket, supportedDialogKinds: ['plan', 'ask'], onUserDialog: noop })],
  ['empty dialog kinds', () => ({ websocket, supportedDialogKinds: [] })],
  ['dialog handler only', () => ({ websocket, onUserDialog: noop })],
  ['json schema', () => ({ websocket, jsonSchema: { type: 'object', properties: { answer: { type: 'string' } } } })],
  ['sdk servers', () => ({ websocket, mcpServers: { calc: calc(), math: calc() } })],
  [
    'hooks',
    () => ({
      websocket,
      hooks: { PreToolUse: [{ matcher: 'Bash', hooks: [noop, noop], timeout: 7 }, { hooks: [noop] }], Stop: [{ hooks: [noop] }] },
    }),
  ],
  ['callbacks', () => ({ websocket, canUseTool: noop, onElicitation: noop })],
  [
    'everything',
    () => ({
      websocket,
      promptSuggestions: true,
      supportedDialogKinds: ['plan'],
      onUserDialog: noop,
      jsonSchema: { type: 'object' },
      mcpServers: { calc: calc() },
      hooks: { Stop: [{ hooks: [noop] }] },
      canUseTool: noop,
      onElicitation: noop,
    }),
  ],
];

const normalize = (text) =>
  text
    .split('\n')
    .filter(Boolean)
    .map((line) => {
      const frame = JSON.parse(line);
      if (frame.request_id) frame.request_id = 'REQUEST';
      return frame;
    });

const recorded = [];
for (const [name, build] of cases) {
  sockets = [];
  const abortController = new AbortController();
  sdk.query({ prompt: idle, abortController, ...build() });
  await wait(30);
  const frames = sockets.flatMap((socket) => socket.sent.flatMap(normalize));
  abortController.abort();
  recorded.push({ name, urls: sockets.map((socket) => socket.url), frames });
}

const errors = [];
const attempt = (name, options) => {
  sockets = [];
  fetches = 0;
  const abortController = new AbortController();
  try {
    sdk.query({ prompt: idle, abortController, ...options });
    errors.push({ name, error: null, websockets: sockets.length, fetches });
  } catch (error) {
    errors.push({ name, error: `${error.name}: ${error.message}`, websockets: sockets.length, fetches });
  }
  abortController.abort();
};
attempt('dialog kinds without a handler', { websocket, supportedDialogKinds: ['plan'] });
attempt('a server the browser cannot host', {
  websocket,
  mcpServers: { remote: { type: 'http', url: 'http://127.0.0.1:1/' } },
});
attempt('an sdk type without an instance', { websocket, mcpServers: { calc: { type: 'sdk', name: 'calc' } } });
attempt('neither transport', {});
attempt('both transports', { websocket, sse });
await wait(30);

// Messages the query hands to its consumer: which envelope getCcrEvent finds for each, and how far
// getSseLastSequenceNum has advanced, while the transport has already read every frame.
const encoder = new TextEncoder();
const frame = (n, payload) =>
  `event: client_event\nid: ${n}\ndata: ${JSON.stringify({ event_id: `e${n}`, sequence_num: n, created_at: `2026-01-01T00:00:0${n % 10}Z`, source: 'worker', payload })}\n\n`;
const result = (text, uuid) => ({
  type: 'result',
  subtype: 'success',
  is_error: false,
  duration_ms: 1,
  duration_api_ms: 1,
  num_turns: 1,
  session_id: 'remote-session',
  result: text,
  total_cost_usd: 0,
  ...(uuid ? { uuid } : {}),
});
const envelope = (event) =>
  event ? { event_id: event.event_id, sequence_num: event.sequence_num, created_at: event.created_at ?? null, source: event.source ?? null } : null;

const sseCases = [];
const sseCase = async (name, seed, events) => {
  const first = (seed ?? 0) + 1;
  let controller;
  const stream = new ReadableStream({
    start(c) {
      controller = c;
    },
  });
  const wire = events.map((payload, index) => ({ sequence_num: first + 1 + index, payload }));
  globalThis.fetch = async (url, init) => {
    if (String(url).endsWith('/stream')) {
      return new Response(stream, { status: 200, headers: { 'content-type': 'text/event-stream' } });
    }
    const payload = JSON.parse(init.body).events[0].payload;
    if (payload.type === 'control_request') {
      const response = { subtype: 'success', request_id: payload.request_id, response: {} };
      controller.enqueue(encoder.encode(frame(first, { type: 'control_response', response })));
    }
    return new Response('', { status: 200 });
  };
  const abortController = new AbortController();
  const sseOptions = { streamUrl: 'http://127.0.0.1:1/stream', sendUrl: 'http://127.0.0.1:1/send', sessionId: 'session' };
  if (seed !== undefined) sseOptions.fromSequenceNum = seed;
  const q = sdk.query({ prompt: idle, abortController, sse: sseOptions });
  const iterator = q[Symbol.asyncIterator]();
  const steps = [];
  const got = [];
  const snap = (at) =>
    steps.push({ at, last: sdk.getSseLastSequenceNum(q) ?? null, ccr: got.map((message) => envelope(sdk.getCcrEvent(q, message))) });
  await wait(30);
  snap('initialized');
  for (const item of wire) controller.enqueue(encoder.encode(frame(item.sequence_num, item.payload)));
  await wait(30);
  snap('read ahead');
  for (let index = 0; index < wire.length; index++) {
    got.push((await iterator.next()).value);
    snap(`message ${index + 1}`);
  }
  abortController.abort();
  sseCases.push({ name, seed: seed ?? null, first, events: wire, steps });
};
await sseCase('read ahead', undefined, [result('a', 'u2'), result('b'), result('c'), result('d', 'u5')]);
await sseCase('seeded', 10, [result('a', 'u12'), result('b')]);

const lines = (items) => `[\n${items.map((item) => JSON.stringify(item)).join(',\n')}\n]`;
const document = `{\n"queries": ${lines(recorded)},\n"errors": ${lines(errors)},\n"sse": ${lines(sseCases)}\n}\n`;
fs.mkdirSync(path.dirname(out), { recursive: true });
fs.writeFileSync(out, document);
process.exit(0);
