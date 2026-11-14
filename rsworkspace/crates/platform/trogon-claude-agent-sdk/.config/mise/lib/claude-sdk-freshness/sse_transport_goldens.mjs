#!/usr/bin/env node
// Runs the pinned TypeScript browser bundle's SSE transport class against a local HTTP server and records what it
// does, as the golden the Rust transport is replayed against.
//
// usage: node sse_transport_goldens.mjs <package-dir> <out-file>
//
// The class is module private, so a copy of `browser-sdk.js` with an extra export line is imported from a temporary
// directory. Timers are shimmed so every wait is divided by DIVISOR (the Rust replay divides by the same number) and
// the requested, unscaled waits are recorded by kind; `Math.random` is pinned to 0 so retry jitter is zero and
// `crypto.randomUUID` is pinned. Every case is one line of the output: a server script, a list of actions and what the
// class did, with the server's address replaced by HOST.
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const [pkg, out] = process.argv.slice(2);
if (!pkg || !out) {
  process.stderr.write('usage: sse_transport_goldens.mjs PACKAGE_DIR OUT_FILE\n');
  process.exit(2);
}

const DIVISOR = 100;
const PLACEHOLDER_UUID = '00000000-0000-4000-8000-000000000001';

const temporary = fs.mkdtempSync(path.join(os.tmpdir(), 'claude-sdk-sse-transport-'));
const copy = path.join(temporary, 'browser-sdk.mjs');
fs.writeFileSync(copy, `${fs.readFileSync(path.join(pkg, 'browser-sdk.js'), 'utf8')}\nexport { Dp as __Transport };\n`);

const realSetTimeout = globalThis.setTimeout;
const realClearTimeout = globalThis.clearTimeout;
let delays = [];
const kindOf = (stack) => {
  if (stack.includes('handleStreamEnd')) return 'reconnect';
  if (stack.includes('resetLivenessTimer')) return 'liveness';
  if (stack.includes('connect')) return 'connect';
  return 'other';
};
globalThis.setTimeout = (fn, ms, ...args) => {
  const wait = Number(ms) || 0;
  delays.push([kindOf(new Error().stack ?? ''), wait]);
  return realSetTimeout(fn, wait / DIVISOR, ...args);
};
globalThis.clearTimeout = realClearTimeout;
AbortSignal.timeout = (ms) => {
  const controller = new AbortController();
  delays.push(['post', ms]);
  realSetTimeout(() => controller.abort(new DOMException('The operation was aborted due to timeout', 'TimeoutError')), ms / DIVISOR);
  return controller.signal;
};
Math.random = () => 0;
Object.defineProperty(globalThis.crypto, 'randomUUID', { value: () => PLACEHOLDER_UUID, configurable: true });

const logs = [];
for (const level of ['debug', 'warn']) {
  console[level] = (...args) => logs.push([level, String(args[0])]);
}

const bundle = await import(pathToFileURL(copy).href);
fs.rmSync(temporary, { recursive: true, force: true });
const Transport = bundle.__Transport;

const ascii = (value) => JSON.stringify(value).replace(/[\u0080-￿]/g, (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, '0')}`);
const sleep = (ms) => new Promise((resolve) => realSetTimeout(resolve, ms));

// ---------------------------------------------------------------------------------------------------------------
// Frames

const frame = (event, data, id) => `${event ? `event: ${event}\n` : ''}${id === undefined ? '' : `id: ${id}\n`}data: ${data}\n\n`;
const client = (seq, payload, extra = {}) =>
  frame('client_event', JSON.stringify({ event_id: `e${seq}`, sequence_num: seq, source: 'worker', created_at: 'then', payload, ...extra }), seq);
const assistant = (text) => ({ type: 'assistant', message: { content: [{ type: 'text', text }] } });

// ---------------------------------------------------------------------------------------------------------------
// Cases. `gets` and `posts` script the server by connection number (the last GET entry repeats; a POST past the end
// answers 200). A GET entry is {status, headers, body} for a plain answer, or {chunks, end} for a stream: `end` is
// `close` (end the body), `hold` (leave it open), `reset` (cut the connection) or `hang` (never answer).

const ERROR_BODY = (type) => JSON.stringify({ error: { type } });
const hold = (...chunks) => ({ chunks, end: 'hold' });
const cases = [];
const add = (name, spec) => cases.push({ name, options: {}, gets: [hold()], posts: [], actions: [], ...spec });

add('session', {
  options: {
    fromSequenceNum: 5,
    headers: { 'X-Custom': 'one', 'x-dup': 'a', 'X-Dup': 'b', Accept: 'user', accept: 'lower', Authorization: 'Bearer token' },
  },
  gets: [
    hold(
      client(6, assistant('first')),
      frame('ephemeral_event', JSON.stringify({ payload: { type: 'stream_event', event: { n: 1 }, ccr_event: 1 } })),
      frame('catch_up_truncated', '{}'),
      frame('delivery_update', JSON.stringify({ event_id: 'e6', status: 'received', timestamp: 'now' })),
      frame('delivery_update', JSON.stringify({ event_id: 5 })),
      client(7, { type: 'user', message: { content: 'typed' } }, { source: 'web' }),
      client(8, { type: 'control_response', response: { request_id: 'r1', subtype: 'success' } }, { source: 'web' }),
      client(9, assistant('last')),
    ),
  ],
  actions: [
    ['read'], ['read'], ['read'], ['read'], ['snapshot'],
    ['write', JSON.stringify({ type: 'control_request', request_id: 'r1', request: { subtype: 'interrupt' } })],
    ['write', JSON.stringify({ type: 'user', uuid: 'user-supplied', message: { content: 'x' } })],
    ['write', JSON.stringify({ type: 'user', uuid: '', message: { content: 'y' } })],
    ['write', '[1,2]'],
    ['write', '"text"'],
    ['waitPosts', 5],
    ['expect', 'r2'],
    ['snapshot'],
  ],
});
add('delivery_handlers_throw', {
  options: { throwing: true },
  gets: [
    hold(
      frame('catch_up_truncated', '{}'),
      frame('delivery_update', JSON.stringify({ event_id: 'e1', status: 'sent' })),
      client(1, assistant('after')),
    ),
  ],
  actions: [['read'], ['snapshot']],
});
for (const [status, type] of [[401, 'authentication_error'], [403, 'permission_error'], [404, 'not_found_error']]) {
  add(`stream_permanent_${status}`, {
    gets: [{ status, body: ERROR_BODY(type) }, hold()],
    actions: [['read'], ['write', '{"type":"user"}'], ['settle', 30], ['snapshot']],
  });
}
add('stream_permanent_without_reason', {
  gets: [{ status: 401, body: 'not json' }, hold()],
  actions: [['write', '{"type":"user"}'], ['read']],
});
add('stream_retries_after_a_server_error', {
  gets: [{ status: 500, body: ERROR_BODY('overloaded_error') }, hold(client(1, assistant('hello')))],
  actions: [['waitGets', 2], ['read'], ['snapshot']],
});
add('reconnect_budget_exhausted', {
  gets: [{ status: 502 }],
  actions: [['read'], ['write', '{"type":"user"}'], ['read']],
});
add('stream_reset_reconnects_from_the_cursor', {
  gets: [{ chunks: [client(3, assistant('one')), client(4, assistant('two'))], end: 'reset' }, hold(client(5, assistant('three')))],
  actions: [['read'], ['read'], ['read'], ['snapshot']],
});
add('stream_end_reconnects', {
  gets: [{ chunks: [client(1, assistant('one'))], end: 'close' }, hold(client(2, assistant('two')))],
  actions: [['read'], ['read'], ['snapshot']],
});
add('liveness_timeout_reconnects', {
  gets: [hold(client(1, assistant('one'))), hold()],
  actions: [['read'], ['waitGets', 2], ['waitReady'], ['snapshot']],
});
add('connect_timeout_reconnects', {
  gets: [{ end: 'hang' }, hold()],
  actions: [['waitGets', 2], ['waitReady'], ['snapshot']],
});
add('suspend_and_resume', {
  gets: [hold(client(1, assistant('one'))), hold(client(2, assistant('two')))],
  actions: [
    ['read'], ['snapshot'], ['suspend'], ['suspend'], ['snapshot'], ['settle', 30],
    ['resume'], ['resume'], ['waitGets', 2], ['read'], ['waitReady'], ['snapshot'],
  ],
});
add('a_write_resumes_a_suspended_stream', {
  gets: [hold(client(1, assistant('one'))), hold()],
  actions: [['read'], ['suspend'], ['write', '{"type":"user"}'], ['waitGets', 2], ['waitPosts', 1], ['waitReady'], ['snapshot']],
});
add('expecting_a_response_resumes_a_suspended_stream', {
  gets: [hold(), hold()],
  actions: [['waitReady'], ['suspend'], ['expect', 'r1'], ['waitGets', 2], ['snapshot']],
});
add('post_waits_for_retry_after', {
  posts: [
    { status: 503, headers: { 'Retry-After': '2' } }, { status: 200 },
    { status: 429, headers: { 'Retry-After': '90' } }, { status: 200 },
    { status: 500, headers: { 'Retry-After': '0' } }, { status: 200 },
    { status: 503, headers: { 'Retry-After': 'soon' } }, { status: 200 },
  ],
  actions: [
    ['write', '{"type":"user","n":1}'], ['waitPosts', 2], ['write', '{"type":"user","n":2}'], ['waitPosts', 4],
    ['write', '{"type":"user","n":3}'], ['waitPosts', 6], ['write', '{"type":"user","n":4}'], ['waitPosts', 8], ['snapshot'],
  ],
});
add('post_is_dropped_after_three_failures', {
  posts: [{ status: 500 }, { status: 500 }, { status: 500 }, { status: 200 }],
  actions: [['write', '{"type":"user","n":1}'], ['write', '{"type":"user","n":2}'], ['waitPosts', 4], ['snapshot']],
});
add('post_permanent_failure_fails_the_transport', {
  posts: [{ status: 403, body: ERROR_BODY('permission_error') }],
  actions: [['write', '{"type":"user"}'], ['waitPosts', 1], ['read'], ['write', '{"type":"user"}'], ['snapshot']],
});
add('post_permanent_failure_without_reason', {
  posts: [{ status: 404 }],
  actions: [['write', '{"type":"user"}'], ['read']],
});
add('post_that_never_answers_times_out', {
  posts: [{ status: 200, end: 'hang' }, { status: 200 }],
  actions: [['write', '{"type":"user","n":1}'], ['waitPosts', 2], ['snapshot']],
});
add('post_to_an_unreachable_server_is_retried', {
  options: { sendUrl: 'closed' },
  actions: [['write', '{"type":"user"}'], ['settle', 60], ['snapshot']],
});
add('abort_before_connecting', {
  options: { abortFirst: true },
  gets: [],
  actions: [['write', '{"type":"user"}'], ['read'], ['snapshot'], ['settle', 30]],
});
add('abort_while_running', {
  actions: [['waitReady'], ['abort'], ['snapshot'], ['write', '{"type":"user"}'], ['read']],
});
add('close_before_connecting', {
  gets: [{ end: 'hang' }],
  actions: [['waitGets', 1], ['close'], ['write', '{"type":"user"}'], ['read'], ['snapshot']],
});
add('write_after_close', {
  actions: [['waitReady'], ['close'], ['close'], ['write', '{"type":"user"}'], ['read'], ['snapshot']],
});
add('stream_url_that_does_not_parse', {
  options: { streamUrl: 'not a url' },
  gets: [],
  actions: [['read'], ['snapshot']],
});
add('stream_server_that_is_unreachable', {
  options: { streamUrl: 'closed' },
  gets: [],
  actions: [['read']],
});
add('header_name_that_is_invalid', {
  options: { headers: { 'bad name': 'x' } },
  gets: [],
  actions: [['read']],
});
add('header_value_that_is_invalid', {
  options: { headers: { 'X-Bad': 'a\nb' } },
  gets: [],
  actions: [['read']],
});
add('the_queue_holds_a_thousand_lines', {
  posts: [{ status: 200, end: 'hang' }],
  actions: [['waitReady'], ['writeMany', 1005], ['close']],
});
add('the_stream_closing_ends_a_read', {
  gets: [hold(client(1, assistant('one')))],
  actions: [['read'], ['close'], ['read']],
});

// ---------------------------------------------------------------------------------------------------------------
// Running a case

const SUBSET = {
  GET: ['accept', 'last-event-id', 'authorization', 'x-custom', 'x-dup'],
  POST: ['content-type', 'authorization', 'x-custom', 'x-dup'],
};

async function closedOrigin() {
  const server = http.createServer();
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address();
  await new Promise((resolve) => server.close(resolve));
  return `http://127.0.0.1:${port}`;
}

async function run(spec) {
  delays = [];
  logs.length = 0;
  const sockets = new Set();
  const seen = { gets: [], posts: [] };
  const server = http.createServer(async (req, res) => {
    const kind = req.method === 'POST' ? 'posts' : 'gets';
    const picked = {};
    for (const name of SUBSET[req.method]) if (req.headers[name] !== undefined) picked[name] = req.headers[name];
    const entry = { headers: picked };
    if (kind === 'posts') {
      let body = '';
      for await (const chunk of req) body += chunk;
      entry.body = JSON.parse(body);
    }
    const index = seen[kind].length;
    seen[kind].push(entry);
    const script = kind === 'gets' ? spec.gets[Math.min(index, spec.gets.length - 1)] : (spec.posts[index] ?? { status: 200 });
    if (!script || script.end === 'hang') return;
    const status = script.status ?? 200;
    if (kind === 'posts' || !script.chunks) {
      res.writeHead(status, script.headers ?? {});
      res.end(script.body ?? '');
      return;
    }
    res.writeHead(status, { 'content-type': 'text/event-stream', ...(script.headers ?? {}) });
    res.flushHeaders();
    for (const chunk of script.chunks) res.write(chunk);
    if (script.end === 'close') res.end();
    else if (script.end === 'reset') realSetTimeout(() => req.socket.destroy(), 60);
  });
  server.on('connection', (socket) => {
    sockets.add(socket);
    socket.on('close', () => sockets.delete(socket));
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  const dead = spec.options.streamUrl === 'closed' || spec.options.sendUrl === 'closed' ? await closedOrigin() : undefined;
  const events = [];
  const abort = new AbortController();
  const options = {
    streamUrl: spec.options.streamUrl === 'closed' ? `${dead}/stream` : (spec.options.streamUrl ?? `${origin}/stream`),
    sendUrl: spec.options.sendUrl === 'closed' ? `${dead}/send` : `${origin}/send`,
    sessionId: 'session-1',
    headers: spec.options.headers,
    fromSequenceNum: spec.options.fromSequenceNum,
    abortController: abort,
    onCatchUpTruncated: () => {
      events.push(['catch_up_truncated']);
      if (spec.options.throwing) throw new Error('boom');
    },
    onDeliveryUpdate: (update) => {
      events.push(['delivery_update', update]);
      if (spec.options.throwing) throw new Error('boom');
    },
  };
  if (spec.options.abortFirst) abort.abort();
  const transport = new Transport(options);
  const iterator = transport.readMessages()[Symbol.asyncIterator]();
  const until = async (condition) => {
    for (let i = 0; i < 600; i++) {
      if (condition()) return true;
      await sleep(5);
    }
    return false;
  };
  const results = [];
  const snapshot = () => ({
    ready: transport.isReady(),
    drops: transport.getDropCounts(),
    last: transport.getLastSequenceNum() ?? null,
    events: events.splice(0),
    logs: logs.splice(0),
  });
  for (const [name, a, b] of spec.actions) {
    let result;
    try {
      if (name === 'read') {
        const next = await Promise.race([iterator.next(), sleep(2000).then(() => 'timeout')]);
        if (next === 'timeout') result = { timeout: true };
        else if (next.done) result = { done: true };
        else {
          transport.markDelivered(next.value);
          result = { message: next.value, ccr: transport.getCcrEvent(next.value) ?? null };
        }
      } else if (name === 'write') {
        await Promise.race([transport.write(a), sleep(2000).then(() => { throw new Error('write timeout'); })]);
        result = { ok: true };
      } else if (name === 'writeMany') {
        let resolved = 0;
        for (let i = 0; i < a; i++) transport.write(JSON.stringify({ type: 'user', n: i })).then(() => resolved++, () => {});
        await sleep(300);
        result = { resolved };
      } else if (name === 'waitGets') result = { reached: await until(() => seen.gets.length >= a) };
      else if (name === 'waitPosts') result = { reached: await until(() => seen.posts.length >= a) };
      else if (name === 'waitReady') result = { reached: await until(() => transport.isReady()) };
      else if (name === 'settle') {
        await sleep(a);
        result = { settled: true };
      } else if (name === 'suspend') result = { suspended: transport.suspend() };
      else if (name === 'resume') result = { resumed: transport.resume() };
      else if (name === 'expect') {
        transport.expectControlResponse(a);
        result = { expected: true };
      } else if (name === 'abort') {
        abort.abort();
        result = { aborted: true };
      } else if (name === 'close') {
        transport.close();
        result = { closed: true };
      } else if (name === 'snapshot') result = snapshot();
      else throw new Error(`unknown action ${name}`);
    } catch (error) {
      result = { error: String(error?.message ?? error) };
    }
    results.push([name, result]);
  }
  const noise = ([kind, ms]) => (kind === 'other' && ms === 3000) || (kind === 'connect' && ms < 15000);
  const recorded = { gets: seen.gets, posts: seen.posts, delays: delays.filter((d) => !noise(d)) };
  transport.close();
  for (const socket of sockets) socket.destroy();
  server.close();
  const text = JSON.stringify({ name: spec.name, actions: spec.actions, result: { results, ...recorded } }).split(origin).join('HOST');
  return JSON.parse(dead ? text.split(dead).join('DEAD') : text);
}

const lines = [];
for (const spec of cases) {
  const result = await run(spec);
  lines.push(ascii({ name: spec.name, spec: { options: spec.options, gets: spec.gets, posts: spec.posts, actions: spec.actions }, divisor: DIVISOR, ...result.result }));
}
fs.mkdirSync(path.dirname(out), { recursive: true });
fs.writeFileSync(out, `${lines.join('\n')}\n`);
process.exit(0);
