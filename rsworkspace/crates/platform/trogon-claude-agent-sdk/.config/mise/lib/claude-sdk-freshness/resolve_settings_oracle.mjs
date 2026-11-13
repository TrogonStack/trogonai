#!/usr/bin/env node
// Runs the pinned TypeScript resolveSettings against a sandbox root and prints one JSON document.
//
// usage: node resolve_settings_oracle.mjs <package-dir> <spec.json>
//
// The spec holds {root, platform, env, process?, writable?, cases: [{name, options, at?, files?}]}. Machine-wide
// locations that the SDK reads (managed settings, managed preferences, /proc/version, plutil) are remapped under
// `root`, and `process.platform` is replaced by `platform`, so every case runs the same way on any host.
//
// `process` pins the ids the process reports ({euid, uid, egid, gid, groups}; a missing or null field removes that
// call, as on a platform without it), and `writable` answers write checks by the path asked about (`*` for every
// other path): `yes` succeeds and an errno name such as `EACCES` fails. Paths without an answer get the real check.
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
const execFile = cp.execFile;
cp.execFile = function (file, args, options, callback) {
  return execFile.call(this, remap(file), (args || []).map(remap), options, callback);
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
// A case may move the clock (`at`, milliseconds since the epoch) and change files under the root before it runs
// (`files`: path under the root to new text, or null to delete), so one process sees the host change between calls
// the way a long-lived SDK process does.
let now = 0;
Date.now = () => now;
const out = [];
for (const entry of spec.cases) {
  // A re-read the previous call left running in the background settles before the next call starts.
  if (typeof entry.at === 'number') {
    await new Promise((resolve) => setTimeout(resolve, 300));
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
  const result = await sdk.resolveSettings(entry.options);
  out.push({ name: entry.name, result, filtered: sdk.filterEscalatingDefaultMode(result) });
}
process.stdout.write(JSON.stringify(out));
