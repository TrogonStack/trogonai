#!/usr/bin/env node
// Records, in virtual time, when the pinned TypeScript package's worker client (bridge.mjs) schedules its timers and
// sends its requests, so the Rust timers are checked against the real code instead of against a reading of it.
//
// usage: node bridge_worker_timeline.mjs <path to bridge.mjs> <out-file> [--check]
//
// `setTimeout`, `setInterval`, `Date.now`, `performance.now` and `Math.random` are replaced before the bundle is
// loaded. A timer fires when the recorder moves the virtual clock to it, after the local server and the client have
// settled, so a case does not depend on how fast the machine is. `Math.random` is a seeded generator; the values the
// heartbeat schedule consumed are kept, because the Rust replay feeds the same values to its jitter source.
//
// Each case yields a timeline of strings, `<virtual ms> <event>`: `schedule <delay ms>` when the heartbeat timer was
// set, `request <METHOD> <path>` when the server saw a request (GET /worker, the stored state read, is left out) and
// `closed <code>` when the session reported its close code. A GET of the read stream is logged with its query, and the
// values `Math.random` gave the reconnect delay of the stream (`handleConnectionError`) are kept apart, in order. A POST to the events route also lists what it carried,
// as `request POST /worker/events <json>`: each event's payload without `session_id`, and `"e":1` on the ones the
// store does not keep. A case may script `writes` (`[virtual ms, frame]`), a `flushAt` and a `closeAt`.
// A case with `conns` leaves `outboundOnly` off and scripts what the service answers to each GET of the stream, in
// order: `'hang'` (no answer), `'reset'` (the socket is destroyed), or `{ status, chunks: [[ms after the answer, text]], then: 'end' | 'reset', hangBody }`; a stream
// that has no more connections scripted stays open and silent. `skews` (`[virtual ms, wall ms, monotonic ms]`) move
// `Date.now` and `performance.now` ahead without firing a timer, the way a paused or sleeping process does. A case with
// `together` runs everything the case does in one virtual millisecond (writes, flush, close, reconnect) in one
// synchronous step, as a host does that calls the handle twice before yielding; without it the recorder lets the sockets settle between two writes.
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const real = { setTimeout: globalThis.setTimeout };
const [bridgeArg, outArg, ...flags] = process.argv.slice(2);
if (!bridgeArg || !outArg) {
  process.stderr.write('usage: bridge_worker_timeline.mjs BRIDGE_MJS OUT_FILE [--check]\n');
  process.exit(2);
}
const bridgePath = path.resolve(bridgeArg);
const out = path.resolve(outArg);
for (const name of Object.keys(process.env)) {
  if (name.startsWith('CLAUDE_CODE_') || name.startsWith('CLAUDE_SESSION_')) delete process.env[name];
}
const work = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'bridge-worker-timeline-')));
process.chdir(work);
const source = fs.readFileSync(bridgePath, 'utf8');

// Where the heartbeat schedule lives in the bundle: `startHeartbeat` and the two closures it defines.
const scheduleStart = source.indexOf('startHeartbeat(){if(this.stopHeartbeat()');
const scheduleEnd = source.indexOf('noteBeatingEventsPostOk(', scheduleStart);
if (scheduleStart < 0 || scheduleEnd < 0) throw new Error('bundle layout changed around startHeartbeat');
const reconnectStart = source.indexOf('handleConnectionError(){');
const reconnectEnd = source.indexOf('reopenAfterSleep(', reconnectStart);
if (reconnectStart < 0 || reconnectEnd < 0) throw new Error('bundle layout changed around handleConnectionError');
const lineStarts = [0];
for (let i = source.indexOf('\n'); i >= 0; i = source.indexOf('\n', i + 1)) lineStarts.push(i + 1);
// True when the innermost bundle frame of the caller sits inside the heartbeat schedule (the request the beat starts
// sets its own timers from the same call chain, and those are not the schedule).
const insideReconnect = () => {
  const stack = new Error().stack ?? '';
  for (const match of stack.matchAll(/bridge\.mjs:(\d+):(\d+)/g)) {
    const offset = lineStarts[Number(match[1]) - 1] + Number(match[2]) - 1;
    if (offset >= reconnectStart && offset < reconnectEnd) return true;
  }
  return false;
};
const insideSchedule = () => {
  const stack = new Error().stack ?? '';
  const match = /bridge\.mjs:(\d+):(\d+)/.exec(stack);
  if (!match) return false;
  const offset = lineStarts[Number(match[1]) - 1] + Number(match[2]) - 1;
  return offset >= scheduleStart && offset < scheduleEnd;
};

// ---- virtual time ----
const BASE = 1_700_000_000_000;
let virtual = 0;
let nextId = 1;
const timers = new Map();
let timeline = [];
let randoms = [];
let pendingRandom = null;
let streamRandoms = [];
let wallSkew = 0;
let monoSkew = 0;
let seed = 1;
const seeded = () => {
  seed = (seed + 0x6d2b79f5) | 0;
  let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
  t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
Math.random = () => {
  const value = seeded();
  if (insideSchedule()) pendingRandom = value;
  if (insideReconnect()) streamRandoms.push(value);
  return value;
};
Date.now = () => BASE + virtual + wallSkew;
performance.now = () => 1000 + virtual + monoSkew;
const handleOf = (id) => ({
  ref() { return this; },
  unref() { return this; },
  hasRef() { return true; },
  refresh() { return this; },
  close() { timers.delete(id); return this; },
  [Symbol.toPrimitive]() { return id; },
});
const schedule = (callback, delay, args, every) => {
  const id = nextId++;
  const wait = Number(delay) >= 1 ? Math.trunc(Number(delay)) : 1;
  if (insideSchedule()) {
    timeline.push(`${virtual} schedule ${wait}`);
    if (pendingRandom !== null) randoms.push(pendingRandom);
    pendingRandom = null;
  }
  timers.set(id, { at: virtual + wait, callback, args, every: every ? wait : 0, order: id });
  return handleOf(id);
};
globalThis.setTimeout = (callback, delay, ...args) => schedule(callback, delay, args, false);
globalThis.setInterval = (callback, delay, ...args) => schedule(callback, delay, args, true);
const unschedule = (handle) => { if (handle !== undefined && handle !== null) timers.delete(Number(handle)); };
globalThis.clearTimeout = unschedule;
globalThis.clearInterval = unschedule;

// Lets the sockets and promise chains the last timer started finish, in real time.
const settle = () => new Promise((resolve) => real.setTimeout(resolve, 40));
const advanceTo = async (target) => {
  for (;;) {
    await settle();
    let next;
    for (const [id, timer] of timers) if (!next || timer.at < next.timer.at || (timer.at === next.timer.at && timer.order < next.timer.order)) next = { id, timer };
    if (!next || next.timer.at > target) break;
    virtual = Math.max(virtual, next.timer.at);
    if (next.timer.every) next.timer.at += next.timer.every; else timers.delete(next.id);
    next.timer.callback(...next.timer.args);
  }
  virtual = Math.max(virtual, target);
  await settle();
};

const bridge = await import(pathToFileURL(bridgePath).href);

// ---- the cases ----
const cases = [];
const add = (name, spec) => cases.push({ name, ...spec });
const accept = { status: 200, body: {} };
const advise = (seconds) => ({ status: 200, body: { heartbeat_interval_seconds: seconds } });

add('the_default_interval_beats_every_twenty_seconds', { until: 70000 });
add('an_advised_longer_interval_applies_from_the_next_schedule', { until: 130000, answers: [advise(60)] });
add('an_advised_shorter_interval_restarts_the_timer', { interval: 30000, until: 80000, answers: [advise(12)] });
add('an_advised_interval_below_the_floor_is_raised_to_it', { interval: 30000, until: 70000, answers: [advise(2)] });
add('an_advised_interval_above_the_ceiling_is_lowered_to_it', { until: 700000, answers: [advise(4000)] });
add('an_advised_interval_that_is_not_positive_changes_nothing', { until: 70000, answers: [advise(0), advise(-5)] });
add('a_beat_is_skipped_while_the_previous_one_is_unanswered', { interval: 1500, until: 10000, answers: ['hang'] });
add('a_given_interval_below_the_floor_is_kept_until_the_service_advises_one', { interval: 7000, until: 30000, answers: [advise(7)] });
const tag = ((n) => () => `f${(n += 1)}`)(0);
const frame = (type, rest) => ({ type, parent_tool_use_id: null, ...rest, uuid: tag() });
const stream = (event, rest = {}) => frame('stream_event', { event, ...rest });
const delta = (kind, key, chunk, rest = {}, index = 0, extra = {}) => stream({ type: 'content_block_delta', index, delta: { type: kind, [key]: chunk, ...extra } }, rest);
const say = (chunk, rest, index) => delta('text_delta', 'text', chunk, rest, index);
const think = (chunk, tokens) => delta('thinking_delta', 'thinking', chunk, {}, 0, tokens === undefined ? {} : { estimated_tokens: tokens });
const assistant = (content) => frame('assistant', { message: { id: 'm1', role: 'assistant', content } });
const user = (content) => frame('user', { message: { role: 'user', content } });
const start = () => stream({ type: 'message_start', message: { id: 'm1', role: 'assistant', content: [], input_transformations: { a: 1 } } });
const open = (kind) => stream({ type: 'content_block_start', index: 0, content_block: { type: kind, text: '' } });
const at = (ms, ...frames) => frames.map((f) => [ms, f]);
const writes = (...groups) => groups.flat();
const big = (n, ch = 'a') => ch.repeat(n);
add('deltas_in_one_window_are_sent_together_when_it_ends', { until: 400, writes: writes(at(0, start()), at(10, say('Hel')), at(20, say('lo')), at(30, say(' wor')), at(250, say('ld'))) });
add('a_row_that_is_not_a_stream_event_sends_the_buffered_deltas_ahead_of_it', { until: 300, writes: writes(at(0, say('a')), at(10, say('b')), at(40, user('hi')), at(60, say('c'))) });
add('an_assistant_text_row_waits_for_the_window_to_end', { until: 300, writes: writes(at(0, say('a')), at(30, assistant([{ type: 'text', text: 'hi' }])), at(40, say('b'))) });
add('an_assistant_row_with_a_tool_call_does_not_wait', { until: 300, writes: writes(at(0, say('a')), at(30, assistant([{ type: 'text', text: 'hi' }, { type: 'tool_use', id: 't1', name: 'Bash', input: {} }]))) });
add('an_assistant_text_row_with_nothing_buffered_is_sent_at_once', { until: 300, writes: writes(at(0, assistant([{ type: 'text', text: 'hi' }]))) });
add('an_assistant_row_without_content_does_not_wait', { until: 300, writes: writes(at(0, say('a')), at(30, assistant([]))) });
add('a_block_start_sends_the_window_when_a_row_waits', { until: 300, writes: writes(at(0, say('a')), at(30, assistant([{ type: 'text', text: 'hi' }])), at(50, open('text')), at(60, say('b'))) });
add('a_block_start_does_not_send_the_window_when_no_row_waits', { until: 300, writes: writes(at(0, say('a')), at(50, open('text'))) });
add('waiting_rows_follow_the_deltas_when_the_window_ends', { until: 300, writes: writes(at(0, say('a')), at(10, assistant([{ type: 'text', text: 'x' }])), at(20, assistant([{ type: 'text', text: 'y' }]))) });
add('deltas_of_another_kind_block_or_parent_are_not_merged', { until: 300, writes: writes(at(0, say('a'), say('b', {}, 1), delta('input_json_delta', 'partial_json', '{"a":'), delta('input_json_delta', 'partial_json', '1}'), say('c', { parent_tool_use_id: 'p' }), say('d', { parent_tool_use_id: 'p' }), say('e', { api_message_id: 'x' }), say('f', { api_message_id: 'x' }), say('g', { thinking_display: 'd' }), say('h', { thinking_display: 'd' }))) });
add('deltas_that_carry_no_text_are_never_merged', { until: 300, writes: writes(at(0, delta('citations_delta', 'citation', { a: 1 }), delta('citations_delta', 'citation', { a: 2 }), delta('signature_delta', 'signature', 's'), delta('signature_delta', 'signature', 't'), delta('text_delta', 'text', 5), delta('text_delta', 'text', 6), say('x'), say('y'), delta('other_delta', 'x', 'q'))) });
add('thinking_deltas_add_up_their_token_estimates', { until: 300, writes: writes(at(0, think('a', 2), think('b'), think('c', 5.5), say('t'), think('x'), think('y'), say('u'), think('p'), think('q', 3))) });
add('a_lone_delta_is_sent_as_it_came', { until: 300, writes: writes(at(0, say('a'))) });
add('the_transformations_of_a_message_are_not_sent', { until: 300, writes: writes(at(0, start(), stream({ type: 'message_delta', delta: { stop_reason: 'end_turn' }, input_transformations: { b: 2 } }), stream({ type: 'message_delta', delta: {}, input_transformations: null }), stream({ type: 'message_start', message: { id: 'm2', input_transformations: 0 } }))) });
add('a_delta_over_the_size_limit_is_dropped', { until: 300, writes: writes(at(0, say(big(70000)), say('b'))) });
add('big_deltas_that_do_not_fit_together_stay_apart', { until: 300, writes: writes(at(0, say(big(40000)), say(big(40000)), say('c'))) });
add('multibyte_deltas_are_measured_in_bytes', { until: 300, writes: writes(at(0, say(big(10000, '\u00e9')), say(big(10000, '\u00e9')), say(big(10000, '\u00e9')))) });
add('escaped_characters_count_as_their_escapes', { until: 300, writes: writes(at(0, say(big(10000, '\n')), say(big(10000, '\n')), say(big(10000, '\n')))) });
add('flushing_sends_the_window_at_once', { until: 300, flushAt: 40, writes: writes(at(0, say('a')), at(10, assistant([{ type: 'text', text: 'hi' }])), at(60, say('b'))) });
add('closing_drops_the_window', { until: 400, closeAt: 50, writes: writes(at(0, say('a')), at(10, assistant([{ type: 'text', text: 'hi' }]))) });
add('reconnecting_sends_the_waiting_rows_and_drops_the_deltas', { until: 400, reconnectAt: 50, writes: writes(at(0, say('a')), at(10, assistant([{ type: 'text', text: 'hi' }])), at(60, say('b'))) });
const tool = (id) => assistant([{ type: 'text', text: 'hi' }, { type: 'tool_use', id, name: 'Bash', input: {} }]);
add('closing_sends_the_row_that_is_ready', { until: 400, closeAt: 0, writes: writes(at(0, tool('t1'))) });
add('closing_sends_the_row_that_waits_behind_the_ready_one', { until: 400, closeAt: 0, writes: writes(at(0, tool('t1')), at(0, assistant([{ type: 'text', text: 'wait' }]))) });
const row = (text) => user(text);
add('two_rows_written_together_are_posted_apart', { until: 400, together: true, writes: writes(at(0, row('a'), row('b'))) });
add('several_rows_written_together_follow_the_first_in_one_post', { until: 400, together: true, writes: writes(at(0, row('a'), row('b'), row('c'), row('d'), row('e'))) });
add('rows_written_together_behind_a_window_follow_the_window_row_in_one_post', { until: 400, together: true, writes: writes(at(0, say('a')), at(10, row('a'), row('b'), row('c'))) });
add('two_rows_written_together_behind_a_window_are_posted_apart', { until: 400, together: true, writes: writes(at(0, say('a')), at(10, row('a'), row('b'))) });
add('rows_written_together_in_two_milliseconds_are_posted_by_millisecond', { until: 400, together: true, writes: writes(at(0, row('a'), row('b'), row('c')), at(5, row('d'), row('e'))) });
add('rows_written_together_and_a_close_in_the_same_step', { until: 400, together: true, closeAt: 0, writes: writes(at(0, row('a'), row('b'), row('c'))) });
add('rows_and_a_flush_in_the_same_step', { until: 400, together: true, flushAt: 0, writes: writes(at(0, row('a'), row('b'), row('c'))) });
add('rows_written_together_behind_a_window_and_a_close_in_the_same_step', { until: 400, together: true, closeAt: 10, writes: writes(at(0, say('a')), at(10, row('a'), row('b'))) });
add('rows_written_together_follow_the_first_in_posts_of_a_hundred', { until: 400, together: true, writes: writes(at(0, ...Array.from({ length: 102 }, (_, n) => row(`r${n}`)))) });
const evt = (id) => `id: ${id}\nevent: client_event\ndata: ${JSON.stringify({ event_id: `e${id}`, sequence_num: id, event_type: 'user', source: 'client', payload: { type: 'user', uuid: `u${id}`, message: { role: 'user', content: 'hi' } } })}\n\n`;
const probe = 'event: ephemeral_event\ndata: {"event_type":"heartbeat_probe"}\n\n';
const ping = ': ka\n\n';
const quiet = 600000;
const streamed = (name, conns, rest = {}) => add(name, { interval: quiet, until: 30000, conns, ...rest });
streamed('a_response_that_never_comes_is_given_up_after_thirty_seconds', ['hang', {}], { until: 100000 });
streamed('a_failed_open_is_retried_with_a_growing_delay', [{ status: 500 }, { status: 502 }, { status: 503 }, {}]);
streamed('a_refusal_whose_body_never_ends_is_given_up_after_three_seconds', [{ status: 500, hangBody: true }, {}]);
streamed('a_stream_that_ends_is_reopened_after_the_last_sequence', [{ chunks: [[0, evt(4)]], then: 'end' }, {}]);
streamed('a_stream_that_opened_starts_the_delay_over', [{ then: 'end' }, { status: 500 }, { chunks: [[0, evt(1)]], then: 'end' }, {}], { until: 40000 });
streamed('a_connection_that_is_reset_is_retried', ['reset', {}]);
streamed('a_stream_that_breaks_is_reopened_after_the_last_sequence', [{ chunks: [[50, evt(2)]], then: 'reset' }, {}]);
streamed('a_permanent_status_closes_the_session', [{ status: 403 }], { until: 5000 });
streamed('a_permanent_status_with_a_body_that_never_ends_closes_after_three_seconds', [{ status: 401, hangBody: true }], { until: 8000 });
streamed('a_silent_stream_is_reopened_by_the_liveness_timer', [{}, {}], { until: 120000 });
streamed('a_frame_postpones_the_liveness_timer', [{ chunks: [[30000, ping]] }, {}], { until: 120000 });
streamed('a_chunk_without_a_whole_frame_does_not_postpone_it', [{ chunks: [[30000, ': k']] }, {}], { until: 120000 });
streamed('a_late_liveness_check_reopens_at_once', [{}, {}], { until: 60000, skews: [[1000, 20000, 20000]] });
streamed('a_late_wall_clock_alone_reopens_at_once', [{}, {}], { until: 60000, skews: [[1000, 20000, 0]] });
streamed('a_late_monotonic_clock_alone_reopens_at_once', [{}, {}], { until: 60000, skews: [[1000, 0, 20000]] });
streamed('a_small_jump_is_a_lost_stream', [{}, {}], { until: 60000, skews: [[1000, 5000, 5000]] });
streamed('a_probe_sends_a_beat_at_once', [{ chunks: [[100, probe]] }], { until: 20000 });
streamed('a_second_probe_waits_for_the_beat_spacing', [{ chunks: [[100, probe], [600, probe]] }], { until: 20000 });
streamed('a_third_probe_does_not_add_a_second_wait', [{ chunks: [[100, probe], [600, probe], [900, probe]] }], { until: 20000 });
streamed('a_probe_during_an_unanswered_beat_is_sent_after_it', [{ chunks: [[100, probe], [600, probe]] }], { until: 60000, answers: ['hang'] });
streamed('closing_stops_the_liveness_timer', [{}, {}], { until: 120000, closeAt: 10000 });
streamed('closing_stops_a_waiting_reconnect', ['reset', {}], { until: 20000, closeAt: 500 });
streamed('closing_stops_a_pending_open', ['hang', {}], { until: 100000, closeAt: 5000 });
streamed('closing_stops_a_stream_that_delivers', [{ chunks: [[100, evt(1)]] }, {}], { until: 120000, closeAt: 20000 });
add('a_reconnect_restarts_the_heartbeat_and_opens_the_stream_after_the_last_sequence', { until: 60000, conns: [{ chunks: [[100, evt(3)]] }, {}], reconnectAt: 5000 });
add('closing_stops_the_timer', { until: 100000, closeAt: 25000 });
add('a_terminal_answer_stops_the_timer', { until: 90000, answers: [{ status: 409, body: {} }] });

async function run(spec) {
  timers.clear();
  timeline = [];
  randoms = [];
  streamRandoms = [];
  pendingRandom = null;
  wallSkew = 0;
  monoSkew = 0;
  virtual = 0;
  seed = 1;
  let beats = 0;
  let streams = 0;
  const conns = spec.conns ?? [];
  const later = (delay, callback) => {
    const id = nextId++;
    timers.set(id, { at: virtual + delay, callback, args: [], every: 0, order: id });
  };
  const events = (body) => JSON.stringify(JSON.parse(body).events.map((event) => {
    const { session_id: _session, ...payload } = event.payload;
    return { p: payload, ...(event.ephemeral && { e: 1 }) };
  }));
  const answers = spec.answers ?? [];
  const server = http.createServer((req, res) => {
    const route = req.url.replace(/^\/v1\/code\/sessions\/cse_abc/, '');
    const chunks = [];
    req.on('data', (chunk) => chunks.push(chunk));
    req.on('end', () => {
      const detail = route === '/worker/events' ? ` ${events(Buffer.concat(chunks).toString())}` : '';
      if (!(req.method === 'GET' && route === '/worker')) timeline.push(`${virtual} request ${req.method} ${route}${detail}`);
      if (req.method === 'GET' && route.split('?')[0] === '/worker/events/stream') {
        const conn = conns[streams] ?? {};
        streams += 1;
        if (conn === 'hang') return;
        if (conn === 'reset') { req.socket.destroy(); return; }
        res.statusCode = conn.status ?? 200;
        res.setHeader('content-type', 'text/event-stream');
        res.flushHeaders();
        for (const [delay, text] of conn.chunks ?? []) later(delay, () => res.write(text));
        const last = Math.max(0, ...(conn.chunks ?? []).map(([delay]) => delay));
        if (conn.then === 'reset') later(last, () => res.socket.destroy());
        if (conn.then === 'end' || (conn.status >= 400 && !conn.hangBody)) later(last, () => res.end());
        return;
      }
      let answer = accept;
      if (route === '/worker/heartbeat') {
        const scripted = answers[beats];
        beats += 1;
        if (scripted === 'hang') return;
        if (scripted) answer = scripted;
      }
      res.statusCode = answer.status;
      res.setHeader('content-type', 'application/json');
      res.end(JSON.stringify(answer.body));
    });
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const base = `http://127.0.0.1:${server.address().port}`;
  const handle = await bridge.attachBridgeSession({
    sessionId: 'cse_abc',
    apiBaseUrl: base,
    ingressToken: 'tok',
    outboundOnly: conns.length === 0,
    epoch: 2,
    heartbeatIntervalMs: spec.interval,
    onClose: (code) => timeline.push(`${virtual} closed ${code}`),
  });
  await settle();
  const steps = [...(spec.writes ?? []).map(([when, item]) => [when, () => handle.write(item)])];
  if (spec.flushAt !== undefined) steps.push([spec.flushAt, () => { handle.flush().catch(() => {}); }]);
  if (spec.closeAt !== undefined) steps.push([spec.closeAt, () => handle.close()]);
  if (spec.reconnectAt !== undefined) steps.push([spec.reconnectAt, () => { handle.reconnectTransport({ apiBaseUrl: base, ingressToken: 'tok2' }).catch(() => {}); }]);
  for (const [when, wall, mono] of spec.skews ?? []) steps.push([when, () => { wallSkew += wall; monoSkew += mono; }]);
  steps.sort((a, b) => a[0] - b[0]);
  if (spec.together) {
    const merged = [];
    for (const [when, step] of steps) {
      const last = merged.at(-1);
      if (last && last[0] === when) last[1].push(step); else merged.push([when, [step]]);
    }
    steps.length = 0;
    for (const [when, group] of merged) steps.push([when, () => { for (const step of group) step(); }]);
  }
  for (const [when, step] of steps) {
    await advanceTo(when);
    step();
  }
  await advanceTo(spec.until);
  try { handle.close(); } catch {}
  server.closeAllConnections();
  await new Promise((resolve) => server.close(resolve));
  const script = { interval: spec.interval ?? null, until: spec.until, close_at: spec.closeAt ?? null, flush_at: spec.flushAt ?? null, reconnect_at: spec.reconnectAt ?? null, ...(spec.together && { together: true }), conns, skews: spec.skews ?? [], writes: spec.writes ?? [], answers: answers.map((a) => a) };
  return { name: spec.name, script, randoms: [...randoms], stream_randoms: [...streamRandoms], timeline: [...timeline] };
}

const results = [];
for (const spec of cases) results.push(await run(spec));
fs.rmSync(work, { recursive: true, force: true });

const text = `{"timelines": [\n${results.map((result) => JSON.stringify(result)).join(',\n')}\n]}\n`;
if (flags.includes('--check')) {
  if (fs.readFileSync(out, 'utf8') !== text) {
    process.stderr.write('bridge worker timelines are stale\n');
    process.exit(1);
  }
} else {
  fs.writeFileSync(out, text);
}
process.exit(0);
