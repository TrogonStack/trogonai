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
const bridge = await import(pathToFileURL(bridgePath).href);

// ---- pure tables, read from the bundle source ----
const between = (from, to) => {
  const start = source.indexOf(from);
  const end = source.indexOf(to, start);
  if (start < 0 || end < 0) throw new Error(`bundle layout changed around ${from}`);
  return source.slice(start, end);
};
const evaluate = (code, names) => new Function(`${code}\nreturn {${names.join(',')}};`)();
const heartbeatTable = evaluate(between('var e5={},I5=', 'function Wn('), ['I5', 't5', 'L5', 'P5', 'N5', 'P2e', 'N2e', 'n5', 'M2e', 'U2e', 'r5', 'F2e', 'o5', 's5']);
const closeTable = evaluate(between('var rHe=10,U5=', 'async function uHe('), ['rHe', 'U5', 'oHe', 'sHe', 'iHe', 'aHe', 'Au', 'cHe']);
const uploadTable = evaluate(between('var dHe=1e4', 'function ZA('), ['dHe', 'pHe', 'l5', 'fHe', 'd5']);
const sessionTable = evaluate(`${between('var S$e=180000', 'async function RC(')}`.replace('var S$e', 'var Sc').replace('y$e', 'yc'), ['Sc', 'yc']);
const markerTable = evaluate(`var mHe=512,hHe="upgrade_relay_marker";`, ['mHe', 'hHe']);
const dedupCapacity = evaluate('var x9=2000;', ['x9']).x9;
if (!/var x9=2000;/.test(source) || !source.includes('mHe=512,hHe="upgrade_relay_marker"')) throw new Error('bundle layout changed');
const classify = evaluate(`${between('function tC(', 'function m4e(')}`, ['tC']).tC;
const requiresAction = evaluate(between('function F5(', 'var gHe='), ['F5']).F5;
const metadataTable = evaluate(between('function p4e(', 'async function eC('), ['p4e']).p4e;
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
const cases = { register: [], init: [], state: [], heartbeat: [], auth: [], queue: [], record: [] };
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
add('queue', 'a_later_patch_joins_the_retry', { epoch: 6, puts: [json(200, {}), raw(500, ''), json(200, {})], steps: [burst(2, { state: 'running' }, { metadata: { a: 1 }, afterMs: 100 })] });
const stored = (metadata) => json(200, { worker: { external_metadata: metadata } });
add('record', 'repeat_of_the_record_is_not_sent', { epoch: 6, record: stored({ last_served_model: 'm1' }), steps: [{ op: 'metadata', value: { last_served_model: 'm1' }, sends: false }, { op: 'metadata', value: { last_served_model: 'm1' }, sends: true }] });
add('record', 'only_the_repeated_keys_are_left_out', { epoch: 6, record: stored({ permission_mode: 'plan' }), steps: [{ op: 'metadata', value: { permission_mode: 'plan', other: 1 }, sends: true }] });
add('record', 'a_changed_value_is_sent', { epoch: 6, record: stored({ permission_mode: 'plan' }), steps: [{ op: 'metadata', value: { permission_mode: 'auto' }, sends: true }] });
add('record', 'a_key_outside_the_set_is_always_sent', { epoch: 6, record: stored({ other: 1 }), steps: [{ op: 'metadata', value: { other: 1 }, sends: true }] });
add('record', 'null_equals_a_missing_key', { epoch: 6, record: stored({}), steps: [{ op: 'metadata', value: { rate_limit_info: null }, sends: false }] });
add('record', 'objects_compare_deeply', { epoch: 6, record: stored({ context_usage: { a: [1, { b: 2 }], c: 1 } }), steps: [{ op: 'metadata', value: { context_usage: { c: 1, a: [1, { b: 2 }] } }, sends: false }, { op: 'metadata', value: { is_ultraplan_mode: true }, sends: true }] });
add('record', 'an_unreadable_record_suppresses_nothing', { epoch: 6, record: raw(400, ''), steps: [{ op: 'metadata', value: { last_served_model: 'm1' }, sends: true }] });
add('record', 'a_record_that_is_not_an_object_counts_as_empty', { epoch: 6, record: json(200, { worker: { external_metadata: 'x' } }), steps: [{ op: 'metadata', value: { rate_limit_info: null }, sends: false }] });

// ---- runner ----
const KEPT = new Set(['accept', 'content-type', 'authorization', 'anthropic-version', 'anthropic-client-platform', 'user-agent', 'x-ccr-ticket-rev', 'x-ccr-ticket-session', 'x-ccr-ticket-seq']);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (predicate, ms) => {
  const end = Date.now() + ms;
  while (!predicate() && Date.now() < end) await sleep(5);
};

async function run(spec) {
  const requests = [];
  const queues = { register: [...(spec.register === undefined ? [json(200, { worker_epoch: 5 })] : [spec.register])], put: [...(spec.puts ?? [])], beat: [...(spec.beat ?? [])], landing: [...(spec.landing ?? [])] };
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
      if (!(req.method === 'GET' && route === '/worker')) requests.push({ method: req.method, path: req.url, headers, body: Buffer.concat(chunks).toString(), at: Date.now() });
      let response = json(200, {});
      if (req.method === 'GET' && route === '/worker') response = spec.record ?? response;
      else if (route === '/worker/landing') response = pick('landing');
      else if (route === '/worker/register') response = pick('register');
      else if (req.method === 'PUT') response = pick('put');
      else if (route === '/worker/heartbeat') response = pick('beat');
      res.statusCode = response.status;
      for (const [name, value] of response.headers) res.setHeader(name, value.replace('{base}', `http://127.0.0.1:${server.address().port}`));
      res.end(response.body);
    });
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const base = `http://127.0.0.1:${server.address().port}`;
  const closes = [];
  const logStart = fs.existsSync(debugLog) ? fs.statSync(debugLog).size : 0;
  const options = { sessionId: 'cse_abc', apiBaseUrl: base, ingressToken: 'tok', outboundOnly: true, heartbeatIntervalMs: spec.interval ?? 600000, onClose: (code) => closes.push(code) };
  if (spec.epoch !== null && spec.epoch !== undefined) options.epoch = spec.epoch;
  const count = (route) => requests.filter((r) => r.path.endsWith(route)).length;
  const result = { name: spec.name, ...(spec.record !== undefined && { record: spec.record }), epoch_option: spec.epoch ?? null, interval_ms: spec.interval ?? 600000, session_token: spec.sessionToken ?? null, register: spec.register ?? null, puts: spec.puts ?? [], beat: spec.beat ?? [], landing: spec.landing ?? [], steps: spec.steps ?? [], beats: 0 };
  let handle;
  if (spec.sessionToken !== undefined) process.env.CLAUDE_CODE_SESSION_ACCESS_TOKEN = spec.sessionToken;
  try {
    handle = await bridge.attachBridgeSession(options);
    result.rejection = null;
  } catch (error) {
    result.rejection = { message: String(error.message), code: error.code ?? null };
  }
  if (handle !== undefined) {
    await until(() => count('/worker') + count('/worker/register') > 0 && requests.some((r) => r.method === 'PUT') || closes.length > 0, 8000);
    for (const step of spec.steps ?? []) {
      const before = requests.filter((r) => r.method === 'PUT').length;
      if (step.op === 'burst') {
        for (const item of step.ops) {
          if (item.afterMs) await sleep(item.afterMs);
          if (item.state !== undefined) handle.reportState(item.state);
          else handle.reportMetadata(item.metadata);
        }
        await until(() => requests.filter((r) => r.method === 'PUT').length >= before + step.expect, 12000);
        await sleep(250);
        continue;
      }
      if (step.op === 'state') handle.reportState(step.state);
      else handle.reportMetadata(step.value);
      if (step.sends) await until(() => requests.filter((r) => r.method === 'PUT').length > before, 3000);
      else await sleep(150);
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
    result.connected = handle.isConnected();
    result.epoch = handle.getEpoch();
  }
  await sleep(120);
  const log = fs.existsSync(debugLog) ? fs.readFileSync(debugLog, 'utf8').slice(logStart) : '';
  const reasons = [...log.matchAll(/terminal request-path condition \((\w+)\)/g)].map((m) => m[1]);
  let kept = requests;
  if (spec.waitClose || spec.beats !== undefined) {
    let seen = 0;
    const limit = spec.waitClose ? Infinity : spec.beats;
    kept = requests.filter((r) => !r.path.endsWith('/worker/heartbeat') || ++seen <= limit);
  }
  result.beats = kept.filter((r) => r.path.endsWith('/worker/heartbeat')).length;
  if (spec.retryBases !== undefined) {
    const puts = requests.filter((r) => r.method === 'PUT').slice(1);
    const gaps = puts.slice(1).map((r, i) => r.at - puts[i].at);
    const ok = gaps.length === spec.retryBases.length && gaps.every((gap, i) => gap >= spec.retryBases[i] - 5 && gap < spec.retryBases[i] + 500 + 400);
    if (!ok) throw new Error(`${spec.name}: retry gaps ${gaps} outside ${spec.retryBases}`);
    result.retry_base_ms = spec.retryBases;
  }
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

const goldens = { pure, register: [], init: [], state: [], heartbeat: [], auth: [], queue: [], record: [], network: [], fetch_failures: [] };
for (const section of ['register', 'init', 'state', 'heartbeat', 'auth', 'queue', 'record']) {
  for (const spec of cases[section]) goldens[section].push(await run(spec));
}
for (const mode of ['refused', 'localhost', 'notfound', '', 'ftp', 'destroy', 'loop', 'truncated']) goldens.network.push(await networkCase(mode));
for (const mode of ['destroy', 'truncated', 'hang']) goldens.fetch_failures.push(await fetchFailureCase(mode));
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
