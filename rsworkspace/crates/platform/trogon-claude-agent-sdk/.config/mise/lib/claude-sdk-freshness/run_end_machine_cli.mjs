#!/usr/bin/env node
// Stand-in CLI for the run end machine recorders. It answers `initialize`, plays the `FAKE_SCRIPT` steps after the
// first user message (`on_user`) or after initialize (`after_init`), and appends every frame it writes and the
// moment its stdin closes to `FAKE_LOG`, one JSON object per line. With `stay_open` it outlives its stdin, so the
// log shows whether the SDK closed stdin and what the CLI had sent by then.
import fs from 'node:fs';
import readline from 'node:readline';

const script = JSON.parse(process.env.FAKE_SCRIPT ?? '{}');
const log = process.env.FAKE_LOG;
let seq = 0;
const rec = (o) => fs.appendFileSync(log, JSON.stringify({ seq: seq++, t: Date.now(), ...o }) + '\n');
const out = (o) => {
  process.stdout.write(JSON.stringify(o) + '\n');
  rec({ kind: 'cli_out', frame: o });
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function play(steps) {
  for (const step of steps) {
    if (step.__wait) await sleep(step.__wait);
    else out(step);
  }
}

let userPlayed = false;
let started = false;
async function onLine(line) {
  const msg = JSON.parse(line);
  rec({ kind: 'sdk_in', line });
  if (msg.type === 'user' && script.on_user && !userPlayed) {
    userPlayed = true;
    await play(script.on_user);
    return;
  }
  if (msg.type !== 'control_request') return;
  out({ type: 'control_response', response: { subtype: 'success', request_id: msg.request_id, response: {} } });
  if (msg.request.subtype === 'initialize' && !started) {
    started = true;
    if (script.after_init) await play(script.after_init);
  }
}

const rl = readline.createInterface({ input: process.stdin });
rl.on('line', (line) => {
  onLine(line);
});
rl.on('close', () => {
  rec({ kind: 'stdin_closed' });
  if (script.stay_open) setTimeout(() => process.exit(0), 2000);
  else process.exit(0);
});
