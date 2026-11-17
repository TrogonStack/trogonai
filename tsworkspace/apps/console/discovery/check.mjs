import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync, realpathSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, relative } from 'node:path';

// The same canonical tree vite.config.ts hands Vitest as its root.
const root = dirname(realpathSync('vite.config.ts'));
const declared = 'src/discovery/declared[12].test.tsx';
const undeclared = 'src/discovery/declared1.test.tsx';

assert.ok(existsSync(declared), `${declared} must be a declared input`);
assert.ok(!existsSync(undeclared), `${undeclared} must not be a declared input`);
assert.ok(
  existsSync(join(root, undeclared)),
  `${undeclared} must sit in the canonical tree for exclusion to be checked`,
);

const vitestBin = join(dirname(createRequire(import.meta.url).resolve('vitest/package.json')), 'vitest.mjs');
const outDir = process.env.TEST_TMPDIR ?? tmpdir();

// Runs the Vitest CLI the way :test does and returns its JSON output.
function vitest(name, args) {
  const output = join(outDir, `${name}.json`);
  const run = spawnSync(process.execPath, [...process.execArgv, vitestBin, ...args(output)], { stdio: 'inherit' });
  assert.ok(existsSync(output), `vitest ${name} exited ${run.status} without writing ${output}`);
  return { status: run.status, output: JSON.parse(readFileSync(output, 'utf8')) };
}

// Every collected test file must be one this target declares.
const listed = vitest('list', (output) => ['list', '--filesOnly', `--json=${output}`]);
assert.equal(listed.status, 0, 'vitest list failed');
const collected = listed.output.map(({ file }) => relative(root, file));
assert.ok(collected.includes(declared), `${declared} was not collected`);
assert.ok(!collected.includes(undeclared), `${undeclared} was collected`);
assert.deepEqual(
  collected.filter((file) => !existsSync(file)),
  [],
  'Vitest collected test files this target does not declare',
);

// The declared test must also run and pass, with its undeclared sibling left out of the run.
const ran = vitest('run', (output) => ['run', 'src/discovery/', '--reporter=json', `--outputFile=${output}`]);
const results = new Map(ran.output.testResults.map((result) => [relative(root, result.name), result]));
assert.ok(!results.has(undeclared), `${undeclared} ran`);
assert.equal(results.get(declared)?.status, 'passed', `${declared} did not pass`);
// A file whose every test was skipped still reports as passed, so its one test must itself pass.
assert.deepEqual(
  results.get(declared).assertionResults.map(({ title, status }) => ({ title, status })),
  [{ title: 'runs under a declared name that holds glob metacharacters', status: 'passed' }],
  `${declared} did not run its one test to a pass`,
);
assert.equal(ran.status, 0, 'vitest run failed');
