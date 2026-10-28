import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { parseArgs } from 'node:util';

import { buildCatalog } from './catalog.ts';
import { createOracle } from './oracle.ts';

const LEDGER_FILE = 'ledger.json';
const COMMIT_PATTERN = /^[0-9a-f]{40}$/;

const USAGE = `Usage: node src/cli.ts --bolt-path <bolt-js checkout> --sdk-path <node-slack-sdk checkout> --out <directory>
  [--bolt-commit <sha>] [--sdk-commit <sha>]

The commit defaults to the checkout HEAD read from its .git directory.`;

function readHeadCommit(checkout: string): string {
  const gitDirectory = path.join(checkout, '.git');
  const head = readFileSync(path.join(gitDirectory, 'HEAD'), 'utf8').trim();
  if (COMMIT_PATTERN.test(head)) {
    return head;
  }
  const reference = head.replace(/^ref: /, '');
  const loose = path.join(gitDirectory, reference);
  if (existsSync(loose)) {
    return readFileSync(loose, 'utf8').trim();
  }
  const packed = readFileSync(path.join(gitDirectory, 'packed-refs'), 'utf8')
    .split('\n')
    .find((line) => line.endsWith(` ${reference}`));
  if (packed === undefined) {
    throw new Error(`Cannot resolve ${reference} in ${checkout}`);
  }
  return packed.split(' ')[0] ?? '';
}

function requireCommit(value: string, label: string): string {
  if (!COMMIT_PATTERN.test(value)) {
    throw new Error(`${label} must be a full 40 character commit SHA, got ${value}`);
  }
  return value;
}

function clearFixtures(directory: string): void {
  if (!existsSync(directory)) {
    return;
  }
  for (const entry of readdirSync(directory)) {
    if (entry.endsWith('.json') && entry !== LEDGER_FILE) {
      rmSync(path.join(directory, entry));
    }
  }
}

async function main(): Promise<void> {
  const { values } = parseArgs({
    options: {
      'bolt-path': { type: 'string' },
      'sdk-path': { type: 'string' },
      out: { type: 'string' },
      'bolt-commit': { type: 'string' },
      'sdk-commit': { type: 'string' },
    },
  });
  const boltPath = values['bolt-path'];
  const sdkPath = values['sdk-path'];
  const out = values.out;
  if (boltPath === undefined || sdkPath === undefined || out === undefined) {
    console.error(USAGE);
    process.exitCode = 2;
    return;
  }

  const oracle = createOracle({
    boltPath: path.resolve(boltPath),
    sdkPath: path.resolve(sdkPath),
    boltCommit: requireCommit(values['bolt-commit'] ?? readHeadCommit(boltPath), 'bolt commit'),
    sdkCommit: requireCommit(values['sdk-commit'] ?? readHeadCommit(sdkPath), 'sdk commit'),
  });
  const fixtures = await buildCatalog({ oracle });

  const directory = path.resolve(out);
  mkdirSync(directory, { recursive: true });
  clearFixtures(directory);
  for (const fixture of fixtures) {
    writeFileSync(path.join(directory, `${fixture.id}.json`), `${JSON.stringify(fixture, null, 2)}\n`);
  }
  console.log(`Wrote ${fixtures.length} fixtures for Bolt ${oracle.version} (${oracle.commit}) to ${directory}`);
}

main().catch((error: unknown) => {
  console.error(error);
  process.exitCode = 1;
});
