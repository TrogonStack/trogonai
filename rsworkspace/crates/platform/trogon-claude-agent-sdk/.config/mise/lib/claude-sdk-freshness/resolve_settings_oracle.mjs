#!/usr/bin/env node
// Runs the pinned TypeScript resolveSettings against a sandbox root and prints one JSON document.
//
// usage: node resolve_settings_oracle.mjs <package-dir> <spec.json>
//
// The spec holds {root, platform, env, cases: [{name, options, at?, files?, durations?}]}. Machine-wide locations that
// the SDK reads (managed settings, managed preferences, /proc/version, plutil) are remapped under `root`, and
// `process.platform` is replaced by `platform`, so every case runs the same way on any host.
//
// Time is simulated. `Date.now` reads a fake clock, timers wait on that clock, and a spawned program ends when the
// clock reaches its start plus the case's `durations` entry for its command line (zero when absent). The clock only
// moves once nothing real is pending (no child process, no file request), straight to the next due timer, so a run
// is the same on any machine however long the real work takes.
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import cp from 'node:child_process';
import path from 'node:path';
import { syncBuiltinESMExports } from 'node:module';
import { pathToFileURL } from 'node:url';

const [pkg, specPath] = process.argv.slice(2);
if (!pkg || !specPath) {
  process.stderr.write('usage: resolve_settings_oracle.mjs PACKAGE_DIR SPEC_JSON\n');
  process.exit(2);
}
const spec = JSON.parse(fs.readFileSync(specPath, 'utf8'));
const root = spec.root;
const VIRTUAL = [
  '/etc/claude-code',
  '/Library/Application Support/ClaudeCode',
  '/Library/Managed Preferences',
  '/mnt/c',
  '/proc/version',
  '/usr/bin/plutil',
];
const remap = (value) => {
  if (typeof value !== 'string') return value;
  for (const prefix of VIRTUAL) {
    if (value === prefix || value.startsWith(`${prefix}/`)) return path.join(root, value);
  }
  return value;
};
const patch = (object, names) => {
  for (const name of names) {
    const original = object[name];
    if (typeof original !== 'function') continue;
    object[name] = function (first, ...rest) {
      return original.call(this, remap(first), ...rest.map(remap));
    };
    for (const key of Object.keys(original)) object[name][key] = original[key];
  }
};
const functions = (object) => Object.keys(object).filter((key) => typeof object[key] === 'function');
patch(fs, functions(fs).filter((name) => /^[a-z]/.test(name) && name !== 'realpathSync'));
if (fs.realpathSync.native) patch(fs.realpathSync, ['native']);
patch(fsp, functions(fsp));
// The fake clock and its timers. At one instant, timers run in the order they were set and a program's end comes after
// them, the way a program that takes exactly as long as a timer finishes once that timer has fired.
let now = 0;
let order = 0;
let durations = {};
const pending = [];
const realSetTimeout = globalThis.setTimeout;
const schedule = (due, phase, run) => {
  const timer = { due, phase, order: order++, run, live: true };
  pending.push(timer);
  return timer;
};
const nextTimer = () => {
  let best = -1;
  for (let index = 0; index < pending.length; index++) {
    const timer = pending[index];
    if (!timer.live) continue;
    const current = best === -1 ? null : pending[best];
    if (
      current === null ||
      timer.due < current.due ||
      (timer.due === current.due && (timer.phase < current.phase || (timer.phase === current.phase && timer.order < current.order)))
    )
      best = index;
  }
  return best === -1 ? null : pending.splice(best, 1)[0];
};
const running = new Set();
const execFile = cp.execFile;
cp.execFile = function (file, args, options, callback) {
  const line = [file, ...(args || [])].join(' ');
  const ends = now + (durations[line] ?? 0);
  const token = {};
  running.add(token);
  return execFile.call(this, remap(file), (args || []).map(remap), options, (...result) => {
    running.delete(token);
    schedule(ends, 1, () => callback(...result));
  });
};
syncBuiltinESMExports();
Object.defineProperty(process, 'platform', { value: spec.platform || process.platform });
for (const [name, value] of Object.entries(spec.env || {})) {
  if (value === null) delete process.env[name];
  else process.env[name] = value;
}
const sdk = await import(pathToFileURL(path.join(pkg, 'sdk.mjs')).href);
Date.now = () => now;
globalThis.setTimeout = (run, delay, ...rest) => {
  const timer = schedule(now + Math.max(0, Number(delay) || 0), 0, () => run(...rest));
  return { timer, ref: () => {}, unref: () => {}, hasRef: () => false };
};
globalThis.clearTimeout = (handle) => {
  if (handle && handle.timer) handle.timer.live = false;
};
// Pending file requests; spawned programs are counted in `running` until their callback, because the standard streams
// may themselves be pipes that stay open for the whole run.
const REAL_WORK = new Set(['FSReqCallback', 'FSReqPromise', 'FileHandleCloseReq']);
// Waits until no real work is left: every callback and promise that can run without the fake clock moving has run.
const quiet = async () => {
  let calm = 0;
  while (calm < 3) {
    await new Promise((resolve) => setImmediate(resolve));
    const busy = running.size > 0 || process.getActiveResourcesInfo().some((kind) => REAL_WORK.has(kind));
    if (busy) {
      calm = 0;
      await new Promise((resolve) => realSetTimeout(resolve, 1));
    } else calm += 1;
  }
};
const fire = (timer) => {
  if (timer.due > now) now = timer.due;
  timer.run();
};
// Runs everything due up to `until` (inclusive) as the clock moves there.
const advanceTo = async (until) => {
  for (;;) {
    await quiet();
    const timer = nextTimer();
    if (timer === null) return;
    if (timer.due > until) {
      pending.push(timer);
      return;
    }
    fire(timer);
  }
};
// Moves the clock from timer to timer until `promise` settles.
const settle = async (promise) => {
  let done = false;
  promise.then(
    () => (done = true),
    () => (done = true),
  );
  for (;;) {
    await quiet();
    if (done) return promise;
    const timer = nextTimer();
    if (timer === null) throw new Error('resolveSettings waits on nothing the fake clock can reach');
    fire(timer);
  }
};
// A case may move the clock (`at`, milliseconds since the epoch) and change files under the root before it runs
// (`files`: path under the root to new text, or null to delete), so one process sees the host change between calls
// the way a long-lived SDK process does. Whatever a previous call left running (a re-read in the background) goes on
// against the same clock: it ends before the case starts only if it is due by `at`.
const out = [];
for (const entry of spec.cases) {
  if (typeof entry.at === 'number') {
    await advanceTo(entry.at);
    now = entry.at;
  }
  for (const [file, text] of Object.entries(entry.files || {})) {
    const location = path.join(root, file);
    if (text === null) fs.rmSync(location, { force: true });
    else {
      fs.mkdirSync(path.dirname(location), { recursive: true });
      fs.writeFileSync(location, text);
      if (entry.executable?.includes(file)) fs.chmodSync(location, 0o755);
    }
  }
  durations = entry.durations || {};
  const result = await settle(sdk.resolveSettings(entry.options));
  out.push({ name: entry.name, result, filtered: sdk.filterEscalatingDefaultMode(result) });
}
// Exits once the output is written: a background re-read may still hold a fake timer that never fires.
process.stdout.write(JSON.stringify(out), () => process.exit(0));
