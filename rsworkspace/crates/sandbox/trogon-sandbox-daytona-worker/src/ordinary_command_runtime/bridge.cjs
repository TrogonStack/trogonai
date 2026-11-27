'use strict';
// Runs inside the sandbox. The worker starts it in a session and feeds it one JSON control frame per input line.
// It starts the pinned command supervisor, relays the supervisor's events into state, acknowledgement and capture
// files under a per-execution prefix, and refuses any frame that does not match the execution it was started for.

const fs = require('node:fs');
const { createHash } = require('node:crypto');
const { spawn, spawnSync } = require('node:child_process');

const VERSION = 1;
const MAX_FRAME = 1024 * 1024;
const MAX_DATA = 4096;
const MAX_HELPER_BYTES = 16 * 1024 * 1024;
const CONTROL_TIMEOUT = 10000;
const MAX_TIMEOUT_MS = 365 * 24 * 60 * 60 * 1000;
const CONTROL_SIGNALS = [1, 2, 3, 9, 10, 12, 15];

// Checks of untrusted values. Each returns the value when it is acceptable and throws otherwise.

function reject(reason) {
  throw new Error('invalid_' + reason);
}

function record(value, keys) {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) reject('object');
  const present = Object.keys(value).sort();
  const wanted = [...keys].sort();
  if (present.length !== wanted.length || present.some((key, index) => key !== wanted[index])) reject('fields');
  return value;
}

function text(value, min, max) {
  if (typeof value !== 'string' || value.length < min || value.length > max) reject('text');
  return value;
}

function integer(value, min, max) {
  if (!Number.isSafeInteger(value) || value < min || value > max) reject('integer');
  return value;
}

function flag(value) {
  if (typeof value !== 'boolean') reject('flag');
  return value;
}

function orNull(value, check) {
  return value === null ? null : check(value);
}

function oneOf(value, allowed) {
  if (!allowed.includes(value)) reject('choice');
  return value;
}

function identifier(value) {
  text(value, 1, 256);
  if (/[\x00-\x1f\x7f]/.test(value)) reject('identifier');
  return value;
}

function hex64(value) {
  if (typeof value !== 'string' || !/^[a-f0-9]{64}$/.test(value)) reject('digest');
  return value;
}

function environment(value) {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) reject('environment');
  const entries = Object.entries(value);
  if (entries.length > 1024) reject('environment');
  let size = 0;
  for (const [key, item] of entries) {
    if (typeof item !== 'string' || key.length === 0 || key.includes('=') || key.includes('\0') || item.includes('\0')) {
      reject('environment');
    }
    size += Buffer.byteLength(key) + Buffer.byteLength(item) + 2;
  }
  if (size > 65536) reject('environment');
  return value;
}

function words(value) {
  if (!Array.isArray(value) || value.length < 1 || value.length > 1024) reject('argv');
  let size = 0;
  for (const item of value) {
    if (typeof item !== 'string' || item.includes('\0')) reject('argv');
    size += Buffer.byteLength(item);
  }
  if (value[0].length === 0 || size > 512 * 1024) reject('argv');
  return value;
}

function directory(value) {
  text(value, 1, 4096);
  if (value.includes('\0')) reject('directory');
  return value;
}

function process_identity(value) {
  record(value, ['pid', 'start_time']);
  integer(value.pid, 2, Number.MAX_SAFE_INTEGER);
  integer(value.start_time, 1, Number.MAX_SAFE_INTEGER);
  return value;
}

function configuration(value) {
  record(value, ['argv', 'env', 'cwd', 'timeout_ms', 'user', 'interactive']);
  words(value.argv);
  environment(value.env);
  orNull(value.cwd, directory);
  orNull(value.timeout_ms, (item) => integer(item, 1, MAX_TIMEOUT_MS));
  orNull(value.user, identifier);
  flag(value.interactive);
  return value;
}

function control_frame(value) {
  if (value === null || typeof value !== 'object') reject('object');
  const shapes = {
    init: ['config'],
    launch: [],
    signal: ['signal'],
    stdin: ['data', 'eof'],
    status: [],
  };
  if (!Object.hasOwn(shapes, value.kind)) reject('kind');
  const extra = shapes[value.kind];
  record(value, ['version', 'execution_id', 'nonce', 'request_id', 'kind', ...extra]);
  if (value.version !== VERSION) reject('version');
  identifier(value.execution_id);
  hex64(value.nonce);
  integer(value.request_id, 1, Number.MAX_SAFE_INTEGER);
  if (value.kind === 'init') configuration(value.config);
  if (value.kind === 'signal') oneOf(value.signal, CONTROL_SIGNALS);
  if (value.kind === 'stdin') {
    text(value.data, 0, 384 * 1024);
    flag(value.eof);
  }
  return value;
}

const NATIVE_ERRORS = [
  'unsupported_platform', 'kernel_prerequisite', 'invalid_frame', 'invalid_command', 'wrong_execution',
  'wrong_phase', 'setup_failed', 'exec_failed', 'identity_unknown', 'process_limit', 'cleanup_pending', 'io_failure',
];

function exit_pair(code, signal) {
  orNull(code, (item) => integer(item, 0, 255));
  orNull(signal, (item) => integer(item, 1, 64));
}

function native_event(value) {
  if (value === null || typeof value !== 'object') reject('object');
  const shapes = {
    hello: ['version', 'environment'],
    ready: ['version', 'helper', 'root', 'process_group', 'session'],
    released: [],
    signal_result: ['request_id', 'signal', 'delivered'],
    output: ['stream', 'data'],
    root_exited: ['exit_code', 'signal'],
    family_absent: [
      'released', 'root_exit_code', 'root_signal', 'timed_out', 'canceled', 'reaped_count',
      'stdout_truncated', 'stderr_truncated',
    ],
    error: ['code', 'uncertain'],
    status: ['phase'],
  };
  if (!Object.hasOwn(shapes, value.kind)) reject('kind');
  const extra = shapes[value.kind];
  record(value, ['execution_id', 'kind', ...extra]);
  identifier(value.execution_id);
  switch (value.kind) {
    case 'hello':
      if (value.version !== VERSION) reject('version');
      environment(value.environment);
      break;
    case 'ready':
      if (value.version !== VERSION) reject('version');
      process_identity(value.helper);
      process_identity(value.root);
      integer(value.process_group, 2, Number.MAX_SAFE_INTEGER);
      integer(value.session, 2, Number.MAX_SAFE_INTEGER);
      break;
    case 'signal_result':
      integer(value.request_id, 1, Number.MAX_SAFE_INTEGER);
      integer(value.signal, 1, 64);
      flag(value.delivered);
      break;
    case 'output':
      oneOf(value.stream, ['stdout', 'stderr']);
      if (!Array.isArray(value.data) || value.data.length > MAX_DATA) reject('output');
      for (const byte of value.data) integer(byte, 0, 255);
      break;
    case 'root_exited':
      exit_pair(value.exit_code, value.signal);
      break;
    case 'family_absent':
      flag(value.released);
      exit_pair(value.root_exit_code, value.root_signal);
      flag(value.timed_out);
      flag(value.canceled);
      integer(value.reaped_count, 0, Number.MAX_SAFE_INTEGER);
      flag(value.stdout_truncated);
      flag(value.stderr_truncated);
      break;
    case 'error':
      oneOf(value.code, NATIVE_ERRORS);
      flag(value.uncertain);
      break;
    case 'status':
      oneOf(value.phase, ['awaiting_init', 'held', 'running', 'draining', 'absent']);
      break;
    default:
      reject('kind');
  }
  return value;
}

function completion_record(value) {
  record(value, [
    'released', 'root_exit_code', 'root_signal', 'timed_out', 'canceled', 'reaped_count',
    'stdout_truncated', 'stderr_truncated', 'stdout_len', 'stderr_len', 'stdout_sha256', 'stderr_sha256',
  ]);
  if (value.released !== true) reject('completion');
  exit_pair(value.root_exit_code, value.root_signal);
  if ((value.root_exit_code === null) === (value.root_signal === null)) reject('completion');
  flag(value.timed_out);
  flag(value.canceled);
  integer(value.reaped_count, 1, Number.MAX_SAFE_INTEGER);
  flag(value.stdout_truncated);
  flag(value.stderr_truncated);
  integer(value.stdout_len, 0, Number.MAX_SAFE_INTEGER);
  integer(value.stderr_len, 0, Number.MAX_SAFE_INTEGER);
  hex64(value.stdout_sha256);
  hex64(value.stderr_sha256);
  return value;
}

// Startup: the arguments bind this process to one execution, one nonce and one pinned supervisor digest.

const argumentsList = process.argv.slice(2);
if (argumentsList.length !== 5) process.exit(78);
const [helperPath, prefix, execution, nonce, expectedHash] = argumentsList;
try {
  text(helperPath, 1, 4096);
  text(prefix, 1, 4096);
  if (!/^[0-9a-f]{32}$/.test(execution)) reject('execution');
  hex64(nonce);
  hex64(expectedHash);
} catch {
  process.exit(78);
}
if (
  prefix !== '/tmp/.trogon-exec-' + execution
  || process.platform !== 'linux'
  || Number(process.versions.node.split('.')[0]) < 22
) {
  process.exit(78);
}
const nativeExecution = execution + '.' + nonce;

// The supervisor is read once through a descriptor opened without following links, and must match its pin.
const helperFile = fs.openSync(helperPath, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW);
const helperStat = fs.fstatSync(helperFile);
if (!helperStat.isFile() || helperStat.size > MAX_HELPER_BYTES) process.exit(78);
const helperBuffer = Buffer.alloc(MAX_HELPER_BYTES + 1);
let helperLength = 0;
while (helperLength < helperBuffer.length) {
  const count = fs.readSync(helperFile, helperBuffer, helperLength, helperBuffer.length - helperLength, null);
  if (count === 0) break;
  helperLength += count;
}
fs.closeSync(helperFile);
const helperBytes = helperBuffer.subarray(0, helperLength);
if (helperBytes.length > MAX_HELPER_BYTES || createHash('sha256').update(helperBytes).digest('hex') !== expectedHash) {
  process.exit(78);
}

let config = null;
let helper = null;
let stdoutFd = null;
let stderrFd = null;
let stdoutLength = 0;
let stderrLength = 0;
const stdoutHash = createHash('sha256');
const stderrHash = createHash('sha256');
let fifoFd = null;
let released = false;
let hello = false;
let rootExit = null;
let terminal = false;
let terminalStatusExpected = false;
let pendingCompletion = null;
let failed = false;
let lastRequest = 0;
let lastFingerprint = '';
let lastAck = null;
let state = {
  version: VERSION, execution_id: execution, nonce, phase: 'starting', helper: null, root: null, completion: null,
  root_exit: null,
};
let waitingLaunch = null;
let waitingSignal = null;
let waitingStatus = null;
const preparedTimer = setTimeout(() => { if (helper === null) fail(); }, 30000);

function writeJson(path, value) {
  const pending = path + '.pending';
  fs.writeFileSync(pending, JSON.stringify(value), { flag: 'wx', mode: 0o600 });
  fs.renameSync(pending, path);
}

function saveState() {
  if (state.completion !== null) completion_record(state.completion);
  writeJson(prefix + '.state', state);
}

function closeFifo() {
  if (fifoFd !== null) {
    fs.closeSync(fifoFd);
    fifoFd = null;
  }
}

function fail() {
  if (failed) return;
  failed = true;
  state = { ...state, phase: 'failed', completion: null };
  try { saveState(); } catch {}
  try { closeFifo(); } catch {}
  if (waitingLaunch !== null) waitingLaunch.reject();
  if (waitingSignal !== null) waitingSignal.reject();
  if (waitingStatus !== null) waitingStatus.reject();
  if (helper !== null) helper.stdin.end();
  if (helper === null) {
    for (const fd of [stdoutFd, stderrFd]) {
      if (fd !== null) {
        try { fs.closeSync(fd); } catch {}
      }
    }
    process.exit(74);
  }
}

function sendNative(command) {
  const frame = JSON.stringify(command) + '\n';
  if (Buffer.byteLength(frame) > MAX_FRAME || helper === null || !helper.stdin.writable) {
    throw new Error('native_control_unavailable');
  }
  helper.stdin.write(frame, (error) => { if (error) fail(); });
}

function writeAll(fd, data) {
  let offset = 0;
  while (offset < data.length) {
    const count = fs.writeSync(fd, data, offset, data.length - offset);
    if (count <= 0) throw new Error('capture_write_failed');
    offset += count;
  }
}

function shellQuote(value) {
  return "'" + value.replaceAll("'", "'\\''") + "'";
}

function acceptNative(raw) {
  const event = native_event(raw);
  if (event.execution_id !== nativeExecution) throw new Error('foreign_native_execution');
  switch (event.kind) {
    case 'hello': {
      if (failed || config === null || hello || state.helper !== null || released) throw new Error('native_order');
      hello = true;
      const argv = config.interactive
        ? ['/bin/sh', '-c', 'f=$1; shift; exec "$@" < "$f"', 'sandbox-ordinary-stdin', prefix + '.stdin', ...config.argv]
        : config.argv;
      sendNative({
        kind: 'init', version: VERSION, execution_id: nativeExecution, argv,
        env: environment({ ...event.environment, ...config.env }), cwd: config.cwd, timeout_ms: config.timeout_ms,
        release_timeout_ms: 30000,
      });
      break;
    }
    case 'ready':
      if (
        failed || !hello || state.helper !== null || event.helper.pid === event.root.pid
        || event.process_group !== event.root.pid || event.session !== event.root.pid
      ) {
        throw new Error('native_identity');
      }
      state = { ...state, phase: 'held', helper: event.helper, root: event.root };
      saveState();
      sendNative({ kind: 'release', execution_id: nativeExecution });
      break;
    case 'released':
      if (state.root === null || released) throw new Error('native_order');
      released = true;
      state = { ...state, phase: 'running' };
      saveState();
      if (waitingLaunch !== null) waitingLaunch.resolve();
      break;
    case 'output': {
      if (!released || terminal) throw new Error('native_output_order');
      const data = Buffer.from(event.data);
      writeAll(event.stream === 'stdout' ? stdoutFd : stderrFd, data);
      if (event.stream === 'stdout') {
        stdoutLength += data.length;
        stdoutHash.update(data);
      } else {
        stderrLength += data.length;
        stderrHash.update(data);
      }
      if (!Number.isSafeInteger(stdoutLength) || !Number.isSafeInteger(stderrLength)) {
        throw new Error('capture_size_overflow');
      }
      break;
    }
    case 'signal_result':
      if (waitingSignal === null || waitingSignal.request !== event.request_id || waitingSignal.signal !== event.signal) {
        throw new Error('foreign_signal_ack');
      }
      terminalStatusExpected = terminal;
      waitingSignal.resolve(event.delivered);
      break;
    case 'root_exited':
      if (!released || rootExit !== null || (event.exit_code === null) === (event.signal === null)) {
        throw new Error('native_exit');
      }
      rootExit = { exit_code: event.exit_code, signal: event.signal };
      state = { ...state, root_exit: rootExit };
      saveState();
      break;
    case 'family_absent':
      if (
        terminal || rootExit === null || !event.released || event.root_exit_code !== rootExit.exit_code
        || event.root_signal !== rootExit.signal
      ) {
        throw new Error('native_completion');
      }
      terminal = true;
      closeFifo();
      fs.fsyncSync(stdoutFd);
      fs.fsyncSync(stderrFd);
      if (fs.fstatSync(stdoutFd).size !== stdoutLength || fs.fstatSync(stderrFd).size !== stderrLength) {
        throw new Error('capture_size_changed');
      }
      fs.closeSync(stdoutFd);
      stdoutFd = null;
      fs.closeSync(stderrFd);
      stderrFd = null;
      if (!failed) {
        pendingCompletion = completion_record({
          released: true, root_exit_code: event.root_exit_code, root_signal: event.root_signal,
          timed_out: event.timed_out, canceled: event.canceled, reaped_count: event.reaped_count,
          stdout_truncated: event.stdout_truncated, stderr_truncated: event.stderr_truncated,
          stdout_len: stdoutLength, stderr_len: stderrLength,
          stdout_sha256: stdoutHash.digest('hex'), stderr_sha256: stderrHash.digest('hex'),
        });
        state = { ...state, phase: 'draining' };
        saveState();
      }
      sendNative({ kind: 'acknowledge', execution_id: nativeExecution });
      break;
    case 'error':
      fail();
      break;
    case 'status':
      if (event.phase === 'absent' && !terminal) throw new Error('unearned_native_absence');
      if (waitingStatus === null) {
        if (terminalStatusExpected && event.phase === 'absent') {
          terminalStatusExpected = false;
          break;
        }
        throw new Error('unexpected_native_status');
      }
      if (!terminal && event.phase === 'draining') {
        state = { ...state, phase: 'draining' };
        saveState();
      }
      waitingStatus.resolve();
      break;
    default:
      throw new Error('unknown_native_event');
  }
}

function waitLaunch() {
  return new Promise((resolve, reject_) => {
    const timer = setTimeout(() => {
      waitingLaunch = null;
      reject_(new Error('native_launch_unconfirmed'));
    }, CONTROL_TIMEOUT);
    waitingLaunch = {
      resolve: () => { clearTimeout(timer); waitingLaunch = null; resolve(); },
      reject: () => { clearTimeout(timer); waitingLaunch = null; reject_(new Error('native_launch_failed')); },
    };
  });
}

function startHelper() {
  const waiting = waitLaunch();
  const nativeArgs = ['--execution-id', nativeExecution];
  const stdio = ['pipe', 'pipe', 'pipe'];
  if (config.user === null) {
    helper = spawn(helperPath, nativeArgs, { stdio });
  } else if (process.getuid && process.getuid() === 0) {
    const line = 'exec ' + [helperPath, ...nativeArgs].map(shellQuote).join(' ');
    helper = spawn('su', ['-s', '/bin/sh', config.user, '-c', line], { stdio });
  } else {
    helper = spawn('sudo', ['-n', '-u', config.user, helperPath, ...nativeArgs], { stdio });
  }
  let frame = Buffer.alloc(0);
  helper.stdout.on('data', (data) => {
    try {
      frame = Buffer.concat([frame, data]);
      for (;;) {
        const newline = frame.indexOf(10);
        if (newline < 0) break;
        if (newline > MAX_FRAME) throw new Error('native_frame_bound');
        acceptNative(JSON.parse(frame.subarray(0, newline).toString('utf8')));
        frame = frame.subarray(newline + 1);
      }
      if (frame.length > MAX_FRAME) throw new Error('native_frame_bound');
    } catch {
      fail();
    }
  });
  helper.stderr.on('data', (data) => { if (data.length) fail(); });
  helper.on('error', () => { fail(); process.exit(78); });
  helper.on('close', (code, signal) => {
    if (!terminal || frame.length !== 0 || code !== 0 || signal !== null) fail();
    if (!failed && pendingCompletion !== null) {
      try {
        const completion = completion_record(pendingCompletion);
        const exit = completion.root_exit_code !== null ? completion.root_exit_code : 128 + completion.root_signal;
        fs.writeFileSync(prefix + '.exit', String(exit), { flag: 'wx', mode: 0o600 });
        fs.writeFileSync(prefix + '.status', completion.timed_out ? 'timed_out' : 'completed', { flag: 'wx', mode: 0o600 });
        state = { ...state, phase: 'absent', completion };
        saveState();
      } catch {
        fail();
      }
    }
    process.exit(failed ? 74 : 0);
  });
  return waiting;
}

async function fifoWrite(data) {
  if (fifoFd === null) throw new Error('stdin_closed');
  let offset = 0;
  const until = Date.now() + CONTROL_TIMEOUT;
  while (offset < data.length) {
    if (terminal || failed || fifoFd === null || Date.now() >= until) throw new Error('stdin_unconfirmed');
    try {
      const count = fs.writeSync(fifoFd, data, offset, data.length - offset);
      if (count <= 0) throw new Error('stdin_unconfirmed');
      offset += count;
    } catch (error) {
      if (error.code !== 'EAGAIN') throw error;
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
  }
}

async function control(raw) {
  const command = control_frame(raw);
  if (command.execution_id !== execution || command.nonce !== nonce) throw new Error('foreign_control');
  const fingerprint = createHash('sha256').update(JSON.stringify(command)).digest('hex');
  if (command.request_id === lastRequest && fingerprint === lastFingerprint && lastAck !== null) {
    writeJson(prefix + '.ack', lastAck);
    return;
  }
  if (failed || command.request_id <= lastRequest) throw new Error('stale_control');
  lastRequest = command.request_id;
  lastFingerprint = fingerprint;
  lastAck = null;
  let delivered = null;
  let bytes = 0;
  let eof = false;
  switch (command.kind) {
    case 'init':
      if (config !== null) throw new Error('duplicate_init');
      config = command.config;
      stdoutFd = fs.openSync(prefix + '.out', 'wx', 0o600);
      stderrFd = fs.openSync(prefix + '.err', 'wx', 0o600);
      if (config.interactive) {
        const created = spawnSync('mkfifo', ['-m', '600', prefix + '.stdin'], { stdio: 'ignore' });
        if (created.status !== 0) throw new Error('fifo_setup_failed');
        fifoFd = fs.openSync(prefix + '.stdin', fs.constants.O_RDWR | fs.constants.O_NONBLOCK);
      }
      state = { ...state, phase: 'prepared' };
      saveState();
      break;
    case 'launch':
      if (config === null || state.phase !== 'prepared') throw new Error('launch_order');
      clearTimeout(preparedTimer);
      await startHelper();
      break;
    case 'signal':
      if (terminal) {
        delivered = false;
        break;
      }
      if (!released || helper === null) throw new Error('signal_unavailable');
      delivered = await new Promise((resolve, reject_) => {
        const timer = setTimeout(() => {
          waitingSignal = null;
          reject_(new Error('signal_unconfirmed'));
        }, CONTROL_TIMEOUT);
        waitingSignal = {
          request: command.request_id,
          signal: command.signal,
          resolve: (result) => { clearTimeout(timer); waitingSignal = null; resolve(result); },
          reject: () => { clearTimeout(timer); waitingSignal = null; reject_(new Error('signal_failed')); },
        };
        sendNative({
          kind: 'signal', execution_id: nativeExecution, request_id: command.request_id, signal: command.signal,
        });
      });
      break;
    case 'stdin': {
      if (terminal) break;
      if (!released || !config || !config.interactive) throw new Error('stdin_unavailable');
      const data = Buffer.from(command.data, 'base64');
      if (data.toString('base64') !== command.data) throw new Error('invalid_stdin_encoding');
      await fifoWrite(data);
      bytes = data.length;
      eof = command.eof;
      if (eof) closeFifo();
      break;
    }
    case 'status':
      if (terminal) break;
      if (!released || helper === null) throw new Error('status_unavailable');
      await new Promise((resolve, reject_) => {
        const timer = setTimeout(() => {
          waitingStatus = null;
          reject_(new Error('status_unconfirmed'));
        }, CONTROL_TIMEOUT);
        waitingStatus = {
          resolve: () => { clearTimeout(timer); waitingStatus = null; resolve(); },
          reject: () => { clearTimeout(timer); waitingStatus = null; reject_(new Error('status_failed')); },
        };
        sendNative({ kind: 'status', execution_id: nativeExecution });
      });
      break;
    default:
      throw new Error('unknown_control');
  }
  lastAck = {
    version: VERSION, execution_id: execution, nonce, request_id: command.request_id, operation: command.kind,
    signal: command.kind === 'signal' ? command.signal : null, delivered, bytes_written: bytes, eof,
  };
  writeJson(prefix + '.ack', lastAck);
}

// Control frames arrive on stdin, one JSON document per line, and are handled strictly in order.
let input = Buffer.alloc(0);
let queue = Promise.resolve();
let queued = 0;
process.stdin.on('data', (data) => {
  input = Buffer.concat([input, data]);
  for (;;) {
    const newline = input.indexOf(10);
    if (newline < 0) break;
    if (newline > MAX_FRAME) {
      fail();
      return;
    }
    const frame = input.subarray(0, newline);
    input = input.subarray(newline + 1);
    queued += 1;
    if (queued > 16) {
      fail();
      return;
    }
    queue = queue
      .then(async () => { await control(JSON.parse(frame.toString('utf8'))); })
      .catch(fail)
      .finally(() => { queued -= 1; });
  }
  if (input.length > MAX_FRAME) fail();
});
process.stdin.on('end', () => { if (!terminal) fail(); });
process.stdin.on('error', fail);
