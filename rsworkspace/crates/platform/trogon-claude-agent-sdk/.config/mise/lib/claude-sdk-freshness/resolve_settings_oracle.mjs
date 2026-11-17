#!/usr/bin/env node
// Runs the pinned TypeScript resolveSettings against a sandbox root and prints one JSON document.
//
// usage: node resolve_settings_oracle.mjs <package-dir> <spec.json>
//
// The spec holds {root, platform, env, process?, writable?, deep?, rejections?, cases: [{name, options, at?, files?,
// durations?}]}. Machine-wide locations that the SDK reads (managed settings, managed preferences, /proc/version,
// plutil) are remapped under `root`, and `process.platform` is replaced by `platform`, so every case runs the same way
// on any host. All of /etc moves, not only the managed settings folder: a write check walks every folder down from the
// root, and a host where /etc is a link (macOS) would otherwise answer it differently from one where it is a folder.
//
// `process` pins the ids the process reports ({euid, uid, egid, gid, groups}; a missing or null field removes that
// call, as on a platform without it), and `writable` answers write checks by the path asked about (`*` for every
// other path): `yes` succeeds and an errno name such as `EACCES` fails. Paths without an answer get the real check.
//
// `deep: true` prints the output without recursing, so a result nested past the stack `JSON.stringify` has still prints
// (the same text `JSON.stringify` gives), and leaves out `filtered`, whose filter recurses as deep as the result. Only
// what runs after `resolveSettings` returns changes, so the stack every call starts on stays the same.
//
// `rejections: true` prints a call that rejects with a `RangeError` as `{name, error: {name, message}}` in place of its
// result, and goes on with the next call; without it the error ends the run.
//
// Time is simulated. `Date.now` reads a fake clock, timers wait on that clock, and a spawned program ends when the
// clock reaches its start plus the case's `durations` entry for its command line (zero when absent). The clock only
// moves once nothing real is pending (no child process, no file request), straight to the next due timer, so a run
// is the same on any machine however long the real work takes.
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import cp from 'node:child_process';
import os from 'node:os';
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
  '/etc',
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
const answers = spec.writable || {};
const access = fs.accessSync;
fs.accessSync = function (target, mode, ...rest) {
  const asked = typeof target === 'string' ? target : String(target);
  const answer = (mode ?? fs.constants.F_OK) & fs.constants.W_OK ? (answers[asked] ?? answers['*']) : undefined;
  if (answer === 'yes') return undefined;
  if (answer !== undefined) {
    const error = new Error(`${answer}: injected, access '${asked}'`);
    error.code = answer;
    error.errno = -(os.constants.errno[answer] ?? 0);
    error.syscall = 'access';
    error.path = asked;
    throw error;
  }
  return access.call(this, target, mode, ...rest);
};
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
if (spec.process) {
  const pinned = spec.process;
  const call = (value) => (value === undefined || value === null ? undefined : () => value);
  process.geteuid = call(pinned.euid);
  process.getuid = call(pinned.uid);
  process.getegid = call(pinned.egid);
  process.getgid = call(pinned.gid);
  // Node appends the effective group id to the list getgroups(2) returns when the list lacks it.
  process.getgroups = call(
    Array.isArray(pinned.groups)
      ? pinned.groups.includes(pinned.egid) || pinned.egid == null
        ? [...pinned.groups]
        : [...pinned.groups, pinned.egid]
      : undefined,
  );
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
  let result;
  try {
    result = await settle(sdk.resolveSettings(entry.options));
  } catch (error) {
    if (!spec.rejections || !(error instanceof RangeError)) throw error;
    out.push({ name: entry.name, error: { name: error.name, message: error.message } });
    continue;
  }
  out.push(spec.deep ? { name: entry.name, result } : { name: entry.name, result, filtered: sdk.filterEscalatingDefaultMode(result) });
}
// `JSON.stringify` of the plain data `resolveSettings` returns, with a work list in place of recursion: each entry is
// either text to print as is or a value (`{ value }`) to print in its place.
const stringifyDeep = (top) => {
  const printable = (value) => value !== undefined && typeof value !== 'function' && typeof value !== 'symbol';
  const parts = [];
  const work = [{ value: top }];
  while (work.length > 0) {
    const item = work.pop();
    if (typeof item === 'string') {
      parts.push(item);
      continue;
    }
    const { value } = item;
    if (value === null || typeof value !== 'object') {
      parts.push(JSON.stringify(value));
      continue;
    }
    const sequence = [];
    if (Array.isArray(value)) {
      sequence.push('[');
      for (let index = 0; index < value.length; index++) {
        if (index > 0) sequence.push(',');
        sequence.push({ value: printable(value[index]) ? value[index] : null });
      }
      sequence.push(']');
    } else {
      sequence.push('{');
      for (const key of Object.keys(value)) {
        if (!printable(value[key])) continue;
        if (sequence.length > 1) sequence.push(',');
        sequence.push(`${JSON.stringify(key)}:`, { value: value[key] });
      }
      sequence.push('}');
    }
    for (let index = sequence.length - 1; index >= 0; index--) work.push(sequence[index]);
  }
  return parts.join('');
};
// Exits once the output is written: a background re-read may still hold a fake timer that never fires.
process.stdout.write(spec.deep ? stringifyDeep(out) : JSON.stringify(out), () => process.exit(0));
