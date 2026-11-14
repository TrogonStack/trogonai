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
// `closed <code>` when the session reported its close code.
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
const lineStarts = [0];
for (let i = source.indexOf('\n'); i >= 0; i = source.indexOf('\n', i + 1)) lineStarts.push(i + 1);
// True when the innermost bundle frame of the caller sits inside the heartbeat schedule (the request the beat starts
// sets its own timers from the same call chain, and those are not the schedule).
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
  return value;
};
Date.now = () => BASE + virtual;
performance.now = () => 1000 + virtual;
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
  const wait = Number(delay) >= 1 ? Number(delay) : 1;
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
add('closing_stops_the_timer', { until: 100000, closeAt: 25000 });
add('a_terminal_answer_stops_the_timer', { until: 90000, answers: [{ status: 409, body: {} }] });

async function run(spec) {
  timers.clear();
  timeline = [];
  randoms = [];
  pendingRandom = null;
  virtual = 0;
  seed = 1;
  let beats = 0;
  const answers = spec.answers ?? [];
  const server = http.createServer((req, res) => {
    const route = req.url.replace(/^\/v1\/code\/sessions\/cse_abc/, '');
    req.resume();
    req.on('end', () => {
      if (!(req.method === 'GET' && route === '/worker')) timeline.push(`${virtual} request ${req.method} ${route}`);
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
  const handle = await bridge.attachBridgeSession({
    sessionId: 'cse_abc',
    apiBaseUrl: `http://127.0.0.1:${server.address().port}`,
    ingressToken: 'tok',
    outboundOnly: true,
    epoch: 2,
    heartbeatIntervalMs: spec.interval,
    onClose: (code) => timeline.push(`${virtual} closed ${code}`),
  });
  await settle();
  if (spec.closeAt !== undefined) {
    await advanceTo(spec.closeAt);
    handle.close();
  }
  await advanceTo(spec.until);
  try { handle.close(); } catch {}
  server.closeAllConnections();
  await new Promise((resolve) => server.close(resolve));
  const script = { interval: spec.interval ?? null, until: spec.until, close_at: spec.closeAt ?? null, answers: answers.map((a) => a) };
  return { name: spec.name, script, randoms: [...randoms], timeline: [...timeline] };
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
