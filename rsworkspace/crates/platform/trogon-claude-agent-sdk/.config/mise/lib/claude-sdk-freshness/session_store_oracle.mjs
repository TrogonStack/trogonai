#!/usr/bin/env node
// Runs the pinned TypeScript bundle's own helpers over fixed inputs and writes the oracle fixtures the Rust
// session store port is checked against.
//
// usage: node session_store_oracle.mjs <package-dir> <out-dir>
//
// The helpers are module private, so a copy of `sdk.mjs` with an extra export line is imported from a temporary
// directory. Inputs avoid characters assigned after Unicode 15 so the fixtures do not depend on the runtime's ICU.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const [pkg, out] = process.argv.slice(2);
if (!pkg || !out) {
  process.stderr.write('usage: session_store_oracle.mjs PACKAGE_DIR OUT_DIR\n');
  process.exit(2);
}

// Bundle names: `Ute` sanitizes Unicode, `Wn(directory, env)` names a project, `YC` and `kt` initialize the module
// state those two read.
const temporary = fs.mkdtempSync(path.join(os.tmpdir(), 'claude-sdk-oracle-'));
const copy = path.join(temporary, 'sdk.mjs');
fs.writeFileSync(
  copy,
  `${fs.readFileSync(path.join(pkg, 'sdk.mjs'), 'utf8')}\nexport { Ute as __Ute, Wn as __Wn, YC as __YC, kt as __kt };\n`,
);
const bundle = await import(pathToFileURL(copy).href);
fs.rmSync(temporary, { recursive: true, force: true });
bundle.__YC();
bundle.__kt();

const write = (name, cases) => {
  fs.mkdirSync(out, { recursive: true });
  fs.writeFileSync(path.join(out, name), `${JSON.stringify(cases, null, 2)}\n`);
};

const zalgo = 'Z' + '̧̖́̀̈'.repeat(8) + 'a';
const unicode = [
  'plain ascii',
  '',
  'café',
  'café',
  'he​llo‌‍‎‏',
  '‪embedded‮',
  '⁦isolate⁩',
  '﻿bom',
  'private',
  'plane15 \u{f0000}',
  'soft­hyphen',
  'tag\u{e0001}\u{e0041}',
  'unassigned͸͹',
  'noncharacter﷐￿',
  'ﬁligature ① Ａｂ',
  '각 jamo',
  'emoji \u{1f468}‍\u{1f469}‍\u{1f467}',
  'flags \u{1f1fa}\u{1f1f8}',
  'variation ❤️',
  zalgo,
  'x'.repeat(126) + '́̂̃' + 'y'.repeat(10),
  'é'.repeat(200),
  ('a' + '̣̇').repeat(70),
  'Ω ohm Å angstrom',
  'ΐ ẛ̣',
  'line para end',
  'control\u0001\u007f\u0085',
];
write(
  'sanitize_unicode.json',
  unicode.map((input) => {
    try {
      return { input, output: bundle.__Ute(input) };
    } catch (error) {
      return { input, error: error.message };
    }
  }),
);

const absolute = [
  '/',
  '/a/b',
  '/a/b/',
  '//a//b//',
  '/a/./b/../c',
  '/../..',
  '/a/b/..',
  '/a/b/.',
  '/a/..../b',
  '/a/.b/c.',
  '/a/b/../../../c',
  '/a/b c/d',
  '/café/x',
];
write(
  'lexical_resolve.json',
  absolute.map((input) => ({ input, output: path.posix.resolve(input) })),
);

const long = `/oracle/${'segment-'.repeat(30)}end`;
const directories = ['/oracle/project', '/oracle/a b.c', '/oracle/\u{1f600}', long];
const environments = [
  {},
  { CLAUDE_CONFIG_DIR: '' },
  { CLAUDE_CONFIG_DIR: '/cfg' },
  { CLAUDE_CONFIG_DIR: '/cfg', CLAUDE_CODE_PROJECT_DIR_NAME: 'my_project-1' },
  { CLAUDE_CONFIG_DIR: '', CLAUDE_CODE_PROJECT_DIR_NAME: 'my_project-1' },
  { CLAUDE_CODE_PROJECT_DIR_NAME: 'my_project-1' },
  { CLAUDE_CONFIG_DIR: '/cfg', CLAUDE_CODE_PROJECT_DIR_NAME: '' },
  { CLAUDE_CONFIG_DIR: '/cfg', CLAUDE_CODE_PROJECT_DIR_NAME: 'a'.repeat(64) },
  { CLAUDE_CONFIG_DIR: '/cfg', CLAUDE_CODE_PROJECT_DIR_NAME: 'a'.repeat(65) },
  { CLAUDE_CONFIG_DIR: '/cfg', CLAUDE_CODE_PROJECT_DIR_NAME: 'has space' },
  { CLAUDE_CONFIG_DIR: '/cfg', CLAUDE_CODE_PROJECT_DIR_NAME: 'dot.name' },
  { CLAUDE_CONFIG_DIR: '/cfg', CLAUDE_CODE_PROJECT_DIR_NAME: 'CON' },
  { CLAUDE_CONFIG_DIR: '/cfg', CLAUDE_CODE_PROJECT_DIR_NAME: 'nul' },
  { CLAUDE_CONFIG_DIR: '/cfg', CLAUDE_CODE_PROJECT_DIR_NAME: 'Com7' },
  { CLAUDE_CONFIG_DIR: '/cfg', CLAUDE_CODE_PROJECT_DIR_NAME: 'lpt0' },
  { CLAUDE_CONFIG_DIR: '/cfg', CLAUDE_CODE_PROJECT_DIR_NAME: 'com10' },
  { CLAUDE_CONFIG_DIR: '/cfg', CLAUDE_CODE_PROJECT_DIR_NAME: 'console' },
  { CLAUDE_CONFIG_DIR: '/cfg', CLAUDE_CODE_PROJECT_DIR_NAME: 'café' },
];
write(
  'project_name.json',
  directories.flatMap((directory) =>
    environments.map((env) => ({
      directory,
      config_dir: env.CLAUDE_CONFIG_DIR ?? null,
      name_override: env.CLAUDE_CODE_PROJECT_DIR_NAME ?? null,
      output: bundle.__Wn(directory, env),
    })),
  ),
);

const documents = [
  '{"b":1,"a":2}',
  '{"2":"two","1":"one","b":"b","0":"zero"}',
  '{"10":1,"9":2,"a":3,"01":4,"-1":5,"1.5":6}',
  '{"4294967294":1,"4294967295":2,"4294967296":3,"0":4}',
  '{"z":{"3":1,"y":2,"1":3},"list":[{"b":1,"0":2}]}',
  '{"a":1,"b":2,"a":3}',
  '{"1":1,"x":2,"1":3}',
  '{"":1," ":2,"00":3,"0":4}',
  '{"12345678901":1,"123":2}',
  '{"x":1,"4294967295":2,"4294967294":3}',
  '{"x":1,"4294967296":2,"9007199254740991":3,"5":4}',
  '{"x":1,"+1":2,"1e3":3," 1":4,"1 ":5,"7":6}',
];
write(
  'js_key_order.json',
  documents.map((input) => ({ input, output: JSON.stringify(JSON.parse(input)) })),
);
