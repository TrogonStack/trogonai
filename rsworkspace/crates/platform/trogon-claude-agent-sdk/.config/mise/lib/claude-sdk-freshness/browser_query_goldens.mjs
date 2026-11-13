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

const lines = (items) => `[\n${items.map((item) => JSON.stringify(item)).join(',\n')}\n]`;
const document = `{\n"queries": ${lines(recorded)},\n"errors": ${lines(errors)}\n}\n`;
fs.mkdirSync(path.dirname(out), { recursive: true });
fs.writeFileSync(out, document);
