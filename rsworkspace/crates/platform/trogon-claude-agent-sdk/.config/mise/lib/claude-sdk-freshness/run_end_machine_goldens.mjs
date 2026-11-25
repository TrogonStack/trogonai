#!/usr/bin/env node
// Records when the TypeScript `query()` closes the CLI's stdin relative to what the CLI sent, for the
// session-state, delegated-task, queued-turn and `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS` scenarios, by running the
// shipped `sdk.mjs` against `run_end_machine_cli.mjs`. The `_ts_*` entries of
// `tests/fixtures/auth_callbacks/run_machine.json` come from here; `run_end_machine_goldens.py` writes the `_py`
// ones into the same file.
//
// Each scenario gets 3.5 seconds before the recorder closes the query itself. A `harness_close` line goes into the
// log first, so the close that shutdown causes is never counted as the SDK ending the run: a run the SDK never
// ended records `never`.
//
// usage: node run_end_machine_goldens.mjs <package-dir> <out-file>
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const [packageDir, out] = process.argv.slice(2);
if (!packageDir || !out) {
  process.stderr.write('usage: run_end_machine_goldens.mjs PACKAGE_DIR OUT_FILE\n');
  process.exit(2);
}

const { query } = await import(pathToFileURL(path.join(packageDir, 'sdk.mjs')).href);
const version = JSON.parse(fs.readFileSync(path.join(packageDir, 'package.json'), 'utf8')).version;
const cli = path.join(path.dirname(fileURLToPath(import.meta.url)), 'run_end_machine_cli.mjs');
const logs = fs.mkdtempSync(path.join(os.tmpdir(), 'run-end-machine-'));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// A placeholder message UUID ending in `n`.
const uuid = (n) => '00000000-0000-4000-8000-' + String(n).padStart(12, '0');
const state = (s) => ({ type: 'system', subtype: 'session_state_changed', state: s, session_id: 's' });
const said = (t) => ({
  type: 'assistant',
  session_id: 's',
  parent_tool_use_id: null,
  uuid: uuid(t.length),
  message: {
    id: 'm' + t,
    type: 'message',
    role: 'assistant',
    model: 'm',
    content: [{ type: 'text', text: t }],
    stop_reason: 'end_turn',
    stop_sequence: null,
    usage: { input_tokens: 1, output_tokens: 1 },
  },
});
const result = {
  type: 'result',
  subtype: 'success',
  is_error: false,
  duration_ms: 1,
  duration_api_ms: 1,
  num_turns: 1,
  session_id: 's',
  result: 'done',
  queued_turn_count: 0,
};
const resultQueued = { ...result, queued_turn_count: 1 };
const task = (subtype) => ({
  type: 'system',
  subtype,
  task_id: 't1',
  task_type: 'local_agent',
  description: 'd',
  uuid: uuid(1),
  session_id: 's',
  ...(subtype === 'task_notification' ? { status: 'completed', output_file: 'f', summary: 'x' } : {}),
});
const W = (ms) => ({ __wait: ms });
const label = (f) =>
  f.type === 'result'
    ? 'result'
    : f.type === 'assistant'
      ? f.message.content[0].text
      : f.type === 'system' && f.state
        ? f.state
        : null;

const ceilingSteps = [W(100), state('running'), W(100), said('A'), result, W(300), state('running'), W(900), state('requires_action')];
const scenarios = {
  task_open_at_idle: [W(100), state('running'), W(100), task('task_started'), said('A'), result, W(300), state('idle'), W(300), said('CCC')],
  task_then_notified: [W(100), state('running'), W(100), task('task_started'), said('A'), result, W(300), task('task_notification'), W(100), state('idle'), W(300), said('CCC')],
  continuation_reported: [W(100), state('running'), W(100), said('A'), resultQueued, W(300), state('idle'), W(300), state('running'), said('BB'), result, W(300), said('CCC')],
  running_then_idle: [W(100), state('running'), W(100), said('A'), result, W(300), state('idle'), W(300), said('CCC')],
  ceiling: ceilingSteps,
  ceiling_zero: ceilingSteps,
  ceiling_exponent: ceilingSteps,
};
const ceilings = { ceiling: '500', ceiling_zero: '0', ceiling_exponent: '5e2' };

async function scenario(name, ceiling, onUser, stream) {
  const log = path.join(logs, `${name}.log`);
  fs.writeFileSync(log, '');
  async function* input() {
    yield { type: 'user', session_id: '', message: { role: 'user', content: 'hi' }, parent_tool_use_id: null };
  }
  const env = { ...process.env, FAKE_LOG: log, FAKE_SCRIPT: JSON.stringify({ on_user: onUser, stay_open: true }) };
  if (ceiling !== undefined) env.CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS = ceiling;
  const q = query({
    prompt: stream ? input() : 'hi',
    options: { canUseTool: async () => ({ behavior: 'allow' }), pathToClaudeCodeExecutable: cli, env },
  });
  const yielded = [];
  let ended = false;
  const reader = (async () => {
    try {
      for await (const m of q) yielded.push(label(m) ?? m.type);
      ended = true;
    } catch (e) {
      yielded.push('error:' + e.message);
    }
  })();
  await Promise.race([reader, sleep(3500)]);
  fs.appendFileSync(log, JSON.stringify({ kind: 'harness_close' }) + '\n');
  q.close();
  await sleep(100);
  const events = fs.readFileSync(log, 'utf8').trim().split('\n').filter(Boolean).map((l) => JSON.parse(l));
  const mark = events.findIndex((e) => e.kind === 'harness_close');
  const closed = events.find((e, i) => e.kind === 'stdin_closed' && i < mark);
  const sent = events
    .filter((e) => e.kind === 'cli_out' && label(e.frame))
    .map((e) => ({ label: label(e.frame), beforeClose: !!closed && e.seq < closed.seq }));
  const before = sent.filter((s) => s.beforeClose).map((s) => s.label);
  return {
    stdin_closed_after: closed ? (before.length ? before[before.length - 1] : 'nothing') : 'never',
    cli_sent: sent.map((s) => s.label),
    yielded,
    ended,
  };
}

const existing = fs.existsSync(out) ? JSON.parse(fs.readFileSync(out, 'utf8')) : {};
const fixture = {
  captured_from: { ...existing.captured_from, typescript: `sdk.mjs ${version}` },
  scenarios: { ...existing.scenarios },
};
for (const [name, steps] of Object.entries(scenarios)) {
  for (const [mode, stream] of [['ts_turn', false], ['ts_stream', true]]) {
    const key = `m_${name}_${mode}`;
    fixture.scenarios[key] = await scenario(key, ceilings[name], steps, stream);
  }
}
fs.rmSync(logs, { recursive: true, force: true });
fs.writeFileSync(out, JSON.stringify(fixture, null, 2) + '\n');
process.exit(0);
