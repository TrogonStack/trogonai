#!/usr/bin/env node
// Records what the TypeScript browser WebSocket transport class does against a local server, so the Rust
// `WebSocketTransport` is checked against the shipped code instead of against a reading of it.
//
// usage: node websocket_transport_goldens.mjs <package-dir> <out-file>
//
// The class (and the queue, error and constant definitions it needs) is cut out of `browser-sdk.js` by brace
// matching and evaluated with `JSON.parse`/`JSON.stringify` for the bundle's `an`/`ut` helpers. The bundle's
// timers are scaled down so the 15000 ms connect timeout and the 50000 ms keep-alive finish quickly; the
// requested delays are recorded unscaled. The server is a minimal RFC 6455 endpoint on 127.0.0.1.
import crypto from 'node:crypto';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import path from 'node:path';

const [packageDir, out] = process.argv.slice(2);
if (!packageDir || !out) {
  process.stderr.write('usage: websocket_transport_goldens.mjs PACKAGE_DIR OUT_FILE\n');
  process.exit(2);
}

const bundle = fs.readFileSync(path.join(packageDir, 'browser-sdk.js'), 'utf8');

function classSource(header) {
  const start = bundle.indexOf(header);
  if (start < 0) throw new Error(`missing ${header}`);
  let depth = 0;
  let quote = null;
  for (let i = bundle.indexOf('{', start); i < bundle.length; i++) {
    const c = bundle[i];
    if (quote) {
      if (c === '\\') i++;
      else if (c === quote) quote = null;
    } else if (c === '"' || c === "'" || c === '`') quote = c;
    else if (c === '{') depth++;
    else if (c === '}' && --depth === 0) return bundle.slice(start, i + 1);
  }
  throw new Error(`unterminated ${header}`);
}

const constants = /var oA=15000,g4=50000;/.exec(bundle);
if (!constants) throw new Error('missing the transport constants');
const source = [
  'var an=JSON.parse,ut=JSON.stringify;',
  classSource('class rn extends Error{}'),
  classSource('class io{'),
  constants[0],
  classSource('class Nb{'),
  'return Nb;',
].join('\n');

const requested = new Set();
let connectTimeoutMs = 15000;
const scale = (ms) => (ms === 15000 ? connectTimeoutMs : ms === 50000 ? 40 : ms);
const shimTimeout = (fn, ms, ...rest) => {
  requested.add(`timeout:${ms}`);
  return setTimeout(fn, scale(ms), ...rest);
};
const shimInterval = (fn, ms, ...rest) => {
  requested.add(`interval:${ms}`);
  return setInterval(fn, scale(ms), ...rest);
};
const Transport = new Function('setTimeout', 'setInterval', source)(shimTimeout, shimInterval);

function encode(opcode, payload) {
  const length = payload.length;
  const head =
    length < 126
      ? Buffer.from([0x80 | opcode, length])
      : Buffer.from([0x80 | opcode, 126, length >> 8, length & 0xff]);
  return Buffer.concat([head, payload]);
}

function decode(buffer) {
  const frames = [];
  for (;;) {
    if (buffer.length < 2) break;
    let length = buffer[1] & 0x7f;
    let offset = 2;
    if (length === 126) {
      if (buffer.length < 4) break;
      length = buffer.readUInt16BE(2);
      offset = 4;
    }
    const total = offset + 4 + length;
    if (buffer.length < total) break;
    const key = buffer.subarray(offset, offset + 4);
    const payload = Buffer.from(buffer.subarray(offset + 4, total).map((byte, index) => byte ^ key[index % 4]));
    frames.push({ opcode: buffer[0] & 0x0f, payload });
    buffer = buffer.subarray(total);
  }
  return [frames, buffer];
}

async function until(predicate) {
  for (let i = 0; i < 400; i++) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  throw new Error('timed out waiting for the recorder condition');
}

// A server that completes the handshake and records the frames its peer sends.
async function serve(onConnection) {
  const received = [];
  const connection = {
    received,
    sockets: [],
    text: (value) => connection.socket.write(encode(1, Buffer.from(value))),
    binary: (bytes) => connection.socket.write(encode(2, Buffer.from(bytes))),
    close: (code, reason) =>
      connection.socket.write(
        encode(
          8,
          code === null
            ? Buffer.alloc(0)
            : Buffer.concat([Buffer.from([code >> 8, code & 0xff]), Buffer.from(reason)]),
        ),
      ),
    drop: () => connection.socket.destroy(),
    socket: null,
  };
  const server = http.createServer();
  server.on('upgrade', (request, socket) => {
    connection.socket = socket;
    const accept = crypto
      .createHash('sha1')
      .update(`${request.headers['sec-websocket-key']}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`)
      .digest('base64');
    socket.write(
      `HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`,
    );
    let pending = Buffer.alloc(0);
    socket.on('data', (chunk) => {
      const [frames, rest] = decode(Buffer.concat([pending, chunk]));
      pending = rest;
      for (const frame of frames) {
        if (frame.opcode === 1) received.push({ kind: 'text', text: frame.payload.toString() });
        if (frame.opcode === 8) {
          received.push({
            kind: 'close',
            code: frame.payload.length >= 2 ? frame.payload.readUInt16BE(0) : null,
            reason: frame.payload.subarray(2).toString(),
          });
          socket.end(encode(8, frame.payload));
        }
      }
    });
    socket.on('error', () => {});
    onConnection?.(connection);
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return {
    connection,
    url: `ws://127.0.0.1:${server.address().port}/`,
    stop: () => {
      server.closeAllConnections();
      server.close();
    },
  };
}

// Drains `readMessages()` the way a consumer does and reports the messages and the terminal error.
function drain(transport) {
  return (async () => {
    const messages = [];
    try {
      for await (const message of transport.readMessages()) messages.push(message);
      return { messages, error: null };
    } catch (error) {
      return { messages, error: error.message, errorName: error.constructor.name };
    }
  })();
}

async function failure(promise) {
  try {
    await promise;
    return null;
  } catch (error) {
    return error.message;
  }
}

const scenarios = {};

{
  const server = await serve();
  const abort = new AbortController();
  const transport = new Transport({
    url: server.url,
    authMessage: { type: 'auth', credential: { type: 'oauth', token: 'token-1' } },
    abortController: abort,
  });
  const reading = drain(transport);
  await transport.readyPromise;
  const readyAfterOpen = transport.isReady();
  await until(() => server.connection.socket);
  server.connection.text('{"a":1}\n{"b"');
  server.connection.text(':2}\n\nnot json\n{"ccr_event":{"x":1},"c":3}\n[1,2]\n"str"\n\n');
  server.connection.binary([0x7b, 0x7d, 0x0a]);
  server.connection.text('{"d":4}');
  server.connection.text('\n');
  await transport.write('{"type":"user","message":"hi"}\n');
  await transport.write('no newline');
  await until(() => server.connection.received.filter((f) => f.kind === 'text').length >= 4);
  const unterminated = server.connection.received.length;
  server.connection.close(1000, 'done');
  const result = await reading;
  scenarios.session = {
    readyAfterOpen,
    readyAfterClose: transport.isReady(),
    received: server.connection.received.slice(0, unterminated).filter((f) => f.kind === 'text').map((f) => f.text),
    ...result,
  };
  server.stop();
}

for (const [name, code, reason] of [
  ['abnormal_close', 4000, 'bye'],
  ['abnormal_close_empty_reason', 1011, ''],
  ['going_away', 1001, 'restart'],
  ['close_without_status', null, ''],
]) {
  const server = await serve();
  const transport = new Transport({ url: server.url });
  const reading = drain(transport);
  await transport.readyPromise;
  await until(() => server.connection.socket);
  server.connection.close(code, reason);
  scenarios[name] = { code, reason, ...(await reading) };
  server.stop();
}

{
  const server = await serve();
  const transport = new Transport({ url: server.url });
  const reading = drain(transport);
  await transport.readyPromise;
  transport.close();
  const result = await reading;
  await until(() => server.connection.received.some((f) => f.kind === 'close'));
  scenarios.client_close = {
    received: server.connection.received,
    readyAfterClose: transport.isReady(),
    writeError: await failure(transport.write('{"type":"user"}\n')),
    ...result,
  };
  server.stop();
}

{
  const server = await serve();
  const abort = new AbortController();
  const transport = new Transport({ url: server.url, abortController: abort });
  const reading = drain(transport);
  await transport.readyPromise;
  abort.abort();
  const result = await reading;
  await until(() => server.connection.received.some((f) => f.kind === 'close'));
  scenarios.abort = {
    received: server.connection.received,
    writeError: await failure(transport.write('{"type":"user"}\n')),
    ...result,
  };
  server.stop();
}

{
  const server = await serve();
  const abort = new AbortController();
  abort.abort();
  const transport = new Transport({ url: server.url, abortController: abort });
  scenarios.abort_before_connect = {
    readError: (await drain(transport)).error,
    writeError: await failure(transport.write('{"type":"user"}\n')),
  };
  server.stop();
}

{
  const server = await serve();
  const transport = new Transport({ url: server.url });
  const reading = drain(transport);
  await transport.readyPromise;
  await until(() => server.connection.socket);
  server.connection.text('{"before":1}\n');
  await new Promise((resolve) => setTimeout(resolve, 50));
  server.connection.drop();
  scenarios.abrupt_drop = await reading;
  server.stop();
}

for (const [name, url] of [
  ['scheme_http', 'http://127.0.0.1:1/'],
  ['scheme_https', 'https://127.0.0.1:1/'],
  ['not_a_url', 'not a url'],
]) {
  let message = null;
  try {
    new Transport({ url });
  } catch (error) {
    message = error.message;
  }
  scenarios[name] = { url, error: message };
}

// Node's WebSocket dispatches `error` synchronously from `close()` while connecting, which preempts the
// timeout's own rejection, so Node reports "WebSocket connection error" here. Browsers dispatch it from a queued
// task, so the timeout message wins; `NeverOpens` is a socket that never opens and fails asynchronously on close.
{
  const stalled = net.createServer((socket) => socket.on('error', () => {}));
  await new Promise((resolve) => stalled.listen(0, '127.0.0.1', resolve));
  connectTimeoutMs = 50;
  const transport = new Transport({ url: `ws://127.0.0.1:${stalled.address().port}/` });
  scenarios.connect_timeout_node = {
    error: await failure(transport.readyPromise),
    writeError: await failure(transport.write('{"type":"user"}\n')),
  };
  transport.close();
  stalled.close();

  class NeverOpens {
    static OPEN = 1;
    readyState = 0;
    close() {
      this.readyState = 3;
      setTimeout(() => this.onerror?.({}), 0);
    }
    send() {}
  }
  const native = globalThis.WebSocket;
  globalThis.WebSocket = NeverOpens;
  try {
    const deferred = new Transport({ url: 'ws://127.0.0.1:1/' });
    scenarios.connect_timeout = {
      error: await failure(deferred.readyPromise),
      writeError: await failure(deferred.write('{"type":"user"}\n')),
    };
  } finally {
    globalThis.WebSocket = native;
    connectTimeoutMs = 15000;
  }
}

{
  const probe = net.createServer();
  await new Promise((resolve) => probe.listen(0, '127.0.0.1', resolve));
  const port = probe.address().port;
  await new Promise((resolve) => probe.close(resolve));
  const transport = new Transport({ url: `ws://127.0.0.1:${port}/` });
  scenarios.connect_refused = {
    error: await failure(transport.readyPromise),
    writeError: await failure(transport.write('{"type":"user"}\n')),
  };
}

const golden = {
  requested_delays: [...requested].sort(),
  scenarios,
};
fs.mkdirSync(path.dirname(out), { recursive: true });
fs.writeFileSync(out, `${JSON.stringify(golden, null, 2)}\n`);
