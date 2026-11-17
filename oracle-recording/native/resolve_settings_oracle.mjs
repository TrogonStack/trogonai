#!/usr/bin/env node
// Runs the pinned TypeScript resolveSettings against a sandbox root the way a real process on this host runs it, and
// prints one JSON document.
//
// usage: node resolve_settings_oracle.mjs <package-dir> <spec.json>
//
// The spec is the one the simulating oracle beside the recorders takes, with three differences that keep every run
// on this host's own code paths:
//
// - The platform is the host's. A spec whose `platform` names another one is refused (exit 3), never emulated.
// - The clock is the real one. `Date.now` and the timers are Node's own, so the first resolve of a process re-reads
//   the managed settings as a real first call does. A case's `at` moves `Date.now` that many milliseconds past the
//   real time the run started; the timers stay real.
// - A spawned program is the real one at the path the SDK names, and its end reaches the SDK from its own exit.
//   `tools` and `durations` do not apply and are recorded as ignored. Each program runs with the root as its working
//   folder, where a registry stand-in finds its answers.
//
// Only machine-wide settings locations are remapped under `root` (managed settings, managed preferences); the
// programs and `/proc/version` stay the host's, so a case that writes a fake `/proc/version` is refused.
//
// On darwin a managed preferences plist written as JSON is rewritten as an XML plist when every value has a plist
// form, so the real plutil reads it; a document holding `null` stays as written.
//
// `call: "second"` runs one resolve with the first case's options before the recorded cases, so they run as the
// later calls of a long-lived process do.
//
// Beside the spec, `<spec.json>.native.json` reports the platform, the clock start, the programs spawned and ended by
// name, what was ignored or refused, and those counts as each case settled.
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import cp from 'node:child_process';
import os from 'node:os';
import path from 'node:path';
import { syncBuiltinESMExports } from 'node:module';
import { pathToFileURL } from 'node:url';

const realNow = Date.now;
const clockStart = realNow();
const realSetTimeout = globalThis.setTimeout;
const [pkg, specPath] = process.argv.slice(2);
if (!pkg || !specPath) {
  process.stderr.write('usage: resolve_settings_oracle.mjs PACKAGE_DIR SPEC_JSON\n');
  process.exit(2);
}
const writeFile = fs.writeFileSync;
const spec = JSON.parse(fs.readFileSync(specPath, 'utf8'));
const root = spec.root;
const hostWindows = process.platform === 'win32';
const report = {
  platform: process.platform,
  arch: process.arch,
  release: os.release(),
  proc_version: null,
  clock: { source: 'real', start: clockStart, offsets: [] },
  call: spec.call ?? 'first',
  spawns: {},
  exits: {},
  ignored: [],
  refused: null,
  cases: [],
};
if (process.platform === 'linux') {
  try {
    report.proc_version = fs.readFileSync('/proc/version', 'utf8');
  } catch {
    report.proc_version = null;
  }
}
process.on('exit', () => writeFile(`${specPath}.native.json`, JSON.stringify(report)));
const refuse = (reason) => {
  report.refused = reason;
  process.stderr.write(`native refusal: ${reason}\n`);
  process.exit(3);
};
if (spec.platform && spec.platform !== process.platform) refuse(`platform ${spec.platform} on a ${process.platform} host`);
if (spec.call !== undefined && spec.call !== 'first' && spec.call !== 'second') refuse(`call ${spec.call}`);
if (fs.existsSync(path.join(root, 'proc'))) refuse('a /proc stand-in under the root');
for (const entry of spec.cases) {
  if (Object.keys(entry.files || {}).some((file) => file === 'proc' || file.startsWith('proc/'))) {
    refuse(`case ${entry.name} writes a /proc stand-in`);
  }
  if (entry.at !== undefined && entry.at !== null && !(typeof entry.at === 'number' && entry.at >= 0)) {
    refuse(`case ${entry.name} sets at ${entry.at}`);
  }
  if (entry.durations && Object.keys(entry.durations).length > 0) report.ignored.push(`durations:${entry.name}`);
}
if (spec.tools !== undefined) report.ignored.push(`tools:${spec.tools}`);
const VIRTUAL = [
  '/etc/claude-code',
  '/Library/Application Support/ClaudeCode',
  '/Library/Managed Preferences',
  '/mnt/c/Program Files/ClaudeCode',
  '/Program Files/ClaudeCode',
];
const remap = (value) => {
  if (typeof value !== 'string') return value;
  const drive = hostWindows ? /^[A-Za-z]:(?=[\\/])/ : /^[A-Za-z]:(?=\\)/;
  const asked = hostWindows || drive.test(value) ? value.replace(drive, '').replaceAll('\\', '/') : value;
  for (const prefix of VIRTUAL) {
    if (asked === prefix || asked.startsWith(`${prefix}/`)) return path.join(root, asked);
  }
  return value;
};
// The XML plist of a JSON document, or null when a value has no plist form. Built from a work list, so a document
// nested deeper than the stack still converts.
const plistOf = (text) => {
  let top;
  try {
    top = JSON.parse(text);
  } catch {
    return null;
  }
  const escape = (value) => value.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;');
  const parts = [];
  const work = [{ value: top }];
  while (work.length > 0) {
    const item = work.pop();
    if (typeof item === 'string') {
      parts.push(item);
      continue;
    }
    const { value } = item;
    if (value === null) return null;
    if (typeof value === 'boolean') parts.push(value ? '<true/>' : '<false/>');
    else if (typeof value === 'number') parts.push(Number.isInteger(value) ? `<integer>${value}</integer>` : `<real>${value}</real>`);
    else if (typeof value === 'string') parts.push(`<string>${escape(value)}</string>`);
    else if (Array.isArray(value)) {
      const sequence = ['<array>', ...value.map((member) => ({ value: member })), '</array>'];
      for (let index = sequence.length - 1; index >= 0; index--) work.push(sequence[index]);
    } else {
      const sequence = ['<dict>'];
      for (const key of Object.keys(value)) sequence.push(`<key>${escape(key)}</key>`, { value: value[key] });
      sequence.push('</dict>');
      for (let index = sequence.length - 1; index >= 0; index--) work.push(sequence[index]);
    }
  }
  return (
    '<?xml version="1.0" encoding="UTF-8"?>\n' +
    '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n' +
    `<plist version="1.0">${parts.join('')}</plist>\n`
  );
};
const preferences = path.join(root, 'Library/Managed Preferences');
const nativePlist = (location) => {
  if (process.platform !== 'darwin' || !location.endsWith('.plist')) return;
  const relative = path.relative(preferences, location);
  if (relative.startsWith('..') || path.isAbsolute(relative)) return;
  const xml = plistOf(fs.readFileSync(location, 'utf8'));
  report.ignored.push(`${xml === null ? 'plist_kept_json' : 'plist_as_xml'}:${relative}`);
  if (xml !== null) writeFile(location, xml);
};
const plists = (folder) => {
  if (!fs.existsSync(folder)) return;
  for (const name of fs.readdirSync(folder)) {
    const location = path.join(folder, name);
    if (fs.statSync(location).isDirectory()) plists(location);
    else nativePlist(location);
  }
};
plists(preferences);
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
// Spawned programs, counted by name from the spawn to the callback the SDK gets from their own end.
const running = new Set();
const count = (table, name) => {
  table[name] = (table[name] ?? 0) + 1;
};
const execFile = cp.execFile;
cp.execFile = function (file, args, options, callback) {
  if (typeof args === 'function') [args, options, callback] = [[], {}, args];
  else if (typeof options === 'function') [options, callback] = [{}, options];
  const name = String(file).split(/[\\/]/).pop().toLowerCase();
  const token = {};
  running.add(token);
  count(report.spawns, name);
  try {
    return execFile.call(this, file, (args || []).map(remap), { ...options, cwd: root }, (...result) => {
      running.delete(token);
      count(report.exits, name);
      if (callback) callback(...result);
    });
  } catch (error) {
    running.delete(token);
    report.spawns[name] -= 1;
    count(report.spawns, `threw:${name}`);
    count(report.exits, `threw:${name}`);
    throw error;
  }
};
syncBuiltinESMExports();
for (const [name, value] of Object.entries(spec.env || {})) {
  if (value === null) delete process.env[name];
  else process.env[name] = value;
}
if (hostWindows && typeof spec.env?.HOME === 'string') process.env.USERPROFILE = spec.env.HOME;
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
// Moves `Date.now` past the real time once a case asks for it; until then it is Node's own.
let offset = 0;
const shift = (by) => {
  if (by === offset) return;
  offset = by;
  report.clock.offsets.push(by);
  Date.now = () => realNow() + offset;
};
// Pending file requests; spawned programs are counted in `running` until their callback, because the standard streams
// may themselves be pipes that stay open for the whole run.
const REAL_WORK = new Set(['FSReqCallback', 'FSReqPromise', 'FileHandleCloseReq']);
// Waits until no program and no file request is pending, ending on an immediate, so the next resolve starts from an
// immediate as it does in the simulating oracle.
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
// Waits for `promise` on the real clock, a real timer (the re-read race) included.
const settle = async (promise) => {
  let done = false;
  promise.then(
    () => (done = true),
    () => (done = true),
  );
  for (;;) {
    await quiet();
    if (done) return promise;
    await new Promise((resolve) => realSetTimeout(resolve, 1));
  }
};
const apply = (entry) => {
  for (const [file, text] of Object.entries(entry.files || {})) {
    const location = path.join(root, file);
    if (text === null) fs.rmSync(location, { force: true });
    else {
      fs.mkdirSync(path.dirname(location), { recursive: true });
      writeFile(location, text);
      if (entry.executable?.includes(file)) fs.chmodSync(location, 0o755);
      nativePlist(location);
    }
  }
};
const snapshot = (name) => ({ name, spawns: { ...report.spawns }, exits: { ...report.exits }, running: running.size });
if (report.call === 'second' && spec.cases.length > 0) {
  const first = spec.cases[0];
  apply(first);
  try {
    await settle(sdk.resolveSettings(first.options));
  } catch {
    // The prime call only moves the process past its first resolve; what it answers is not recorded.
  }
  report.cases.push(snapshot('prime'));
}
const out = [];
for (const entry of spec.cases) {
  if (typeof entry.at === 'number') shift(entry.at);
  apply(entry);
  if (entry.resume === 'file') await fsp.stat(root);
  let result;
  try {
    result = await settle(sdk.resolveSettings(entry.options));
  } catch (error) {
    report.cases.push(snapshot(entry.name));
    if (!spec.rejections || !(error instanceof RangeError)) throw error;
    out.push({ name: entry.name, error: { name: error.name, message: error.message } });
    continue;
  }
  report.cases.push(snapshot(entry.name));
  out.push(spec.deep ? { name: entry.name, result } : { name: entry.name, result, filtered: sdk.filterEscalatingDefaultMode(result) });
}
// A background re-read may still run a program; it ends (or times out, 5 s upstream) before the output is written, so
// every program spawned is counted as ended.
const until = realNow() + 15000;
while (running.size > 0 && realNow() < until) await new Promise((resolve) => realSetTimeout(resolve, 10));
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
process.stdout.write(spec.deep ? stringifyDeep(out) : JSON.stringify(out), () => process.exit(0));
