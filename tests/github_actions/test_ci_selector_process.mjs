import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync, spawnSync } from 'node:child_process';
import { test } from 'node:test';

const root = fileURLToPath(new URL('../../', import.meta.url));
const selector = path.join(root, '.config/mise/tasks/github/workflows/ci-selection/filter');
const head = 'a'.repeat(40);
const base = 'b'.repeat(40);

function fixture(t) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'ci-selector-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const env = { ...process.env };
  for (const name of ['GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE']) delete env[name];
  const git = (...args) => execFileSync('git', args, { cwd: directory, env, encoding: 'utf8' }).trim();
  const write = (file, contents) => {
    const target = path.join(directory, file);
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, contents);
  };
  const commit = () => {
    git('add', '--all');
    git('-c', 'core.hooksPath=/dev/null', '-c', 'commit.gpgsign=false', '-c', 'user.name=Fixture', '-c', 'user.email=fixture', 'commit', '--quiet', '--allow-empty', '-m', 'fixture');
    return git('rev-parse', 'HEAD');
  };
  git('init', '--quiet', '-b', 'main');
  for (const name of ['ci', 'codeql']) write(`.github/filters/${name}.yml`, fs.readFileSync(path.join(root, `.github/filters/${name}.yml`)));
  write('rsworkspace/src/lib.rs', 'before');
  commit();

  function run(eventName, event, responses = [], overrides = {}) {
    write('event.json', JSON.stringify({ repository: { full_name: 'example/project' }, ...event }));
    write('responses.json', JSON.stringify(responses));
    write('calls.json', '[]');
    write('output', '');
    write('bin/package.json', '{"type":"module"}');
    write('bin/gh', `#!${process.execPath}\n`
      + "import fs from 'node:fs'; const p = process.env.GITHUB_WORKSPACE;\n"
      + "const calls = JSON.parse(fs.readFileSync(p + '/calls.json'));\n"
      + "const response = JSON.parse(fs.readFileSync(p + '/responses.json'))[calls.length];\n"
      + "calls.push(process.argv.slice(2)); fs.writeFileSync(p + '/calls.json', JSON.stringify(calls));\n"
      + "if (response?.fail) { process.stderr.write('not authorized'); process.exit(1); }\n"
      + "process.stdout.write(JSON.stringify(response));\n");
    fs.chmodSync(path.join(directory, 'bin/gh'), 0o755);
    const result = spawnSync(selector, [], {
      cwd: directory, encoding: 'utf8', env: {
        ...env, PATH: [path.join(directory, 'bin'), path.dirname(process.execPath), env.PATH].join(path.delimiter),
        GITHUB_WORKSPACE: directory, GITHUB_EVENT_PATH: path.join(directory, 'event.json'),
        GITHUB_EVENT_NAME: eventName, GITHUB_REPOSITORY: 'example/project', GITHUB_SHA: git('rev-parse', 'HEAD'),
        GITHUB_OUTPUT: path.join(directory, 'output'), CI_FILTERS: '.github/filters/ci.yml', ...overrides,
      },
    });
    const output = fs.readFileSync(path.join(directory, 'output'), 'utf8');
    const values = Object.fromEntries(output.trim().split('\n').filter(Boolean).map((line) => [line.slice(0, line.indexOf('=')), line.slice(line.indexOf('=') + 1)]));
    return { ...result, output, values, calls: JSON.parse(fs.readFileSync(path.join(directory, 'calls.json'))) };
  }
  return { directory, git, write, commit, run };
}

const event = () => ({ number: 7, pull_request: { head: { sha: head }, base: { sha: base } } });
const metadata = (count, overrides = {}) => ({ head: { sha: head }, base: { sha: base }, changed_files: count, ...overrides });
const row = (filename) => ({ filename, status: 'modified' });
function failed(result) {
  assert.notEqual(result.status, 0);
  assert.equal(result.output, '');
  assert.equal(result.stdout, '');
}

test('push compares real before/head commits and keeps the canary fix selective', (t) => {
  const f = fixture(t); const before = f.git('rev-parse', 'HEAD');
  f.write('.config/mise/tasks/github-actions/canary-is-branch-tip', 'change');
  f.write('buf.gen.yaml', 'change');
  const after = f.commit();
  const result = f.run('push', { before, after });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.values, { rust: 'false', go: 'false', proto: 'true', changes: '["proto"]' });
  assert.deepEqual(result.calls, []);
});

test('initial pushes select every check without comparing a nonexistent commit', (t) => {
  const f = fixture(t); const after = f.git('rev-parse', 'HEAD');
  for (const before of [undefined, null, '', '0'.repeat(40)]) {
    const result = f.run('push', { before, after });
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(result.values, { rust: 'true', go: 'true', proto: 'true', changes: '["rust","go","proto"]' });
    assert.deepEqual(result.calls, []);
    const codeql = f.run('push', { before, after }, [], { CI_FILTERS: '.github/filters/codeql.yml' });
    assert.equal(codeql.status, 0, codeql.stderr);
    assert.equal(codeql.values.changes, '["go","javascript-typescript","rust"]');
  }
});

test('manual dispatch selects every check without consulting the API', (t) => {
  const f = fixture(t);
  const result = f.run('workflow_dispatch', {});
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.values, { rust: 'true', go: 'true', proto: 'true', changes: '["rust","go","proto"]' });
  assert.deepEqual(result.calls, []);
  failed(f.run('schedule', {}));
});

test('push uses NUL paths and recognizes deleted ownership after a real rename', (t) => {
  const f = fixture(t); const before = f.git('rev-parse', 'HEAD');
  f.write('notes/moved.txt', '');
  fs.unlinkSync(path.join(f.directory, 'notes/moved.txt'));
  f.git('mv', 'rsworkspace/src/lib.rs', 'notes/moved.txt');
  f.write('goworkspace/name\nwith-newline.go', 'change');
  const after = f.commit();
  const result = f.run('push', { before, after });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.values.rust, 'true'); assert.equal(result.values.go, 'true');
});

test('PR pagination includes previous names and binds metadata around collection', (t) => {
  const f = fixture(t);
  const pages = [[{ filename: 'notes/moved.txt', previous_filename: 'rsworkspace/src/lib.rs', status: 'renamed' }], [row('goworkspace/new.go')]];
  const result = f.run('pull_request', event(), [metadata(2), pages, metadata(2)]);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.values.rust, 'true'); assert.equal(result.values.go, 'true');
  assert.deepEqual(result.calls, [
    ['api', 'repos/example/project/pulls/7'],
    ['api', '--paginate', '--slurp', 'repos/example/project/pulls/7/files?per_page=100'],
    ['api', 'repos/example/project/pulls/7'],
  ]);
});

test('PR metadata permits an empty complete list and selects only CodeQL input languages', (t) => {
  const f = fixture(t);
  const empty = f.run('pull_request', event(), [metadata(0), [], metadata(0)]);
  assert.equal(empty.status, 0, empty.stderr); assert.equal(empty.values.changes, '[]');
  const rust = f.run('pull_request', event(), [metadata(1), [[row('rsworkspace/src/lib.rs')]], metadata(1)], { CI_FILTERS: '.github/filters/codeql.yml' });
  assert.equal(rust.status, 0, rust.stderr); assert.equal(rust.values.changes, '["rust"]');
});

test('PR head and base changes before or during pagination cannot exclude checks', (t) => {
  const f = fixture(t);
  for (const field of ['head', 'base']) {
    const changed = metadata(1, { [field]: { sha: 'c'.repeat(40) } });
    failed(f.run('pull_request', event(), [changed]));
    failed(f.run('pull_request', event(), [metadata(1), [[row('notes/readme.md')]], changed]));
  }
});

test('API failure, truncation, duplicate pages and malformed file data fail closed', (t) => {
  const f = fixture(t);
  for (const responses of [
    [{ fail: true }],
    [metadata(3001), [[row('notes/readme.md')]], metadata(3001)],
    [metadata(2), [[row('notes/readme.md')], [row('notes/readme.md')]], metadata(2)],
    [metadata(1), {}, metadata(1)],
    [metadata(1), [[{ filename: 'notes/moved.txt', status: 'renamed' }]], metadata(1)],
    [metadata(1), [[row('../outside')]], metadata(1)],
    [metadata(1), [[{ filename: 'notes/file', status: 'unknown' }]], metadata(1)],
  ]) failed(f.run('pull_request', event(), responses));
});

test('unknown push history, malformed SHAs and mismatched event commits fail closed', (t) => {
  const f = fixture(t); const after = f.git('rev-parse', 'HEAD');
  for (const before of ['d'.repeat(40), '--help', '0'.repeat(39), '0'.repeat(41)]) failed(f.run('push', { before, after }));
  failed(f.run('push', { before: after, after: 'e'.repeat(40) }));
  failed(f.run('push', { before: after, after }, [], { GITHUB_SHA: 'e'.repeat(40) }));
});

test('repository and filter schema errors do not publish partial selection output', (t) => {
  const f = fixture(t);
  failed(f.run('pull_request', { ...event(), repository: { full_name: 'another/project' } }));
  f.write('.github/filters/ci.yml', 'rust: ["rsworkspace/**"]\n');
  failed(f.run('pull_request', event(), [metadata(0), [], metadata(0)]));
});
