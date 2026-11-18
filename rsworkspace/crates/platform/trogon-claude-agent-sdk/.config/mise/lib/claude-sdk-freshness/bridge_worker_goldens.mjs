#!/usr/bin/env node
// Records what the pinned TypeScript package's worker client (bridge.mjs) sends to a local HTTP server and how it
// reacts, so the Rust worker client is checked against the real code instead of against a reading of it.
//
// usage: node bridge_worker_goldens.mjs <path to bridge.mjs> <out-file> [--check]
//
// The worker client is not exported, so every case drives `attachBridgeSession` with `outboundOnly` (no read stream)
// and watches the local server: the register call, the PUT /worker the client opens with, the state and metadata
// reports, the heartbeats, and the close code the session reports. The reason of an epoch mismatch is read from the
// debug log the bundle writes. The pure tables (constants, close codes, classification) are read from the bundle's
// own source text. GET /worker (the stored state read) is left out of every request list.
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

// The recorder reads the real clock. The bundle reads `Date.now`, `performance.now` and `setTimeout` through the
// globals, which a case can bend (`spec.clock`): the first liveness timer of the run (45 s) fires after 300 ms instead,
// and the clocks jump forward by what the silence would have been, as if the process had slept. The wall clock and the
// monotonic clock jump separately, because the transport reads the longer of the two gaps.
const realNow = Date.now.bind(Date);
const realPerformanceNow = performance.now.bind(performance);
const realSetTimeout = globalThis.setTimeout;
const clock = { wall: 0, mono: 0, jump: null, deadlines: [] };
Date.now = () => realNow() + clock.wall;
performance.now = () => realPerformanceNow() + clock.mono;
globalThis.setTimeout = (callback, delay, ...args) => {
  // The side question deadline (660000 ms) is shortened to 300 ms, and the delay the bundle asked for is kept.
  if (delay === 660000) {
    clock.deadlines.push(delay);
    return realSetTimeout(callback, 300, ...args);
  }
  if (clock.jump !== null && delay === 45000) {
    const jump = clock.jump;
    clock.jump = null;
    return realSetTimeout(() => {
      clock.wall += jump.wall;
      clock.mono += jump.mono;
      callback(...args);
    }, 300);
  }
  return realSetTimeout(callback, delay, ...args);
};

const [bridgeArg, outArg, ...flags] = process.argv.slice(2);
const bridgePath = bridgeArg ? path.resolve(bridgeArg) : bridgeArg;
const out = outArg ? path.resolve(outArg) : outArg;
if (!bridgePath || !out) {
  process.stderr.write('usage: bridge_worker_goldens.mjs BRIDGE_MJS OUT_FILE [--check]\n');
  process.exit(2);
}
for (const name of Object.keys(process.env)) {
  if (name.startsWith('CLAUDE_CODE_') || name.startsWith('CLAUDE_SESSION_')) delete process.env[name];
}
const work = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'bridge-worker-goldens-')));
const debugLog = path.join(work, 'debug.txt');
process.env.CLAUDE_CODE_DEBUG_LOGS_DIR = debugLog;
process.argv.push('--debug');
process.chdir(work);
const source = fs.readFileSync(bridgePath, 'utf8');
const realFetch = globalThis.fetch;
const fetchErrors = [];
globalThis.fetch = async (...args) => {
  try {
    return await realFetch(...args);
  } catch (error) {
    fetchErrors.push({ url: String(args[0]), name: error.name, code: typeof error.code === 'string' ? error.code : null });
    throw error;
  }
};
// The bundle is loaded from a copy that also exports `YPe`, the queued notification schema, because it is built from
// the bundle's schema library and cannot be lifted as text.
const bundleCopy = fs.mkdtempSync(path.join(os.tmpdir(), 'bridge-worker-bundle-'));
fs.writeFileSync(path.join(bundleCopy, 'bridge.mjs'), `${source}\nexport { YPe as __YPe };\n`);
const bridge = await import(pathToFileURL(path.join(bundleCopy, 'bridge.mjs')).href);
fs.rmSync(bundleCopy, { recursive: true, force: true });

// ---- pure tables, read from the bundle source ----
const between = (from, to) => {
  const start = source.indexOf(from);
  const end = source.indexOf(to, start);
  if (start < 0 || end < 0) throw new Error(`bundle layout changed around ${from}`);
  return source.slice(start, end);
};
const evaluate = (code, names, scope = {}) => new Function(...Object.keys(scope), `${code}\nreturn {${names.join(',')}};`)(...Object.values(scope));
const heartbeatTable = evaluate(between('var e5={},I5=', 'function Wn('), ['I5', 't5', 'L5', 'P5', 'N5', 'P2e', 'N2e', 'n5', 'M2e', 'U2e', 'r5', 'F2e', 'o5', 's5']);
const closeTable = evaluate(between('var rHe=10,U5=', 'async function uHe('), ['rHe', 'U5', 'oHe', 'sHe', 'iHe', 'aHe', 'Au', 'cHe']);
const uploadTable = evaluate(between('var dHe=1e4', 'function ZA('), ['dHe', 'pHe', 'l5', 'fHe', 'd5']);
const sessionTable = evaluate(`${between('var S$e=180000', 'async function RC(')}`.replace('var S$e', 'var Sc').replace('y$e', 'yc'), ['Sc', 'yc']);
const eventTable = evaluate(`var ${between('oC="/worker/event-batch"', 'var X2e=')};var ${between('JA={', ',mHe=512')};`, ['JA', 'a5', 'sC', 'iC', 'hm']);
const markerTable = evaluate(`var mHe=512,hHe="upgrade_relay_marker";`, ['mHe', 'hHe']);
const dedupCapacity = evaluate('var x9=2000;', ['x9']).x9;
if (!/var x9=2000;/.test(source) || !source.includes('mHe=512,hHe="upgrade_relay_marker"')) throw new Error('bundle layout changed');
const classify = evaluate(`${between('function tC(', 'function m4e(')}`, ['tC']).tC;
const requiresAction = evaluate(between('function F5(', 'var gHe='), ['F5']).F5;
const metadataTable = evaluate(between('function p4e(', 'async function eC('), ['p4e']).p4e;
const streamTable = evaluate(`${between('function fl(', 'class ml{')};var ${between('W4e=1000,j4e=30000,', ',E9=new DOMException').replace(',K4e=60000', '')};var ${between('S9=3,X4e=', ',y9=25')};var b9=2000,t$e=new Set([401,403,404]);`, ['fl', 'W4e', 'j4e', 'V4e', 'bC', 'z4e', 'S9', 'X4e', 'b9', 't$e']);
const frameParser = evaluate(`${between('var T4e={stream:!0};', 'var e9=30000')}`, ['dC']).dC;
// `Kt`, the text cleaner every control_response error goes through, read from the bundle (its two regular
// expressions are set up lazily inside the bundle, so they are lifted here by their literal text).
const sanitizer = evaluate(`${between('function Dn(e,t){', 'function Pq(e){')}function Pq(e){return Buffer.from(e,"utf16le").toString("utf16le")}${between('function Hq(e){', 'var Bq,$q,')}var Bq=${between('Bq=/', ';$q=').slice(3)},$q=${between('$q=/', '})').slice(3)},mk=1024;`, ['Kt']).Kt;
const statuses = [undefined, 0, 200, 399, 400, 404, 499, 500, 599];

const sessionUrl = evaluate(`const GX=/^[a-zA-Z0-9_-]+$/;${between('function sd(', 'function UD(')}${between('function TC(', 'async function R9(')}`, ['TC']).TC;
const urlResult = (base, id) => {
  try { return { url: sessionUrl(base, id) }; } catch (error) { return { error: error.message }; }
};

// The wait a 429 asks for, read the way the request path reads it (the bundle keeps it inline in that path).
const retryAfter = (header) => {
  const seconds = header ? parseInt(header, 10) : NaN;
  return !isNaN(seconds) && seconds >= 0 ? seconds * 1000 : null;
};

const refusal = evaluate(between('var wz=/^req_', 'function Dh('), ['Us', 'sl']);
const refusalSource = (headers) => refusal.Us((name) => headers[name] ?? null);
const tokenExpiry = evaluate(`const Zt=(e)=>JSON.parse(e);${between('function V1e(', 'ci();var PG=')}`, ['eA']).eA;
const jwt = (claims) => `h.${Buffer.from(typeof claims === 'string' ? claims : JSON.stringify(claims)).toString('base64url')}.s`;

// The inbound router `J4` (what `onInboundMessage`, `onPermissionResponse`, the control dispatch and the cancel hook
// are fed from) read from the bundle with the logging and telemetry calls stubbed. The queued notification check
// `ZPe` and parser `XPe` are read from the bundle too, and `XPe` validates with the bundle's own `YPe` schema.
const routerHarness = evaluate(`const Zt=(e)=>JSON.parse(e);const A=()=>{},we=()=>{},le=()=>{},ie=()=>{},X=(e)=>String(e);${between('function H4(', 'Ee();class $4')}${between('function Y4(', 'Ne();var yEt=')}${between('class If{', 'var YPe=')}${between('function XPe(', 'ne();Sn();Ee();var s$=')}${between('function X4(', 'var NPe=')}`, ['J4', 'If'], { YPe: bridge.__YPe });
const plain = (value) => JSON.parse(JSON.stringify(value));
const routed = (lines, { echo = [], buffer = false } = {}) => {
  const { J4, If } = routerHarness;
  const echoSet = new If(2000);
  const delivered = new If(2000);
  for (const uuid of echo) echoSet.add(uuid);
  const out = [];
  for (const line of lines) {
    J4(line, echoSet, delivered, (m) => out.push(['user', plain(m)]), (m) => out.push(['control_response', plain(m)]), (m) => out.push(['control_request', plain(m)]), (id) => out.push(['control_cancel', id]), buffer ? (m) => out.push(['queued_notification', plain(m)]) : undefined);
    out.push(['next']);
  }
  return { echo, buffer, lines, out };
};
const user = (uuid, extra = {}) => JSON.stringify({ type: 'user', ...(uuid === undefined ? {} : { uuid }), message: { role: 'user', content: 'hi' }, ...extra });
const notice = { type: 'queued_notification', notification: { notification_id: 'n1', origin: 'mcp_send_message', content: 'c' } };
const inboundCases = [
  routed([JSON.stringify({ type: 'control_response', response: { subtype: 'success', request_id: 'r1' } })]),
  routed([JSON.stringify({ type: 'control_response', requestId: 'top', response: { subtype: 'success', requestId: 'r1' } })]),
  routed([JSON.stringify({ type: 'control_response', response: { subtype: 'success', requestId: 'late-bind-1' } })]),
  routed([JSON.stringify({ type: 'control_response', response: { subtype: 'success', requestId: 'LATE-BIND-x', request_id: 'keep' } })]),
  routed([JSON.stringify({ type: 'control_response', response: { subtype: 'success', requestId: null } })]),
  routed([JSON.stringify({ type: 'control_response', response: { subtype: 'success', requestId: 7 } })]),
  routed([JSON.stringify({ type: 'control_response', response: null, uuid: 'c1' })]),
  routed([JSON.stringify({ type: 'control_response', uuid: 'c2' })]),
  routed([JSON.stringify({ type: 'control_response', response: 'text' })]),
  routed([JSON.stringify({ type: 'control_response', response: [1] })]),
  routed([JSON.stringify({ type: 'control_request', request_id: 'r1', request: { subtype: 'interrupt' } })]),
  routed([JSON.stringify({ type: 'control_request', requestId: 'r2', request: { subtype: 'interrupt' } })]),
  routed([JSON.stringify({ type: 'control_request', requestId: 'late-bind-9', request: { subtype: 'interrupt' } })]),
  routed([JSON.stringify({ type: 'control_request', request_id: null, request: { subtype: 'x' } })]),
  routed([JSON.stringify({ type: 'control_request', request_id: 'r3', request: null })]),
  routed([JSON.stringify({ type: 'control_request', request_id: 'r3' })]),
  routed([JSON.stringify({ type: 'control_request', request: { subtype: 'x' } })]),
  routed([JSON.stringify({ type: 'control_cancel_request', request_id: 'r1' })]),
  routed([JSON.stringify({ type: 'control_cancel_request', requestId: 'r9' })]),
  routed([JSON.stringify({ type: 'control_cancel_request', request_id: 5 })]),
  routed([JSON.stringify({ type: 'control_cancel_request' })]),
  routed([JSON.stringify(notice)]),
  routed([JSON.stringify(notice)], { buffer: true }),
  routed([JSON.stringify({ type: 'queued_notification' })], { buffer: true }),
  routed([JSON.stringify({ type: 'queued_notification', notification: null })]),
  routed([user('u1')]),
  routed([user('u1'), user('u1'), user('u2')]),
  routed([user('u1')], { echo: ['u1'] }),
  routed([user('u1'), user('u1')], { echo: ['u2'] }),
  routed([user(undefined), user(undefined)]),
  routed([user(''), user('')]),
  routed([user(5), user(5)]),
  routed([user('u1', { isReplay: true }), user('u1')]),
  routed([user('u1', { isReplay: false }), user('u1')]),
  routed([user('u1', { isReplay: 'true' })]),
  routed([user('u1', { parent_tool_use_id: 'tool' }), user('u1')]),
  routed([user('u1', { parent_tool_use_id: null }), user('u1')]),
  routed([user('u1', { parent_tool_use_id: '' })]),
  routed([user('u1', { historical: true, display_only: true }), user('u1')]),
  routed([user('u1', { historical: true })]),
  routed([user('u1', { display_only: true })]),
  routed([user('u1', { historical: 'true', display_only: true })]),
  routed([JSON.stringify({ type: 'assistant', uuid: 'a1', message: {} }), JSON.stringify({ type: 'assistant', uuid: 'a1', message: {} })]),
  routed([JSON.stringify({ type: 'system', uuid: 'a1' })], { echo: ['a1'] }),
  routed([JSON.stringify({ type: 'result' })]),
  routed([JSON.stringify({ type: 5 })]),
  routed([JSON.stringify({ type: null })]),
  routed([JSON.stringify({})]),
  routed(['null', '5', '"text"', 'true', '[]', '[{"type":"user"}]', '{', '', 'not json', '{"type":"user"']),
  routed([user('u1'), JSON.stringify({ type: 'control_response', response: { subtype: 'success' } }), user('u1'), user('u3')]),
];
const uuidSetOps = (capacity, ops) => {
  const set = new routerHarness.If(capacity);
  const results = ops.map(([op, uuid]) => {
    if (op === 'add') { set.add(uuid); return null; }
    if (op === 'clear') { set.clear(); return null; }
    return set.has(uuid);
  });
  return { capacity, ops, results };
};
const uuidSetCases = [
  uuidSetOps(3, [['add', 'a'], ['add', 'b'], ['add', 'c'], ['has', 'a'], ['add', 'd'], ['has', 'a'], ['has', 'b'], ['add', 'b'], ['add', 'e'], ['has', 'b'], ['has', 'c'], ['has', 'd'], ['has', 'e'], ['clear', ''], ['has', 'e'], ['add', 'x'], ['has', 'x']]),
  uuidSetOps(1, [['add', 'a'], ['add', 'a'], ['add', 'b'], ['has', 'a'], ['has', 'b']]),
  uuidSetOps(2000, [...Array.from({ length: 2001 }, (_, i) => ['add', `u${i}`]), ['has', 'u0'], ['has', 'u1'], ['has', 'u2000'], ['add', 'u1'], ['add', 'late'], ['has', 'u1'], ['has', 'u2'], ['has', 'late']]),
];

const pure = {
  refusal_sources: [
    {}, { 'request-id': 'req_abc' }, { 'request-id': ' req_abc ' }, { 'request-id': 'req_' }, { 'request-id': 'req_' + 'a'.repeat(36) },
    { 'request-id': 'req_' + 'a'.repeat(37) }, { 'request-id': 'abc' }, { 'request-id': 'req_a b' }, { 'cf-ray': 'abc-LHR' }, { 'cf-ray': '' },
    { server: 'cloudflare' }, { server: 'Cloudflare-nginx' }, { server: 'nginx' }, { 'request-id': 'bad', 'cf-ray': 'x' }, { 'request-id': 'req_ok', server: 'cloudflare' },
  ].map((headers) => { const source = refusalSource(headers); return { headers, source, non_origin: refusal.sl(source) }; }),
  token_expiry: [
    jwt({ exp: 1 }), `sk-ant-si-${jwt({ exp: 2.5 })}`, jwt({ exp: 99999999999 }), jwt({}), jwt({ exp: '5' }), jwt(null), jwt([1]), jwt('not json'), jwt({ exp: -4 }), jwt({ exp: 1e400 }),
    'a.b', 'a..c', 'a.b.c.d', 'x', '', 'h.e30.s', 'h.e30=.s', 'h.eyJleHAiOjd9.s', 'h.ey Jle HAiOjd9.s', 'h.eyJleHAiOjd9==junk.s', 'h.eyJleHAiOjd9!.s', 'h.eyJleHAiOjd9ZZ.s', 'h.eyJleHAiOjd+/.s', `h.${Buffer.from('{"exp":7}').toString('base64')}.s`,
  ].map((token) => ({ token, exp: tokenExpiry(token) })),
  retry_after: ['1', '+3', '-1', 'abc', '1.9', '0', '-0', '99999999999999999999999', '1e3', '0x10', '7x', ' 5'].map((header) => ({ header, ms: retryAfter(header) })),
  session_urls: [['http://h', 'cse_abc'], ['http://h/', 'cse_abc'], ['http://h///', 'x-1'], ['https://h:8080/p/', 'A_9'], ['', 'x'], ['http://h', 'bad/id'], ['http://h', ''], ['http://h', 'a b'], ['http://h', 'caf\u00e9']]
    .map(([base, id]) => ({ base, id, ...urlResult(base, id) })),
  constants: {
    heartbeat_interval_ms: heartbeatTable.I5,
    heartbeat_interval_floor_ms: heartbeatTable.t5,
    heartbeat_timeout_ceiling_ms: heartbeatTable.L5,
    heartbeat_timeout_margin_ms: heartbeatTable.P5,
    heartbeat_timeout_floor_ms: heartbeatTable.P2e,
    heartbeat_interval_ceiling_ms: heartbeatTable.N2e,
    auth_failures_before_exit: closeTable.rHe,
    not_found_before_exit: closeTable.sHe,
    read_attempts: closeTable.iHe,
    oversize_read_attempts: closeTable.aHe,
    list_attempts: closeTable.Au,
    agent_metadata_batch: closeTable.cHe,
    init_state_read_bound_ms: uploadTable.dHe,
    transcript_tip_wait_ms: uploadTable.pHe,
    refused_wait_ms: uploadTable.l5,
    long_wait_for_room_ms: uploadTable.fHe,
    non_origin_window_ms: sessionTable.Sc,
    non_origin_max_gap_ms: sessionTable.yc,
    dedup_capacity: dedupCapacity,
    refused_uuid_capacity: markerTable.mHe,
    upgrade_relay_marker: markerTable.hHe,
  },
  lanes: Object.entries(eventTable.JA),
  upload_timeouts: [
    [100, true, 0], [100, false, 0], [262143, false, 3], [262144, true, 0], [262144, false, 0], [262144, false, 1], [262144, false, 2],
    [1048576, false, 0], [1048576, false, 4], [3145728, false, 0], [10485760, false, 0], [10485760, true, 5], [0, false, 9],
  ].map(([bytes, cuttable, retryCount]) => [bytes, cuttable, retryCount, eventTable.a5({ bytes, cuttable, retryCount }, { maxMs: 120000 })]),
  upload_floor_ms: eventTable.sC,
  upload_shrink_from_bytes: eventTable.iC,
  close_codes: closeTable.U5,
  retryable: closeTable.oHe,
  classify_status: statuses.map((status) => [status ?? null, uploadTable.d5(status)]),
  conflict_reasons: [
    'superseded_by_worker', 'epoch_stale', 'session_not_active', 'organization_moved_away', 'epoch_conflict',
    'session_not_found', 'token_expired', 'auth_exhausted', '', 'unknown', 1, null,
  ].map((input) => [input, classify(input) ?? null]),
  requires_action: [
    { tool_name: 'Bash', display_tool_name: 'Run', action_description: 'a', raw_command: 'ls', request_id: 'r', tool_use_id: 't', extra: 1 },
    { tool_name: 'Bash', action_description: 'a', request_id: 'r', tool_use_id: 't' },
  ].map((input) => ({ input, output: JSON.parse(JSON.stringify(requiresAction(input))) })),
  inbound: inboundCases,
  uuid_set: uuidSetCases,
  stream: {
    reconnect_base_ms: streamTable.W4e,
    reconnect_cap_ms: streamTable.j4e,
    keepalive_seconds: streamTable.bC,
    keepalive_ceiling_seconds: streamTable.z4e,
    keepalive_tolerance_factor: streamTable.S9,
    keepalive_tolerance_param: streamTable.X4e,
    liveness_grace_ms: streamTable.V4e,
    silent_connection_ms: streamTable.b9,
    permanent_statuses: [...streamTable.t$e],
    // jitter 0 is the middle of the symmetric 25% band: random 0.5, the low end random 0 and the high end random 1.
    backoff: [1, 2, 3, 4, 5, 6, 7, 8].map((attempt) => [attempt, ...[0, 0.5, 1].map((random) => streamTable.fl({ attempt, baseMs: streamTable.W4e, capMs: streamTable.j4e, jitter: { kind: 'symmetric', ratio: 0.25 }, random: () => random }))]),
    frames: [
      ['id: 1\nevent: e\ndata: x\n\n'],
      ['id: 1\r\nevent: e\r\ndata: x\r\n\r\n'],
      ['id: 1\revent: e\rdata: x\r\r'],
      ['id: 1\nev', 'ent: e\nda', 'ta: x\n', '\n'],
      ['id: 1\r', '\nevent: e\r', '\ndata: x\r', '\n\r', '\n'],
      ['id: 1\nevent: e\ndata: x\n'],
      ['id: 1\nevent: e\ndata: x\n\nid: 2\nevent: e\ndata: y\n\n'],
      ['id: 1\nevent: e\ndata: x\n\nid: 2\nevent: e\ndata: y\n', '\n'],
      [': keepalive\n\n'],
      [': keepalive\n\nevent: e\ndata: x\n\n'],
      ['event: e\ndata: a\ndata: b\ndata: c\n\n'],
      ['event: e\ndata:\ndata: b\n\n'],
      ['event: e\ndata:\n\n'],
      ['event: e\ndata: x\n\n\n\nevent: f\ndata: y\n\n'],
      ['\n\nevent: e\ndata: x\n\n'],
      ['\r\n\r\nevent: e\ndata: x\n\n'],
      ['data: x\n\n'],
      ['event: e\n\n'],
      ['   \n\n'],
      ['id:7\nevent:e\ndata:x\n\n'],
      ['id:  7\nevent:  e\ndata:  x\n\n'],
      ['id: 1\nid: 2\nevent: e\nevent: f\ndata: x\n\n'],
      ['unknown: 1\nevent: e\ndata: x\nno colon line\n\n'],
      ['event: e\ndata: caf\u00e9 \u{1F600}\n\n'],
      [new Uint8Array(Buffer.from('event: e\ndata: caf\u00e9\n\n')).slice(0, 18), new Uint8Array(Buffer.from('event: e\ndata: caf\u00e9\n\n')).slice(18)],
      ['event: e\r\rdata: x\n\n'],
      ['event: e\ndata: x\r\n\n'],
      ['event: e\ndata: x\n\r\nevent: f\ndata: y\n\r\n'],
      [new Uint8Array(Buffer.from('event: e\ndata: \u{1F600}\n\n')).slice(0, 17), new Uint8Array(Buffer.from('event: e\ndata: \u{1F600}\n\n')).slice(17)],
      [new Uint8Array([...Buffer.from('event: e\ndata: '), 0xc3]), new Uint8Array(Buffer.from('\n\n'))],
    ].map((chunks) => {
      const parser = new frameParser();
      const frames = [];
      for (const chunk of chunks) for (const frame of parser.push(chunk)) frames.push(frame);
      return { chunks: chunks.map((chunk) => (typeof chunk === 'string' ? chunk : { bytes: [...chunk] })), frames, pending: parser.buffered() };
    }),
  },
  sanitize: [
    'plain', '', 'a\u0000b', 'tab\there', 'line\nbreak', 'a\u200bb', 'a\u00a0b', 'a\u2028b', 'a\u2800b', 'a\ufe0fb', 'a\u{e0001}b', 'a\u3000b', 'a  b',
    'caf\u00e9', '\u{1f600}', 'x'.repeat(1024), 'x'.repeat(1025), `${'x'.repeat(1023)}\u{1f600}`, `${'x'.repeat(1022)}\u{1f600}tail`, 'y'.repeat(4096), `${'y'.repeat(4095)}\u{1f600}`,
    `${'\u200b'.repeat(4000)}${'z'.repeat(2000)}`, 'a\u061cb', 'a\u0600b', 'a\u00adb', 'a\u00b6b', 'e\u0301',
  ].map((input) => ({ input, output: sanitizer(input) })),
  metadata: [
    { model: 'm' },
    {},
    { post_turn_summary: { status_category: 'blocked', note: 'x' } },
    { post_turn_summary: { status_category: 'done' } },
    { post_turn_summary: 'blocked' },
    { post_turn_summary: null, other: [1] },
  ].map((input) => ({ input, output: JSON.parse(JSON.stringify(metadataTable(input))) })),
};

// ---- cases that drive the real session ----
const json = (status, value, headers = []) => ({ status, headers: [['content-type', 'application/json'], ...headers], body: JSON.stringify(value) });
const raw = (status, body, headers = []) => ({ status, headers, body });
const conflict = (reason) => json(409, {}, [['x-ccr-conflict-reason', reason]]);
const cases = { register: [], init: [], state: [], heartbeat: [], auth: [], queue: [], record: [], events: [], delivery: [], served: [], stream: [], control: [], attach: [], handle: [] };
const expired = jwt({ exp: 1 });
const future = jwt({ exp: 99999999999 });
const originRefusal = (status) => raw(status, '', [['request-id', 'req_abc']]);
const edgeRefusal = (status) => raw(status, '', [['cf-ray', 'abc-LHR']]);
const landing = '{base}/v1/code/sessions/cse_abc/worker/landing';
const add = (section, name, spec) => cases[section].push({ name, ...spec });

for (const [name, response] of [
  ['epoch_number', json(200, { worker_epoch: 5 })],
  ['epoch_string', json(200, { worker_epoch: '7' })],
  ['epoch_padded_string', json(200, { worker_epoch: ' 8\n' })],
  ['epoch_hex_string', json(200, { worker_epoch: '0x10' })],
  ['epoch_empty_string', json(200, { worker_epoch: '' })],
  ['epoch_negative', json(200, { worker_epoch: -3 })],
  ['epoch_max_safe', json(200, { worker_epoch: 9007199254740991 })],
  ['epoch_fraction', json(200, { worker_epoch: 1.5 })],
  ['epoch_unsafe', json(200, { worker_epoch: 9007199254740992 })],
  ['epoch_word', json(200, { worker_epoch: 'abc' })],
  ['epoch_null', json(200, { worker_epoch: null })],
  ['epoch_missing', json(200, {})],
  ['epoch_boolean', json(200, { worker_epoch: true })],
  ['body_array', raw(200, '[1,2]')],
  ['body_text', raw(200, 'not json')],
  ['body_empty', raw(200, '')],
  ['body_null', raw(200, 'null')],
  ['body_too_long', json(200, { worker_epoch: 'x'.repeat(1100) })],
  ['body_control_character', json(200, { worker_epoch: 'a\u0007b' })],
  ['body_accented', json(200, { worker_epoch: 'café' })],
  ['body_backtick', json(200, { worker_epoch: 'a`b' })],
  ['status_400', json(400, {})],
  ['status_401', json(401, {})],
  ['status_404', raw(404, '')],
  ['status_500', raw(500, 'boom')],
  ['status_503', raw(503, '')],
]) {
  add('register', name, { epoch: null, register: response, puts: [], beats: 0 });
}
add('register', 'register_then_state', { epoch: null, register: json(200, { worker_epoch: 9 }), puts: [], beats: 1, interval: 30 });
add('register', 'epoch_option_skips_register', { epoch: 4, puts: [], beats: 0 });

add('init', 'ok', { epoch: 3 });
add('init', 'status_422_closes_4091', { epoch: 3, puts: [json(422, {})], waitClose: true });
add('init', 'status_413_closes_4091', { epoch: 3, puts: [json(413, {})], waitClose: true });
add('init', 'status_400_closes_4091', { epoch: 3, puts: [json(400, {})], waitClose: true });
add('init', 'conflict_stale', { epoch: 3, puts: [conflict('epoch_stale')], waitClose: true });
add('init', 'conflict_superseded_body_reason', { epoch: 3, puts: [json(409, { error: { reason: 'superseded_by_worker' } })], waitClose: true });
add('init', 'conflict_unattributed', { epoch: 3, puts: [json(409, {})], waitClose: true });
add('init', 'not_found_three_times', { epoch: 3, puts: [raw(404, '')], waitClose: true });

add('state', 'running', { epoch: 6, steps: [{ op: 'state', state: 'running', sends: true }] });
add('state', 'idle_is_not_repeated', { epoch: 6, steps: [{ op: 'state', state: 'idle', sends: false }, { op: 'state', state: 'running', sends: true }, { op: 'state', state: 'running', sends: false }, { op: 'state', state: 'requires_action', sends: true }, { op: 'state', state: 'idle', sends: true }] });
add('state', 'metadata', { epoch: 6, steps: [{ op: 'metadata', value: { model: 'm', nested: { a: [1, null] } }, sends: true }, { op: 'metadata', value: {}, sends: true }] });
add('state', 'metadata_blocked_summary', { epoch: 6, steps: [{ op: 'metadata', value: { post_turn_summary: { status_category: 'blocked', n: 1 } }, sends: true }] });
add('state', 'ticket_headers_are_echoed', { epoch: 6, puts: [json(200, {}, [['x-ccr-ticket-rev', 'rev1'], ['x-ccr-ticket-session', 'sess_1'], ['x-ccr-ticket-seq', '7']]), json(200, {}, [['x-ccr-ticket-rev', '']]), json(200, {}, [['x-ccr-ticket-seq', 'bad value']])], steps: [{ op: 'state', state: 'running', sends: true }, { op: 'metadata', value: { a: 1 }, sends: true }, { op: 'metadata', value: { b: 1 }, sends: true }] });
add('state', 'ticket_headers_are_held_between_requests', { epoch: 6, puts: [json(200, {}, [['x-ccr-ticket-rev', 'rev1']]), json(200, {})], steps: [{ op: 'state', state: 'running', sends: true }, { op: 'metadata', value: { a: 1 }, sends: true }] });

add('heartbeat', 'three_beats', { epoch: 2, interval: 30, beats: 3 });
add('heartbeat', 'interval_answer_slows_the_cadence', { epoch: 2, interval: 30, beats: 2, beat: [json(200, { heartbeat_interval_seconds: 30 })], quiet: true });
add('heartbeat', 'interval_answer_fractional', { epoch: 2, interval: 30, beats: 2, beat: [json(200, { heartbeat_interval_seconds: 0.01 })], quiet: true });
for (const [name, answer] of [['zero', 0], ['negative', -1], ['text', 'x'], ['null', null], ['missing', undefined]]) {
  add('heartbeat', `interval_answer_${name}_keeps_the_cadence`, { epoch: 2, interval: 30, beats: 2, beat: [json(200, answer === undefined ? {} : { heartbeat_interval_seconds: answer })], quiet: false });
}
add('heartbeat', 'answer_not_json', { epoch: 2, interval: 30, beats: 2, beat: [raw(200, 'ok')], quiet: false });
add('heartbeat', 'failures_do_not_close', { epoch: 2, interval: 30, beats: 4, beat: [raw(500, ''), raw(429, '', [['retry-after', '1']]), raw(400, ''), json(200, {})] });
add('heartbeat', 'not_found_resets_on_success', { epoch: 2, interval: 30, beats: 6, beat: [raw(404, ''), raw(404, ''), json(200, {}), raw(404, ''), raw(404, ''), json(200, {})] });
add('heartbeat', 'not_found_three_times', { epoch: 2, interval: 30, beat: [raw(404, '')], waitClose: true });
add('heartbeat', 'unauthorized_ten_times', { epoch: 2, interval: 30, beat: [raw(401, '')], waitClose: true });
add('heartbeat', 'forbidden_ten_times', { epoch: 2, interval: 30, beat: [raw(403, '')], waitClose: true });
add('heartbeat', 'auth_failures_reset_on_success', { epoch: 2, interval: 30, beat: [...Array(9).fill(raw(401, '')), json(200, {}), ...Array(9).fill(raw(403, '')), json(200, {})], beats: 20 });
for (const reason of ['superseded_by_worker', 'epoch_stale', 'session_not_active', 'organization_moved_away']) {
  add('heartbeat', `conflict_header_${reason}`, { epoch: 2, interval: 30, beat: [conflict(reason)], waitClose: true });
}
add('heartbeat', 'conflict_body_error_reason', { epoch: 2, interval: 30, beat: [json(409, { error: { reason: 'epoch_stale' } })], waitClose: true });
add('heartbeat', 'conflict_body_reason', { epoch: 2, interval: 30, beat: [json(409, { reason: 'session_not_active' })], waitClose: true });
add('heartbeat', 'conflict_body_error_type', { epoch: 2, interval: 30, beat: [json(409, { error: { type: 'organization_moved_away' } })], waitClose: true });
add('heartbeat', 'conflict_header_beats_body', { epoch: 2, interval: 30, beat: [json(409, { error: { reason: 'epoch_stale' } }, [['x-ccr-conflict-reason', 'session_not_active']])], waitClose: true });
add('heartbeat', 'conflict_bad_header_uses_body', { epoch: 2, interval: 30, beat: [json(409, { reason: 'epoch_stale' }, [['x-ccr-conflict-reason', 'nope']])], waitClose: true });
add('heartbeat', 'conflict_unknown_reason', { epoch: 2, interval: 30, beat: [conflict('nope')], waitClose: true });
add('heartbeat', 'conflict_text_body', { epoch: 2, interval: 30, beat: [raw(409, 'no')], waitClose: true });
add('heartbeat', 'conflict_empty_body', { epoch: 2, interval: 30, beat: [raw(409, '')], waitClose: true });
add('heartbeat', 'conflict_error_not_object', { epoch: 2, interval: 30, beat: [json(409, { error: 'epoch_stale' })], waitClose: true });

add('state', 'redirected_answer_drops_the_tickets', { epoch: 6, puts: [json(200, {}, [['x-ccr-ticket-rev', 'rev1']]), raw(307, '', [['location', landing]]), json(200, {})], landing: [json(200, {}, [['x-ccr-ticket-rev', 'rev9']])], steps: [{ op: 'state', state: 'running', sends: true }, { op: 'metadata', value: { a: 1 }, sends: true }, { op: 'metadata', value: { b: 1 }, sends: true }] });

add('auth', 'expired_token_on_401', { epoch: 2, interval: 30, sessionToken: expired, beat: [raw(401, '')], waitClose: true });
add('auth', 'expired_token_on_403', { epoch: 2, interval: 30, sessionToken: expired, beat: [originRefusal(403)], waitClose: true });
add('auth', 'expired_token_with_prefix', { epoch: 2, interval: 30, sessionToken: `sk-ant-si-${expired}`, beat: [raw(401, '')], waitClose: true });
add('auth', 'expired_token_on_non_origin_403', { epoch: 2, interval: 30, sessionToken: expired, beat: [raw(403, '')], waitClose: true });
add('auth', 'expired_token_on_500_is_ignored', { epoch: 2, interval: 30, sessionToken: expired, beat: [raw(500, '')], beats: 4 });
add('auth', 'expired_token_on_success_is_ignored', { epoch: 2, interval: 30, sessionToken: expired, beats: 3 });
add('auth', 'future_token_counts_failures', { epoch: 2, interval: 30, sessionToken: future, beat: [raw(401, '')], waitClose: true });
add('auth', 'token_without_exp_counts_failures', { epoch: 2, interval: 30, sessionToken: jwt({}), beat: [raw(401, '')], waitClose: true });
add('auth', 'opaque_token_counts_failures', { epoch: 2, interval: 30, sessionToken: 'opaque', beat: [raw(401, '')], waitClose: true });
add('auth', 'origin_403_counts_failures', { epoch: 2, interval: 30, beat: [originRefusal(403)], waitClose: true });
add('auth', 'non_origin_403_never_exhausts', { epoch: 2, interval: 30, beat: [raw(403, '')], beats: 14 });
add('auth', 'edge_403_never_exhausts', { epoch: 2, interval: 30, beat: [edgeRefusal(403)], beats: 14 });
add('auth', 'non_origin_401_still_counts', { epoch: 2, interval: 30, beat: [edgeRefusal(401)], waitClose: true });
add('auth', 'non_origin_403_does_not_reset_the_count', { epoch: 2, interval: 30, beat: [...Array(9).fill(originRefusal(403)), ...Array(5).fill(raw(403, '')), originRefusal(403)], waitClose: true });

const burst = (expect, ...ops) => ({ op: 'burst', expect, ops });
add('queue', 'burst_merges_metadata_while_a_state_is_in_flight', { epoch: 6, steps: [burst(2, { state: 'running' }, { metadata: { a: 1 } }, { metadata: { b: 2 } }, { metadata: { a: 3 } })] });
add('queue', 'burst_of_states_keeps_the_last_one_waiting', { epoch: 6, steps: [burst(2, { state: 'running' }, { state: 'requires_action' }, { state: 'idle' })] });
add('queue', 'failures_retry_with_a_doubling_delay', { epoch: 6, puts: [json(200, {}), raw(500, ''), raw(429, '', [['retry-after', '3']]), raw(503, ''), json(200, {})], retryBases: [500, 1000, 2000], steps: [burst(4, { state: 'running' })] });
add('queue', 'refused_state_is_dropped', { epoch: 6, puts: [json(200, {}), raw(400, ''), json(200, {})], steps: [burst(1, { state: 'running' }), burst(1, { state: 'idle' })] });
add('queue', 'refused_metadata_is_dropped', { epoch: 6, puts: [json(200, {}), raw(413, ''), json(200, {})], steps: [burst(1, { metadata: { a: 1 } }), burst(1, { metadata: { b: 1 } })] });
add('queue', 'refused_mixed_patch_requeues_the_status', { epoch: 6, puts: [json(200, {}), json(200, {}), raw(422, ''), json(200, {})], steps: [burst(3, { state: 'running' }, { state: 'idle' }, { metadata: { a: 1 } })] });
add('queue', 'refused_mixed_patch_requeues_owed_metadata', { epoch: 6, puts: [json(200, {}), json(200, {}), raw(422, ''), json(200, {})], steps: [burst(4, { state: 'running' }, { state: 'idle' }, { metadata: { plan_doc: 'x', model: 'm' } })] });
add('queue', 'closed_client_drops_a_conflicting_patch', { epoch: 6, puts: [json(200, {}), raw(409, ''), json(200, {})], steps: [{ ...burst(1, { state: 'running' }), close: true }] });
add('queue', 'a_later_patch_joins_the_retry', { epoch: 6, puts: [json(200, {}), raw(500, ''), json(200, {})], steps: [burst(2, { state: 'running' }, { metadata: { a: 1 }, afterMs: 100 })] });
const stored = (metadata) => json(200, { worker: { external_metadata: metadata } });
add('record', 'slow_read_is_abandoned_at_registration', { epoch: 6, record: stored({ last_served_model: 'm1' }), recordDelayMs: 10600, initWaitMs: 16000, steps: [{ op: 'metadata', value: { last_served_model: 'm1' }, sends: true }] });
add('record', 'read_finishing_inside_the_bound_is_used', { epoch: 6, record: stored({ last_served_model: 'm1' }), recordDelayMs: 1500, steps: [{ op: 'metadata', value: { last_served_model: 'm1' }, sends: false }] });
add('record', 'repeat_of_the_record_is_not_sent', { epoch: 6, record: stored({ last_served_model: 'm1' }), steps: [{ op: 'metadata', value: { last_served_model: 'm1' }, sends: false }, { op: 'metadata', value: { last_served_model: 'm1' }, sends: true }] });
add('record', 'only_the_repeated_keys_are_left_out', { epoch: 6, record: stored({ permission_mode: 'plan' }), steps: [{ op: 'metadata', value: { permission_mode: 'plan', other: 1 }, sends: true }] });
add('record', 'a_changed_value_is_sent', { epoch: 6, record: stored({ permission_mode: 'plan' }), steps: [{ op: 'metadata', value: { permission_mode: 'auto' }, sends: true }] });
add('record', 'a_key_outside_the_set_is_always_sent', { epoch: 6, record: stored({ other: 1 }), steps: [{ op: 'metadata', value: { other: 1 }, sends: true }] });
add('record', 'null_equals_a_missing_key', { epoch: 6, record: stored({}), steps: [{ op: 'metadata', value: { rate_limit_info: null }, sends: false }] });
add('record', 'objects_compare_deeply', { epoch: 6, record: stored({ context_usage: { a: [1, { b: 2 }], c: 1 } }), steps: [{ op: 'metadata', value: { context_usage: { c: 1, a: [1, { b: 2 }] } }, sends: false }, { op: 'metadata', value: { is_ultraplan_mode: true }, sends: true }] });
add('record', 'an_unreadable_record_suppresses_nothing', { epoch: 6, record: raw(400, ''), steps: [{ op: 'metadata', value: { last_served_model: 'm1' }, sends: true }] });
add('record', 'a_record_that_is_not_an_object_counts_as_empty', { epoch: 6, record: json(200, { worker: { external_metadata: 'x' } }), steps: [{ op: 'metadata', value: { rate_limit_info: null }, sends: false }] });

// ---- client events (POST /worker/events) ----
const note = (text, uuid) => ({ type: 'assistant', message: { role: 'assistant', content: [{ type: 'text', text }] }, ...(uuid && { uuid }) });
const thinking = (uuid) => ({ type: 'system', subtype: 'thinking_tokens', uuid });
const post = (expect, writes, extra = {}) => ({ op: 'events', expect, writes, ...extra });
add('events', 'a_write_is_posted_with_the_epoch', { epoch: 6, steps: [post(1, [note('a', 'u1')])] });
add('events', 'a_burst_after_the_first_is_one_batch', { epoch: 6, steps: [post(2, [note('a', 'u1'), note('b', 'u2'), note('c', 'u3')])] });
add('events', 'a_missing_uuid_is_filled_in', { epoch: 6, steps: [post(1, [note('a')])] });
add('events', 'a_failed_batch_is_resent_with_what_arrived_meanwhile', { epoch: 6, events: [raw(500, ''), json(200, {})], eventGaps: [500], steps: [post(2, [note('a', 'u1'), note('b', 'u2')])] });
add('events', 'a_refused_batch_is_resent_whole', { epoch: 6, events: [raw(503, ''), json(200, {})], eventGaps: [500], steps: [post(2, [note('a', 'u1')])] });
add('events', 'retry_after_sets_the_wait', { epoch: 6, events: [raw(429, '', [['retry-after', '2']]), json(200, {})], eventGaps: [2000], steps: [post(2, [note('a', 'u1')])] });
add('events', 'the_wait_doubles_while_it_keeps_failing', { epoch: 6, events: [raw(500, ''), raw(500, ''), raw(500, ''), json(200, {})], eventGaps: [500, 1000, 2000], steps: [post(4, [note('a', 'u1')])] });
add('events', 'a_refused_durable_batch_is_dropped', { epoch: 6, events: [raw(400, ''), json(200, {})], steps: [post(1, [note('a', 'u1')]), post(1, [note('b', 'u2')])] });
add('events', 'a_too_large_batch_is_dropped', { epoch: 6, events: [raw(413, ''), json(200, {})], steps: [post(1, [note('a', 'u1')]), post(1, [note('b', 'u2')])] });
add('events', 'a_mixed_batch_keeps_only_the_durable_rows', { epoch: 6, events: [raw(500, ''), raw(422, ''), json(200, {})], steps: [post(3, [note('a', 'u1'), thinking('t1')])] });
add('events', 'an_ephemeral_batch_that_is_refused_is_dropped', { epoch: 6, events: [raw(400, ''), json(200, {})], steps: [post(1, [thinking('t1')]), post(1, [note('b', 'u2')])] });
add('events', 'a_refused_resend_of_a_mixed_batch_is_dropped', { epoch: 6, events: [raw(500, ''), raw(400, ''), raw(400, ''), json(200, {})], steps: [post(3, [note('a', 'u1'), thinking('t1')]), post(1, [note('c', 'u3')])] });
add('events', 'a_dropped_batch_does_not_stall_the_next', { epoch: 6, events: [raw(400, ''), json(200, {}), json(200, {})], steps: [post(1, [note('a', 'u1')]), post(1, [note('b', 'u2')])] });

// ---- delivery acknowledgements (handle.reportDelivery, POST /worker/events/delivery) ----
const report = (expect, reports, extra = {}) => ({ op: 'delivery', expect, reports, ...extra });
add('delivery', 'a_received_ack_is_posted_with_the_epoch', { epoch: 6, steps: [report(1, [['u1', 'received']])] });
add('delivery', 'a_processed_ack_is_posted', { epoch: 6, steps: [report(1, [['u1', 'processed']])] });
add('delivery', 'a_processing_ack_is_posted', { epoch: 6, steps: [report(1, [['u1', 'processing']])] });
add('delivery', 'a_burst_after_the_first_is_one_batch', { epoch: 6, steps: [report(2, [['u1', 'received'], ['u2', 'received'], ['u3', 'received'], ['u1', 'processed']])] });
add('delivery', 'processing_acks_of_a_burst_are_one_post', { epoch: 6, steps: [report(1, [['u1', 'processing'], ['u2', 'processing'], ['u3', 'processing']])] });
add('delivery', 'a_mixed_burst_sends_received_then_processing_then_processed', { epoch: 6, steps: [report(3, [['u2', 'received'], ['u2', 'processed'], ['u9', 'processing']])] });
add('delivery', 'a_failed_batch_is_resent_with_what_arrived_meanwhile', { epoch: 6, deliveries: [raw(500, ''), json(200, {})], deliveryGaps: [500], steps: [report(2, [['u1', 'received'], ['u2', 'received']])] });
add('delivery', 'retry_after_sets_the_wait', { epoch: 6, deliveries: [raw(429, '', [['retry-after', '2']]), json(200, {})], deliveryGaps: [2000], steps: [report(2, [['u1', 'received']])] });
add('delivery', 'the_wait_doubles_while_it_keeps_failing', { epoch: 6, deliveries: [raw(500, ''), raw(503, ''), raw(500, ''), json(200, {})], deliveryGaps: [500, 1000, 2000], steps: [report(4, [['u1', 'received']])] });
add('delivery', 'a_refused_batch_is_dropped', { epoch: 6, deliveries: [raw(400, ''), json(200, {})], steps: [report(1, [['u1', 'received']]), report(1, [['u2', 'received']])] });
add('delivery', 'a_too_large_batch_is_dropped', { epoch: 6, deliveries: [raw(413, ''), json(200, {})], steps: [report(1, [['u1', 'received']]), report(1, [['u2', 'processed']])] });
add('delivery', 'an_unprocessable_batch_is_dropped', { epoch: 6, deliveries: [raw(422, ''), json(200, {})], steps: [report(1, [['u1', 'received']]), report(1, [['u2', 'received']])] });
add('delivery', 'a_failed_processing_ack_is_not_resent', { epoch: 6, deliveries: [raw(500, ''), json(200, {})], steps: [report(1, [['u1', 'processing']], { settleMs: 1400 }), report(1, [['u2', 'received']])] });
add('delivery', 'a_refused_processing_ack_is_not_resent', { epoch: 6, deliveries: [raw(400, ''), json(200, {})], steps: [report(1, [['u1', 'processing']], { settleMs: 1400 }), report(1, [['u2', 'received']])] });
add('delivery', 'processing_goes_out_while_a_failed_batch_waits', { epoch: 6, deliveries: [raw(500, ''), json(200, {}), json(200, {})], steps: [report(3, [['u1', 'received'], ['u2', 'processing']])] });
add('delivery', 'closing_drops_the_acks_still_queued', { epoch: 6, deliveries: [raw(500, '')], steps: [report(1, [['u1', 'received'], ['u2', 'received'], ['u3', 'processed']], { close: true, settleMs: 1400 })] });
add('delivery', 'a_closed_client_sends_no_processing_ack', { epoch: 6, steps: [report(1, [['u1', 'received']], { close: true }), report(0, [['u2', 'processing'], ['u3', 'received']])] });

// ---- what the service can serve to hold uploads: the heartbeat policy and the has_subscribers answer ----
// `attachBridgeSession` never sets followsServedPolicy, uploadHoldMarker or noSubscriberUploadHoldMs, so none of these
// answers may hold a post. Every run here uses the default options, serves hold-shaped answers, and requires each post
// to arrive promptly (a hold of the served age would show as a late post and fail the recording).
// A `beat` step serves one heartbeat answer and waits for that beat; the other beats the short interval causes are
// answered with the same body and left out of the request list, so the replay can make exactly the beats it records.
const beat = (answer) => ({ op: 'beat', answer });
const subscribers = (has) => json(200, { has_subscribers: has });
const running = { op: 'state', state: 'running', sends: true };
const policy = (extra = {}) => json(200, { event_flush_policy: { max_events: 3, max_age_ms: 1500, ...extra } });
const fullPolicy = {
  max_events: 5, max_age_ms: 1500, max_bytes: 4096, flush_on_sampling_end: true, flush_on_assistant: true, event_batch_upload: true,
  hold_across_model_calls: true, hold_again: { soft_age_ms: 1000, soft_events: 3, after: ['Bash'], never_after_tools: ['Read'], tools_whose_results_wait: ['Bash'], later_worker_holds_first_turn: true },
  may_wait_types: ['system/task_summary'], flush_after_tool_names: ['Write'], flush_on_call_of_tool_names: ['Bash'], settings_receipts_max_wait_ms: 2000,
  messages_received_waits: true, receipts_ride_uploads: true, flush_before_tools_wait_ms: 1000, flush_after_tools: 'all', must_flush_types: ['assistant'],
  live_twin_types: ['system/status'], live_only_types: ['system/task_summary'], transcript_max_age_ms: 3000, no_turn_max_age_ms: 1000, merge_worker_reports: true,
  control_traffic_keeps_hold: true, empty_checkpoint_keeps_hold: true, first_turn_holds_each_call: true, settings_answers_max_wait_ms: 1000, init_max_wait_ms: 1000,
  metadata_waits_for_status: ['task_summary'], metadata_never_sent: ['recap'], metadata_max_wait_ms: 1000, cost_report_max_wait_ms: 1000, policy_version: 'v1',
};
// ---- control traffic: the urgent lane needs servedCallRequestsJumpQueue, which attachBridgeSession never passes ----
// A control request, response or cancel written behind two events in flight must arrive behind them, in write order.
const control = (via, payload) => ({ via, payload });
const permission = { type: 'control_request', request_id: 'r1', request: { subtype: 'can_use_tool', tool_name: 'Bash', input: { command: 'ls' }, tool_use_id: 'tu1' } };
const served = (name, spec) => add('served', name, { epoch: 6, interval: 100, prompt: true, gated: true, ...spec });
const burst3 = post(2, [note('b', 'u2'), note('c', 'u3'), note('d', 'u4')]);
served('a_served_policy_holds_nothing', { steps: [beat(policy()), running, post(1, [note('a', 'u1')]), burst3, post(1, [note('e', 'u5')])] });
served('a_served_policy_with_a_long_age_holds_nothing', { steps: [beat(policy({ max_age_ms: 120000, max_events: 90 })), running, post(1, [note('a', 'u1')]), burst3] });
served('a_served_policy_that_holds_across_model_calls_holds_nothing', { steps: [beat(policy({ hold_across_model_calls: true, max_age_ms: 60000 })), running, post(1, [note('a', 'u1')]), burst3, post(2, [note('e', 'u5'), thinking('t1')])] });
served('a_served_policy_with_every_field_holds_nothing', { steps: [beat(json(200, { event_flush_policy: fullPolicy })), running, post(1, [note('a', 'u1')]), burst3, post(2, [note('e', 'u5'), thinking('t1')])] });
served('a_served_policy_with_every_field_leaves_metadata_alone', { steps: [beat(json(200, { event_flush_policy: fullPolicy })), running, { op: 'metadata', value: { task_summary: 's1' }, sends: true }, { op: 'metadata', value: { recap: 'r1' }, sends: true }, { op: 'state', state: 'idle', sends: true }] });
served('a_served_policy_that_is_unreadable_holds_nothing', { steps: [beat(policy({ max_bytes: 'big' })), running, post(1, [note('a', 'u1')]), burst3] });
served('a_served_policy_that_is_withdrawn_holds_nothing', { steps: [beat(policy()), running, post(1, [note('a', 'u1')]), beat(json(200, {})), burst3] });
served('a_served_policy_that_changes_holds_nothing', { steps: [beat(policy()), running, post(1, [note('a', 'u1')]), beat(policy({ max_events: 2 })), burst3] });
served('a_report_that_nobody_watches_holds_nothing', { events: [subscribers(false)], steps: [running, post(1, [note('a', 'u1')]), burst3, post(1, [note('e', 'u5')])] });
served('a_report_that_somebody_watches_holds_nothing', { events: [subscribers(true)], steps: [running, post(1, [note('a', 'u1')]), burst3] });
served('reports_that_flip_between_watched_and_not_hold_nothing', { events: [subscribers(false), subscribers(false), subscribers(true), subscribers(false)], steps: [running, post(1, [note('a', 'u1')]), post(1, [note('b', 'u2')]), post(1, [note('c', 'u3')]), post(1, [note('d', 'u4')])] });
served('an_unreadable_report_holds_nothing', { events: [json(200, { has_subscribers: 'no' }), json(200, { has_subscribers: null })], steps: [running, post(1, [note('a', 'u1')]), burst3, post(1, [note('e', 'u5')])] });
served('a_served_policy_and_an_unwatched_session_hold_nothing', { events: [subscribers(false), subscribers(false), subscribers(true)], steps: [beat(policy()), running, post(1, [note('a', 'u1')]), burst3, post(2, [note('e', 'u5'), thinking('t1')]), post(1, [note('f', 'u6')])] });
served('a_full_policy_and_an_unwatched_session_hold_nothing', { events: [subscribers(false)], steps: [beat(json(200, { event_flush_policy: fullPolicy })), running, post(1, [note('a', 'u1')]), burst3, post(1, [note('e', 'u5')])] });
served('a_control_request_does_not_jump_the_queue', { steps: [running, post(1, [note('a', 'u1')]), post(2, [note('b', 'u2'), note('c', 'u3'), control('sendControlRequest', permission)])] });
served('a_control_request_first_goes_out_alone_in_order', { steps: [running, post(2, [control('sendControlRequest', permission), note('b', 'u2'), note('c', 'u3')])] });
served('a_control_response_does_not_jump_the_queue', { steps: [running, post(1, [note('a', 'u1')]), post(2, [note('b', 'u2'), note('c', 'u3'), control('sendControlResponse', { type: 'control_response', response: { subtype: 'success', request_id: 'r1', response: { behavior: 'allow' } } })])] });
served('a_control_cancel_does_not_jump_the_queue', { steps: [running, post(1, [note('a', 'u1')]), post(2, [note('b', 'u2'), note('c', 'u3'), control('sendControlCancelRequest', { type: 'control_cancel_request', request_id: 'r1' })])] });
served('a_request_with_a_held_model_call_policy_does_not_jump_the_queue', { steps: [beat(policy({ hold_across_model_calls: true, max_age_ms: 60000 })), running, post(1, [note('a', 'u1')]), post(2, [note('b', 'u2'), note('c', 'u3'), control('sendControlRequest', permission)])] });
served('the_handle_reaches_no_internal_hooks', { handleKeys: true, steps: [running] });

// ---- the read stream (GET /worker/events/stream), reached when `outboundOnly` is false ----
// Each case serves a script of connections to the real `attachBridgeSession`: a connection answers with a status and
// then either streams its chunks and stays open (`hold`) or ends. The recording keeps the stream requests, the
// acknowledgements and heartbeats the frames cause, what the bundle handed to `onInboundMessage` and the sequence
// number the handle reports.
const sseFrame = (id, event, data) => `${id === null ? '' : `id: ${id}\n`}event: ${event}\ndata: ${typeof data === 'string' ? data : JSON.stringify(data)}\n\n`;
const clientEvent = (sequence, eventId, payload, extra = {}) => sseFrame(sequence, 'client_event', { event_id: eventId, sequence_num: sequence, event_type: payload.type, source: 'client', payload, ...extra });
const userPayload = (uuid, text = 'hi') => ({ type: 'user', uuid, session_id: 'cse_abc', message: { role: 'user', content: text } });
const ephemeral = (kind) => sseFrame(null, 'ephemeral_event', { event_type: kind });
const connection = (chunks, then = 'hold', extra = {}) => ({ status: 200, headers: [['content-type', 'text/event-stream']], chunks, then, ...extra });
const refused = (status, headers = []) => ({ status, headers, chunks: [], body: '', then: 'end' });
const streamed = (name, connections, spec = {}) => add('stream', name, { epoch: 6, stream: connections, ...spec });
const delivered = (count) => ['/worker/events/delivery', count];
streamed('the_stream_opens_with_the_session_headers', [connection([])], { waitFor: [['/worker/events/stream', 1]] });
streamed('a_client_event_is_delivered_and_acknowledged', [connection([clientEvent(1, 'e1', userPayload('u1'))])], { waitFor: [delivered(2)] });
streamed('events_in_one_chunk_are_acknowledged_in_order', [connection([clientEvent(1, 'e1', userPayload('u1')) + clientEvent(2, 'e2', userPayload('u2'))])], { waitFor: [delivered(2)] });
streamed('events_in_separate_chunks_are_acknowledged_one_by_one', [connection([clientEvent(1, 'e1', userPayload('u1')), clientEvent(2, 'e2', userPayload('u2'))], 'hold', { gapMs: 150 })], { waitFor: [delivered(4)] });
streamed('a_frame_split_across_chunks_is_read_whole', [connection(['id: 1\r\nevent: client_event\r\nda', `ta: ${JSON.stringify({ event_id: 'e1', sequence_num: 1, event_type: 'user', source: 'client', payload: userPayload('u1') })}\r`, '\n\r', '\n'], 'hold', { gapMs: 60 })], { waitFor: [delivered(2)] });
streamed('a_payload_on_several_data_lines_is_joined', [connection(['id: 1\nevent: client_event\ndata: {"event_id":"e1","sequence_num":1,\ndata: "event_type":"user","source":"client",\ndata: "payload":{"type":"user","uuid":"u1","message":{"role":"user","content":"hi"}}}\n\n'])], { waitFor: [delivered(2)] });
streamed('comments_and_frames_without_an_event_are_dropped', [connection([': keepalive\n\n', 'data: {"event_id":"x"}\n\n', sseFrame(2, 'something_else', { event_id: 'x' }), clientEvent(3, 'e3', userPayload('u3'))])], { waitFor: [delivered(2)] });
streamed('a_frame_that_is_not_json_is_dropped', [connection([sseFrame(1, 'client_event', 'not json'), clientEvent(2, 'e2', userPayload('u2'))])], { waitFor: [delivered(2)] });
streamed('a_worker_control_request_is_acknowledged_and_not_delivered', [connection([clientEvent(1, 'e1', { type: 'control_request', request_id: 'r1', request: { subtype: 'interrupt' } }, { source: 'worker' }), clientEvent(2, 'e2', userPayload('u2'))])], { waitFor: [delivered(2)] });
streamed('a_server_only_type_that_does_not_match_is_acknowledged_and_not_delivered', [connection([clientEvent(1, 'e1', { type: 'session_notice', text: 'n' }, { event_type: 'user' }), clientEvent(2, 'e2', userPayload('u2'))])], { waitFor: [delivered(2)] });
streamed('a_client_event_without_a_typed_payload_is_acknowledged', [connection([clientEvent(1, 'e1', { text: 'no type' }), clientEvent(2, 'e2', userPayload('u2'))])], { waitFor: [delivered(2)] });
streamed('a_repeated_user_uuid_is_delivered_once', [connection([clientEvent(1, 'e1', userPayload('u1')), clientEvent(2, 'e2', userPayload('u1')), clientEvent(3, 'e3', userPayload('u3'))], 'hold', { gapMs: 100 })], { waitFor: [delivered(6)] });
streamed('a_repeated_sequence_is_still_acknowledged', [connection([clientEvent(5, 'e5', userPayload('u5')), clientEvent(5, 'e5b', userPayload('u6')), clientEvent(3, 'e3', userPayload('u7'))], 'hold', { gapMs: 100 })], { waitFor: [delivered(6)] });
streamed('ephemeral_frames_do_not_move_the_sequence', [connection([sseFrame(9, 'ephemeral_event', { event_type: 'owner_present' }), clientEvent(2, 'e2', userPayload('u2'))])], { waitFor: [delivered(2)] });
streamed('a_heartbeat_probe_sends_a_heartbeat_at_once', [connection([ephemeral('heartbeat_probe')])], { waitFor: [['/worker/heartbeat', 1]] });
streamed('a_second_probe_waits_for_the_beat_spacing', [connection([ephemeral('heartbeat_probe'), ephemeral('heartbeat_probe')], 'hold', { gapMs: 150 })], { waitFor: [['/worker/heartbeat', 2]], waitMs: 15000 });
streamed('other_ephemeral_frames_send_nothing', [connection([ephemeral('owner_present'), ephemeral('checkpoint_request'), ephemeral('session_claimed'), ephemeral('something_new'), sseFrame(null, 'ephemeral_event', 'not json'), sseFrame(null, 'ephemeral_event', { no_type: 1 })])], { waitFor: [['/worker/events/stream', 1]], settleMs: 800 });
streamed('a_stream_that_ends_is_reopened_after_the_last_sequence', [connection([clientEvent(4, 'e4', userPayload('u4'))], 'end'), connection([])], { waitFor: [['/worker/events/stream', 2], delivered(2)], reconnectBases: [1000] });
streamed('a_failed_open_is_retried_with_a_growing_delay', [refused(500), refused(502), refused(503), connection([])], { waitFor: [['/worker/events/stream', 4]], reconnectBases: [1000, 2000, 4000], settleMs: 200 });
streamed('a_throttled_open_is_retried_like_any_other_failure', [refused(429, [['retry-after', '5']]), connection([])], { waitFor: [['/worker/events/stream', 2]], reconnectBases: [1000], settleMs: 200 });
streamed('a_reconnect_after_a_good_stream_starts_the_delay_over', [connection([], 'end'), refused(500), connection([clientEvent(1, 'e1', userPayload('u1'))], 'end'), connection([])], { waitFor: [['/worker/events/stream', 4]], reconnectBases: [1000, 2000, 1000], settleMs: 200 });
streamed('a_reconnect_resumes_from_the_highest_sequence', [connection([clientEvent(7, 'e7', userPayload('u7')), clientEvent(6, 'e6', userPayload('u6'))], 'end'), connection([])], { waitFor: [['/worker/events/stream', 2], delivered(4)], reconnectBases: [1000] });
streamed('an_unauthorized_stream_closes_the_session', [refused(401)], { waitClose: true });
streamed('a_forbidden_stream_closes_the_session', [refused(403)], { waitClose: true });
streamed('a_forbidden_stream_from_the_edge_closes_the_session', [refused(403, [['cf-ray', 'abc-LHR']])], { waitClose: true });
streamed('a_missing_stream_closes_the_session', [refused(404)], { waitClose: true });
streamed('a_stream_that_is_not_event_stream_is_still_read', [connection([clientEvent(1, 'e1', userPayload('u1'))], 'hold', { headers: [['content-type', 'text/plain']] })], { waitFor: [delivered(2)] });

// The inbound router behind `onInboundMessage` (`J4`): every branch a client event can reach through the real session.
const plainUser = (uuid, extra = {}) => ({ type: 'user', ...(uuid === undefined ? {} : { uuid }), message: { role: 'user', content: 'hi' }, ...extra });
const routedStream = (name, payloads, spec = {}) => streamed(name, [connection(payloads.map((payload, i) => clientEvent(i + 1, `e${i + 1}`, payload)), 'hold', { gapMs: 60, startMs: spec.startMs ?? 0 })], { waitFor: [delivered(payloads.length * 2)], ...spec });
routedStream('a_permission_response_is_handed_to_the_host', [{ type: 'control_response', response: { subtype: 'success', request_id: 'r1', response: { behavior: 'allow' } } }]);
routedStream('a_camel_case_request_id_is_normalized', [{ type: 'control_response', requestId: 'top', response: { subtype: 'success', requestId: 'r1' } }, { type: 'control_response', response: { subtype: 'success', requestId: 'late-bind-1' } }]);
routedStream('a_control_response_without_a_response_is_ignored', [{ type: 'control_response', uuid: 'c1' }, { type: 'control_response', response: null }, plainUser('u2')]);
routedStream('a_control_request_with_an_unknown_subtype_is_refused_to_the_server', [{ type: 'control_request', request_id: 'r1', request: { subtype: 'no_such_subtype' } }], { settleMs: 2500 });
routedStream('a_control_request_with_a_camel_case_id_is_answered_under_the_snake_case_id', [{ type: 'control_request', requestId: 'r2', request: { subtype: 'no_such_subtype' } }], { settleMs: 2500 });
routedStream('a_control_request_with_a_late_bind_id_is_ignored', [{ type: 'control_request', requestId: 'late-bind-9', request: { subtype: 'no_such_subtype' } }, plainUser('u2')], { settleMs: 1500 });
routedStream('a_control_cancel_for_an_unknown_request_does_nothing', [{ type: 'control_cancel_request', request_id: 'r1' }, { type: 'control_cancel_request', request_id: 5 }, plainUser('u2')]);
routedStream('a_queued_notification_is_delivered_as_its_user_frame', [{ type: 'queued_notification', notification: { notification_id: 'n1', origin: 'mcp_send_message', content: 'c' } }, { type: 'queued_notification', notification: { notification_id: 'n1', origin: 'mcp_send_message', content: 'again' } }]);
routedStream('a_queued_notification_that_does_not_parse_is_ignored', [{ type: 'queued_notification' }, { type: 'queued_notification', notification: null }, { type: 'queued_notification', notification: { notification_id: '', origin: 'mcp_send_message', content: 'c' } }, { type: 'queued_notification', notification: { notification_id: 'n1', origin: 'other', content: 'c' } }, { type: 'queued_notification', notification: { notification_id: 'n1', origin: 'mcp_send_message', content: 5 } }, plainUser('u2')]);
routedStream('a_notification_id_that_was_delivered_is_dropped_as_a_user_message', [plainUser('n1'), { type: 'queued_notification', notification: { notification_id: 'n1', origin: 'mcp_send_message', content: 'c' } }]);
routedStream('a_user_message_without_a_uuid_is_always_delivered', [plainUser(undefined), plainUser(undefined), plainUser('')]);
routedStream('a_replay_a_parented_frame_and_a_fork_frame_are_not_delivered', [plainUser('u1', { isReplay: true }), plainUser('u2', { parent_tool_use_id: 'tool' }), plainUser('u3', { historical: true, display_only: true }), plainUser('u1'), plainUser('u2'), plainUser('u3')]);
routedStream('a_user_frame_with_look_alike_flags_is_delivered', [plainUser('u1', { isReplay: false }), plainUser('u2', { parent_tool_use_id: null }), plainUser('u3', { historical: true }), plainUser('u4', { display_only: true }), plainUser('u5', { historical: 'true', display_only: true })]);
routedStream('messages_that_are_not_user_messages_are_not_delivered', [{ type: 'assistant', uuid: 'a1', message: {} }, { type: 'system', uuid: 's1' }, { type: 'result' }, plainUser('u1')]);
add('stream', 'a_message_the_host_wrote_is_not_delivered_back', { epoch: 6, stream: [connection([clientEvent(1, 'e1', plainUser('w1')), clientEvent(2, 'e2', plainUser('u2'))], 'hold', { gapMs: 60, startMs: 1200 })], steps: [{ op: 'write', frame: { type: 'user', uuid: 'w1', message: { role: 'user', content: 'out' } } }], waitFor: [delivered(4)] });
add('stream', 'a_message_the_host_wrote_does_not_hide_another_uuid', { epoch: 6, stream: [connection([clientEvent(1, 'e1', plainUser('w2'))], 'hold', { gapMs: 60, startMs: 1200 })], steps: [{ op: 'write', frame: { type: 'user', uuid: 'w1', message: { role: 'user', content: 'out' } } }], waitFor: [delivered(2)] });

// A silent stream is reopened with no backoff when the liveness check runs late (a paused or sleeping process). The
// clocks are bent (see `clock` above): `late_wall` and `late_mono` say how far past the 45 s timeout each clock reads
// when the timer fires, `null` for a clock that did not jump. A silent stream that is not reopened at once is a lost
// stream counted twice (the liveness handler and the read loop that its abort ends both report it), so the first wait
// is already the second step of the backoff.
const slept = (name, clockSpec, bases, rest = []) => streamed(name, [connection([]), ...rest, connection([])], { clock: clockSpec, waitFor: [['/worker/events/stream', 2 + rest.length]], reconnectBases: bases, settleMs: 300 });
slept('a_stream_that_slept_is_reopened_at_once', { late_wall: 20000, late_mono: 20000 }, [0]);
slept('a_wall_clock_jump_alone_reopens_at_once', { late_wall: 20000, late_mono: null }, [0]);
slept('a_monotonic_gap_alone_reopens_at_once', { late_wall: null, late_mono: 20000 }, [0]);
slept('a_check_just_past_the_tolerance_reopens_at_once', { late_wall: 10500, late_mono: 10500 }, [0]);
slept('a_check_inside_the_tolerance_waits_for_the_backoff', { late_wall: 9500, late_mono: 9500 }, [2000]);
slept('a_silent_stream_that_did_not_sleep_waits_for_the_backoff', { late_wall: 0, late_mono: 0 }, [2000]);
slept('a_sleep_reopen_does_not_count_as_a_failed_attempt', { late_wall: 20000, late_mono: 20000 }, [0, 1000], [refused(500)]);

// The control dispatch behind a client `control_request` (`t$`), driven through the real session. The attach path wires
// only some callbacks (`onInterrupt`, `onSetModel`, `onSetMaxThinkingTokens`, `onSetPermissionMode`, `onRenameSession`,
// and the asynchronous `onStopTask`, `onBackgroundTasks`, `onSideQuestion`) and the version string, so those are the only
// ones a case can register. Each case sends one request and keeps the control_response frames the session posted.
const request = (subtype, extra = {}) => ({ type: 'control_request', request_id: 'r1', request: { subtype, ...extra } });
const controlCase = (name, payload, spec = {}) => add('control', name, { epoch: 6, stream: [connection([clientEvent(1, 'e1', payload)], 'hold', { gapMs: 60, startMs: spec.startMs ?? 0 })], waitFor: [['=/worker/events', spec.posts ?? 1]], settleMs: 400, ...spec });
const returns = (value) => ({ returns: value });
controlCase('interrupt_calls_the_host_and_succeeds', request('interrupt'), { callbacks: { onInterrupt: {} } });
controlCase('interrupt_without_a_host_callback_still_succeeds', request('interrupt'));
controlCase('get_usage_is_unsupported', request('get_usage'));
controlCase('mcp_status_lists_no_servers', request('mcp_status'));
controlCase('mcp_status_with_a_live_reply_flag_is_still_plain', request('mcp_status', { liveReply: true }));
for (const subtype of ['set_color', 'file_suggestions', 'read_file', 'get_workspace_diff', 'get_context_usage', 'mcp_authenticate', 'mcp_oauth_callback_url', 'mcp_reconnect', 'cancel_async_message']) {
  controlCase(`${subtype}_is_unsupported_without_a_callback`, request(subtype, subtype === 'cancel_async_message' ? { message_uuid: 'm1' } : {}));
}
controlCase('cancel_async_message_needs_a_string_uuid', request('cancel_async_message', { message_uuid: 5 }));
controlCase('an_unknown_subtype_is_unsupported', request('no_such_subtype'));
controlCase('an_unknown_subtype_is_cleaned_in_the_error', request('bad\u0007​sub\ttype'));
controlCase('a_long_unknown_subtype_is_cut_in_the_error', request('s'.repeat(5000)));
controlCase('a_non_string_subtype_is_unsupported', request(7));
for (const [name, subtype] of [['null', null], ['an_array', [1, null, 'a']], ['an_object', {}], ['true', true], ['a_float', 1.5]]) controlCase(`a_subtype_that_is_${name}_is_unsupported`, request(subtype));
controlCase('a_request_without_a_subtype_is_unsupported', { type: 'control_request', request_id: 'r1', request: {} });
controlCase('a_request_that_is_a_string_is_unsupported', { type: 'control_request', request_id: 'r1', request: 'text' });
controlCase('a_request_that_is_a_number_is_unsupported', { type: 'control_request', request_id: 'r1', request: 5 });
controlCase('a_request_id_that_is_a_number_is_answered_as_it_is', { type: 'control_request', request_id: 12, request: { subtype: 'get_usage' } });
controlCase('fork_conversation_is_not_supported_without_a_callback', request('fork_conversation', { message_uuid: 'x' }));
controlCase('rename_session_needs_a_string_title', request('rename_session', { title: 5 }), { callbacks: { onRenameSession: returns({ ok: true }) } });
controlCase('rename_session_without_a_callback_is_unsupported', request('rename_session', { title: 'T' }));
controlCase('rename_session_succeeds', request('rename_session', { title: 'T' }), { callbacks: { onRenameSession: returns({ ok: true }) } });
controlCase('rename_session_that_fails_reports_the_cleaned_error', request('rename_session', { title: 'T' }), { callbacks: { onRenameSession: returns({ ok: false, error: 'no​ way\n' }) } });
controlCase('rename_session_that_fails_without_text_reports_an_empty_error', request('rename_session', { title: 'T' }), { callbacks: { onRenameSession: returns({ ok: false }) } });
controlCase('rename_session_returning_nothing_is_unsupported', request('rename_session', { title: 'T' }), { callbacks: { onRenameSession: {} } });
controlCase('set_permission_mode_accepts_a_known_mode', request('set_permission_mode', { mode: 'plan' }), { callbacks: { onSetPermissionMode: returns({ ok: true }) } });
controlCase('set_permission_mode_maps_manual_to_default', request('set_permission_mode', { mode: 'manual' }), { callbacks: { onSetPermissionMode: returns({ ok: true }) } });
controlCase('set_permission_mode_refuses_an_unknown_mode', request('set_permission_mode', { mode: 'chaos' }), { callbacks: { onSetPermissionMode: returns({ ok: true }) } });
controlCase('set_permission_mode_refuses_a_non_string_mode', request('set_permission_mode', { mode: 3 }), { callbacks: { onSetPermissionMode: returns({ ok: true }) } });
controlCase('set_permission_mode_without_a_callback_is_unsupported', request('set_permission_mode', { mode: 'plan' }));
controlCase('set_permission_mode_returning_nothing_is_unsupported', request('set_permission_mode', { mode: 'plan' }), { callbacks: { onSetPermissionMode: {} } });
controlCase('set_permission_mode_failure_carries_the_code', request('set_permission_mode', { mode: 'auto' }), { callbacks: { onSetPermissionMode: returns({ ok: false, error: 'blocked', code: 'policy' }) } });
controlCase('set_permission_mode_failure_without_a_code_has_none', request('set_permission_mode', { mode: 'auto' }), { callbacks: { onSetPermissionMode: returns({ ok: false, error: 'blocked' }) } });
for (const mode of ['acceptEdits', 'auto', 'bypassPermissions', 'default', 'dontAsk']) {
  controlCase(`set_permission_mode_passes_${mode}_through`, request('set_permission_mode', { mode }), { callbacks: { onSetPermissionMode: returns({ ok: true }) } });
}
controlCase('set_model_with_a_model_succeeds', request('set_model', { model: 'm1' }), { callbacks: { onSetModel: returns({ ok: true }) } });
controlCase('set_model_without_a_model_passes_nothing', request('set_model'), { callbacks: { onSetModel: returns({ ok: true }) } });
controlCase('set_model_with_a_null_model_passes_nothing', request('set_model', { model: null }), { callbacks: { onSetModel: returns({ ok: true }) } });
controlCase('set_model_without_a_callback_succeeds', request('set_model', { model: 'm1' }));
controlCase('set_model_returning_nothing_succeeds', request('set_model', { model: 'm1' }), { callbacks: { onSetModel: {} } });
controlCase('set_model_refuses_a_non_string_model', request('set_model', { model: 4 }), { callbacks: { onSetModel: returns({ ok: true }) } });
controlCase('set_model_refuses_a_conditional_switch', request('set_model', { model: 'm1', only_if_model_is_current: true }), { callbacks: { onSetModel: returns({ ok: true }) } });
controlCase('set_model_refuses_a_null_condition', request('set_model', { model: 'm1', only_if_model_is_current: null }), { callbacks: { onSetModel: returns({ ok: true }) } });
controlCase('set_model_accepts_a_false_condition', request('set_model', { model: 'm1', only_if_model_is_current: false }), { callbacks: { onSetModel: returns({ ok: true }) } });
controlCase('set_model_failure_reports_the_cleaned_error', request('set_model', { model: 'm1' }), { callbacks: { onSetModel: returns({ ok: false, error: 'bad\tmodel' }) } });
controlCase('set_model_failure_without_text_reports_an_empty_error', request('set_model', { model: 'm1' }), { callbacks: { onSetModel: returns({ ok: false }) } });
const thinkingRequest = (extra) => request('set_max_thinking_tokens', extra);
for (const [name, extra] of [
  ['an_integer', { max_thinking_tokens: 2000 }], ['null', { max_thinking_tokens: null }], ['nothing', {}], ['a_negative_integer', { max_thinking_tokens: -5 }],
  ['a_whole_float', { max_thinking_tokens: 3.0 }], ['a_huge_whole_number', { max_thinking_tokens: 1e21 }], ['a_fraction', { max_thinking_tokens: 1.5 }], ['a_string', { max_thinking_tokens: '5' }],
  ['a_display', { max_thinking_tokens: 5, thinking_display: 'omitted' }], ['a_null_display', { thinking_display: null }], ['highlights', { thinking_display: 'highlights' }],
  ['summarized', { thinking_display: 'summarized' }], ['an_unknown_display', { thinking_display: 'loud' }], ['a_numeric_display', { thinking_display: 1 }],
]) controlCase(`set_max_thinking_tokens_with_${name}`, thinkingRequest(extra), { callbacks: { onSetMaxThinkingTokens: returns({ ok: true }) } });
controlCase('set_max_thinking_tokens_without_a_callback_succeeds', thinkingRequest({ max_thinking_tokens: 5 }));
controlCase('set_max_thinking_tokens_returning_nothing_succeeds', thinkingRequest({ max_thinking_tokens: 5 }), { callbacks: { onSetMaxThinkingTokens: {} } });
controlCase('set_max_thinking_tokens_failure_reports_the_cleaned_error', thinkingRequest({ max_thinking_tokens: 5 }), { callbacks: { onSetMaxThinkingTokens: returns({ ok: false, error: 'no​pe' }) } });
controlCase('set_max_thinking_tokens_failure_without_text_reports_an_empty_error', thinkingRequest({ max_thinking_tokens: 5 }), { callbacks: { onSetMaxThinkingTokens: returns({ ok: false }) } });
controlCase('mcp_set_servers_with_a_hint_and_no_servers_is_acknowledged', request('mcp_set_servers', { hint: 'x' }));
controlCase('mcp_set_servers_with_a_hint_and_servers_is_unsupported', request('mcp_set_servers', { hint: 'x', servers: {} }));
controlCase('mcp_set_servers_with_a_non_object_is_refused', request('mcp_set_servers', { servers: [] }));
controlCase('mcp_set_servers_with_a_null_is_refused', request('mcp_set_servers', { servers: null }));
controlCase('mcp_set_servers_without_a_callback_is_unsupported', request('mcp_set_servers', { servers: {} }));
controlCase('mcp_set_servers_with_a_non_string_hint_is_refused', request('mcp_set_servers', { hint: 5 }));
controlCase('stop_task_needs_a_string_id', request('stop_task', { task_id: 5 }), { callbacks: { onStopTask: {} } });
controlCase('stop_task_without_a_callback_is_unsupported', request('stop_task', { task_id: 't1' }));
controlCase('background_tasks_needs_a_string_tool_use_id', request('background_tasks', { tool_use_id: 5 }), { callbacks: { onBackgroundTasks: {} } });
controlCase('background_tasks_without_a_callback_is_unsupported', request('background_tasks'));
controlCase('background_tasks_without_a_callback_and_a_tool_use_id_is_unsupported', request('background_tasks', { tool_use_id: 'tu1' }));
controlCase('side_question_needs_a_question', request('side_question', {}), { callbacks: { onSideQuestion: {} } });
controlCase('side_question_needs_a_string_question', request('side_question', { question: 5 }), { callbacks: { onSideQuestion: {} } });
controlCase('side_question_history_must_be_pairs', request('side_question', { question: 'q', history: [{ question: 'a' }] }), { callbacks: { onSideQuestion: {} } });
controlCase('side_question_history_must_be_an_array', request('side_question', { question: 'q', history: 'x' }), { callbacks: { onSideQuestion: {} } });
controlCase('side_question_without_a_callback_is_unsupported', request('side_question', { question: 'q' }));
controlCase('side_question_with_an_invalid_fallback_notice_is_refused', request('side_question', { question: 'q', history: [{ question: 'a', response: 'b', fallback_notice: 3 }] }));

// Replies that wait for a host promise. `resolves`, `rejects`, `rejectsAbort` (an AbortError), `never` and
// `rejectsOnAbort` say what the promise does; `progress` lists what the callback reports before it settles.
const slow = { settleMs: 900 };
const stopTask = (name, behaviour, extra = {}) => controlCase(`stop_task_${name}`, request('stop_task', { task_id: 't1' }), { callbacks: { onStopTask: behaviour }, ...extra });
stopTask('replies_with_what_the_host_resolves', { resolves: { stopped: true } });
stopTask('replies_with_an_empty_object_for_nothing', { resolvesNothing: true });
stopTask('replies_with_an_empty_object_for_null', { resolves: null });
stopTask('that_rejects_says_it_failed', { rejects: 'boom' });
stopTask('that_is_aborted_reports_the_cleaned_message', { rejectsAbort: 'stop\tded​ early' });
const backgroundTasks = (name, extra, behaviour, spec = {}) => controlCase(`background_tasks_${name}`, request('background_tasks', extra), { callbacks: { onBackgroundTasks: behaviour }, ...spec });
backgroundTasks('without_a_tool_use_id_replies_with_an_empty_object', {}, { resolves: true });
backgroundTasks('with_a_null_tool_use_id_replies_with_an_empty_object', { tool_use_id: null }, { resolves: true });
backgroundTasks('with_an_empty_tool_use_id_replies_with_an_empty_object', { tool_use_id: '' }, { resolves: true });
backgroundTasks('with_a_tool_use_id_reports_what_was_backgrounded', { tool_use_id: 'tu1' }, { resolves: true });
backgroundTasks('with_a_tool_use_id_reports_a_structured_answer', { tool_use_id: 'tu1' }, { resolves: { ids: ['a'] } });
backgroundTasks('with_a_tool_use_id_and_no_answer_replies_with_an_empty_object', { tool_use_id: 'tu1' }, { resolvesNothing: true });
backgroundTasks('that_rejects_says_it_failed', { tool_use_id: 'tu1' }, { rejects: 'boom' });
backgroundTasks('that_is_aborted_reports_the_cleaned_message', {}, { rejectsAbort: 'cut\u0007 short' });
backgroundTasks('are_refused_when_disabled_by_the_environment', { tool_use_id: 'tu1' }, { resolves: true }, { env: { CLAUDE_CODE_DISABLE_BACKGROUND_TASKS: '1' } });
backgroundTasks('are_refused_in_simple_mode', {}, { resolves: true }, { env: { CLAUDE_CODE_SIMPLE: '1' } });
backgroundTasks('ignore_a_falsy_disable_flag', {}, { resolves: true }, { env: { CLAUDE_CODE_DISABLE_BACKGROUND_TASKS: '0' } });
backgroundTasks('read_a_true_word_as_disabled', {}, { resolves: true }, { env: { CLAUDE_CODE_DISABLE_BACKGROUND_TASKS: ' True ' } });
backgroundTasks('with_a_bad_id_are_refused_before_the_environment', { tool_use_id: 5 }, { resolves: true }, { env: { CLAUDE_CODE_DISABLE_BACKGROUND_TASKS: '1' } });
const sideQuestion = (name, behaviour, extra = {}, spec = {}) => controlCase(`side_question_${name}`, request('side_question', { question: 'q', ...extra }), { callbacks: { onSideQuestion: behaviour }, ...slow, ...spec });
sideQuestion('passes_a_real_answer_through', { resolves: { response: 'a', synthetic: false } });
sideQuestion('replaces_an_answer_that_is_not_marked_real', { resolves: { response: 'a' } });
sideQuestion('replaces_a_synthetic_answer', { resolves: { response: 'a', synthetic: true, note: 1 } });
sideQuestion('keeps_a_null_answer', { resolves: { response: null } });
sideQuestion('keeps_a_null_answer_that_is_synthetic', { resolves: { response: null, synthetic: true } });
sideQuestion('without_a_response_key_is_replaced', { resolves: { extra: 1 } });
sideQuestion('without_an_answer_fails', { resolvesNothing: true });
sideQuestion('that_rejects_says_it_failed', { rejects: 'boom' });
sideQuestion('that_is_aborted_reports_the_cleaned_message', { rejectsAbort: 'gave\tup' });
sideQuestion('hands_over_the_question_and_history', { resolves: { response: 'a', synthetic: false } }, { history: [{ question: 'a', response: 'b', fallback_notice: 'n', extra: 1 }, { question: 'c', response: 'd' }], extra: 2 });
sideQuestion('reports_progress_before_the_answer', { progress: [{ message: 'one' }, { message: 'two', percent: 5 }], resolves: { response: 'a', synthetic: false } });
sideQuestion('progress_may_name_the_envelope_keys', { progress: [{ type: 'x', subtype: 'y', request_id: 'z', uuid: 'u', session_id: 's', more: true }], resolves: { response: 'a', synthetic: false } });
sideQuestion('times_out_and_aborts_the_host', { never: true });
const twoEvents = (name, second, behaviour, spec = {}) => add('control', name, { epoch: 6, stream: [connection([clientEvent(1, 'e1', request('side_question', { question: 'q' })), clientEvent(2, 'e2', second)], 'hold', { gapMs: 200 })], callbacks: { onSideQuestion: behaviour }, waitFor: [['=/worker/events', 1]], settleMs: 900, ...spec });
twoEvents('side_question_cancelled_by_the_client_aborts_the_host', { type: 'control_cancel_request', request_id: 'r1' }, { rejectsOnAbort: true });
twoEvents('side_question_cancelled_after_the_host_answered_changes_nothing', { type: 'control_cancel_request', request_id: 'r1' }, { resolves: { response: 'a', synthetic: false } });
const lateAnswer = { resolvesAfterMs: 260, resolves: { response: 'a', synthetic: false } };
twoEvents('side_question_cancelled_by_the_client_is_still_answered_when_the_host_ignores_it', { type: 'control_cancel_request', request_id: 'r1' }, lateAnswer);
twoEvents('side_question_delivered_twice_is_answered_once', request('side_question', { question: 'q' }), { never: true });
twoEvents('side_question_cancel_for_another_request_leaves_it_running', { type: 'control_cancel_request', request_id: 'other' }, lateAnswer);

const setModel = (name, behaviour, extra = { model: 'm1' }, spec = {}) => controlCase(`set_model_${name}`, request('set_model', extra), { callbacks: { onSetModel: behaviour }, ...slow, ...spec });
setModel('verdict_that_succeeds', { resolves: { ok: true } });
setModel('verdict_that_names_the_model', { resolves: { ok: true, model: 'm​2\n' } });
setModel('verdict_that_carries_notices', { resolves: { ok: true, notices: ['one\t1', 5, 'two', null] } });
setModel('verdict_with_notices_that_is_not_a_list', { resolves: { ok: true, notices: 'x' } });
setModel('verdict_with_no_answer_succeeds', { resolvesNothing: true });
setModel('verdict_that_fails_reports_each_cleaned_line', { resolves: { ok: false, error: 'bad​ one\nsecond\tline' } });
setModel('verdict_that_fails_with_a_known_code', { resolves: { ok: false, error: 'slow down', code: 'rate_limit' } });
setModel('verdict_that_fails_with_every_known_code_kind', { resolves: { ok: false, error: 'e', code: 'verdict_pending', model: 'keep\tme' } });
setModel('verdict_that_fails_with_an_unknown_code_drops_it', { resolves: { ok: false, error: 'e', code: 'mystery' } });
setModel('verdict_that_fails_without_text_has_an_empty_error', { resolves: { ok: false } });
setModel('verdict_that_fails_with_a_non_text_error_has_an_empty_error', { resolves: { ok: false, error: 7 } });
setModel('verdict_that_rejects_is_an_internal_error', { rejects: 'boom' });
setModel('verdict_that_is_aborted_is_an_internal_error', { rejectsAbort: 'stopped' });
const thinking2 = (name, behaviour, extra = { max_thinking_tokens: 5 }) => controlCase(`set_max_thinking_tokens_${name}`, thinkingRequest(extra), { callbacks: { onSetMaxThinkingTokens: behaviour }, ...slow });
thinking2('verdict_that_succeeds', { resolves: { ok: true } });
thinking2('verdict_with_no_answer_succeeds', { resolvesNothing: true });
thinking2('verdict_that_fails_reports_the_cleaned_error', { resolves: { ok: false, error: 'no​pe\t!' } });
thinking2('verdict_that_fails_without_text_has_an_empty_error', { resolves: { ok: false } });
thinking2('verdict_that_rejects_says_it_failed', { rejects: 'boom' });
thinking2('verdict_that_is_aborted_reports_the_cleaned_message', { rejectsAbort: 'cut\tshort' });
thinking2('verdict_gets_the_display', { resolves: { ok: true } }, { max_thinking_tokens: null, thinking_display: 'omitted' });
// `apply_flag_settings`: the settings are checked on the spot and the attach path registers no handler for them.
const flagSettings = (name, settings) => controlCase(`apply_flag_settings_${name}`, request('apply_flag_settings', settings === undefined ? {} : { settings }));
flagSettings('without_settings_must_be_an_object', undefined);
flagSettings('with_null_settings_must_be_an_object', null);
flagSettings('with_text_settings_must_be_an_object', 'x');
flagSettings('with_list_settings_must_be_an_object', []);
flagSettings('with_number_settings_must_be_an_object', 5);
flagSettings('with_no_keys_has_nothing_to_apply', {});
flagSettings('with_an_effort_level_reaches_the_missing_handler', { effortLevel: 'low' });
flagSettings('with_a_null_effort_level_reaches_the_missing_handler', { effortLevel: null });
flagSettings('with_ultracode_reaches_the_missing_handler', { ultracode: false });
flagSettings('with_both_known_keys_reaches_the_missing_handler', { ultracode: true, effortLevel: 'high' });
flagSettings('with_a_numeric_effort_level_is_refused', { effortLevel: 5 });
flagSettings('with_a_text_ultracode_is_refused', { ultracode: 'yes' });
flagSettings('with_a_bad_effort_level_and_a_bad_ultracode_names_the_effort_level', { ultracode: 1, effortLevel: 5 });
flagSettings('with_a_good_effort_level_and_a_bad_ultracode_names_ultracode', { ultracode: 1, effortLevel: 'x' });
flagSettings('with_an_unknown_key_names_it', { foo: 1 });
flagSettings('with_an_unknown_key_before_a_bad_type_names_the_key', { effortLevel: 5, foo: 1 });
flagSettings('with_several_unknown_keys_names_five_and_counts_the_rest', { a: 1, b: 1, c: 1, d: 1, e: 1, f: 1, g: 1, h: 1 });
flagSettings('with_exactly_five_unknown_keys_names_them_all', { a: 1, b: 1, c: 1, d: 1, e: 1 });
flagSettings('with_index_like_keys_lists_them_first_in_order', { b: 1, 2: 1, 1: 1, a: 1, '01': 1, 10: 1 });
flagSettings('with_an_unknown_key_that_has_escape_codes', { 'a\u001b[31mred\u001b[0mb': 1 });
flagSettings('with_an_unknown_key_that_rebuilds_an_escape_code_when_stripped', { 'a\u001b\u001b[31m[31mb': 1 });
flagSettings('with_an_unknown_key_nested_deeper_than_the_strip_passes', { 'x\u001b\u001b\u001b\u001b\u001b[31m[31m[31m[31m[31mmmmmmy': 1 });
flagSettings('with_an_unknown_key_that_has_control_and_format_characters', { 'tab\tkey​z\u0000q r': 1 });
flagSettings('with_an_unknown_key_that_has_runs_of_spaces', { '  two   words  ': 1 });
flagSettings('with_an_unknown_key_that_has_backticks', { 'a`b｀cˋd`e‵f': 1 });
flagSettings('with_an_unknown_key_that_starts_with_a_combining_mark', { '́x ́y -́z q́': 1 });
flagSettings('with_an_unknown_key_that_needs_composing', { 'é': 1 });
flagSettings('with_an_unknown_key_that_cleans_to_nothing', { '​': 1 });
flagSettings('with_an_unknown_key_of_exactly_the_limit', { ['k'.repeat(40)]: 1 });
flagSettings('with_an_unknown_key_over_the_limit', { ['k'.repeat(41)]: 1 });
flagSettings('with_an_unknown_key_cut_inside_a_surrogate_pair', { ['k'.repeat(39) + '\u{1f600}tail']: 1 });
flagSettings('with_an_unknown_key_cut_after_a_surrogate_pair', { ['k'.repeat(38) + '\u{1f600}tail']: 1 });
// A key longer than 320 units is cut there first, and an escape code the cut splits is dropped whole when the full text
// finishes it, so the pad of short escape codes pushes the text that shows past the cut.
const pad = '\u001b[0m'.repeat(77) + 'abcdefghi';
flagSettings('with_an_unknown_key_cut_at_a_pair_before_the_eight_fold_limit', { ['k'.repeat(319) + '\u{1f600}tail']: 1 });
flagSettings('with_an_unknown_key_that_cuts_a_whole_escape_code', { [pad + '\u001b[31mtail']: 1 });
flagSettings('with_an_unknown_key_that_cuts_an_escape_code_missing_its_end', { [pad + '\u001b[31\u0001tail']: 1 });
flagSettings('with_an_unknown_key_that_cuts_a_whole_string_escape', { [pad + '\u001b]0;title\u0007tail']: 1 });
flagSettings('with_an_unknown_key_that_cuts_a_string_escape_missing_its_end', { [pad + '\u001b]0;title and more text without an end']: 1 });
flagSettings('with_an_unknown_key_that_cuts_a_string_escape_before_its_closer', { [pad + '\u001b]\u001b\\tail']: 1 });
flagSettings('with_an_unknown_key_that_cuts_a_device_control_escape', { [pad + '\u001bPdata\u001b\\tail']: 1 });
flagSettings('with_an_unknown_key_that_starts_a_string_escape_early_and_ends_it_late', { ['abc\u001b]0;' + 'x'.repeat(400) + '\u0007tail']: 1 });
flagSettings('with_an_unknown_key_where_the_cut_ends_a_whole_escape_code', { [pad + '\u001b[mtail']: 1 });
flagSettings('with_an_unknown_key_where_the_cut_leaves_a_lone_introducer', { [pad + 'ab\u001b[31mtail']: 1 });
flagSettings('with_a_short_unknown_key_that_ends_in_a_partial_escape_code', { 'a\u001b[3': 1 });
flagSettings('with_an_unknown_key_that_is_long_and_plain', { ['k'.repeat(400)]: 1 });
controlCase('initialize_lists_the_defaults', request('initialize'));
controlCase('initialize_names_the_client_version', request('initialize'), { version: '9.9.9' });
controlCase('initialize_ignores_dialog_kinds_it_cannot_use', request('initialize', { supportedDialogKinds: ['a', 5] }));
add('control', 'initialize_lists_the_prompts_still_waiting', {
  epoch: 6,
  stream: [connection([clientEvent(1, 'e1', request('initialize'))], 'hold', { gapMs: 60, startMs: 1500 })],
  steps: [{ op: 'events', expect: 2, writes: [
    control('sendControlRequest', { type: 'control_request', request_id: 'p1', request: { subtype: 'can_use_tool', tool_name: 'Bash', input: { command: 'ls' }, tool_use_id: 'tu1' } }),
    control('sendControlRequest', { type: 'control_request', request_id: 'p2', request: { subtype: 'request_user_dialog', dialog_kind: 'k', payload: {} } }),
    control('sendControlRequest', { type: 'control_request', request_id: 'p3', request: { subtype: 'interrupt' } }),
  ] }],
  waitFor: [['=/worker/events', 3]], settleMs: 400,
});

// The attestation filter (`_$`) never drops an event: its policy is the default one, `enforce: false`, because nothing
// in the bundle assigns `filterPolicy`. Frames that carry every kind of `device_attestation_status` are delivered.
const attestationStatuses = ['VERIFIED', 'ABSENT', 'INVALID', 'UNCHECKED', 'UNSPECIFIED', 'TRANSITIVELY_VOUCHED', 'SERVICE_VOUCHED', 'SERVER_AUTHORED', 'server_replayed', 'DEVICE_ATTESTATION_STATUS_INVALID', 3, 99, null, 'junk'];
add('stream', 'frames_with_any_attestation_status_are_delivered', {
  epoch: 6,
  stream: [connection([
    ...attestationStatuses.map((status, i) => clientEvent(i + 1, `u${i}`, plainUser(`a${i}`), { device_attestation_status: status })),
    ...attestationStatuses.map((status, i) => clientEvent(attestationStatuses.length + i + 1, `c${i}`, { type: 'control_response', response: { subtype: 'success', request_id: `r${i}` } }, { device_attestation_status: status })),
  ], 'hold', { gapMs: 20 })],
  waitFor: [delivered(attestationStatuses.length * 4)],
});

// ---- runner ----
const KEPT = new Set(['accept', 'content-type', 'authorization', 'anthropic-version', 'anthropic-client-platform', 'user-agent', 'last-event-id', 'x-ccr-ticket-rev', 'x-ccr-ticket-session', 'x-ccr-ticket-seq', 'x-ccr-upload-attempt']);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (predicate, ms) => {
  const end = realNow() + ms;
  while (!predicate() && realNow() < end) await sleep(5);
};

async function run(spec) {
  clock.wall = 0;
  clock.mono = 0;
  const lateness = spec.clock;
  clock.jump = lateness === undefined ? null : { wall: lateness.late_wall === null ? 0 : 44700 + lateness.late_wall, mono: lateness.late_mono === null ? 0 : 44700 + lateness.late_mono };
  const requests = [];
  const gate = { on: spec.gated === true || (spec.steps ?? []).some((step) => step.op === 'beat'), pending: false, answer: null, current: null };
  const queues = { register: [...(spec.register === undefined ? [json(200, { worker_epoch: 5 })] : [spec.register])], put: [...(spec.puts ?? [])], beat: [...(spec.beat ?? [])], events: [...(spec.events ?? [])], deliveries: [...(spec.deliveries ?? [])], landing: [...(spec.landing ?? [])], stream: [...(spec.stream ?? [])] };
  const pick = (key) => {
    const queue = queues[key];
    if (queue.length === 0) return json(200, {});
    return queue.length === 1 ? queue[0] : queue.shift();
  };
  const server = http.createServer((req, res) => {
    const chunks = [];
    req.on('data', (chunk) => chunks.push(chunk));
    req.on('end', () => {
      const headers = [];
      for (let i = 0; i < req.rawHeaders.length; i += 2) if (KEPT.has(req.rawHeaders[i].toLowerCase())) headers.push([req.rawHeaders[i], req.rawHeaders[i + 1]]);
      const route = req.url.replace(/^\/v1\/code\/sessions\/cse_abc/, '');
      const isRead = req.method === 'GET' && route === '/worker';
      if (req.method === 'GET' && route.startsWith('/worker/events/stream')) {
        requests.push({ method: req.method, path: req.url, headers, body: '', at: realNow() });
        const script = queues.stream.length > 1 ? queues.stream.shift() : (queues.stream[0] ?? { status: 200, chunks: [], then: 'hold' });
        res.statusCode = script.status;
        for (const [name, value] of script.headers ?? [['content-type', 'text/event-stream']]) res.setHeader(name, value);
        if (script.chunks.length === 0 && script.then === 'end') { res.end(script.body ?? ''); return; }
        res.flushHeaders();
        (async () => {
          if (script.startMs) await sleep(script.startMs);
          for (const chunk of script.chunks) {
            res.write(chunk);
            await sleep(script.gapMs ?? 30);
          }
          if (script.then === 'end') res.end();
        })();
        return;
      }
      const gated = gate.on && route === '/worker/heartbeat';
      const background = gated && !gate.pending;
      if (!isRead && !background) requests.push({ method: req.method, path: req.url, headers, body: Buffer.concat(chunks).toString(), at: realNow() });
      let response = json(200, {});
      if (req.method === 'GET' && route === '/worker') response = spec.record ?? response;
      else if (route === '/worker/landing') response = pick('landing');
      else if (route === '/worker/register') response = pick('register');
      else if (req.method === 'PUT') response = pick('put');
      else if (gated) {
        if (gate.pending) gate.current = gate.answer;
        response = gate.current ?? json(200, {});
        gate.pending = false;
      } else if (route === '/worker/heartbeat') response = pick('beat');
      else if (route === '/worker/events') response = pick('events');
      else if (route === '/worker/events/delivery') response = pick('deliveries');
      res.statusCode = response.status;
      for (const [name, value] of response.headers) res.setHeader(name, value.replace('{base}', `http://127.0.0.1:${server.address().port}`));
      if (isRead && spec.recordDelayMs) setTimeout(() => res.end(response.body), spec.recordDelayMs);
      else res.end(response.body);
    });
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const base = `http://127.0.0.1:${server.address().port}`;
  const closes = [];
  const inbound = [];
  const permissions = [];
  const logStart = fs.existsSync(debugLog) ? fs.statSync(debugLog).size : 0;
  const options = { sessionId: spec.sessionId ?? 'cse_abc', apiBaseUrl: base, ingressToken: 'tok', outboundOnly: spec.stream === undefined, heartbeatIntervalMs: spec.interval ?? 600000, onClose: (code) => closes.push(code), onInboundMessage: (message) => inbound.push(JSON.parse(JSON.stringify(message))), onPermissionResponse: (message) => { permissions.push(JSON.parse(JSON.stringify(message))); return true; } };
  if (spec.epoch !== null && spec.epoch !== undefined) options.epoch = spec.epoch;
  if (spec.initialSequenceNum !== undefined) options.initialSequenceNum = spec.initialSequenceNum;
  // Host callbacks the control dispatch is handed (`spec.callbacks`): each records its arguments and returns what the
  // case says (`returns`), or nothing when the case names none. `undefined` arguments are written as "<undefined>".
  const calls = [];
  const signals = [];
  const copy = (value) => JSON.parse(JSON.stringify(value));
  const callback = (name, behaviour) => (...args) => {
    const options = args.find((arg) => arg !== null && typeof arg === 'object' && 'signal' in arg);
    if (options !== undefined) signals.push(options.signal);
    calls.push([name, ...args.map((arg) => (arg === undefined ? '<undefined>' : arg === options ? '<options>' : arg))]);
    if ('returns' in behaviour) return copy(behaviour.returns);
    for (const progress of behaviour.progress ?? []) options.onProgress(copy(progress));
    const abort = (message) => Object.assign(new Error(message), { name: 'AbortError' });
    if ('resolvesAfterMs' in behaviour) return new Promise((resolve) => setTimeout(() => resolve(copy(behaviour.resolves)), behaviour.resolvesAfterMs));
    if ('resolves' in behaviour) return Promise.resolve(copy(behaviour.resolves));
    if (behaviour.resolvesNothing) return Promise.resolve(undefined);
    if ('rejects' in behaviour) return Promise.reject(new Error(behaviour.rejects));
    if ('rejectsAbort' in behaviour) return Promise.reject(abort(behaviour.rejectsAbort));
    if (behaviour.never) return new Promise(() => {});
    if (behaviour.rejectsOnAbort) return new Promise((resolve, reject) => options.signal.addEventListener('abort', () => reject(abort('the host stopped'))));
    return undefined;
  };
  for (const [key, behaviour] of Object.entries(spec.callbacks ?? {})) options[key] = callback(key, behaviour);
  if (spec.version !== undefined) options.claudeCodeVersion = spec.version;
  for (const [key, value] of Object.entries(spec.env ?? {})) process.env[key] = value;
  const emit = (item) => {
    if (item.via === undefined) handle.write(item);
    else handle[item.via](item.via === 'sendControlCancelRequest' ? item.payload.request_id : item.payload);
  };
  const count = (route) => requests.filter((r) => r.path.endsWith(route)).length;
  const result = { name: spec.name, ...(spec.record !== undefined && { record: spec.record }), ...(spec.recordDelayMs !== undefined && { record_delay_ms: spec.recordDelayMs }), epoch_option: spec.epoch ?? null, ...(spec.sessionId !== undefined && { session_id: spec.sessionId }), ...(spec.initialSequenceNum !== undefined && { initial_sequence_num: spec.initialSequenceNum }), interval_ms: spec.interval ?? 600000, session_token: spec.sessionToken ?? null, register: spec.register ?? null, puts: spec.puts ?? [], beat: spec.beat ?? [], ...(spec.events !== undefined && { events: spec.events }), ...(spec.deliveries !== undefined && { deliveries: spec.deliveries }), ...(spec.stream !== undefined && { stream: spec.stream }), ...(spec.clock !== undefined && { clock: spec.clock }), landing: spec.landing ?? [], steps: spec.steps ?? [], beats: 0 };
  let handle;
  if (spec.sessionToken !== undefined) process.env.CLAUDE_CODE_SESSION_ACCESS_TOKEN = spec.sessionToken;
  try {
    handle = await bridge.attachBridgeSession(options);
    result.rejection = null;
  } catch (error) {
    result.rejection = { message: String(error.message), code: error.code ?? null };
  }
  if (handle !== undefined) {
    await until(() => count('/worker') + count('/worker/register') > 0 && requests.some((r) => r.method === 'PUT') || closes.length > 0, spec.initWaitMs ?? 8000);
    for (const step of spec.steps ?? []) {
      const before = requests.filter((r) => r.method === 'PUT').length;
      if (step.op === 'events') {
        const seen = () => requests.filter((r) => r.path.endsWith('/worker/events')).length;
        const start = seen();
        const wroteAt = realNow();
        for (const item of step.writes) emit(item);
        await until(() => seen() >= start + step.expect, 12000);
        if (spec.prompt && realNow() - wroteAt > 600) throw new Error(`${spec.name}: a post was held for ${realNow() - wroteAt} ms`);
        if (seen() < start + step.expect) throw new Error(`${spec.name}: expected ${step.expect} event posts`);
        await sleep(250);
        continue;
      }
      if (step.op === 'result') {
        const seen = () => requests.filter((r) => r.path.endsWith('/worker/events')).length;
        const start = seen();
        handle.sendResult();
        await until(() => seen() >= start + step.expect, 12000);
        if (seen() < start + step.expect) throw new Error(`${spec.name}: expected ${step.expect} event posts`);
        await sleep(250);
        continue;
      }
      if (step.op === 'flush') {
        for (const item of step.writes ?? []) emit(item);
        await handle.flush();
        await sleep(250);
        continue;
      }
      if (step.op === 'reconnect') {
        for (const item of step.writes ?? []) emit(item);
        if (step.afterMs) await sleep(step.afterMs);
        const options = { apiBaseUrl: step.badBase ?? base, ingressToken: step.token ?? 'tok2' };
        if (step.epoch !== undefined) options.epoch = step.epoch;
        if (step.thenClose) {
          // The close is issued while the rebuild is awaited, so the call is not awaited before it.
          const pending = handle.reconnectTransport(options);
          handle.close();
          await pending;
        } else {
          try {
            await handle.reconnectTransport(options);
          } catch (error) {
            result.rejected = String(error?.message ?? error).split('\n')[0];
          }
        }
        await sleep(step.settleMs ?? 400);
        continue;
      }
      if (step.op === 'write') {
        handle.write(step.frame);
        await sleep(150);
        continue;
      }
      if (step.op === 'close') {
        handle.close();
        if (step.again) handle.close();
        if (step.sends) await until(() => requests.filter((r) => r.method === 'PUT').length > before, 3000);
        await sleep(300);
        continue;
      }
      if (step.op === 'delivery') {
        const seen = () => requests.filter((r) => r.path.endsWith('/worker/events/delivery')).length;
        const start = seen();
        for (const [id, status] of step.reports) handle.reportDelivery(id, status);
        if (step.close) handle.close();
        await until(() => seen() >= start + step.expect, 12000);
        if (seen() < start + step.expect) throw new Error(`${spec.name}: expected ${step.expect} delivery posts`);
        await sleep(step.settleMs ?? 250);
        continue;
      }
      if (step.op === 'beat') {
        gate.answer = step.answer;
        gate.pending = true;
        await until(() => !gate.pending, 3000);
        if (gate.pending) throw new Error(`${spec.name}: no heartbeat arrived`);
        await sleep(150);
        continue;
      }
      if (step.op === 'burst') {
        for (const item of step.ops) {
          if (item.afterMs) await sleep(item.afterMs);
          if (item.state !== undefined) handle.reportState(item.state);
          else handle.reportMetadata(item.metadata);
        }
        if (step.close) handle.close();
        await until(() => requests.filter((r) => r.method === 'PUT').length >= before + step.expect, 12000);
        await sleep(250);
        continue;
      }
      if (step.op === 'state') handle.reportState(step.state);
      else handle.reportMetadata(step.value);
      if (step.sends) await until(() => requests.filter((r) => r.method === 'PUT').length > before, 3000);
      else await sleep(150);
    }
    if (spec.waitFor !== undefined) {
      const reached = () => spec.waitFor.every(([suffix, wanted]) => requests.filter((r) => (suffix.startsWith('=') ? r.path.endsWith(suffix.slice(1)) : r.path.includes(suffix))).length >= wanted);
      await until(reached, spec.waitMs ?? 12000);
      if (!reached()) throw new Error(`${spec.name}: the awaited requests did not arrive`);
      await sleep(spec.settleMs ?? 400);
    }
    if (spec.beats !== undefined) await until(() => count('/worker/heartbeat') >= spec.beats, 5000);
    if (spec.waitClose) await until(() => closes.length > 0, 8000);
    else await sleep(80);
    if (spec.quiet !== undefined) {
      const seen = count('/worker/heartbeat');
      await sleep(400);
      result.quiet = count('/worker/heartbeat') === seen;
      if (result.quiet !== spec.quiet) throw new Error(`${spec.name}: expected quiet=${spec.quiet}`);
    }
    if (spec.handleKeys) result.handle_keys = Object.keys(handle).sort();
    if (spec.stream !== undefined) { result.inbound = inbound; result.permission_responses = permissions; result.last_sequence = handle.getSequenceNum(); }
    if (spec.callbacks !== undefined) result.calls = signals.length === 0 ? calls : [...calls, ['<signals aborted>', signals.map((signal) => signal.aborted)]];
    if (spec.callbacks !== undefined) result.deadlines = clock.deadlines.splice(0);
    result.connected = handle.isConnected();
    result.epoch = handle.getEpoch();
  }
  for (const key of Object.keys(spec.env ?? {})) delete process.env[key];
  await sleep(120);
  // The bundle's debug log writer is buffered, so the line that names the mismatch reason lands some time after the
  // close. A session that closed always ended on a mismatch or on a refusal that names none, so wait for the line
  // itself (bounded) instead of reading after a fixed timer.
  const readReasons = () => {
    const log = fs.existsSync(debugLog) ? fs.readFileSync(debugLog, 'utf8').slice(logStart) : '';
    return [...log.matchAll(/terminal request-path condition \((\w+)\)/g)].map((m) => m[1]);
  };
  if (closes.length > 0) await until(() => readReasons().length > 0, 6000);
  const reasons = readReasons();
  let kept = requests;
  if (spec.waitClose || spec.beats !== undefined) {
    let seen = 0;
    const limit = spec.waitClose ? Infinity : spec.beats;
    kept = requests.filter((r) => !r.path.endsWith('/worker/heartbeat') || ++seen <= limit);
  }
  result.beats = gate.on ? 0 : kept.filter((r) => r.path.endsWith('/worker/heartbeat')).length;
  if (spec.retryBases !== undefined) {
    const puts = requests.filter((r) => r.method === 'PUT').slice(1);
    const gaps = puts.slice(1).map((r, i) => r.at - puts[i].at);
    const ok = gaps.length === spec.retryBases.length && gaps.every((gap, i) => gap >= spec.retryBases[i] - 5 && gap < spec.retryBases[i] + 500 + 400);
    if (!ok) throw new Error(`${spec.name}: retry gaps ${gaps} outside ${spec.retryBases}`);
    result.retry_base_ms = spec.retryBases;
  }
  if (spec.deliveryGaps !== undefined) {
    const posts = requests.filter((r) => r.path.endsWith('/worker/events/delivery'));
    const gaps = posts.slice(1).map((r, i) => r.at - posts[i].at);
    const ok = gaps.length === spec.deliveryGaps.length && gaps.every((gap, i) => spec.deliveryGaps[i] === null || (gap >= spec.deliveryGaps[i] - 5 && gap < spec.deliveryGaps[i] + 500 + 400));
    if (!ok) throw new Error(`${spec.name}: delivery gaps ${gaps} outside ${spec.deliveryGaps}`);
    result.delivery_gaps_ms = spec.deliveryGaps;
  }
  if (spec.reconnectBases !== undefined) {
    const opens = requests.filter((r) => r.path.includes('/worker/events/stream'));
    const gaps = opens.slice(1).map((r, i) => r.at - opens[i].at);
    const ok = gaps.length === spec.reconnectBases.length && gaps.every((gap, i) => gap >= spec.reconnectBases[i] * 0.75 - 10 && gap <= spec.reconnectBases[i] * 1.25 + 500);
    if (!ok) throw new Error(`${spec.name}: reconnect gaps ${gaps} outside ${spec.reconnectBases}`);
    result.reconnect_base_ms = spec.reconnectBases;
  }
  if (spec.eventGaps !== undefined) {
    const posts = requests.filter((r) => r.path.endsWith('/worker/events'));
    const gaps = posts.slice(1).map((r, i) => r.at - posts[i].at);
    const ok = gaps.length === spec.eventGaps.length && gaps.every((gap, i) => gap >= spec.eventGaps[i] - 5 && gap < spec.eventGaps[i] + 500 + 400);
    if (!ok) throw new Error(`${spec.name}: event gaps ${gaps} outside ${spec.eventGaps}`);
    result.event_gaps_ms = spec.eventGaps;
  }
  const uuidPattern = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g;
  for (const r of kept) if (r.path.endsWith('/worker/events')) r.body = r.body.replace(uuidPattern, '<uuid>');
  result.requests = kept.map(({ at, ...r }) => ({ ...r, path: r.path.replace(/^\/v1\/code\/sessions\/cse_abc/, '') }));
  result.closes = [...closes];
  result.reason = reasons[0] ?? null;
  try { handle?.close(); } catch {}
  delete process.env.CLAUDE_CODE_SESSION_ACCESS_TOKEN;
  server.closeAllConnections();
  await new Promise((resolve) => server.close(resolve));
  return result;
}

// ---- transport failures ----
const listen = (handler) => new Promise((resolve) => {
  const server = http.createServer(handler);
  server.listen(0, '127.0.0.1', () => resolve(server));
});
const stop = (server) => { server.closeAllConnections(); return new Promise((resolve) => server.close(resolve)); };
const portOf = (server) => server.address().port;

// What registerWorker throws when the request never gets an answer.
async function networkCase(mode) {
  let server;
  let port = null;
  let base = '';
  if (mode === 'refused' || mode === 'localhost') {
    server = await listen(() => {});
    port = portOf(server);
    await stop(server);
    server = undefined;
    base = `http://${mode === 'localhost' ? 'localhost' : '127.0.0.1'}:${port}`;
  } else if (mode === 'notfound') {
    base = 'http://nonexistent.invalid';
  } else if (mode === 'ftp') {
    base = 'ftp://h';
  } else if (mode === 'destroy') {
    server = await listen((req) => req.socket.destroy());
  } else if (mode === 'loop') {
    server = await listen((req, res) => { res.statusCode = 302; res.setHeader('location', `${req.url}x`); res.end(); });
  } else if (mode === 'truncated') {
    server = await listen((req, res) => { res.writeHead(200, { 'content-length': '100', 'content-type': 'application/json' }); res.write('{"wor'); setTimeout(() => req.socket.destroy(), 20); });
  }
  if (server) base = `http://127.0.0.1:${portOf(server)}`;
  let message = null;
  let code = null;
  try {
    await bridge.attachBridgeSession({ sessionId: 'cse_abc', apiBaseUrl: base, ingressToken: 'tok', outboundOnly: true });
  } catch (error) {
    message = String(error.message);
    code = error.code ?? null;
  }
  if (server) await stop(server);
  return { name: mode || 'invalid', mode: mode || 'invalid', message: port === null ? message : message?.replace(String(port), '<port>'), code };
}

// What attaching throws when the epoch is given (so nothing is registered) and the base URL is not a URL.
async function invalidUrlCase(base) {
  let message = null;
  let code = null;
  try {
    await bridge.attachBridgeSession({ sessionId: 'cse_abc', apiBaseUrl: base, ingressToken: 'tok', outboundOnly: true, epoch: 2 });
  } catch (error) {
    message = String(error.message);
    code = error.code ?? null;
  }
  return { name: base, base, message, code };
}

// What the fetch under a worker request throws, as the request path names it (`lm`: the code, else the name).
async function fetchFailureCase(mode) {
  const server = await listen((req, res) => {
    const route = req.url.replace(/^\/v1\/code\/sessions\/cse_abc/, '');
    req.resume();
    req.on('end', () => {
      if (route === '/worker/heartbeat' && mode === 'destroy') return req.socket.destroy();
      if (route === '/worker/heartbeat' && mode === 'hang') return;
      if (route === '/worker/heartbeat' && mode === 'truncated') {
        res.writeHead(200, { 'content-length': '100', 'content-type': 'application/json' });
        res.write('{"heartbeat_inter');
        return setTimeout(() => req.socket.destroy(), 20);
      }
      res.setHeader('content-type', 'application/json');
      res.end('{}');
    });
  });
  fetchErrors.length = 0;
  const closes = [];
  const handle = await bridge.attachBridgeSession({ sessionId: 'cse_abc', apiBaseUrl: `http://127.0.0.1:${portOf(server)}`, ingressToken: 'tok', outboundOnly: true, epoch: 2, heartbeatIntervalMs: 30, onClose: (code) => closes.push(code) });
  const failed = () => fetchErrors.find((entry) => entry.url.endsWith('/worker/heartbeat'));
  await until(() => failed() !== undefined, mode === 'hang' ? 9000 : 1500);
  await sleep(300);
  const first = failed();
  const result = { name: mode, mode, reason: first ? `fetch_failed:${first.code ?? first.name}` : null, connected: handle.isConnected(), closes: [...closes] };
  try { handle.close(); } catch {}
  await stop(server);
  return result;
}

// ---- the attach entrypoint itself: what the options do and what closing the handle sends ----
const attached = (name, spec) => add('attach', name, { epoch: 6, ...spec });
const idleStream = [connection([])];
for (const [label, seed] of [['a_positive', 7], ['zero', 0], ['a_negative', -3]]) attached(`${label}_initial_sequence_resumes_the_stream`, { initialSequenceNum: seed, stream: idleStream, waitFor: [['/worker/events/stream', 1]], settleMs: 200 });
attached('a_seed_is_kept_when_older_events_arrive', { initialSequenceNum: 7, stream: [connection([clientEvent(3, 'e3', userPayload('u3'))], 'end'), connection([])], waitFor: [['/worker/events/stream', 2], delivered(2)], reconnectBases: [1000] });
attached('a_session_id_with_unsafe_characters_is_refused', { sessionId: 'a/b', epoch: null });
attached('closing_an_idle_worker_says_goodbye', { steps: [{ op: 'close', sends: true }] });
attached('closing_twice_says_goodbye_once', { steps: [{ op: 'close', sends: true, again: true }] });
attached('closing_a_running_worker_says_nothing', { steps: [running, { op: 'close', sends: false }] });
attached('writes_after_closing_are_dropped', { steps: [{ op: 'close', sends: true }, post(0, [note('a', 'u1')])] });
attached('closing_after_the_epoch_was_taken_over_says_nothing', { puts: [json(200, {}), conflict('superseded_by_worker')], steps: [{ op: 'metadata', value: { task_summary: 's1' }, sends: true }, { op: 'close', sends: false }], waitClose: true });

// ---- the handle `attachBridgeSession` returns: each public call (the worker, delivery and state calls are the same
// calls the sections above make; these are the ones only the handle has) ----
const handled = (name, spec) => add('handle', name, { epoch: 6, ...spec });
const via = (method, payload) => ({ via: method, payload });
const prompt = (id, subtype, extra = {}) => ({ type: 'control_request', request_id: id, request: { subtype, ...extra } });
const reply = (id, extra = {}) => ({ type: 'control_response', response: { subtype: 'success', request_id: id, response: {} }, ...extra });
handled('a_uuid_is_written_once', { steps: [post(1, [note('a', 'u1')]), post(0, [note('b', 'u1')])] });
handled('a_write_without_a_uuid_is_posted', { steps: [post(1, [note('a')])] });
handled('a_historical_write_is_flagged', { steps: [post(1, [{ ...note('a', 'u1'), historical: true }])] });
handled('a_write_that_is_not_historical_is_not_flagged', { steps: [post(1, [{ ...note('a', 'u1'), historical: false }])] });
handled('a_historical_flag_that_is_not_true_is_not_flagged', { steps: [post(1, [{ ...note('a', 'u1'), historical: 1 }])] });
handled('a_historical_thinking_row_is_flagged_and_live_only', { steps: [post(1, [{ type: 'system', subtype: 'thinking_tokens', uuid: 'u1', historical: true }])] });
handled('a_result_is_the_empty_success_of_a_turn', { steps: [{ op: 'result', expect: 1 }] });
handled('a_tool_permission_prompt_is_posted_with_the_session_id', { steps: [post(1, [via('sendControlRequest', prompt('r1', 'can_use_tool', { tool_name: 'Bash', input: {}, tool_use_id: 't1' }))])] });
handled('a_dialog_prompt_is_posted_with_the_session_id', { steps: [post(1, [via('sendControlRequest', prompt('r2', 'request_user_dialog', { dialog_kind: 'x', payload: {} }))])] });
handled('another_control_request_is_posted_with_the_session_id', { steps: [post(1, [via('sendControlRequest', prompt('r3', 'interrupt'))])] });
handled('a_control_response_is_posted_with_the_session_id', { steps: [post(1, [via('sendControlResponse', reply('r1'))])] });
handled('a_live_only_control_response_is_posted', { steps: [post(1, [via('sendControlResponse', reply('r1', { live_only: true }))])] });
handled('a_cancel_is_posted_with_the_session_id', { steps: [post(1, [via('sendControlCancelRequest', { request_id: 'r1' })])] });
handled('a_prompt_is_answered_after_it_was_asked', { steps: [post(1, [via('sendControlRequest', prompt('r1', 'can_use_tool', { tool_name: 'Bash', input: {}, tool_use_id: 't1' }))]), post(1, [via('sendControlResponse', reply('r1'))]), post(1, [via('sendControlCancelRequest', { request_id: 'r1' })])] });
handled('calls_after_closing_send_nothing', { steps: [{ op: 'close', sends: true }, post(0, [note('a', 'u1'), via('sendControlRequest', prompt('r1', 'can_use_tool', { tool_name: 'Bash', input: {}, tool_use_id: 't1' })), via('sendControlResponse', reply('r1')), via('sendControlCancelRequest', { request_id: 'r1' })]), { op: 'state', state: 'running', sends: false }, { op: 'metadata', value: { task_summary: 's1' }, sends: false }, { op: 'delivery', expect: 0, reports: [['u1', 'received']] }, { op: 'result', expect: 0 }, { op: 'flush' }] });
handled('flush_sends_what_is_held', { steps: [{ op: 'flush', writes: [note('a', 'u1')] }] });
handled('flush_waits_for_a_failed_batch_to_be_resent', { events: [raw(500, ''), json(200, {})], eventGaps: [500], steps: [{ op: 'flush', writes: [note('a', 'u1')] }] });
handled('reconnecting_starts_a_new_worker_under_the_given_epoch', { steps: [post(1, [note('a', 'u1')]), { op: 'reconnect', epoch: 7 }, post(1, [note('b', 'u2')])] });
handled('reconnecting_without_an_epoch_keeps_the_epoch', { steps: [{ op: 'reconnect' }, post(1, [note('b', 'u2')])] });
handled('reconnecting_after_closing_does_nothing', { steps: [{ op: 'close', sends: true }, { op: 'reconnect', epoch: 7 }] });
handled('what_the_old_worker_did_not_deliver_moves_to_the_new_one', { events: [raw(500, ''), json(200, {})], steps: [{ op: 'reconnect', epoch: 7, writes: [note('a', 'u1'), via('sendControlRequest', prompt('r1', 'can_use_tool', { tool_name: 'Bash', input: {}, tool_use_id: 't1' }))], afterMs: 200 }], waitFor: [['=/worker/events', 2]] });
handled('a_carried_write_is_not_delivered_back_by_the_new_stream', { stream: [connection([]), connection([clientEvent(1, 'e1', plainUser('u1')), clientEvent(2, 'e2', plainUser('u2'))])], events: [raw(500, ''), json(200, {})], steps: [{ op: 'reconnect', epoch: 7, writes: [note('a', 'u1')], afterMs: 200 }], waitFor: [['=/worker/events', 2], delivered(4)] });

handled('a_rebuild_that_fails_closes_nothing_and_drops_the_old_worker', { events: [raw(500, ''), json(200, {})], steps: [{ op: 'reconnect', epoch: 7, badBase: 'not a url', writes: [note('a', 'u1')], afterMs: 200 }, post(0, [note('b', 'u2')])] });
handled('closing_while_the_rebuild_is_awaited', { steps: [post(1, [note('a', 'u1')]), { op: 'reconnect', epoch: 7, thenClose: true }, post(0, [note('b', 'u2')])] });

// What the worker client does while a request is open: the order the server saw each request start (`in`) and end
// (`out`). The server holds the first events post until the thing the case waits for has happened, so the order is
// the order of the client, not of a race.
async function concurrencyCase(mode) {
  const log = [];
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  let eventsSeen = 0;
  const server = await listen((req, res) => {
    const route = req.url.replace(/^\/v1\/code\/sessions\/cse_abc/, '');
    req.resume();
    req.on('end', async () => {
      if (route === '/worker/events') {
        eventsSeen += 1;
        log.push('events in');
        if (eventsSeen === 1) await held;
        log.push('events out');
      } else if (route === '/worker/heartbeat') {
        log.push('heartbeat in');
      }
      res.setHeader('content-type', 'application/json');
      res.end('{}');
      if (route === '/worker/heartbeat') {
        log.push('heartbeat out');
        if (mode === 'heartbeat_during_upload') release();
      }
    });
  });
  const handle = await bridge.attachBridgeSession({ sessionId: 'cse_abc', apiBaseUrl: `http://127.0.0.1:${portOf(server)}`, ingressToken: 'tok', outboundOnly: true, epoch: 2, heartbeatIntervalMs: 200 });
  const note = (uuid) => ({ type: 'user', uuid, message: { role: 'user', content: 'x' } });
  handle.write(note('u1'));
  if (mode === 'heartbeat_during_upload') {
    await until(() => log.length >= 4, 3000);
  } else {
    await until(() => log.includes('events in'), 1500);
    handle.write(note('u2'));
    release();
    await until(() => log.filter((line) => line === 'events out').length === 2, 3000);
  }
  try { handle.close(); } catch {}
  await stop(server);
  return { name: mode, mode, log };
}

// `--only=<case name>` records just that case, as many times as `--times=<n>` says, and prints the results.
const only = flags.find((flag) => flag.startsWith('--only='))?.slice('--only='.length);
if (only !== undefined) {
  const times = Number(flags.find((flag) => flag.startsWith('--times='))?.slice('--times='.length) ?? 1);
  const spec = Object.values(cases).flat().find((candidate) => candidate.name === only);
  if (!spec) throw new Error(`no case named ${only}`);
  for (let i = 0; i < times; i += 1) {
    const result = await run(spec);
    process.stdout.write(`${JSON.stringify(flags.includes("--dump") ? result : { name: result.name, reason: result.reason, closes: result.closes, requests: result.requests.length })}\n`);
  }
  fs.rmSync(work, { recursive: true, force: true });
  process.exit(0);
}

const goldens = { pure, register: [], init: [], state: [], heartbeat: [], auth: [], queue: [], record: [], events: [], delivery: [], served: [], stream: [], control: [], attach: [], handle: [], network: [], fetch_failures: [], concurrency: [], invalid_url: [] };
// `--section=<name>` records only that section and keeps every other part of the existing goldens file as it is.
const sectionOnly = flags.find((flag) => flag.startsWith('--section='))?.slice('--section='.length);
const existing = sectionOnly === undefined ? null : JSON.parse(fs.readFileSync(out, 'utf8'));
for (const section of ['register', 'init', 'state', 'heartbeat', 'auth', 'queue', 'record', 'events', 'delivery', 'served', 'stream', 'attach', 'handle']) {
  if (sectionOnly !== undefined && section !== sectionOnly) {
    goldens[section] = existing[section];
    continue;
  }
  for (const spec of cases[section]) goldens[section].push(await run(spec));
}
// A control case keeps the request, what the host callbacks saw and the control_response frames the session posted
// (the posted uuid is replaced by `<uuid>` already; the process id is replaced by 0).
for (const spec of sectionOnly === undefined ? cases.control : []) {
  const result = await run(spec);
  const frames = result.requests.filter((r) => r.method === 'POST' && r.path === '/worker/events')
    .flatMap((r) => JSON.parse(r.body).events.map((event) => event.payload)).filter((payload) => payload.type === 'control_response' || payload.type === 'system');
  for (const frame of frames) if (typeof frame.response?.response?.pid === 'number') frame.response.response.pid = 0;
  const first = spec.stream[0].chunks.length > 0 ? JSON.parse(spec.stream[0].chunks[0].split('data: ')[1]).payload : null;
  const later = spec.stream[0].chunks.slice(1).map((chunk) => JSON.parse(chunk.split('data: ')[1]).payload);
  const prompts = (spec.steps ?? []).flatMap((step) => step.writes ?? []).map((write) => write.payload);
  goldens.control.push({ name: spec.name, request: first, later, version: spec.version ?? null, callbacks: spec.callbacks ?? {}, prompts, env: spec.env ?? {}, deadlines: result.deadlines ?? [], calls: result.calls ?? [], frames });
}
if (existing === null) {
  for (const mode of ['refused', 'localhost', 'notfound', '', 'ftp', 'destroy', 'loop', 'truncated']) goldens.network.push(await networkCase(mode));
  for (const mode of ['destroy', 'truncated', 'hang']) goldens.fetch_failures.push(await fetchFailureCase(mode));
  for (const mode of ['heartbeat_during_upload', 'events_while_a_batch_is_open']) goldens.concurrency.push(await concurrencyCase(mode));
  for (const base of ['not a url', '', '/relative']) goldens.invalid_url.push(await invalidUrlCase(base));
} else {
  for (const key of ['pure', 'control', 'network', 'fetch_failures']) if (key !== sectionOnly) goldens[key] = existing[key];
  if (sectionOnly === 'concurrency') for (const mode of ['heartbeat_during_upload', 'events_while_a_batch_is_open']) goldens.concurrency.push(await concurrencyCase(mode));
  else goldens.concurrency = existing.concurrency ?? [];
  if (sectionOnly === 'invalid_url') for (const base of ['not a url', '', '/relative']) goldens.invalid_url.push(await invalidUrlCase(base));
  else goldens.invalid_url = existing.invalid_url ?? [];
}
fs.rmSync(work, { recursive: true, force: true });

const compact = (key, value) => (Array.isArray(value) && key !== 'pure' ? `[\n${value.map((c) => JSON.stringify(c)).join(',\n')}\n]` : JSON.stringify(value));
let text = `{\n${Object.entries(goldens).map(([key, value]) => `${JSON.stringify(key)}: ${compact(key, value)}`).join(',\n')}\n}\n`;
text = text.split(JSON.stringify(work).slice(1, -1)).join('<work>');
if (flags.includes('--check')) {
  if (fs.readFileSync(out, 'utf8') !== text) {
    process.stderr.write('bridge worker goldens are stale\n');
    process.exit(1);
  }
} else {
  fs.writeFileSync(out, text);
}
process.exit(0);
