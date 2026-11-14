#!/usr/bin/env node
// Records `gated.json`: the default branch the upstream bridge guesses for real repositories, including repositories
// that keep their references in the reftable format, where the answer comes from a trust-gated Git subprocess.
//
// The gate consults two pieces of process-wide state (the session working directory and whether the host is blocked).
// A standalone process has neither, so the recorder loads a copy of the bundle in which exactly those two accessors
// read `PROBE_SESSION_CWD` and `PROBE_BLOCKED` instead; everything else is the unmodified bundle.
//
// Usage: node bridge_create_session_git_goldens.mjs BRIDGE_MJS FIXTURES_DIR [--check]
import { execFileSync, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const self = fileURLToPath(import.meta.url);
if (process.argv[2] === '--child') {
  await child(JSON.parse(process.argv[3]));
  process.exit(0);
}
const [bridgePath, fixturesDir, flag] = process.argv.slice(2);
if (!bridgePath || !fixturesDir) {
  process.stderr.write('usage: node bridge_create_session_git_goldens.mjs BRIDGE_MJS FIXTURES_DIR [--check]\n');
  process.exit(2);
}
const check = flag === '--check';

const OLD_SESSION = 'function jo(){if(mw()?.originalCwd===void 0&&ft().project===null)return null;return kl()}';
const OLD_BLOCKED = 'function Kp(){return Ry.of(_e().host).blocked()}';
let source = fs.readFileSync(bridgePath, 'utf8');
for (const [old, replacement] of [
  [OLD_SESSION, 'function jo(){return process.env.PROBE_SESSION_CWD??null}'],
  [OLD_BLOCKED, 'function Kp(){return process.env.PROBE_BLOCKED==="1"}'],
]) {
  if (source.split(old).length !== 2) throw new Error(`bundle accessor not found exactly once: ${old}`);
  source = source.replace(old, () => replacement);
}
const patchedDir = fs.mkdtempSync(path.join(os.tmpdir(), 'bridge-gated-bundle-'));
const patchedPath = path.join(patchedDir, 'bridge.mjs');
fs.writeFileSync(patchedPath, source);

const quiet = ['-c', 'user.name=t', '-c', 'user.email=t@t', '-c', 'commit.gpgsign=false', '-c', 'core.hooksPath=/dev/null'];
const gitEnv = { ...process.env, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_SYSTEM: '/dev/null', GIT_CONFIG_NOSYSTEM: '1' };

const init = (dir, format) => ({ cwd: '.', git: ['init', '-q', '-b', 'main', ...(format === 'reftable' ? ['--ref-format=reftable'] : []), dir] });
const commit = (dir) => ({ cwd: dir, git: [...quiet, 'commit', '-q', '--allow-empty', '-m', 'x'] });
const remoteRef = (dir, name) => ({ cwd: dir, git: ['update-ref', `refs/remotes/origin/${name}`, 'HEAD'] });
const originHead = (dir, target) => ({ cwd: dir, git: ['symbolic-ref', 'refs/remotes/origin/HEAD', target] });
const repo = (dir, format, { head = null, remotes = [] } = {}) => [
  init(dir, format),
  commit(dir),
  ...remotes.map((name) => remoteRef(dir, name)),
  ...(head === null ? [] : [originHead(dir, head)]),
];
const origin = (name) => `refs/remotes/origin/${name}`;

const scenarios = [];
for (const format of ['reftable', 'files']) {
  const add = (name, spec) => scenarios.push({ name: `${format}_${name}`, format, ...spec });
  const develop = { head: origin('develop'), remotes: ['develop', 'main'] };
  add('session_same', { setup: repo('main', format, develop), process: 'main', session: 'main' });
  add('session_none', { setup: repo('main', format, develop), process: 'main', session: null });
  add('blocked', { setup: repo('main', format, develop), process: 'main', session: 'main', blocked: true });
  add('session_other_repo', { setup: [...repo('main', format, develop), ...repo('other', format, { remotes: ['main'] })], process: 'main', session: 'other' });
  add('session_not_a_repo', { setup: [...repo('main', format, develop), { mkdir: 'plain' }], process: 'main', session: 'plain' });
  add('session_subdirectory', { setup: [...repo('main', format, develop), { mkdir: 'main/sub/deep' }], process: 'main', session: 'main/sub/deep' });
  add('session_through_link', { setup: [...repo('main', format, develop), { symlink: ['sess', '<root>/main'] }], process: 'main', session: 'sess' });
  add('no_origin_head_main', { setup: repo('main', format, { remotes: ['main'] }), process: 'main', session: 'main' });
  add('no_origin_head_master', { setup: repo('main', format, { remotes: ['master'] }), process: 'main', session: 'main' });
  add('origin_head_target_missing', { setup: repo('main', format, { head: origin('develop'), remotes: ['master'] }), process: 'main', session: 'main' });
  add('origin_head_outside_origin', { setup: repo('main', format, { head: 'refs/remotes/upstream/dev', remotes: ['main'] }), process: 'main', session: 'main' });
  add('nested_branch', { setup: repo('main', format, { head: origin('release/1.0'), remotes: ['release/1.0'] }), process: 'main', session: 'main' });
  add('no_remote_refs', { setup: repo('main', format), process: 'main', session: 'main' });
  const linked = [...repo('main', format, develop), { cwd: 'main', git: ['worktree', 'add', '-q', '--detach', '../wt', 'HEAD'] }];
  add('worktree_session_worktree', { setup: linked, process: 'wt', session: 'wt' });
  add('worktree_session_main', { setup: linked, process: 'wt', session: 'main' });
  add('worktree_process_main_session_worktree', { setup: linked, process: 'main', session: 'wt' });
  add('worktree_session_other_repo', { setup: [...linked, ...repo('other', format, { remotes: ['main'] })], process: 'wt', session: 'other' });
  add('worktree_commondir_mismatch', {
    setup: [...linked, ...repo('other', format, { remotes: ['main'] }), { write: ['main/.git/worktrees/wt/commondir', '<root>/other/.git\n'] }],
    process: 'wt',
    session: 'wt',
  });
  add('worktree_commondir_missing', { setup: [...linked, { remove: 'main/.git/worktrees/wt/commondir' }], process: 'wt', session: 'wt' });
}

function materialize(root, scenario) {
  for (const step of scenario.setup) {
    if (step.git) {
      execFileSync('git', step.git, { cwd: path.join(root, step.cwd), env: gitEnv, stdio: 'pipe' });
    } else if (step.mkdir) {
      fs.mkdirSync(path.join(root, step.mkdir), { recursive: true });
    } else if (step.write) {
      fs.writeFileSync(path.join(root, step.write[0]), step.write[1].split('<root>').join(root));
    } else if (step.remove) {
      fs.rmSync(path.join(root, step.remove), { force: true });
    } else if (step.symlink) {
      fs.symlinkSync(step.symlink[1].split('<root>').join(root), path.join(root, step.symlink[0]));
    }
  }
}

const cases = [];
for (const scenario of scenarios) {
  const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'bridge-gated-')));
  materialize(root, scenario);
  const run = spawnSync(process.execPath, [self, '--child', JSON.stringify({
    bridgePath: patchedPath,
    cwd: path.join(root, scenario.process),
    session: scenario.session === null ? null : path.join(root, scenario.session),
    blocked: scenario.blocked === true,
  })], { encoding: 'utf8', timeout: 60000 });
  if (run.status !== 0) throw new Error(`${scenario.name}: ${run.stderr.split('\n').map((line) => line.slice(0, 240)).join('\n')}`);
  fs.rmSync(root, { recursive: true, force: true });
  cases.push({ ...scenario, blocked: scenario.blocked === true, guess: run.stdout.trim() });
}
fs.rmSync(patchedDir, { recursive: true, force: true });

const text = `[\n${cases.map((entry) => JSON.stringify(entry)).join(',\n')}\n]\n`;
const target = path.join(fixturesDir, 'gated.json');
if (check) {
  if (!fs.existsSync(target) || fs.readFileSync(target, 'utf8') !== text) {
    process.stderr.write(`${target} is stale\n`);
    process.exit(1);
  }
} else {
  fs.mkdirSync(fixturesDir, { recursive: true });
  fs.writeFileSync(target, text);
}

// Runs one guess in its own process: the bridge reads the working directory once, when it is loaded.
async function child({ bridgePath: bridge, cwd, session, blocked }) {
  process.chdir(cwd);
  delete process.env.CLAUDE_CODE_ENTRYPOINT;
  // The bundle looks the Git executable up with `Bun.which` even when it runs under Node; a PATH search is all it needs.
  globalThis.Bun = {
    which: (name) => (process.env.PATH ?? '').split(path.delimiter).map((dir) => path.join(dir, name)).find((file) => fs.existsSync(file)) ?? null,
  };
  // Session creation always has a working directory; a scenario without one stands for a directory that is no
  // repository at all.
  process.env.PROBE_SESSION_CWD = session ?? '/nonexistent-session-directory';
  if (blocked) process.env.PROBE_BLOCKED = '1';
  const module = await import(pathToFileURL(bridge).href).catch((error) => {
    process.stderr.write(`import failed: ${String(error?.message ?? error).slice(0, 300)}\n`);
    process.exit(3);
  });
  let body = '';
  const server = http.createServer((req, res) => {
    const chunks = [];
    req.on('data', (chunk) => chunks.push(chunk));
    req.on('end', () => {
      body = Buffer.concat(chunks).toString('utf8');
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end('{"session":{"id":"cse_abc"}}');
    });
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  await module.createCodeSession(`http://127.0.0.1:${server.address().port}`, 't', 'T', 5000, undefined, { gitRepoUrl: 'https://github.com/acme/widgets', branch: '' }, cwd);
  process.stdout.write(JSON.parse(body).config.sources[0].revision);
}
