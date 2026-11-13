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
// zod's `superRefine` wraps the refinement in a closure, so the copy keeps the refinement on the check
// where the serializer can read its source.
const REFINE_ANCHOR = ',e(r.value,r)),t);return n}';
const unique = (text, anchor, what) => {
  const first = text.indexOf(anchor);
  if (first === -1 || text.indexOf(anchor, first + 1) !== -1) {
    throw new Error(`the ${what} anchor is not unique in the pinned bundle`);
  }
};
async function main() {
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'settings-schema-'));
  let captured;
  try {
    fs.cpSync(pkg, scratch, { recursive: true });
    const bundle = path.join(scratch, 'sdk.mjs');
    const text = fs.readFileSync(bundle, 'utf8');
    unique(text, HOOK_ANCHOR, 'settings schema holder constructor');
    unique(text, REFINE_ANCHOR, 'superRefine');
    const hook = `${HOOK_ANCHOR};if(this.keys.includes("permissions")&&!globalThis.__settingsSchema){globalThis.__settingsSchema={fields:this.fields,active:[...this.activeWhenAbsent]}}`;
    const refine = ',e(r.value,r)),t);n._zod.refinement=e;return n}';
    fs.writeFileSync(bundle, text.replace(HOOK_ANCHOR, hook).replace(REFINE_ANCHOR, refine));
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
    const rendered = render(document);
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

// Pretty prints the document but keeps each recorded probe list and battery on one line.
const INLINE_KEYS = new Set(['probes', 'string', 'object', 'value', 'rule']);
function render(document) {
  const inline = [];
  const text = JSON.stringify(
    document,
    (key, value) => {
      if (INLINE_KEYS.has(key) && Array.isArray(value)) {
        inline.push(JSON.stringify(value));
        return `@@${inline.length - 1}@@`;
      }
      return value;
    },
    2,
  );
  return `${text.replace(/"@@(\d+)@@"/g, (_match, index) => inline[Number(index)])}\n`;
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
  const checkOf = (check, kind) => {
    const def = check._zod.def;
    const batteryName = kind === 'string' ? 'string' : 'object';
    const battery = batteryName === 'string' ? STRING_BATTERY : OBJECT_BATTERY;
    const result = { check: def.check };
    for (const [key, value] of Object.entries(def)) {
      if (key === 'check' || key === 'error' || key === 'abort' || key === 'when') continue;
      if (typeof value === 'function') {
        result[key] = { source: value.toString(), battery: batteryName, probes: probe(value, battery) };
      } else if (value instanceof RegExp) {
        result[key] = { regex: value.source, flags: value.flags };
      } else if (value instanceof Set) {
        result[key] = [...value];
      } else {
        result[key] = value;
      }
    }
    if (def.check === 'custom' && def.fn === undefined) {
      const shown = check._zod.refinement ?? check._zod.check;
      const source = shown.toString();
      const payloadBattery = source.includes('=Kk(') ? 'rule' : batteryName;
      result.payloadCheck = { source, battery: payloadBattery, probes: probePayload(check._zod.check, payloadBattery === 'rule' ? RULE_BATTERY : battery) };
    }
    return result;
  };
  const node = (schema) => {
    const def = definitionOf(schema);
    const checks = (def.checks ?? []).map((check) => checkOf(check, def.type));
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
        return { type: 'transform', source: def.transform.toString(), battery: 'value', probes: probe(def.transform) };
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
  return { keys, active, batteries: { value: encodeBattery(VALUE_BATTERY), string: encodeBattery(STRING_BATTERY), object: encodeBattery(OBJECT_BATTERY), rule: encodeBattery(RULE_BATTERY) }, fields: shape, definitions };
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
  ...[0, 5, 6, 300000, 300001, 65536, 2147483647, 2147483648, -1, 1.5].map((n) => n),
  '/abs/x',
  'rel',
  '',
  { mode: 'deny' },
  { mode: 'deny', extract: 1, onExtractNoMatch: 'x', decode: 'x', maskClaims: [1], maskDuplicates: 'y', injectHosts: [1] },
  { mode: 'deny', extract: 'a', onExtractNoMatch: 'warn', decode: 'jwt', maskClaims: ['a'], maskDuplicates: true, injectHosts: ['h'] },
  { mode: 'deny', onExtractNoMatch: 'deny' },
  { mode: 'deny', onExtractNoMatch: 'error', maskClaims: [], injectHosts: 'h' },
  { mode: 'allow', extract: 1 },
  { mode: 1 },
  { extract: 1 },
  { constructor: 1, a: 1 },
  { toString: 1, valueOf: 2, hasOwnProperty: 3, x: 4 },
  { __proto__x: 1, isPrototypeOf: 1, propertyIsEnumerable: 1, toLocaleString: 1 },
  { timeout: 1, request_timeout_ms: 400000, x: 1 },
  { request_timeout_ms: 400000 },
  { request_timeout_ms: 5 },
  { request_timeout_ms: 5, timeout: 9 },
  { request_timeout_ms: 'x' },
  { request_timeout_ms: null },
  { timeout: null, request_timeout_ms: 7 },
  { path: null, script: null },
  { path: null, script: null, defaultSettings: null, extra: 1 },
  { path: '/a', script: null },
  { path: null, script: 's', defaultSettings: null },
  { script: null },
  { defaultSettings: null },
  ['anthropic', 'bedrock', 'bogus', 3, null],
  ['customEndpoint', 'vertex', 'foundry', 'anthropicAws', 'mantle', 'gateway'],
  ['Anthropic'],
  ['skills', 'agents', 'hooks', 'mcp', 'other', 1],
  ['a', {}, [], 1, null, { b: 1 }],
  { engines: { vscode: '1' } },
];


// Inputs for the checks and payload checks that run on objects, records and arrays.
const OBJECT_BATTERY = [
  null,
  1,
  'x',
  true,
  [],
  [1],
  {},
  { a: 1 },
  { path: '/x' },
  { path: '/x', script: 's' },
  { path: '/x', script: 's', interpreter: 'sh', defaultSettings: {} },
  { script: 's' },
  { script: 's', interpreter: 'sh' },
  { interpreter: 'sh' },
  { script: 's', interpreter: 'sh', path: undefined },
  { defaultSettings: {} },
  { path: '/x', interpreter: 'sh' },
  { path: '/x', defaultSettings: {} },
  { path: '/x', script: undefined, interpreter: undefined, defaultSettings: undefined },
  { policyHelper: {} },
  { policyHelpers: {} },
  { policyHelper: null },
  { policyHelpers: null },
  { policyHelper: undefined },
  { path: 'x' },
  { script: 'x' },
  { interpreter: 'x' },
  { outputBehavior: 'x' },
  { onFailure: 'x' },
  { retries: 1 },
  { timeoutMs: 1 },
  { refreshIntervalMs: 1 },
  { defaultSettings: 1 },
  { path: null },
  { managedSettings: {} },
  { appendSystemPrompt: 'x' },
  { managedSettings: null },
  { macos: {} },
  { linux: {} },
  { windows: {} },
  { wsl: {} },
  { default: {} },
  { default: null },
  { macos: null, other: 1 },
  { serverName: 'a' },
  { serverCommand: 'a' },
  { serverUrl: 'a' },
  { serverName: 'a', serverCommand: 'b' },
  { serverName: 'a', serverCommand: 'b', serverUrl: 'c' },
  { serverUrl: 'c', serverName: undefined },
  { command: 'x' },
  { args: [] },
  { env: {} },
  { headersHelper: 'x' },
  { type: 'http', url: 'https://example.com' },
  { type: 'sse', url: 'https://example.com/x' },
  { type: 'stdio', url: 'https://example.com' },
  { type: 'streamable-http', url: 'https://example.com' },
  { type: 'http' },
  { type: 'http', url: 'http://example.com' },
  { type: 'http', url: 'https://' },
  { type: 'http', url: 'https://localhost' },
  { type: 'http', url: 5 },
  { type: 'http', url: 'https://example.com', headers: { a: 'b' } },
  { type: 'http', url: 'https://example.com', headers: { 'a\u0007': 'b' } },
  { type: 'http', url: 'https://example.com', headers: { a: 'b\u001b[31m' } },
  { type: 'http', url: 'https://example.com', headers: { a: '${TOKEN}' } },
  { type: 'http', url: 'https://example.com', headers: { a: '${TOKEN:-x}' } },
  { type: 'http', url: 'https://example.com', headers: { a: '${1}' } },
  { type: 'http', url: 'https://example.com', headers: { '${K}': 'v' } },
  { type: 'http', url: 'https://example.com', headers: { a: '\u200b' } },
  { type: 'http', url: 'https://example.com', headers: { a: '\u2028' } },
  { type: 'http', url: 'https://example.com', headers: { a: { b: { c: { d: { e: { f: '\u0007' } } } } } } },
  { type: 'http', url: 'https://example.com', headers: { a: { b: { c: { d: '${X}' } } } } },
  { type: 'http', url: 'https://example.com', oauth: { clientId: '${X}' } },
  { type: 'http', url: 'https://example.com', command: 'x', args: ['a'], env: { A: 'b' }, headersHelper: 'h' },
  { type: 'http', url: 'https://example.com', env: {} },
  { type: 'bogus', url: 'https://example.com', args: [] },
  { url: 'https://example.com' },
  { type: 'sse', url: '${URL}' },
  { type: 'sse', url: 'https://example.com', headers: [{ a: '${X}' }, '\u0007'] },
  { type: 'sse', url: 'https://example.com', headers: ['${X}'] },
  { a: { source: { source: 'settings', name: 'a' } } },
  { a: { source: { source: 'settings', name: 'b' } } },
  { a: { source: { source: 'github', repo: 'o/r' } } },
  { a: { source: { source: 'settings', name: 'a' } }, b: { source: { source: 'settings', name: 'c' } } },
  { a: { source: { source: 'settings', name: 'a' } }, b: { source: { source: 'url', url: 'x' } } },
  { a: { source: { source: 'settings' } } },
  { a: { source: { source: 'settings', name: 3 } } },
  { 'a b': { source: { source: 'settings', name: 'a b' } } },
  { constructor: { source: { source: 'settings', name: 'constructor' } } },
  { __proto__: { source: { source: 'settings', name: 'x' } } },
  { a: { source: { name: 'a' } } },
  ['a'],
  ['/a', '/b'],
  ['a', null, 1],
  ['a', 'a'],
  ['*'],
  ['a*b'],
  ['Bash(*)'],
  ['mcp__server'],
  ['mcp__server__tool'],
  ['mcp__a__b__c'],
  ['mcp__'],
  ['mcp__x__'],
  ['constructor'],
  ['toString'],
  ['claude.ai Gmail'],
  ['a b'],
  ['skills', 'agents'],
  ['skills', 'agents', 'hooks', 'mcp', 'other'],
  ['regex'],
  { source: 'x' },
  { source: { source: 'unsupported' } },
  { source: { source: 'github' } },
  { source: 5 },
  { source: null },
  {},
  { mode: 'mask', path: '/a/' },
  { mode: 'mask', path: '/a' },
  { mode: 'deny', path: '/a/' },
  { mode: 'mask', path: '/a', extract: '(' },
  { mode: 'mask', path: '/a', extract: 'a' },
  { mode: 'mask', path: '/a', extract: 'token:\\s*(\\S+)' },
  { mode: 'mask', path: '/a/', extract: '[', maskClaims: [] },
  { mode: 'mask', path: '/a', maskClaims: ['sub'] },
  { mode: 'mask', path: '/a', maskClaims: ['sub'], decode: 'jwt' },
  { mode: 'mask', path: '/a', maskClaims: [''], decode: 'jwt' },
  { mode: 'mask', path: '/a', maskClaims: [], decode: 'jwt' },
  { mode: 'mask', path: '/a', maskClaims: [[]] },
  { mode: 'mask', path: '/a', maskClaims: { length: 0 } },
  { mode: 'mask', path: '/a', maskClaims: '' },
  { mode: 'mask', path: '/a', extract: 5 },
  { mode: 'mask', path: '/a', extract: ['(a)'] },
  { mode: 'mask', path: '/a', extract: '(?<n>a)|(?<n>b)' },
  { mode: 'mask', path: '/a', extract: '(?<n>a)(?<n>b)' },
  { mode: 'mask', path: '/a', extract: '(?i:(a))' },
  { mode: 'mask', path: '/a', extract: '(?:a)\\1' },
  { mode: 'mask', name: 'A', extract: '(a)', decode: 'jwt' },
  { mode: 'mask', name: 'A', decode: 'jwt', onExtractNoMatch: 'deny' },
  { mode: 'mask', name: 'A', decode: 'jwt', onExtractNoMatch: 'error' },
  { mode: 'mask', name: 'A', decode: 'jwt', onExtractNoMatch: 'warn' },
  { mode: 'mask', name: 'A', decode: null, extract: '(a)', onExtractNoMatch: 'deny' },
  { mode: 'mask', name: 'A', extract: '(a)', onExtractNoMatch: 'deny' },
  { mode: 'mask', extract: '\\k<a>(?<a>x)', maskClaims: ['a', ''], decode: 'jwt', onExtractNoMatch: 'error' },
  { accessKeyIdVar: 'A', secretAccessKeyVar: 'A' },
  { accessKeyIdVar: 'A', secretAccessKeyVar: 'B', sessionTokenVar: 'A' },
  { accessKeyIdVar: 'A', secretAccessKeyVar: 'A', sessionTokenVar: 'A' },
  { accessKeyIdVar: null, secretAccessKeyVar: null },
  { accessKeyIdVar: 1, secretAccessKeyVar: '1' },
  { accessKeyIdVar: {}, secretAccessKeyVar: {} },
  { awsPairs: [] },
  { awsPairs: null },
  { awsPairs: [{ accessKeyIdVar: 'A', secretAccessKeyVar: 'B' }, { accessKeyIdVar: 'A', secretAccessKeyVar: 'C' }] },
  { awsPairs: [{ accessKeyIdVar: 'A', secretAccessKeyVar: 'A' }] },
  { awsPairs: [{ accessKeyIdVar: 'A', secretAccessKeyVar: 'A' }, { sessionTokenVar: 'A' }] },
  { awsPairs: [{ accessKeyIdVar: 'A', secretAccessKeyVar: 'B', sessionTokenVar: 'C' }, { accessKeyIdVar: 'C', secretAccessKeyVar: 'B', sessionTokenVar: 'D' }] },
  { awsPairs: [1, 'x', { accessKeyIdVar: null }, { secretAccessKeyVar: null }] },
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
  // Paths in every shape the policy helper path checks and the platform validators distinguish.
  '/', '//', '/a', '/a/', '/a//b', '/a/./b', '/a/../b', '/a/b/', '/a b/c', '/net/host', '/net', '/net/host/x',
  '/Network/Servers', '/network/servers/x', '/NETWORK/SERVERS', '/.vol', '/.vol/x', '/.VOL', '/.file/x', '/.nofollow',
  '/.resolve/id', '/x/.vol', '/proc', '/proc/1/exe', '/dev/fd/3', '/dev/stdin', '/dev/null', '/usr/bin/tool',
  '/opt/helper.ps1', '/opt/helper.exe', '/a\u0007b', '/a\u200bb', '/a\u2028b', '/a\u00adb', '/a\u00a0b', '/caf\u00e9',
  '/tmp/\ud83d\ude00',
  'C:\\', 'C:\\x', 'C:\\x\\', 'C:\\x\\y.exe', 'C:\\x\\y.ps1', 'C:\\x\\y.EXE', 'C:\\x\\y.PS1', 'C:/x/y.exe', 'c:\\x\\..\\y.exe',
  'C:\\x\\.\\y.exe', 'C:\\x\\\\y.exe', 'C:\\x\\y.exe.', 'C:\\x\\y.exe ', 'C:\\x\\y:z.exe', 'C:\\x:y\\z.exe', 'C:x\\y.exe', 'C:', 'C:y',
  '\\x\\y.exe', '\\\\?\\C:\\x.exe', '\\\\.\\C:\\x.exe', '//?/C:/x.exe', '\\\\server\\share\\x.exe', '\\\\server\\share', '\\\\\\x.exe',
  '//server/share/x.exe', 'C:\\x\\[y].ps1', 'C:\\x\\y*.ps1', 'C:\\x\\y?.ps1', 'C:\\x\\y`.ps1', 'C:\\x\\y[.exe', 'C:\\x\\a b\\y.exe',
  '\\\\', '\\\\s', '\\', 'D:\\', 'Z:\\a\\b.exe',
  'x'.repeat(1024), 'x'.repeat(1025), '/'.concat('x'.repeat(1024)),
  // Header helper and control characters.
  'a    b', 'a   b', '\u007f', '\u0080', '\u009f', '\u00a0', '\u200e', '\u202a', '\u2066', '\u2069', '\ufeff', '\u00ad', '\u200b', '\u061c', '\u180e',
  // Marketplace and plugin names.
  'anthropic-marketplace', 'claude-code-plugins', 'claude-plugins-official', 'healthcare', 'claude-community', 'anthropic-plugin-directory',
  'claude-plugin-directory', 'Claude-Plugins-Official', 'official-claude-plugins', 'anthropic official', 'claude_official', 'claude-marketplace',
  'anthropic-plugins-x', 'claudeofficial', 'official anthropic', 'anthropic-plugins', 'anthropic.official', 'my-marketplace', 'inline', 'builtin',
  'skills-dir', 'synced', 'claude-plugin-test', 'INLINE', 'Builtin', 'toString', 'hasOwnProperty', 'valueOf', 'isPrototypeOf', 'propertyIsEnumerable',
  '__defineGetter__', '__lookupSetter__', 'toLocaleString',
  '@scope/pkg', '@Scope/pkg', '@scope/', '@/pkg', 'pkg', 'Pkg', 'pkg..x', 'pkg//x', '@scope/pkg/x', '-pkg', '.pkg', 'pkg-._x', '@a-b/c.d',
  // Server names, tool-name fragments and environment references.
  'server', 'my server', 'my.server', 'claude.ai Gmail', 'claude.ai  x ', 'a__b', 'mcp__x', '_', '__', '-', 'a-b_c', 'x y', '${HOME}', '${A:-b}', '${1A}', 'a${B}',
  // URLs whose host exercises the private address and metadata checks.
  'https://', 'https:///x', 'https://a', 'http://a', 'https://exa mple.com', 'https://example.com', 'https://EXAMPLE.com./x', 'https://user@example.com',
  'https://localhost', 'https://LOCALHOST', 'https://localhost.', 'https://a.localhost', 'https://metadata.google.internal', 'https://metadata.goog',
  'https://metadata', 'https://instance-data', 'https://instance-data.ec2.internal', 'https://ip6-localhost', 'https://localhost.localdomain',
  'https://localhost4', 'https://localhost6.localdomain6', 'https://instance-data.us-east-1.compute.internal', 'https://instance-data.x.y',
  'https://127.0.0.1', 'https://127.1', 'https://0.0.0.0', 'https://0', 'https://0x7f.1', 'https://2130706433', 'https://10.0.0.1', 'https://172.16.0.1',
  'https://192.168.1.1', 'https://169.254.169.254', 'https://169.254.0.1', 'https://100.100.100.200', 'https://168.63.129.16', 'https://192.0.0.192',
  'https://8.8.8.8', 'https://1.2.3.4', 'https://255.255.255.255', 'https://0.1.2.3', 'https://127.255.255.255', 'https://[::1]', 'https://[::]',
  'https://[0:0:0:0:0:0:0:1]', 'https://[::2]', 'https://[fe80::1]', 'https://[fe80::]', 'https://[febf::1]', 'https://[fec0::1]', 'https://[fd00:ec2::254]',
  'https://[fd00:ec2::253]', 'https://[::ffff:127.0.0.1]', 'https://[::ffff:7f00:1]', 'https://[::ffff:8.8.8.8]', 'https://[::ffff:a9fe:a9fe]',
  'https://[::127.0.0.1]', 'https://[::8.8.8.8]', 'https://[64:ff9b::7f00:1]', 'https://[64:ff9b::8.8.8.8]', 'https://[64:ff9b:1::7f00:1]',
  'https://[64:ff9b:1::1]', 'https://[2002:7f00:1::]', 'https://[2002:808:808::]', 'https://[2001:db8::1]', 'https://[2606:4700::1111]',
  'https://[::ffff:100.100.100.200]', 'https://[fe80::1%25eth0]', 'https://[2001:0:0:0:0:5efe:7f00:1]', 'https://[2001:0:0:0:0:5efe:808:808]',
  'https://[2001:db8::5efe:7f00:1]', 'https://[2001:db8:0:0:0:200:5efe:7f00:1]', 'https://[1:2:3:4:5:6:7:8]', 'https://[1::2:3:4:5:6:7]',
  'https://[::1.2.3.4]', 'https://[::ffff:0:0]', 'https://[::ffff:0.0.0.0]', 'https://[0:0:0:0:0:0:0:0]',
  'http://127.0.0.1:8080', 'http://[::1]:80', 'ws://example.com', 'HTTPS://example.com', 'https:example.com', 'https:/example.com', 'https:\\\\example.com',
  'https://\\x', 'http:///x', 'http://\\x', 'file://x', 'javascript:alert(1)', 'mailto:a@b', 'data:text/plain,x', 'https://example.com:99999',
  'https://\u00e9xample.com', 'https://xn--n3h.example', 'https://a.b.c.d.example.com/path?q=1#f',
  // Scheme-prefixed sources.
  'git://x/y', 'git+http://x', 'git+https://x', 'ssh://x', 'gitlab:o/r', 'bitbucket:o/r', 'FILE:///x', 'GitHub:o/r', 'git+ftp://x', './x', '../x', 'x/../y',
  'a.mcpb', 'a.dxt', './a.mcpb', './a.json', './a.md', '.mcpb', 'https://x/a.mcpb',
];

// Permission rule strings: every rejection and acceptance branch of the rule validator and its per-tool
// content validators. The rule validator runs in two modes (allow and the others), so the entries cover
// the wildcard names only allow rules refuse as well.
const RULE_BATTERY = [
  // Empty and whitespace-only rules, including the characters `trim` strips and the ones it keeps.
  '', ' ', '\t', '\n', '\u00a0', '\u1680', '\u2003', '\u2028', '\u2029', '\u202f', '\u205f', '\u3000', '\ufeff', '\u0085', '\u180e', '\u200b',
  ' Bash', 'Bash ', 'Bash\u00a0', '\u3000Bash',
  // Bare tool names: uppercase start, lowercase start, underscores, leading non-letters, case mappings.
  'Bash', 'Read', 'Edit', 'Write', 'Glob', 'Grep', 'WebFetch', 'WebSearch', 'Task', 'KillShell', 'Brief', 'bash', 'read', 'x', 'X', 'xY', 'my_tool', 'My_tool', '_x', '_', 'a_B',
  '1abc', '-x', '.x', '$x', 'a b', 'A b', 'mcp', 'Mcp__x', 'task', 'killshell',
  '\u00df', '\u00dfx', '\u0149', '\u01f0', '\u01c6', '\u01c5', '\u01c4', '\u0390', '\ufb03', '\u1fb3', '\u0130', '\u0131', '\u00e9', '\u00c9', '\u03c3', '\u03a3', '\u0561', '\u2170', '\u24d0', '\uff41', '\uff21',
  // Characters outside the basic plane: the first UTF-16 unit is a surrogate, so the case test never fires.
  '\ud83d\ude00', '\ud801\udc28', '\ud801\udc28x', '\ud801\udc00', '\ud801\udc28(x)', '\ud801\udc00(x)', '\ud83d\ude00(x)', 'A\ud83d\ude00', 'A\ud83d\ude00(x)', 'a\ud83d\ude00',
  // Parentheses: malformed, empty, escaped and unbalanced forms.
  '(', ')', '()', '(x)', '(*)', ' (x)', 'Bash(', 'Bash)', 'Bash()', 'Bash( )', 'Bash(ls', 'Bash(ls))', 'Bash((ls)', 'Bash(ls)x', 'Bash(ls) ', ' Bash(ls)', 'Bash (ls)', 'Bash(a(b)c)', 'Bash(a)(b)',
  'Ba(sh)(x)', 'Ba(sh(x)', 'Ba)sh(x)', 'Ba\\(sh(x)', 'Ba\\(sh\\)(x)', 'Bash(ls\\)', 'Bash\\(', 'Bash\\)', 'Bash\\(x\\)', 'Bash\\(x)', 'Bash(x\\)', 'Bash(\\)', 'Bash(\\\\)', 'Bash(\\\\\\)',
  'Bash(*)', 'Bash(**)', 'Bash(ls)', 'Bash(npm run build)',
  '(\\(x)', '\\(x)', 'x\\(x)', 'a\\\\(x)', 'A\\\\(x)', 'A\\\\\\(x)',
  // Escapes beside parentheses in the rule content, balanced and not.
  'Bash(\\(x)', 'Bash(\\)x)', 'Bash(\\(x\\))', 'Bash(\\(x\\)', 'Bash(a\\(b\\)c)', 'Bash(a\\(b(c)', 'Bash(a\\)b(c)', 'Bash(a\\(b)c)', 'Bash(\\\\(x)', 'Bash(\\\\\\(x)', 'Bash(\\\\\\\\(x)', 'Bash(x\\\\(y)',
  'Bash(\\(', 'Bash(\\(\\()', 'Bash(\\(()', 'Bash(()\\()', 'Bash(\\\\()', 'Bash(\\\\\\()', 'Bash(\\a)', 'Bash(a\\b)', 'Bash(a\\\\b)', 'Bash(\\\\\\\\)',
  'Read(C:\\Projects\\(drafts)\\**)', 'Read(C:\\Projects\\\\(drafts)\\**)', 'Read(C:\\Projects\\[(]drafts)\\**)', 'Read(C:/Projects/(drafts)/**)', 'Read(C:\\tools\\\\(x86)\\run.exe)',
  'Edit(a\\(b)', 'Edit(a\\)b)', 'Edit(\\(\\))', 'WebFetch(domain:a\\(b)', 'WebSearch(a\\(b)', 'WebSearch(a\\(b\\))',
  // Model Context Protocol rules: server and tool positions, wildcards, parentheses.
  'mcp__', 'mcp__srv', 'mcp__srv__', 'mcp__srv__tool', 'mcp__srv__a__b', 'mcp____x', 'mcp____', 'mcp__a__', 'mcp__a____b', 'mcp__srv__a_b', 'mcp__Srv__Tool', 'Mcp__srv', 'MCP__srv',
  'mcp__*', 'mcp__*__x', 'mcp__*__*', 'mcp__srv*', 'mcp__s*v', 'mcp__s*__x', 'mcp__srv__*', 'mcp__srv__get_*', 'mcp__srv__*__a', 'mcp__srv__a__*', 'mcp__srv__*a*', 'mcp__srv__**',
  'mcp__srv(x)', 'mcp__srv__tool(x)', 'mcp__srv(*)', 'mcp__srv()', 'mcp__srv__*(x)', 'mcp__*(x)', 'mcp__srv(a)b', 'mcp__srv(', 'mcp__srv)', 'mcp__srv__a__b(x)', 'mcp__\\(x', 'mcp__srv\\(x\\)',
  'mcp__\ud83d\ude00', 'mcp__\ud83d\ude00__*', 'mcp__srv__\ud83d\ude00', 'mcp__ __x',
  // Wildcard tool names outside the Model Context Protocol position.
  '*', '**', 'Bash*', '*Bash', 'B*sh', 'b*', 'Task*', 'Re*', 'Re*(x)', 'a*', 'a_*', '*_x', 'Bash\\*', '\\*', 'Bash(\\*)',
  // Per-tool content: file pattern tools reject the :* prefix syntax.
  'Read(*.ts)', 'Read(src/**)', 'Read(a:*)', 'Read(:*)', 'Read(a\\:*)', 'Read(a:\\*)', 'Read(a:* )', 'Read(a: *)', 'Read(*)', 'Read(\\(a:*)', 'Read(\ud83d\ude00:*)',
  'Edit(a:*)', 'Write(a:*)', 'Glob(a:*)', 'Cd(a:*)', 'NotebookRead(x:*)', 'NotebookEdit(a:*)', 'Grep(a:*)', 'MultiEdit(a:*)', 'LS(a:*)', 'Cd(a)', 'Cd(a/b)',
  'Write(src)', 'Write(src/**)', 'NotebookEdit(a)', 'MultiEdit(a)', 'Glob(a)', 'Glob(*.ts)', 'Grep(a)', 'LS(a)', 'read(a:*)', 'read(a)',
  // WebSearch and WebFetch content validators.
  'WebSearch(a*)', 'WebSearch(a?)', 'WebSearch(claude ai)', 'WebSearch(*)', 'WebSearch()', 'WebSearch(?)', 'WebSearch(a\\*)', 'WebSearch( )', 'WebSearch(a:*)',
  'WebFetch(https://a)', 'WebFetch(http)', 'WebFetch(httpx)', 'WebFetch(domain:a.com)', 'WebFetch(a.com)', 'WebFetch(domain:*.a.com)', 'WebFetch(Domain:a)', 'WebFetch(*)', 'WebFetch()',
  'WebFetch(a://)', 'WebFetch(domain:a://b)', 'WebFetch(domain:)', 'WebFetch(domain)', 'WebFetch(domain: a)', 'WebFetch( domain:a)', 'WebFetch(ftp://a)', 'WebFetch(domain:a:*)', 'webfetch(domain:a)', 'webfetch(a)',
  // Bash prefix rules: the bare :* and the warning shaped forms, all valid except the empty prefix.
  'Bash(:*)', 'Bash( :*)', 'Bash(\t:*)', 'Bash(\n:*)', 'Bash(::*)', 'Bash(npm:*)', 'Bash(npm :*)', 'Bash(npm:* )', 'Bash(a:*b)', 'Bash(a*:*)', 'Bash(*:*)', 'Bash(a\\*:*)', 'Bash(a\n:*)', 'Bash(a:*:*)',
  'Bash(FOO=* ls)', 'Bash(FOO=a* ls)', 'Bash(FOO=bar ls)', 'Bash(FOO=* ls:*)', 'Bash(A+=* x)', 'Bash(* ls)', 'Bash(git * status)', 'Bash(git -c * status)', 'Bash(git * -c status)', 'Bash(ls | * x)',
  'Bash(a b:*)', 'Bash(a b *:*)', 'Bash(npm * run:*)', 'Bash(\ud83d\ude00:*)', 'Bash(\ud83d\ude00 :*)', 'Bash(\\(:*)', 'Bash(a\\(b:*)', 'Bash(a(b):*)', 'bash(ls)', 'bash(:*)', 'bash(npm:*)',
  // Other tools, aliases and content forms.
  'Foo(x)', 'foo(x)', 'Foo_bar(x)', 'foo_bar(x)', 'Task(x)', 'Task(a:*)', 'KillShell(x)', 'Brief(x)', 'Agent(Explore)', 'Skill(foo)', 'Edit', 'Edit(*)', 'Edit(src)',
  'Foo(a:*)', 'Foo(*)', 'Foo()', 'Foo(a\\(b)', 'Foo(\\\\)', 'Foo(a)b', 'Foo(\u00a0)', 'Foo(x)\u00a0', 'Foo(\ud83d\ude00)', 'Foo\ud83d\ude00(x)',
];

// Long strings are recorded as a repeat marker so the document stays small and the consumer rebuilds them.
function encodeBattery(battery) {
  return battery.map((input) => (typeof input === 'string' && input.length > 100 && input === input[0].repeat(input.length) ? { repeat: [input[0], input.length] } : input));
}

// The recorded issues a check that reports through `issues` raises for each battery input.
function probePayload(check, battery) {
  return battery.map((input) => {
    try {
      const payload = { value: structuredClone(input), issues: [] };
      check(payload);
      return JSON.stringify(payload.issues.map((issue) => ({ message: issue.message, path: issue.path ?? [] })));
    } catch (error) {
      return `error: ${error.message}`;
    }
  });
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
