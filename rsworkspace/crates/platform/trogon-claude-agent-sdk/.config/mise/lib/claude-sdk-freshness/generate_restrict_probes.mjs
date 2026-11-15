#!/usr/bin/env node
// Records what the pinned TypeScript SDK keeps of a user-writable managed settings file when the session takes its
// managed settings from its organization.
//
// The restriction (`H7e` upstream) is a private function of the minified bundle, so the generator copies the pinned
// package into a scratch directory, exports the function from the copy and runs it over a battery of settings
// documents. Each probe holds the document and the document the restriction replaces it with, or null when the
// restriction keeps the document as written. The Rust side replays every probe.
//
// Usage: generate_restrict_probes.mjs --package DIR --out FILE [--check]
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { DOCUMENTS, seededDocuments } from './settings_restrict_battery.mjs';

const args = process.argv.slice(2);
const option = (name) => {
  const index = args.indexOf(name);
  return index === -1 ? undefined : args[index + 1];
};
const pkg = option('--package');
const out = option('--out');
const check = args.includes('--check');
if (!pkg || !out) {
  process.stderr.write('usage: generate_restrict_probes.mjs --package DIR --out FILE [--check]\n');
  process.exit(2);
}

// Upstream names: the restriction, plus the two halves it is built from, so a renamed bundle fails loudly.
const RESTRICT = 'H7e';
const SANITIZE = 'm5';
const DIFFERENCES = '$7e';
const SEEDED_COUNT = 600;
const SEED = 20251113;

async function main() {
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'settings-restrict-'));
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'settings-restrict-home-'));
  const previousHome = process.env.HOME;
  process.env.HOME = home;
  try {
    fs.cpSync(pkg, scratch, { recursive: true });
    const bundle = path.join(scratch, 'sdk.mjs');
    const text = fs.readFileSync(bundle, 'utf8');
    for (const name of [RESTRICT, SANITIZE, DIFFERENCES]) {
      if (!text.includes(`function ${name}(`)) throw new Error(`the pinned bundle has no function ${name}`);
    }
    const capture = `\n;globalThis.__settingsRestrict={restrict:${RESTRICT},sanitize:${SANITIZE},differences:${DIFFERENCES}};\n`;
    fs.writeFileSync(bundle, text + capture);
    const sdk = await import(pathToFileURL(bundle).href);
    // Drive the public resolver once so every lazily initialized module the restriction reads is ready.
    fs.mkdirSync(path.join(home, '.claude'));
    fs.writeFileSync(path.join(home, '.claude', 'settings.json'), '{"model":"x"}');
    await sdk.resolveSettings({ cwd: home });
    const { restrict } = globalThis.__settingsRestrict;
    const documents = [...DOCUMENTS, ...seededDocuments(SEEDED_COUNT, SEED)];
    const probes = documents.map((document, index) => {
      const input = JSON.parse(JSON.stringify(document));
      const entry = { settings: input, errors: [] };
      const result = restrict(entry, input, `/probe/${index}/managed-settings.json`);
      return { input: document, output: result === entry ? null : result.settings };
    });
    const rendered = `{\n  "probes": [\n${probes.map((probe) => `    ${JSON.stringify(probe)}`).join(',\n')}\n  ]\n}\n`;
    if (check) {
      const existing = fs.existsSync(out) ? fs.readFileSync(out, 'utf8') : '';
      if (existing !== rendered) {
        process.stderr.write(`${out} differs from the pinned upstream restriction; run claude-sdk:schema\n`);
        process.exitCode = 1;
      }
    } else {
      fs.writeFileSync(out, rendered);
    }
  } finally {
    if (previousHome === undefined) delete process.env.HOME;
    else process.env.HOME = previousHome;
    fs.rmSync(home, { recursive: true, force: true });
    fs.rmSync(scratch, { recursive: true, force: true });
  }
}

await main();
