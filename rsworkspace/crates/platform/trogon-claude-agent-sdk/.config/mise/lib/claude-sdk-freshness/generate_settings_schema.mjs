#!/usr/bin/env node
// Extracts the settings schema the pinned TypeScript SDK validates every settings document with.
//
// The schema lives in the minified bundle behind a private class, so the generator copies the pinned
// package into a scratch directory, adds a one-line hook to the copy's settings schema holder and
// drives the public `resolveSettings` once so the hook fires. The captured zod v4 schemas are
// serialized into a closed JSON vocabulary. Anything the vocabulary cannot express (an unknown
// schema kind, a function the Rust side has no operation for) fails the run, so the checked-in file
// can never silently describe something the interpreter does not implement.
//
// Usage: generate_settings_schema.mjs --package DIR --out FILE [--check]
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const args = process.argv.slice(2);
const option = (name) => {
  const index = args.indexOf(name);
  return index === -1 ? undefined : args[index + 1];
};
const pkg = option('--package');
const out = option('--out');
const check = args.includes('--check');
if (!pkg || !out) {
  process.stderr.write('usage: generate_settings_schema.mjs --package DIR --out FILE [--check]\n');
  process.exit(2);
}

const HOOK_ANCHOR = 'this.keys=Object.keys(e).filter((n)=>e[n]!==void 0)';
async function main() {
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'settings-schema-'));
  let captured;
  try {
    fs.cpSync(pkg, scratch, { recursive: true });
    const bundle = path.join(scratch, 'sdk.mjs');
    const text = fs.readFileSync(bundle, 'utf8');
    const first = text.indexOf(HOOK_ANCHOR);
    if (first === -1 || text.indexOf(HOOK_ANCHOR, first + 1) !== -1) {
      throw new Error('the settings schema holder constructor anchor is not unique in the pinned bundle');
    }
    const hook = `${HOOK_ANCHOR};if(this.keys.includes("permissions")&&!globalThis.__settingsSchema){globalThis.__settingsSchema={fields:this.fields,active:[...this.activeWhenAbsent]}}`;
    fs.writeFileSync(bundle, text.replace(HOOK_ANCHOR, hook));
    const sdk = await import(pathToFileURL(bundle).href);
    const home = fs.mkdtempSync(path.join(os.tmpdir(), 'settings-schema-home-'));
    const previousHome = process.env.HOME;
    process.env.HOME = home;
    fs.mkdirSync(path.join(home, '.claude'));
    fs.writeFileSync(path.join(home, '.claude', 'settings.json'), '{"model":"x"}');
    try {
      await sdk.resolveSettings({ cwd: home });
    } finally {
      if (previousHome === undefined) delete process.env.HOME;
      else process.env.HOME = previousHome;
      fs.rmSync(home, { recursive: true, force: true });
    }
    captured = globalThis.__settingsSchema;
    if (!captured) throw new Error('the settings schema holder never ran');
    const document = serialize(captured);
    const rendered = `${JSON.stringify(document, null, 2)}\n`;
    if (check) {
      const existing = fs.existsSync(out) ? fs.readFileSync(out, 'utf8') : '';
      if (existing !== rendered) {
        process.stderr.write(`${out} differs from the pinned upstream settings schema; run claude-sdk:schema\n`);
        process.exitCode = 1;
      }
    } else {
      fs.writeFileSync(out, rendered);
    }
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true });
  }
}

function definitionOf(schema) {
  const def = schema?._zod?.def;
  if (!def) throw new Error('not a zod v4 schema');
  return def;
}

function serialize({ fields, active }) {
  // First pass: count how often each schema object is reached so shared and recursive ones become
  // named definitions and everything else is inlined.
  const reach = new Map();
  const children = (schema) => {
    const def = definitionOf(schema);
    const found = [];
    for (const key of ['innerType', 'in', 'out', 'element', 'keyType', 'valueType', 'catchall']) {
      if (def[key] && typeof def[key] === 'object' && def[key]._zod) found.push(def[key]);
    }
    if (def.shape) for (const value of Object.values(def.shape)) found.push(value);
    if (def.options) found.push(...def.options);
    if (def.type === 'lazy') found.push(def.getter());
    return found;
  };
  const visit = (schema) => {
    const count = (reach.get(schema) ?? 0) + 1;
    reach.set(schema, count);
    if (count > 1) return;
    for (const child of children(schema)) visit(child);
  };
  const keys = Object.keys(fields).filter((key) => fields[key] !== undefined);
  const built = new Map(keys.map((key) => [key, fields[key]()]));
  for (const schema of built.values()) visit(schema);
  const named = new Map();
  const definitions = {};
  const emit = (schema) => {
    const shared = (reach.get(schema) ?? 0) > 1 || definitionOf(schema).type === 'lazy';
    if (!shared) return node(schema);
    if (!named.has(schema)) {
      const id = `n${named.size}`;
      named.set(schema, id);
      definitions[id] = null;
      definitions[id] = node(schema);
    }
    return { ref: named.get(schema) };
  };
  const checkOf = (check) => {
    const def = check._zod.def;
    const result = { check: def.check };
    for (const [key, value] of Object.entries(def)) {
      if (key === 'check' || key === 'error' || key === 'abort' || key === 'when') continue;
      if (typeof value === 'function') {
        result[key] = { source: value.toString(), probes: probe(value, STRING_BATTERY) };
      } else if (value instanceof RegExp) {
        result[key] = { regex: value.source, flags: value.flags };
      } else if (value instanceof Set) {
        result[key] = [...value];
      } else {
        result[key] = value;
      }
    }
    if (def.check === 'custom' && def.fn === undefined) {
      result.payloadCheck = { source: check._zod.check.toString() };
    }
    return result;
  };
  const node = (schema) => {
    const def = definitionOf(schema);
    const checks = (def.checks ?? []).map(checkOf);
    const withChecks = (value) => (checks.length > 0 ? { ...value, checks } : value);
    switch (def.type) {
      case 'string':
      case 'boolean':
      case 'number':
      case 'unknown':
      case 'undefined':
        return withChecks({ type: def.type });
      case 'literal':
        return withChecks({ type: 'literal', values: def.values });
      case 'enum':
        return withChecks({ type: 'enum', values: Object.values(def.entries) });
      case 'optional':
        return withChecks({ type: 'optional', inner: emit(def.innerType) });
      case 'array':
        return withChecks({ type: 'array', element: emit(def.element) });
      case 'union':
        return withChecks({ type: 'union', options: def.options.map(emit) });
      case 'record':
        return withChecks({ type: 'record', key: emit(def.keyType), value: emit(def.valueType) });
      case 'pipe':
        return withChecks({ type: 'pipe', in: emit(def.in), out: emit(def.out) });
      case 'lazy':
        return withChecks({ type: 'lazy', inner: emit(def.getter()) });
      case 'catch': {
        const value = def.catchValue({ issues: [], value: undefined });
        return withChecks({ type: 'catch', inner: emit(def.innerType), value: value === undefined ? { absent: true } : { value } });
      }
      case 'transform':
        return { type: 'transform', source: def.transform.toString(), probes: probe(def.transform) };
      case 'object': {
        const shape = {};
        for (const [key, value] of Object.entries(def.shape)) shape[key] = emit(value);
        const catchall = def.catchall;
        const mode = catchall === undefined ? 'strip' : catchall._zod.def.type === 'never' ? 'strict' : 'passthrough';
        if (mode === 'passthrough' && catchall._zod.def.type !== 'unknown') {
          throw new Error('an object catchall other than unknown is not in the vocabulary');
        }
        return withChecks({ type: 'object', mode, shape });
      }
      default:
        throw new Error(`unsupported settings schema kind: ${def.type}`);
    }
  };
  const shape = {};
  for (const key of keys) shape[key] = emit(built.get(key));
  return { keys, active, batteries: { value: encodeBattery(VALUE_BATTERY), string: encodeBattery(STRING_BATTERY) }, fields: shape, definitions };
}

const VALUE_BATTERY = [
  null,
  1,
  'x',
  true,
  false,
  [],
  ['a', null, 1],
  {},
  { path: null },
  { path: null, script: null, defaultSettings: null },
  { a: 1 },
  ['skills', 'bogus'],
  ['Bash'],
  ['/a/'],
  9999999999,
];

const STRING_BATTERY = [
  '',
  ' ',
  'x',
  'a b',
  ' padded ',
  'a/b',
  'a\\b',
  '..',
  '.',
  'a/../b',
  'a//b',
  '/abs/path',
  'C:\\win\\path',
  '\\\\server\\share',
  'rel/path',
  './rel',
  'a\u0000b',
  'caf\u00e9',
  'https://example.com/x',
  'http://example.com/x',
  'https://localhost/x',
  'https://127.0.0.1/x',
  'ftp://example.com',
  'git+ssh://host/repo',
  'file:///a/../b',
  'github:owner/repo',
  'constructor',
  'prototype',
  '__proto__',
  'Name_1-x',
  'with.dot',
  '$VAR',
  'a*b',
  'prefix*',
  '*',
  'Bash(ls)',
  'ALLCAPS',
  'tool.exe',
  'x'.repeat(70000),
  'a'.repeat(501),
  '00:00',
  '9:30',
];

// Long strings are recorded as a repeat marker so the document stays small and the consumer rebuilds them.
function encodeBattery(battery) {
  return battery.map((input) => (typeof input === 'string' && input.length > 100 ? { repeat: [input[0], input.length] } : input));
}

// The recorded results of running a transform on a fixed battery of inputs. Transforms close over
// values the source text alone does not show, so the battery pins their behaviour.
function probe(transform, battery = VALUE_BATTERY) {
  const inputs = battery;
  const shorten = (value) => JSON.stringify(value, (_key, item) => (typeof item === 'string' && item.length > 100 ? `<${item.length} characters>` : item));
  const results = inputs.map((input) => {
    try {
      const result = transform(structuredClone(input));
      if (result === undefined) return 'absent';
      if (typeof result === 'boolean') return result ? 'true' : 'false';
      return shorten(result);
    } catch (error) {
      return `error: ${error.message}`;
    }
  });
  return results;
}

await main();
