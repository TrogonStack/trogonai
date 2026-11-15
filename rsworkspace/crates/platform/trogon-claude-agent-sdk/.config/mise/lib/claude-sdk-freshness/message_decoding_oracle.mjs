#!/usr/bin/env node
// Streams each corpus message through the pinned TypeScript query() and prints what it yields.
//
// usage: node message_decoding_oracle.mjs <package-dir> <cases.json>
//
// Each case runs against a stand-in CLI that answers the initialize request, writes the case message on
// stdout and exits. The output maps every case name to the list of messages query() yielded for it.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const [pkg, casesPath] = process.argv.slice(2);
if (!pkg || !casesPath) {
  process.stderr.write('usage: message_decoding_oracle.mjs PACKAGE_DIR CASES_JSON\n');
  process.exit(2);
}
const { cases } = JSON.parse(fs.readFileSync(casesPath, 'utf8'));
const sdk = await import(pathToFileURL(path.join(pkg, 'sdk.mjs')).href);
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'message-decoding-'));
const cli = path.join(scratch, 'cli.mjs');
fs.writeFileSync(
  cli,
  `import readline from 'node:readline';
const message = process.env.ORACLE_MESSAGE;
const lines = readline.createInterface({ input: process.stdin });
lines.on('line', (line) => {
  const frame = JSON.parse(line);
  if (frame.type !== 'control_request') return;
  process.stdout.write(JSON.stringify({ type: 'control_response', response: { subtype: 'success', request_id: frame.request_id, response: {} } }) + '\\n');
  if (frame.request.subtype !== 'initialize') return;
  process.stdout.write(message + '\\n', () => process.exit(0));
});
`,
);

const results = {};
try {
  for (const testCase of cases) {
    const yielded = [];
    let error = null;
    const stream = sdk.query({
      prompt: (async function* () {})(),
      options: {
        pathToClaudeCodeExecutable: cli,
        executable: 'node',
        env: { ...process.env, ORACLE_MESSAGE: JSON.stringify(testCase.message) },
        settingSources: [],
      },
    });
    try {
      for await (const message of stream) yielded.push(message);
    } catch (caught) {
      error = String(caught?.message ?? caught);
    }
    results[testCase.name] = { yielded, error };
  }
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}
process.stdout.write(`${JSON.stringify(results, null, 2)}\n`);
