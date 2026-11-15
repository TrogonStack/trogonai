#!/usr/bin/env node
// Runs the pinned TypeScript browser bundle's own SSE helpers and transport over fixed inputs and writes the oracle
// fixtures the Rust SSE frame port is checked against.
//
// usage: node sse_frames_oracle.mjs <package-dir> <out-dir>
//
// The helpers are module private, so a copy of `browser-sdk.js` with an extra export line is imported from a
// temporary directory. Bundle names: `$b` is the frame parser, `Dp` the SSE transport (driven here through a
// scripted `fetch`, which exercises its frame handling, cursor, drop log and write path), `nA` reads an HTTP error
// reason, `HI`, `VI`, `GI` and `qI` are the durable-frame filters, `Ob` reads a sequence number, `Db` and `tu`
// sanitize log text. Output is ASCII (every other character is a \u escape) and deterministic: random inputs come
// from a seeded generator and `crypto.randomUUID` is pinned.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const [pkg, out] = process.argv.slice(2);
if (!pkg || !out) {
  process.stderr.write('usage: sse_frames_oracle.mjs PACKAGE_DIR OUT_DIR\n');
  process.exit(2);
}

const temporary = fs.mkdtempSync(path.join(os.tmpdir(), 'claude-sdk-sse-oracle-'));
const copy = path.join(temporary, 'browser-sdk.mjs');
fs.writeFileSync(
  copy,
  `${fs.readFileSync(path.join(pkg, 'browser-sdk.js'), 'utf8')}\nexport { $b as __Parser, Dp as __Transport, nA as __reason, HI as __HI, VI as __VI, GI as __GI, qI as __qI, Ob as __Ob, Db as __Db, tu as __tu, Xr as __Xr };\n`,
);
const bundle = await import(pathToFileURL(copy).href);
fs.rmSync(temporary, { recursive: true, force: true });

const PLACEHOLDER_UUID = '00000000-0000-4000-8000-000000000001';
Object.defineProperty(globalThis.crypto, 'randomUUID', { value: () => PLACEHOLDER_UUID, configurable: true });

const ascii = (value) =>
  JSON.stringify(value).replace(/[\u0080-\uffff]/g, (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, '0')}`);
// One compact case per line keeps the fixtures reviewable: an array puts each case on its own line, and an object
// (the helper sections) does the same for the cases of each section.
const lines = (cases) => `[\n${cases.map((c) => ascii(c)).join(',\n')}\n]`;
const write = (name, cases) => {
  fs.mkdirSync(out, { recursive: true });
  const text = Array.isArray(cases)
    ? lines(cases)
    : `{\n${Object.entries(cases)
        .map(([key, section]) => `${ascii(key)}: ${lines(section)}`)
        .join(',\n')}\n}`;
  fs.writeFileSync(path.join(out, name), `${text}\n`);
};

let seed = 0x51ed270b;
function next32() {
  seed = (seed + 0x6d2b79f5) >>> 0;
  let t = seed;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return (t ^ (t >>> 14)) >>> 0;
}
const below = (n) => next32() % n;
const pick = (items) => items[below(items.length)];
const chance = (n) => below(n) === 0;

const hexOf = (bytes) => Buffer.from(bytes).toString('hex');
const bytesOf = (text) => Buffer.from(text, 'utf8');

// ---------------------------------------------------------------------------------------------------------------
// Parser

const parserStreams = [
  'event: client_event\nid: 1\ndata: {"a":1}\n\n',
  'event: client_event\r\nid: 2\r\ndata: x\r\n\r\n',
  'event: client_event\rid: 3\rdata: y\r\r',
  'data: a\ndata: b\ndata: c\n\n',
  'data:a\n\n',
  'data:  two spaces\n\n',
  'data\n\n',
  'data:\n\n',
  'data:\ndata: x\n\n',
  'data: x\ndata:\ndata: y\n\n',
  ': comment only\n\n',
  ': comment\ndata: x\n\n',
  ':\n\n',
  'event: a\nevent: b\ndata: x\n\n',
  'id: 1\nid: 2\ndata: x\n\n',
  'event:\ndata: x\n\n',
  'id:\ndata: x\n\n',
  'unknown: field\ndata: x\n\n',
  'no colon line\ndata: x\n\n',
  'retry: 100\ndata: x\n\n',
  'data: x\n\n\n\ndata: y\n\n',
  '\n\n\n\n',
  '\r\n\r\n',
  '\r\r\r',
  'data: x\r\n\n',
  'data: x\n\r\n',
  'data: x\r\r\n',
  'data: x\n\r',
  'data: x\r',
  'data: x\n',
  'data: x',
  'data: x\n\ndata: y',
  'data: x\n\ndata: y\n',
  '   \n\n',
  ' \t \n\n',
  '\u00a0\n\n',
  '\u2028\n\n',
  '\ufeff\n\n',
  '\u0085\n\n',
  '\ufeffdata: x\n\n',
  'data: caf\u00e9 \u65e5\u672c \u{1f600}\n\n',
  'data: a\u2028b\u2029c\u0085d\n\n',
  'DATA: upper\n\n',
  'data : spaced\n\n',
  'data: a:b:c\n\n',
  ' data: leading space\n\n',
  'event: client_event\nid: 5\n\n',
  ': c1\n: c2\n\nevent: e\ndata: d\n\n',
  'data: 1\r\ndata: 2\rdata: 3\ndata: 4\n\n',
  'data: x\r\n\r\ndata: y\r\n\r\n',
  'data: x\r\n\r\r\ndata: y\n\n',
  '\n\ndata: x\n\n',
  'data: x\n\n\r\ndata: y\n\n',
];

const fragments = [
  'data: a',
  'data: b b',
  'data:',
  'data',
  'event: client_event',
  'event: x',
  'id: 7',
  ': note',
  ':',
  '\n',
  '\n',
  '\r',
  '\r\n',
  '\r\n',
  '\n\n',
  '\r\n\r\n',
  '\r\r',
  ' ',
  '\t',
  '\u00e9',
  '\u{1f600}',
  '\ufeff',
  '\u2028',
  '\u00a0',
  'plain',
];

function randomStream() {
  const parts = [];
  for (let count = 1 + below(14); count > 0; count -= 1) parts.push(pick(fragments));
  return parts.join('');
}

function randomBytes() {
  const buffer = [];
  for (let count = below(8); count > 0; count -= 1) {
    buffer.push(pick([0xff, 0xfe, 0x80, 0xc3, 0xe2, 0x82, 0xf0, 0x9f, 0x98, 0xc0, 0xed, 0xa0]));
  }
  return Buffer.from(buffer);
}

function splitAt(buffer, cuts) {
  const points = [...new Set(cuts)].filter((point) => point > 0 && point < buffer.length).sort((a, b) => a - b);
  const chunks = [];
  let start = 0;
  for (const point of points) {
    chunks.push(buffer.subarray(start, point));
    start = point;
  }
  chunks.push(buffer.subarray(start));
  return chunks;
}

function runParser(chunks) {
  const parser = new bundle.__Parser();
  const steps = [];
  for (const chunk of chunks) {
    const frames = typeof chunk === 'string' ? parser.push(chunk) : parser.push(new Uint8Array(chunk));
    steps.push({ frames, pending_chars: parser.pendingChars, dispatched: parser.framesDispatched });
  }
  const buffered = parser.buffered();
  const flushed = parser.flush();
  return { steps, buffered, flushed, dispatched: parser.framesDispatched };
}

const parserCases = [];
const addParserCase = (chunks) => {
  const encoded = chunks.map((chunk) => (typeof chunk === 'string' ? { text: chunk } : { hex: hexOf(chunk) }));
  parserCases.push({ chunks: encoded, ...runParser(chunks) });
};
for (const text of parserStreams) {
  const buffer = bytesOf(text);
  addParserCase([buffer]);
  addParserCase(Array.from({ length: buffer.length }, (_, index) => buffer.subarray(index, index + 1)));
  addParserCase([text]);
  for (let round = 0; round < 3; round += 1) {
    addParserCase(splitAt(buffer, Array.from({ length: 1 + below(4) }, () => below(buffer.length + 1))));
  }
}
for (let round = 0; round < 700; round += 1) {
  const buffer = Buffer.concat([bytesOf(randomStream()), chance(8) ? randomBytes() : Buffer.alloc(0), bytesOf(randomStream())]);
  const cuts = chance(3) ? Array.from({ length: buffer.length }, (_, index) => index) : Array.from({ length: below(6) }, () => below(buffer.length + 1));
  addParserCase(splitAt(buffer, cuts));
}
// Invalid and cut UTF-8, a byte order mark split over chunks, empty chunks.
addParserCase([Buffer.from([0xef, 0xbb]), Buffer.from([0xbf, 0x64, 0x61, 0x74, 0x61, 0x3a, 0x20, 0x78, 0x0a, 0x0a])]);
addParserCase([Buffer.from([]), Buffer.from([0xef, 0xbb, 0xbf]), Buffer.from('data: x\n\n')]);
addParserCase([Buffer.from([0x64, 0x61, 0x74, 0x61, 0x3a, 0x20, 0xe2, 0x82]), Buffer.from([0xac, 0x0a, 0x0a])]);
addParserCase([Buffer.from([0x64, 0x61, 0x74, 0x61, 0x3a, 0x20, 0xe2, 0x82, 0x0a, 0x0a])]);
addParserCase([Buffer.from([0x64, 0x61, 0x74, 0x61, 0x3a, 0x20, 0xf0, 0x9f, 0x98])]);
addParserCase([Buffer.from([0x64, 0x61, 0x74, 0x61, 0x3a, 0x20, 0xff, 0xfe, 0x0a, 0x0a])]);
addParserCase([Buffer.from([0x64, 0x61, 0x74, 0x61, 0x3a, 0x20, 0xed, 0xa0, 0x80, 0x0a, 0x0a])]);
addParserCase([Buffer.from([0x64, 0x61, 0x74, 0x61, 0x3a, 0x20, 0xc0, 0xaf, 0xf4, 0x90, 0x80, 0x80, 0x0a, 0x0a])]);
write('parser.json', parserCases);

// ---------------------------------------------------------------------------------------------------------------
// Helpers: sequence numbers, text truncation and log sanitizing, HTTP error reasons, worker-output markers.

const JS_VALUES = [
  'undefined', 'null', 'true', 'false', '0', '1', '-1', '5', '5.0', '5.5', '1e3', '1e21', '-0', '0.5',
  '9007199254740991', '9007199254740992', '9007199254740993', '123456789012345678901234567890',
  '"5"', '" 7"', '"12abc"', '"abc"', '""', '"0"', '"-3"', '"+4"', '"0x10"', '"1e3"', '"5.9"', '"00012"',
  '"\\u00a0\\u20288"', '"9007199254740993"', '[]', '[7]', '[7,8]', '[null,3]', '["9"]', '{}', '{"a":1}', '[[2]]',
];
const obCases = JS_VALUES.map((source) => {
  const value = source === 'undefined' ? undefined : JSON.parse(source);
  const result = bundle.__Ob(value);
  return { json: source === 'undefined' ? null : source, undefined: source === 'undefined', result: result ?? null };
});
const stringCases = JS_VALUES.map((source) => {
  const value = source === 'undefined' ? undefined : JSON.parse(source);
  return { json: source === 'undefined' ? null : source, undefined: source === 'undefined', tu: bundle.__tu(value) };
});

const textPool = [
  '', 'plain', 'a'.repeat(63), 'a'.repeat(64), 'a'.repeat(65), 'a'.repeat(200),
  '\u{1f600}'.repeat(32), '\u{1f600}'.repeat(33), `${'a'.repeat(63)}\u{1f600}`, `${'a'.repeat(62)}\u{1f600}`,
  `${'a'.repeat(127)}\u{1f600}`, `${'\u00e9'.repeat(64)}x`, 'ctl\u0000\u0007\u001b\u007f\u0085end', 'fmt\u200b\u200e\u202e\u2066\ufeffend',
  'sep\u2028\u2029end', 'tag\u{e0041}\u{e007f}end', 'zs a\u00a0b\u3000c', 'nl\r\ncr\nlf', 
  '\u200b\u200c\u200d', 'cafe\u0301', 'private\ue000', '\u{f0000}',
];
const sanitizeCases = [];
for (const text of textPool) {
  sanitizeCases.push({ text, db: bundle.__Db(text) });
}
for (const [text, limit] of [['', 0], ['abc', 0], ['abc', -1], ['abc', 2], ['abc', 3], ['abc', 4], ['a\u{1f600}b', 2], ['a\u{1f600}b', 3], ['\u{1f600}', 1], ['\u{1f600}', 2], ['\u00e9\u00e9\u00e9', 2]]) {
  sanitizeCases.push({ text, limit, xr: bundle.__Xr(text, limit) });
}

async function reasonOf(chunks) {
  const stream = new ReadableStream({
    start(controller) {
      for (const chunk of chunks) controller.enqueue(new Uint8Array(chunk));
      controller.close();
    },
  });
  const result = await bundle.__reason({ body: stream });
  return result ?? null;
}
const reasonBodies = [
  '{"error":{"resource":"workspace","type":"not_found"}}',
  '{"error":{"type":"permission_error"}}',
  '{"error":{"resource":null,"type":"fallback"}}',
  '{"error":{"resource":5,"type":"fallback"}}',
  '{"error":{"resource":"","type":"fallback"}}',
  '{"error":{"resource":"\\u200b\\u0007"}}',
  '{"error":{"resource":"a\\ud800b"}}',
  '{"error":{"resource":"\\udc00"}}',
  '{"error":{"resource":"ok\\ud83d\\ude00"}}',
  '{"error":{"resource":"\\ud83d"}}',
  '{"error":{"resource":"\\ud83dx\\ude00"}}',
  `{"error":{"resource":"${'r'.repeat(128)}"}}`,
  `{"error":{"resource":"${'r'.repeat(129)}"}}`,
  `{"error":{"resource":"${'r'.repeat(127)}\u{1f600}"}}`,
  `{"error":{"resource":"${'r'.repeat(126)}\u{1f600}z"}}`,
  `{"error":{"resource":"${'\u00e9'.repeat(200)}"}}`,
  '{"error":"text"}',
  '{"error":["x"]}',
  '{"error":null}',
  '{"error":{}}',
  '{"error":{"resource":["x"]}}',
  '{"error":{"resource":"a\\nb\\tc\\u2028d"}}',
  '{"nope":1}',
  '[]',
  'null',
  '5',
  '"error"',
  'not json',
  '',
  '\ufeff{"error":{"type":"bom"}}',
  '{"error":{"type":"a\\u0000b\\ufeffc"}}',
  '{"error":{"type":"x"}} trailing',
  '{"error":{"type":"x","type":"y"}}',
  '{"error":{"resource":"first","resource":"second"}}',
];
const reasonCases = [];
for (const body of reasonBodies) {
  const buffer = bytesOf(body);
  reasonCases.push({ hex: hexOf(buffer), reason: await reasonOf([buffer]) });
  if (buffer.length > 3) {
    const half = Math.floor(buffer.length / 2);
    reasonCases.push({ hex: hexOf(buffer), split: half, reason: await reasonOf([buffer.subarray(0, half), buffer.subarray(half)]) });
  }
}
// The 4096-byte cap: a body that is valid JSON only within the cap, one cut inside a multi-byte character, one
// whose valid prefix is padded past the cap.
{
  const pad = ' '.repeat(4096 - '{"error":{"type":"cap"}}'.length);
  const exact = bytesOf(`{"error":{"type":"cap"}}${pad}`);
  reasonCases.push({ hex: hexOf(exact), reason: await reasonOf([exact]) });
  const over = bytesOf(`{"error":{"type":"cap"}}${pad}garbage`);
  reasonCases.push({ hex: hexOf(over), reason: await reasonOf([over]) });
  const cut = bytesOf(`{"error":{"type":"${'\u00e9'.repeat(2100)}"}}`);
  reasonCases.push({ hex: hexOf(cut), reason: await reasonOf([cut]) });
  const manyChunks = bytesOf('{"error":{"type":"chunked"}}');
  reasonCases.push({ hex: hexOf(manyChunks), split: 1, reason: await reasonOf(Array.from(manyChunks, (_, index) => manyChunks.subarray(index, index + 1))) });
}

const tick = ['<tick>', '<tick x>', '<TICK\t>', '<tick', '<ticker>', '<tick\u00a0>', '<tick\u2028>', '<tick\ufeff>', '<tick\u0085>', '<tick\u180e>', '<TiCk>', '<t\u0130ck>', '<\u017fick>'];
const markers = [
  '', 'hello', '<bash-stdout>x</bash-stdout>', '<bash-stderr', 'x<bash-stdout', '<local-command-stdout>', '<local-command-stderr>y',
  '<local-command-caveat>x', '<channel source="slack">hi', '<channel source=x', '<channel>', '<teammate-message from="a">hi',
  '<teammate-message>hi', '<teammate-messagex>', '<teammate-message', '<cross-session-message>', '<cross-session-message from="x">',
  '<cross-session-message x>y', '<cross-session-message\tx>', '<cross-session-message\nx>', '<cross-session-message\u2028x>', '<cross-session-message\u00a0x>',
  '<cross-session-message x', '<cross-session-messagex>', ' <cross-session-message>',
  'A message arrived from peer\n<x>', 'A message arrived from peer\nnot a tag', 'A message arrived from peer', 'A message arrived from \n<',
  'A message arrived from peer\n\n<x>', 'Another Claude session sent a message while you were working:\n<x>',
  'Another Claude session sent a message while you were working:\n<cross-session-message>', 'Another Claude session sent a message:\n<cross-session-message from="s">body',
  'Another Claude session sent a message:\nnot a tag', 'Another Claude session sent a message:\n', 'Another Claude session sent a message: \n<x>',
  'Another Claude session sent a message', 'Another Claude session sent a message\n<x>',
  'A peer session sent a message while you were working:\n<x>', 'A peer session sent a message while you were working:\n<cross-session-message>',
  'A peer session sent a message while you were working:\nplain', 'A peer session sent a message while you were working:\n',
  'A peer session sent a message while you were working: \n<x>', 'A peer session sent a message while you were working:<x>',
  'text with <bash-input>x', 'text with <command-message>x', 'text with <user-memory-input>x', 'text with <task-notification>', 'text with <task-notification',
  'text with <mcp-resource-update', 'text with <mcp-polling-update x', 'text with <fork-boilerplate>', 'text with <local-command-caveat>',
  'text with <fork-boilerplate', 'text with </fork-boilerplate>', 'text <command-message', 'plain <bash-stdout> mid',
  ...tick.map((open) => `${open} body </tick>`), ...tick.map((open) => `${open} body`), '</tick>', '</TICK>', '</ tick>', '<tick> </Tick>',
  '<tick>\n</tick>', 'x </tick> <tick>', 'prefix <tick>x</tick> suffix', 'prefix <tick/>', '<tick>only open', 'only close </tick>',
];
const markerCases = markers.map((text) => ({ text, hi: bundle.__HI(text) }));

write('helpers.json', { ob: obCases, strings: stringCases, sanitize: sanitizeCases, reasons: reasonCases, markers: markerCases });

// Whole-message filters over JSON values.
const messageShapes = [];
const addShape = (value) => {
  const source = JSON.stringify(value);
  const parsed = JSON.parse(source);
  messageShapes.push({ json: source, vi: bundle.__VI(parsed), gi: bundle.__GI(parsed) });
};
for (const text of markers.slice(0, 40)) {
  addShape({ message: { content: text } });
  addShape({ message: { content: [{ type: 'text', text }] } });
}
for (const value of [
  {}, { message: null }, { message: 5 }, { message: 'text' }, { message: [] }, { message: {} }, { message: { content: null } },
  { message: { content: 5 } }, { message: { content: {} } }, { message: { content: [] } }, { message: { content: 'plain' } },
  { tool_use_result: null }, { tool_use_result: 1 }, { plugin_prompt: null }, { plugin_prompt: 'x' },
  { message: { content: [{ type: 'tool_result' }] } }, { message: { content: [{ type: 'tool_result', content: 'x' }] } },
  { message: { content: [{ type: 'text' }] } }, { message: { content: [{ type: 'text', text: 5 }] } },
  { message: { content: [{ type: 'text', text: null }] } }, { message: { content: [{ type: 'text', text: 'ok' }] } },
  { message: { content: [{ type: 'image' }] } }, { message: { content: [{}] } }, { message: { content: [null] } },
  { message: { content: [5] } }, { message: { content: ['str'] } }, { message: { content: [[]] } }, { message: { content: [['type']] } },
  { message: { content: [{ type: 'text', text: 'ok' }, null] } }, { message: { content: [{ type: 'text', text: 'ok' }, { type: 'text', text: 1 }] } },
  { message: { content: [{ type: 'image' }, { type: 'text', text: '<bash-stdout>' }] } },
  { message: { content: [{ type: 5, text: 'x' }] } }, { message: { content: [{ text: '<bash-stdout>' }] } },
  { message: { content: [{ type: 'text', text: '<bash-stdout>' }] } }, { message: { content: [{ type: 'other', text: '<bash-stdout>' }] } },
]) {
  addShape(value);
}
write('message_filters.json', messageShapes);

// ---------------------------------------------------------------------------------------------------------------
// Transport sessions

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const settle = async () => {
  for (let round = 0; round < 4; round += 1) await sleep(1);
};

const frameText = ({ event, id, data }) => {
  let text = '';
  if (event !== undefined) text += `event: ${event}\n`;
  if (id !== undefined) text += `id: ${id}\n`;
  if (data !== undefined) text += `${String(data).split('\n').map((line) => `data: ${line}`).join('\n')}\n`;
  return `${text}\n`;
};

async function runSession(session) {
  const events = [];
  const record = (event) => events.push(event);
  const levels = ['debug', 'warn', 'log', 'info', 'error'];
  const saved = Object.fromEntries(levels.map((level) => [level, console[level]]));
  for (const level of levels) {
    console[level] = (...args) => record({ k: 'log', level, text: args.map((arg) => (typeof arg === 'string' ? arg : String(arg))).join(' ') });
  }
  const savedFetch = globalThis.fetch;
  let controller;
  const gets = [];
  globalThis.fetch = async (url, init) => {
    if (init.method === 'POST') {
      record({ k: 'post', url, headers: init.headers, body: init.body });
      return new Response('{}', { status: 200 });
    }
    gets.push({ url, headers: init.headers });
    if (gets.length > 1) return new Promise(() => {});
    const body = new ReadableStream({
      start(c) {
        controller = c;
      },
    });
    return new Response(body, { status: 200 });
  };
  let transport;
  try {
    const options = {
      streamUrl: 'https://example.invalid/stream',
      sendUrl: 'https://example.invalid/send',
      sessionId: 'session-1',
      headers: session.headers,
      fromSequenceNum: session.from,
      onCatchUpTruncated: () => record({ k: 'catch_up' }),
      onDeliveryUpdate: (update) => record({ k: 'delivery', update }),
    };
    transport = new bundle.__Transport(options);
    const originalEnqueue = transport.messages.enqueue.bind(transport.messages);
    transport.messages.enqueue = (message) => {
      record({ k: 'message', message: JSON.parse(JSON.stringify(message)), ccr: transport.getCcrEvent(message) ?? null });
      transport.markDelivered(message);
      record({ k: 'delivered', value: transport.getLastSequenceNum() ?? null });
      originalEnqueue(message);
    };
    await transport.readyPromise;
    for (const step of session.steps) {
      if (step.write !== undefined) {
        try {
          await transport.write(step.write);
        } catch (error) {
          record({ k: 'write_error', name: error.name });
        }
      } else if (step.expect !== undefined) {
        transport.expectControlResponse(step.expect);
      } else {
        controller.enqueue(new TextEncoder().encode(step.chunk));
        await settle();
      }
    }
    await settle();
    const issued = [...transport.issuedRequestIds];
    const expected = [...transport.expectedControlResponseIds];
    return {
      session,
      events,
      drops: transport.getDropCounts(),
      drop_order: Object.keys(transport.getDropCounts()),
      cursor: transport.lastSequenceNum,
      delivered: transport.getLastSequenceNum() ?? null,
      issued: { length: issued.length, first: issued.slice(0, 2), last: issued.slice(-2) },
      expected: { length: expected.length, first: expected.slice(0, 2), last: expected.slice(-2) },
      stream_headers: gets[0].headers,
    };
  } finally {
    transport?.close();
    globalThis.fetch = savedFetch;
    for (const level of levels) console[level] = saved[level];
  }
}

const frame = (event, id, payload, extra = {}) => ({
  event,
  id,
  data: JSON.stringify({ event_id: 'e1', sequence_num: 9, payload, source: 'client', ...extra }),
});
const chunks = (...frames) => frames.map((item) => ({ chunk: frameText(item) }));

const sessions = [];
const session = (name, steps, extra = {}) => sessions.push({ name, steps, ...extra });

session('client event cursor from id', chunks(frame('client_event', '3', { type: 'user', message: { content: 'hi' } })));
session('client event cursor from payload', chunks(frame('client_event', undefined, { type: 'user' })));
session('client event cursor id wins', chunks(frame('client_event', '4', { type: 'user' }, { sequence_num: 8 })));
session('client event cursor never regresses', chunks(frame('client_event', '8', { type: 'user' }), frame('client_event', '2', { type: 'user' })));
session('client event cursor without data', chunks({ event: 'client_event', id: '12' }));
session('ephemeral cursor ignored', chunks(frame('ephemeral_event', '30', { type: 'stream_event', event: {} })));
session('other event ignores id', chunks({ event: 'other', id: '15', data: '{}' }, { id: '16', data: '{}' }));
session('seeded cursor', chunks(frame('client_event', '3', { type: 'user' }), frame('client_event', '70', { type: 'user' })), { from: 50 });
session('seeded cursor from invalid', chunks(frame('client_event', '3', { type: 'user' })), { from: -4 });
session('seeded cursor float', chunks(frame('client_event', '3', { type: 'user' })), { from: 1.5 });
session('headers merge', chunks(frame('client_event', '3', { type: 'user' })), { headers: { Authorization: 'Bearer x', Accept: 'text/plain' }, from: 5 });
session('catch up truncated', chunks({ event: 'catch_up_truncated', data: '{}' }, { event: 'catch_up_truncated' }, { event: 'catch_up_truncated', data: 'x' }));
session('delivery update ok', chunks(
  { event: 'delivery_update', data: '{"event_id":"e","status":"delivered"}' },
  { event: 'delivery_update', data: '{"event_id":"e","status":"s","timestamp":"t","extra":1}' },
  { event: 'delivery_update', data: '{"status":"s","event_id":"e"}' },
));
session('delivery update malformed', chunks(
  { event: 'delivery_update', data: '{"event_id":"e"}' },
  { event: 'delivery_update', data: '{"event_id":1,"status":"s"}' },
  { event: 'delivery_update', data: '{"event_id":"e","status":"s","timestamp":null}' },
  { event: 'delivery_update', data: '{"event_id":"e","status":"s","timestamp":5}' },
  { event: 'delivery_update', data: 'null' },
  { event: 'delivery_update', data: '[]' },
  { event: 'delivery_update', data: '5' },
  { event: 'delivery_update', data: '"s"' },
  { event: 'delivery_update', data: 'not json' },
  { event: 'delivery_update', data: '{}' },
));
session('delivery update empty strings', chunks({ event: 'delivery_update', data: '{"event_id":"","status":""}' }));
session('missing type', chunks(
  frame('client_event', '1', {}), frame('client_event', '2', null), frame('client_event', '3', 5), frame('client_event', '4', 'user'),
  frame('client_event', '5', { type: 5 }), frame('client_event', '6', []), frame('ephemeral_event', undefined, { subtype: 'x' }),
  { event: 'client_event', id: '7', data: '{"event_id":"e"}' }, { event: 'client_event', id: '8', data: '{"payload":{"type":"user"}}' },
));
session('unparseable frames', chunks(
  { event: 'client_event', id: '1', data: 'not json' }, { event: 'client_event', id: '2', data: 'null' }, { event: 'client_event', id: '3', data: '5' },
  { event: 'client_event', id: '4', data: '"s"' }, { event: 'client_event', id: '5', data: '[]' }, { event: 'ephemeral_event', data: 'null' },
  { event: 'client_event', data: 'null' }, { event: 'client_event', data: '[1]' }, { event: 'client_event', data: 'true' },
));
session('ephemeral allowed', chunks(
  frame('ephemeral_event', undefined, { type: 'stream_event', event: { a: 1 }, ccr_event: { x: 1 } }),
  frame('ephemeral_event', undefined, { type: 'sources_changed', sources: [1], ccr_event: 1 }),
  frame('ephemeral_event', undefined, { type: 'system', subtype: 'thinking_tokens', uuid: 'u', session_id: 's', estimated_tokens: 5, estimated_tokens_delta: 1, user_message_uuid: 'm', extra: 1, ccr_event: 2 }),
  frame('ephemeral_event', undefined, { type: 'system', subtype: 'post_turn_summary', uuid: 'u', session_id: 's', summarizes_uuid: 'a', status_category: 'b', status_detail: 'c', needs_action: false, other: 1 }),
  frame('ephemeral_event', undefined, { type: 'system', subtype: 'task_summary', uuid: 'u', session_id: 's', detail: null, other: 1 }),
  frame('ephemeral_event', undefined, { type: 'system', subtype: 'commands_changed', uuid: 'u', session_id: 's', commands: [{ name: 'x' }] }),
  frame('ephemeral_event', undefined, { type: 'system', subtype: 'task_summary' }),
  frame('ephemeral_event', undefined, { type: 'system', subtype: 'thinking_tokens', uuid: 5, session_id: null }),
));
session('ephemeral dropped', chunks(
  frame('ephemeral_event', undefined, { type: 'user' }), frame('ephemeral_event', undefined, { type: 'system' }),
  frame('ephemeral_event', undefined, { type: 'system', subtype: 5 }), frame('ephemeral_event', undefined, { type: 'system', subtype: 'init' }),
  frame('ephemeral_event', undefined, { type: 'assistant', subtype: 'thinking_tokens' }), frame('ephemeral_event', undefined, { type: 'control_response' }),
  frame('ephemeral_event', undefined, { type: 'system', subtype: 'x\u0007y\u200b' + 'z'.repeat(100) }), frame('ephemeral_event', undefined, { type: 'result' }),
));
session('durable envelope', chunks(
  frame('client_event', '1', { type: 'assistant' }, { event_id: 'e1', created_at: 'c', source: 'worker' }),
  frame('client_event', '2', { type: 'assistant' }, { event_id: 5 }),
  frame('client_event', '3', { type: 'assistant' }, { event_id: 'e3', created_at: 5, source: 7 }),
  frame('client_event', '4', { type: 'assistant', ccr_event: { a: 1 } }, { source: 'worker' }),
  frame('client_event', undefined, { type: 'assistant' }, { sequence_num: 'x', event_id: 'e5', source: 'worker' }),
  frame('client_event', undefined, { type: 'assistant' }, { sequence_num: '11', event_id: 'e6', source: 'worker' }),
  frame('client_event', undefined, { type: 'assistant' }, { sequence_num: 0, event_id: 'e7', source: 'worker' }),
  frame('client_event', undefined, { type: 'assistant' }, { sequence_num: [13], event_id: 'e8', source: 'worker' }),
  frame('client_event', '2', { type: 'assistant' }, { event_id: 'e9', source: 'worker' }),
));
session('sources and non worker types', chunks(
  frame('client_event', '1', { type: 'assistant' }, { source: 'client' }), frame('client_event', '2', { type: 'result' }, { source: undefined }),
  frame('client_event', '3', { type: 'user', message: { content: 'hello' } }, { source: 'client' }), frame('client_event', '4', { type: 'env_manager_log', x: 1 }, { source: 'server' }),
  frame('client_event', '5', { type: 'system' }, { source: null }), frame('client_event', '6', { type: 'control_request', request_id: 'r' }, { source: 5 }),
  frame('client_event', '7', { type: 'assistant' }, { source: ['worker'] }), frame('client_event', '8', { type: 'assistant' }, { source: { a: 1 } }),
  frame('client_event', '9', { type: 'assistant' }, { source: 'x\u0007\u200by'.repeat(30) }), frame('client_event', '10', { type: 'assistant' }, { source: true }),
  frame('client_event', '12', { type: 'assistant' }, { source: { toString: 1 } }),
  frame('client_event', '13', { type: 'assistant' }, { source: [{ toString: 1 }] }),
  frame('client_event', '14', { type: 'assistant' }, { source: [null, [1, [2]], 'a', true] }),
  frame('client_event', '15', { type: 'control_response', response: { request_id: 'zz' } }, { source: { toString: 1 } }),
  frame('client_event', '16', { type: 'user', tool_use_result: 1 }, { source: { toString: 1 } }),
  frame('client_event', '11', { type: 'control_cancel_request', request_id: 'r' }, { source: 'client' }),
));
session('expected echo', [
  { write: JSON.stringify({ type: 'control_request', request_id: 'r1', request: {} }) },
  { write: JSON.stringify({ type: 'control_request', request_id: 'r2', request: {} }) },
  ...chunks(
    frame('client_event', '1', { type: 'control_request', request_id: 'r1' }, { source: 'client' }),
    frame('client_event', '2', { type: 'control_cancel_request', request_id: 'r2' }, { source: 'client' }),
    frame('client_event', '3', { type: 'control_request', request_id: 'zzz' }, { source: 'client' }),
    frame('client_event', '4', { type: 'control_cancel_request', request_id: 5 }, { source: 'client' }),
    frame('client_event', '5', { type: 'control_cancel_request' }, { source: 'client' }),
    frame('client_event', '6', { type: 'control_request', request_id: 'r1' }, { source: 'worker' }),
    frame('client_event', '7', { type: 'control_response', response: { request_id: 'r1' } }, { source: 'worker' }),
    frame('client_event', '8', { type: 'control_request', request_id: 'r1' }, { source: 'client' }),
  ),
]);
session('control response rules', [
  { write: JSON.stringify({ type: 'control_request', request_id: 'issued', request: {} }) },
  { expect: 'expected' },
  ...chunks(
    frame('client_event', '1', { type: 'control_response' }), frame('client_event', '2', { type: 'control_response', response: null }),
    frame('client_event', '3', { type: 'control_response', response: 'x' }), frame('client_event', '4', { type: 'control_response', response: {} }),
    frame('client_event', '5', { type: 'control_response', response: { request_id: 5 } }), frame('client_event', '6', { type: 'control_response', response: [] }),
    frame('client_event', '7', { type: 'control_response', response: { request_id: 'issued' } }, { source: 'client' }),
    frame('client_event', '8', { type: 'control_response', response: { request_id: 'expected' } }, { source: 'client' }),
    frame('client_event', '9', { type: 'control_response', response: { request_id: 'other', subtype: 'success', pending_user_dialog_requests: [1], pending_permission_requests: [2], pending_elicitation_requests: [3], error_code: 'e', keep: 1 }, extra: 1 }, { source: 'client' }),
    frame('client_event', '10', { type: 'control_response', response: { request_id: 'other2', error_code: 'e', pending_permission_requests: [] } }, { source: undefined }),
    frame('client_event', '11', { type: 'control_response', response: { request_id: 'issued', pending_permission_requests: [], error_code: 'e' } }, { source: 'worker' }),
    frame('client_event', '12', { type: 'control_response', response: { request_id: 'issued' } }, { source: 'client' }),
    frame('client_event', '13', { type: 'control_response', response: { request_id: 'expected', error_code: 'e' } }, { source: 'worker' }),
    frame('client_event', '14', { type: 'control_response', response: { request_id: 'expected' } }, { source: 'client' }),
    frame('client_event', '15', { type: 'control_response', response: { request_id: 'pending', error_code: 1 } }, { source: 'worker' }),
  ),
]);
session('user frames', chunks(
  frame('client_event', '1', { type: 'user', message: { content: 'plain' } }, { source: 'client' }),
  frame('client_event', '2', { type: 'user', message: { content: '<bash-stdout>x' } }, { source: 'client' }),
  frame('client_event', '3', { type: 'user', message: { content: '<bash-stdout>x' } }, { source: 'worker' }),
  frame('client_event', '4', { type: 'user', tool_use_result: null }, { source: 'client' }),
  frame('client_event', '5', { type: 'user', plugin_prompt: 'p' }, { source: 'client' }),
  frame('client_event', '6', { type: 'user', plugin_prompt: 'p' }, { source: 'worker' }),
  frame('client_event', '7', { type: 'user', message: { content: [{ type: 'tool_result' }] } }, { source: 'client' }),
  frame('client_event', '8', { type: 'user', message: { content: [{ type: 'tool_result' }] } }, { source: 'worker' }),
  frame('client_event', '9', { type: 'user', message: { content: [{ type: 'text', text: 5 }] } }, { source: 'worker' }),
  frame('client_event', '10', { type: 'user', message: { content: [{ type: 'text', text: 5 }] } }, { source: 'client' }),
  frame('client_event', '11', { type: 'user', message: { content: [{ type: 'text', text: 'ok' }, { type: 'image' }] } }, { source: 'client' }),
  frame('client_event', '12', { type: 'user', message: { content: [{ type: 'text', text: '<local-command-stdout>' }, { type: 'text', text: 1 }] } }, { source: 'client' }),
  frame('client_event', '13', { type: 'user', message: { content: [null] } }, { source: 'worker' }),
  frame('client_event', '14', { type: 'user', message: { content: [{ type: 'text', text: '<tick>x</tick>' }] } }, { source: 'client' }),
  frame('client_event', '15', { type: 'user' }, { source: 'client' }),
  frame('client_event', '16', { type: 'user', message: null }, { source: 'client' }),
  frame('client_event', '17', { type: 'assistant', message: { content: '<bash-stdout>x' } }, { source: 'worker' }),
));
session('write uuid injection', [
  { write: JSON.stringify({ type: 'user', message: { content: 'a' } }) },
  { write: JSON.stringify({ type: 'user', uuid: 'given' }) },
  { write: JSON.stringify({ type: 'user', uuid: '' }) },
  { write: JSON.stringify({ type: 'user', uuid: 5 }) },
  { write: JSON.stringify({ type: 'user', uuid: null }) },
  { write: JSON.stringify({ uuid: '', type: 'user', z: 1 }) },
  { write: '{"1":"a","b":2,"0":"c","type":"user"}\n' },
  { write: '{"n":1.0,"m":1e21,"big":9007199254740993,"s":"\\u00e9\\ud83d\\ude00"}' },
  { write: '[1,2,3]' },
  { write: '"text"' },
  { write: '5' },
  { write: 'null' },
  { write: 'true' },
  { write: '{"type":"control_request","request_id":"w1","request":{"subtype":"interrupt"}}\n' },
  { write: '{"type":"control_request","request_id":5}' },
  { write: '{"type":"control_request"}' },
  { write: '{"type":"control_cancel_request","request_id":"w2"}' },
  { write: 'not json' },
  { write: '' },
  { write: '{"type":"user"} trailing' },
]);
{
  const writes = Array.from({ length: 505 }, (_, index) => ({ write: JSON.stringify({ type: 'control_request', request_id: `q${index}`, request: {} }) }));
  session('issued cap', [
    ...writes,
    ...chunks(
      frame('client_event', '1', { type: 'control_response', response: { request_id: 'q0' } }, { source: 'client' }),
      frame('client_event', '2', { type: 'control_response', response: { request_id: 'q4' } }, { source: 'client' }),
      frame('client_event', '3', { type: 'control_response', response: { request_id: 'q5' } }, { source: 'client' }),
      frame('client_event', '4', { type: 'control_response', response: { request_id: 'q504' } }, { source: 'client' }),
    ),
  ]);
  session('issued cap with expected shadow', [
    { expect: 'q0' },
    { expect: 'q1' },
    ...writes,
  ]);
  session('issued repeat does not evict', [
    { write: JSON.stringify({ type: 'control_request', request_id: 'dup', request: {} }) },
    { write: JSON.stringify({ type: 'control_request', request_id: 'dup', request: {} }) },
    { write: JSON.stringify({ type: 'control_request', request_id: 'dup', request: {} }) },
  ]);
}
{
  const expects = Array.from({ length: 1030 }, (_, index) => ({ expect: `x${index}` }));
  session('expected cap', [
    ...expects,
    ...chunks(
      frame('client_event', '1', { type: 'control_response', response: { request_id: 'x0' } }, { source: 'client' }),
      frame('client_event', '2', { type: 'control_response', response: { request_id: 'x5' } }, { source: 'client' }),
      frame('client_event', '3', { type: 'control_response', response: { request_id: 'x6' } }, { source: 'client' }),
    ),
  ]);
  session('expected cap with issued shadow', [
    { write: JSON.stringify({ type: 'control_request', request_id: 'x0', request: {} }) },
    ...expects,
  ]);
  session('expected repeat does not evict', [{ expect: 'e' }, { expect: 'e' }]);
}
session('drop sampling', chunks(...Array.from({ length: 130 }, (_, index) => frame('client_event', String(index + 1), { type: 5 }))));
session('drop sampling warn', chunks(...Array.from({ length: 110 }, (_, index) => frame('client_event', String(index + 1), { type: 'control_response' }))));
session('catch up sampling', chunks(...Array.from({ length: 105 }, () => ({ event: 'catch_up_truncated', data: '{}' }))));
session('all categories', chunks(
  { event: 'catch_up_truncated', data: '{}' }, { event: 'delivery_update', data: '{}' }, frame('client_event', '1', {}), frame('ephemeral_event', undefined, { type: 'user' }),
  frame('client_event', '2', { type: 'control_response' }), frame('client_event', '3', { type: 'user', tool_use_result: 1 }),
  frame('client_event', '4', { type: 'user', message: { content: [{ type: 'text', text: 1 }] } }, { source: 'worker' }),
  { event: 'client_event', data: 'null' }, frame('client_event', '5', { type: 'assistant' }),
));
session('multi line data and chunking', [
  { chunk: 'event: client_event\nid: 1\ndata: {"event_id":"e","payload":\ndata: {"type":"user"}}\n\n' },
  { chunk: 'event: client_event\r\nid: 2\r\ndata: {"payload":{"type":"user","message":{"content":"x"}}}\r\n\r\n' },
  { chunk: 'event: client_event\rid: 3\rdata: {"payload":{"type":"user"}}\r\r' },
  { chunk: 'event: client_event\nid: 4\ndata: {"pay' },
  { chunk: 'load":{"type":"user"}}\n' },
  { chunk: '\n' },
  { chunk: ': keepalive\n\n' },
  { chunk: 'data: stray\n\n' },
]);

const sessionCases = [];
for (const item of sessions) sessionCases.push(await runSession(item));

// Randomly generated sessions.
const idPool = [undefined, undefined, '1', '5', '0', '-1', 'abc', '12abc', ' 7', '1e3', '9007199254740993', '9007199254740991', '00012', '+8'];
const sourcePool = [undefined, 'worker', 'worker', 'client', 'server', '', null, 5, ['a'], { x: 1 }, true];
const requestIds = ['r1', 'r2', 'r3', 'x', 5, undefined];
const contentTexts = markers.slice(0, 40).concat(['plain', 'hello world', '']);
function randomContent() {
  switch (below(7)) {
    case 0: return pick(contentTexts);
    case 1: return [{ type: 'text', text: pick(contentTexts) }];
    case 2: return [{ type: 'text', text: pick(contentTexts) }, { type: pick(['text', 'image', 'tool_result', 5]), text: pick([1, 'x', null]) }];
    case 3: return [pick([null, 5, 'str', [], {}]), { type: 'text', text: 'ok' }];
    case 4: return pick([null, 5, {}, []]);
    case 5: return [{ type: 'tool_result', content: 'x' }];
    default: return undefined;
  }
}
function randomPayload() {
  const extra = chance(6) ? { ccr_event: { a: 1 } } : {};
  switch (below(12)) {
    case 0:
    case 1:
    case 2: {
      const payload = { type: 'user', ...extra };
      if (!chance(5)) payload.message = chance(6) ? pick([null, 5, 'text', []]) : { content: randomContent() };
      if (chance(8)) payload.tool_use_result = pick([null, 1, 'x']);
      if (chance(8)) payload.plugin_prompt = pick([null, 'p']);
      return payload;
    }
    case 3:
    case 4: {
      const response = chance(8) ? pick([null, 'x', 5, [], {}]) : { request_id: pick(requestIds), subtype: 'success' };
      if (response && typeof response === 'object' && chance(2)) {
        response.pending_user_dialog_requests = [1];
        response.pending_permission_requests = [];
        response.pending_elicitation_requests = {};
        response.error_code = pick(['e', 1, null]);
        response.keep = 1;
      }
      return { type: 'control_response', response, ...extra };
    }
    case 5: return { type: pick(['control_request', 'control_cancel_request']), request_id: pick(requestIds), ...extra };
    case 6: return { type: pick(['env_manager_log', 'assistant', 'result', 'system', 'weird']), n: 1, ...extra };
    case 7: return { type: 'system', subtype: pick(['thinking_tokens', 'post_turn_summary', 'task_summary', 'commands_changed', 'init', 5, undefined]), uuid: pick(['u', undefined, 5]), session_id: pick(['s', null]), estimated_tokens: 1, detail: 'd', commands: [], needs_action: true, junk: 1, ...extra };
    case 8: return { type: pick(['stream_event', 'sources_changed']), event: { e: 1 }, ...extra };
    case 9: return pick([{}, null, 5, 'x', [], { type: null }, { type: 5 }]);
    default: return { type: 'assistant', message: { content: 'x' }, ...extra };
  }
}
function randomFrame() {
  const kind = below(20);
  if (kind === 0) return { event: 'catch_up_truncated', data: pick(['{}', 'x', undefined]) };
  if (kind === 1) {
    return { event: 'delivery_update', data: pick(['{"event_id":"e","status":"s"}', '{"event_id":"e","status":"s","timestamp":"t"}', '{"event_id":1,"status":"s"}', '{}', 'null', 'bad']) };
  }
  if (kind === 2) return { event: pick(['ping', undefined, 'message']), id: pick(idPool), data: pick(['{}', 'x']) };
  if (kind === 3) return { event: pick(['client_event', 'ephemeral_event']), id: pick(idPool), data: pick(['bad', 'null', '5', '[]', '"s"', 'true', '', ' ']) };
  const event = chance(5) ? 'ephemeral_event' : 'client_event';
  const envelope = {
    event_id: pick(['e1', 'e2', 5, undefined]),
    sequence_num: pick([undefined, { toString: 1 }, 3, 7, 7.5, '9', 'x', 0, -2, [11], null, 9007199254740993, 1e3]),
    payload: randomPayload(),
    created_at: pick([undefined, 'c', 5]),
    source: pick(sourcePool),
  };
  return { event, id: pick(idPool), data: JSON.stringify(envelope) };
}
const randomSessions = [];
for (let round = 0; round < 260; round += 1) {
  const steps = [];
  for (let count = below(3); count > 0; count -= 1) {
    steps.push(chance(3) ? { expect: pick(['r1', 'r2', 'r3']) } : { write: JSON.stringify({ type: 'control_request', request_id: pick(['r1', 'r2', 'r3']), request: {} }) });
  }
  const items = Array.from({ length: 1 + below(8) }, randomFrame);
  const text = items.map(frameText).join('');
  const buffer = bytesOf(text);
  const pieces = chance(2) ? [text] : splitAt(buffer, Array.from({ length: 1 + below(4) }, () => below(buffer.length + 1)));
  for (const piece of pieces) steps.push({ chunk: typeof piece === 'string' ? piece : piece.toString('latin1') });
  // Chunks must stay valid text for the scripted body, so cut at character boundaries instead of bytes.
  randomSessions.push({ name: `random ${round}`, steps: collapseChunks(steps, text), from: chance(4) ? pick([5, 20, 0]) : undefined });
}
function collapseChunks(steps, text) {
  const head = steps.filter((step) => step.chunk === undefined);
  const characters = [...text];
  const cuts = new Set(Array.from({ length: below(4) }, () => below(characters.length + 1)));
  const out = [];
  let current = '';
  characters.forEach((character, index) => {
    if (cuts.has(index) && current) {
      out.push({ chunk: current });
      current = '';
    }
    current += character;
  });
  if (current) out.push({ chunk: current });
  return [...head, ...out];
}
for (const item of randomSessions) sessionCases.push(await runSession(item));
write('sessions.json', sessionCases);
