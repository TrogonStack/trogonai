#!/usr/bin/env node
// Runs the pinned TypeScript resolveSettings against a sandbox root and prints one JSON document.
//
// usage: node resolve_settings_oracle.mjs <package-dir> <spec.json>
//
// The spec holds {root, platform, env, cases: [{name, options}]}. Machine-wide locations that the SDK reads
// (managed settings, managed preferences, /proc/version, plutil) are remapped under `root`, and
// `process.platform` is replaced by `platform`, so every case runs the same way on any host.
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
const sdk = await import(pathToFileURL(path.join(pkg, 'sdk.mjs')).href);
const out = [];
for (const entry of spec.cases) {
  const result = await sdk.resolveSettings(entry.options);
  out.push({ name: entry.name, result, filtered: sdk.filterEscalatingDefaultMode(result) });
}
process.stdout.write(JSON.stringify(out));
