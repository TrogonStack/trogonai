import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import {
  InputError, countIdle, decideTrust, parseLabels, select,
} from '../../.github/actions/runs-on-fallback/lib/index.mjs';

const repository = 'TrogonStack/trogonai';
const runner = (labels, { status = 'online', busy = false } = {}) => ({
  status, busy, labels: labels.map((name) => ({ name })),
});

function envFor(eventName, event) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'runs-on-fallback-'));
  const env = { GITHUB_EVENT_NAME: eventName, GITHUB_REPOSITORY: repository };
  if (event !== undefined) {
    env.GITHUB_EVENT_PATH = path.join(dir, 'event.json');
    fs.writeFileSync(env.GITHUB_EVENT_PATH, JSON.stringify(event));
  }
  return env;
}

const pushEnv = () => envFor('push', {});

function response(runners, { status = 200, link } = {}) {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: (name) => (name.toLowerCase() === 'link' ? link ?? null : null) },
    json: async () => ({ total_count: runners.length, runners }),
  };
}

function fakeFetch(pages) {
  const calls = [];
  const fn = async (url, init) => {
    calls.push({ url: String(url), init });
    const page = pages[calls.length - 1];
    if (page instanceof Error) throw page;
    return page;
  };
  fn.calls = calls;
  return fn;
}

const base = (overrides = {}) => ({
  githubToken: 'token',
  primaryLabels: 'homelab, linux',
  fallbackLabels: 'ubuntu-latest',
  scope: 'repository',
  jobs: 'default',
  env: pushEnv(),
  fetch: fakeFetch([response([runner(['self-hosted', 'homelab', 'linux'])])]),
  ...overrides,
});

test('trust: push, dispatch, schedule and merge_group are trusted', () => {
  for (const eventName of ['push', 'workflow_dispatch', 'schedule', 'merge_group']) {
    assert.deepEqual(decideTrust({ eventName, event: {}, repository }), { trusted: true });
  }
});

test('trust: same-repository pull request is trusted, case-insensitively', () => {
  for (const full_name of [repository, repository.toUpperCase()]) {
    const event = { pull_request: { head: { repo: { full_name } } } };
    assert.deepEqual(decideTrust({ eventName: 'pull_request', event, repository }), { trusted: true });
  }
});

test('trust: fork pull request, deleted fork and missing head are untrusted', () => {
  for (const event of [
    { pull_request: { head: { repo: { full_name: 'someone/trogonai' } } } },
    { pull_request: { head: { repo: null } } },
    { pull_request: {} },
  ]) {
    assert.deepEqual(decideTrust({ eventName: 'pull_request', event, repository }), { trusted: false, reason: 'fork-pull-request' });
  }
});

test('trust: pull_request_target and unknown events are untrusted', () => {
  for (const eventName of ['pull_request_target', 'issue_comment', 'workflow_run', '', undefined]) {
    assert.deepEqual(decideTrust({ eventName, event: {}, repository }), { trusted: false, reason: 'untrusted-event' });
  }
});

test('trust: an unreadable payload fails closed', () => {
  assert.deepEqual(decideTrust({ eventName: 'push', event: null, repository }), { trusted: false, reason: 'untrusted-event' });
});

test('parseLabels splits on commas and newlines and drops blanks', () => {
  assert.deepEqual(parseLabels('a, b\n c ,,\n'), ['a', 'b', 'c']);
  assert.deepEqual(parseLabels(''), []);
  assert.deepEqual(parseLabels(undefined), []);
});

test('countIdle requires every primary label, ignoring case', () => {
  const runners = [
    runner(['self-hosted', 'HomeLab', 'linux']),
    runner(['self-hosted', 'homelab']),
    runner(['homelab', 'linux', 'x64', 'gpu']),
  ];
  assert.equal(countIdle(runners, ['homelab', 'LINUX']), 2);
  assert.equal(countIdle(runners, ['homelab']), 3);
  assert.equal(countIdle(runners, ['homelab', 'arm']), 0);
});

test('countIdle skips busy and offline runners', () => {
  const runners = [
    runner(['homelab'], { busy: true }),
    runner(['homelab'], { status: 'offline' }),
    runner(['homelab']),
  ];
  assert.equal(countIdle(runners, ['homelab']), 1);
});

test('countIdle tolerates malformed runners', () => {
  assert.equal(countIdle([null, {}, { status: 'online', busy: false }, runner(['homelab'])], ['homelab']), 1);
});

test('an idle trusted runner is selected', async () => {
  const fetch = fakeFetch([response([runner(['self-hosted', 'homelab', 'linux'])])]);
  const result = await select(base({ fetch }));
  assert.deepEqual(result.runsOn, ['homelab', 'linux']);
  assert.equal(result.selfHosted, true);
  assert.equal(result.reason, 'idle-runner');
  assert.deepEqual(result.warnings, []);
  assert.equal(fetch.calls.length, 1);
  assert.equal(fetch.calls[0].url, 'https://api.github.com/repos/TrogonStack/trogonai/actions/runners?per_page=100&page=1');
  assert.equal(fetch.calls[0].init.headers.Authorization, 'Bearer token');
});

const groupsUrl = /\/orgs\/TrogonStack\/actions\/runner-groups\?/;
const groupRunnersUrl = (id) => new RegExp(`/orgs/TrogonStack/actions/runner-groups/${id}/runners\\?`);

function groupResponse(groups, options) {
  const res = response([], options);
  res.json = async () => ({ total_count: groups.length, runner_groups: groups });
  return res;
}

// Answers by URL, so a test states which endpoints exist rather than the order
// they are called in. An unrouted URL is a failure: it was never meant to be asked.
function routedFetch(routes) {
  const calls = [];
  const fn = async (url, init) => {
    calls.push({ url: String(url), init });
    const route = routes.find(([pattern]) => pattern.test(String(url)));
    if (!route) throw new Error(`unrouted request ${url}`);
    const [, answers] = route;
    const index = calls.filter((call) => route[0].test(call.url)).length - 1;
    return answers[Math.min(index, answers.length - 1)];
  };
  fn.calls = calls;
  return fn;
}

const orgBase = (fetch, overrides = {}) => base({ scope: 'organization', fetch, ...overrides });

test('organization scope asks for the groups visible to this repository', async () => {
  const fetch = routedFetch([[groupsUrl, [groupResponse([])]]]);
  const result = await select(orgBase(fetch));
  assert.equal(result.reason, 'no-idle-runner');
  assert.equal(fetch.calls[0].url, 'https://api.github.com/orgs/TrogonStack/actions/runner-groups?visible_to_repository=trogonai&per_page=100&page=1');
});

test('organization scope counts runners of the visible group only', async () => {
  const fetch = routedFetch([
    [groupsUrl, [groupResponse([{ id: 7, restricted_to_workflows: false }])]],
    [groupRunnersUrl(7), [response([runner(['homelab', 'linux'])])]],
  ]);
  const result = await select(orgBase(fetch));
  assert.equal(result.reason, 'idle-runner');
  assert.equal(fetch.calls[1].url, 'https://api.github.com/orgs/TrogonStack/actions/runner-groups/7/runners?per_page=100&page=1');
  assert.equal(fetch.calls.some((call) => /\/actions\/runners\?/.test(call.url)), false);
});

test('a group invisible to this repository is never requested', async () => {
  const fetch = routedFetch([
    [groupsUrl, [groupResponse([{ id: 7, restricted_to_workflows: false }])]],
    [groupRunnersUrl(7), [response([runner(['homelab', 'linux'], { busy: true })])]],
  ]);
  await select(orgBase(fetch));
  assert.equal(fetch.calls.some((call) => /runner-groups\/9\//.test(call.url)), false);
  assert.equal(fetch.calls.length, 2);
});

test('a workflow-restricted group is skipped without listing its runners', async () => {
  const fetch = routedFetch([
    [groupsUrl, [groupResponse([{ id: 5, restricted_to_workflows: true }])]],
  ]);
  const result = await select(orgBase(fetch));
  assert.equal(result.reason, 'no-idle-runner');
  assert.equal(fetch.calls.length, 1);
});

test('idle runners of several groups are summed', async () => {
  const fetch = routedFetch([
    [groupsUrl, [groupResponse([{ id: 1 }, { id: 5, restricted_to_workflows: true }, { id: 2, restricted_to_workflows: false }])]],
    [groupRunnersUrl(1), [response([runner(['homelab', 'linux']), runner(['homelab', 'linux'], { busy: true })])]],
    [groupRunnersUrl(2), [response([runner(['homelab', 'linux']), runner(['other'])])]],
  ]);
  const result = await select(orgBase(fetch, { jobs: 'a b c' }));
  assert.equal(result.selfHostedCount, 2);
  assert.deepEqual(result.runners, { a: ['homelab', 'linux'], b: ['homelab', 'linux'], c: ['ubuntu-latest'] });
});

test('group listing and group runner listing both paginate', async () => {
  const groups = Array.from({ length: 100 }, (_, i) => ({ id: 100 + i, restricted_to_workflows: true }));
  const next = '<https://api.github.com/x?page=2>; rel="next"';
  const busy = Array.from({ length: 100 }, () => runner(['homelab', 'linux'], { busy: true }));
  const fetch = routedFetch([
    [groupsUrl, [groupResponse(groups, { link: next }), groupResponse([{ id: 3 }])]],
    [groupRunnersUrl(3), [response(busy, { link: next }), response([runner(['homelab', 'linux'])])]],
  ]);
  const result = await select(orgBase(fetch));
  assert.equal(result.reason, 'idle-runner');
  assert.equal(fetch.calls.length, 4);
  assert.match(fetch.calls[1].url, /runner-groups\?.*page=2/);
  assert.match(fetch.calls[3].url, /runner-groups\/3\/runners\?.*page=2/);
});

test('group runner listing stops early once every job is placed', async () => {
  const idle = Array.from({ length: 100 }, () => runner(['homelab', 'linux']));
  const fetch = routedFetch([
    [groupsUrl, [groupResponse([{ id: 1 }, { id: 2 }])]],
    [groupRunnersUrl(1), [response(idle, { link: '<x>; rel="next"' })]],
    [groupRunnersUrl(2), [response(idle)]],
  ]);
  const result = await select(orgBase(fetch, { jobs: 'a b' }));
  assert.equal(result.selfHostedCount, 2);
  assert.equal(fetch.calls.length, 2);
});

test('a failing group call falls back with a warning', async () => {
  const fetch = routedFetch([[groupsUrl, [groupResponse([], { status: 404 })]]]);
  const result = await select(orgBase(fetch));
  assert.equal(result.reason, 'api-error');
  assert.match(result.warnings[0], /404/);
});

test('API URL honours GITHUB_API_URL', async () => {
  const fetch = fakeFetch([response([runner(['homelab', 'linux'])])]);
  await select(base({ fetch, env: { ...pushEnv(), GITHUB_API_URL: 'https://ghe.example/api/v3' } }));
  assert.match(fetch.calls[0].url, /^https:\/\/ghe\.example\/api\/v3\/repos\//);
});

test('no idle runner falls back', async () => {
  const fetch = fakeFetch([response([runner(['homelab', 'linux'], { busy: true })])]);
  const result = await select(base({ fetch }));
  assert.deepEqual(result.runsOn, ['ubuntu-latest']);
  assert.equal(result.selfHosted, false);
  assert.equal(result.reason, 'no-idle-runner');
  assert.deepEqual(result.warnings, []);
});

test('idle runners go to the first jobs and the rest take the fallback', async () => {
  const runners = [runner(['homelab', 'linux']), runner(['homelab', 'linux']), runner(['homelab', 'linux'], { busy: true })];
  const result = await select(base({ jobs: 'test build wasm-decider, lint', fetch: fakeFetch([response(runners)]) }));
  assert.deepEqual(result.runners, {
    test: ['homelab', 'linux'],
    build: ['homelab', 'linux'],
    'wasm-decider': ['ubuntu-latest'],
    lint: ['ubuntu-latest'],
  });
  assert.deepEqual(result.runsOn, ['homelab', 'linux']);
  assert.equal(result.selfHostedCount, 2);
  assert.equal(result.selfHosted, true);
  assert.equal(result.reason, 'idle-runner');
});

test('more idle runners than jobs leaves no job on the fallback', async () => {
  const runners = Array.from({ length: 5 }, () => runner(['homelab', 'linux']));
  const result = await select(base({ jobs: 'a b', fetch: fakeFetch([response(runners)]) }));
  assert.deepEqual(result.runners, { a: ['homelab', 'linux'], b: ['homelab', 'linux'] });
  assert.equal(result.selfHostedCount, 2);
});

test('every job holds the fallback when nothing is self-hosted', async () => {
  const result = await select(base({ jobs: 'a\nb, c', primaryLabels: '' }));
  assert.deepEqual(result.runners, { a: ['ubuntu-latest'], b: ['ubuntu-latest'], c: ['ubuntu-latest'] });
  assert.equal(result.selfHostedCount, 0);
  assert.equal(result.selfHosted, false);
});

test('the default job name is default', async () => {
  const result = await select(base({ jobs: '' }));
  assert.deepEqual(Object.keys(result.runners), ['default']);
});

test('pagination stops once every job is placed', async () => {
  const page = Array.from({ length: 100 }, () => runner(['homelab', 'linux']));
  const fetch = fakeFetch([response(page, { link: '<https://api.github.com/x?page=2>; rel="next"' }), response(page)]);
  const result = await select(base({ jobs: 'a b c', fetch }));
  assert.equal(result.selfHostedCount, 3);
  assert.equal(fetch.calls.length, 1);
});

test('idle runners counted across pages are placed', async () => {
  const idle = Array.from({ length: 99 }, () => runner(['homelab', 'linux']));
  const fetch = fakeFetch([
    response([...idle, runner(['homelab', 'linux'], { busy: true })], { link: '<https://api.github.com/x?page=2>; rel="next"' }),
    response([runner(['homelab', 'linux'])]),
  ]);
  const names = Array.from({ length: 101 }, (_, i) => `j${i}`).join(' ');
  const result = await select(base({ jobs: names, fetch }));
  assert.equal(result.selfHostedCount, 100);
  assert.equal(Object.keys(result.runners).length, 101);
});

test('not configured falls back without an API call', async () => {
  const fetch = fakeFetch([]);
  const result = await select(base({ primaryLabels: ' \n ', fetch }));
  assert.equal(result.reason, 'not-configured');
  assert.deepEqual(result.runsOn, ['ubuntu-latest']);
  assert.equal(fetch.calls.length, 0);
});

test('untrusted events fall back without an API call', async () => {
  const fetch = fakeFetch([]);
  const fork = envFor('pull_request', { pull_request: { head: { repo: { full_name: 'x/y' } } } });
  const forkResult = await select(base({ env: fork, fetch }));
  assert.equal(forkResult.reason, 'fork-pull-request');
  const target = await select(base({ env: envFor('pull_request_target', {}), fetch }));
  assert.equal(target.reason, 'untrusted-event');
  const missing = await select(base({ env: envFor('push'), fetch }));
  assert.equal(missing.reason, 'untrusted-event');
  assert.equal(fetch.calls.length, 0);
});

test('a missing token falls back without an API call', async () => {
  const fetch = fakeFetch([]);
  const result = await select(base({ githubToken: '', fetch }));
  assert.equal(result.reason, 'no-token');
  assert.equal(result.selfHosted, false);
  assert.equal(fetch.calls.length, 0);
});

test('trust is decided before the token check', async () => {
  const result = await select(base({ githubToken: '', env: envFor('pull_request_target', {}) }));
  assert.equal(result.reason, 'untrusted-event');
});

test('pagination follows full pages until a short one', async () => {
  const busy = Array.from({ length: 100 }, () => runner(['homelab', 'linux'], { busy: true }));
  const fetch = fakeFetch([
    response(busy, { link: '<https://api.github.com/x?page=2>; rel="next"' }),
    response([runner(['homelab', 'linux'])]),
  ]);
  const result = await select(base({ fetch }));
  assert.equal(result.reason, 'idle-runner');
  assert.equal(fetch.calls.length, 2);
  assert.match(fetch.calls[1].url, /page=2/);
});

test('pagination stops when Link has no next page', async () => {
  const busy = Array.from({ length: 100 }, () => runner(['homelab', 'linux'], { busy: true }));
  const fetch = fakeFetch([response(busy, { link: '<https://api.github.com/x?page=1>; rel="prev"' })]);
  const result = await select(base({ fetch }));
  assert.equal(result.reason, 'no-idle-runner');
  assert.equal(fetch.calls.length, 1);
});

test('a non-200 response falls back with a warning', async () => {
  const result = await select(base({ fetch: fakeFetch([response([], { status: 403 })]) }));
  assert.equal(result.reason, 'api-error');
  assert.equal(result.selfHosted, false);
  assert.deepEqual(result.runsOn, ['ubuntu-latest']);
  assert.equal(result.warnings.length, 1);
  assert.match(result.warnings[0], /403/);
});

test('a thrown fetch falls back with a warning', async () => {
  const result = await select(base({ fetch: fakeFetch([new Error('socket hang up')]) }));
  assert.equal(result.reason, 'api-error');
  assert.match(result.warnings[0], /socket hang up/);
});

test('every request carries a timeout signal and an abort falls back', async () => {
  const fetch = fakeFetch([response([runner(['homelab', 'linux'])])]);
  await select(base({ fetch }));
  assert.ok(fetch.calls[0].init.signal instanceof AbortSignal);
  const aborted = Object.assign(new Error('The operation was aborted due to timeout'), { name: 'TimeoutError' });
  const result = await select(base({ fetch: fakeFetch([aborted]) }));
  assert.equal(result.reason, 'api-error');
  assert.match(result.warnings[0], /timeout/);
});

test('the whole search shares one deadline and falls back when it passes', async () => {
  // Each page answers quickly, well inside the per-request timeout, and
  // promises another, so only the search-wide deadline can stop the walk.
  let calls = 0;
  const fetch = (url, init) => new Promise((resolve, reject) => {
    calls += 1;
    const timer = setTimeout(() => resolve(response(Array.from({ length: 100 }, () => runner(['other'])))), 5);
    init.signal.addEventListener('abort', () => { clearTimeout(timer); reject(init.signal.reason); }, { once: true });
  });
  const result = await select(base({ fetch, searchTimeoutMs: 40 }));
  assert.equal(result.reason, 'api-error');
  assert.ok(calls > 1 && calls < 50, `${calls} requests`);
});

test('a malformed response body falls back with a warning', async () => {
  const bad = { ...response([]), json: async () => ({ nope: true }) };
  const result = await select(base({ fetch: fakeFetch([bad]) }));
  assert.equal(result.reason, 'api-error');
  assert.equal(result.warnings.length, 1);
});

test('the result is shaped for runs-on and fromJSON', async () => {
  const result = await select(base({ fallbackLabels: 'ubuntu-latest, x64' }));
  assert.equal(JSON.stringify(result.runsOn), '["homelab","linux"]');
  const fallback = await select(base({ primaryLabels: '', fallbackLabels: 'ubuntu-latest\nx64' }));
  assert.equal(JSON.stringify(fallback.runsOn), '["ubuntu-latest","x64"]');
  assert.equal(typeof result.selfHosted, 'boolean');
  assert.equal(typeof result.reason, 'string');
});

test('bad inputs throw InputError', async () => {
  for (const overrides of [
    { scope: 'enterprise' },
    { jobs: 'Test' },
    { jobs: '-lint' },
    { jobs: 'a b a' },
    { jobs: 'a.b' },
    { fallbackLabels: ' , ' },
  ]) {
    await assert.rejects(select(base(overrides)), InputError, JSON.stringify(overrides));
  }
});

test('defaults apply to empty scope and jobs', async () => {
  const fetch = fakeFetch([response([runner(['homelab', 'linux'])])]);
  const result = await select(base({ scope: '', jobs: '', fetch }));
  assert.equal(result.reason, 'idle-runner');
  assert.match(fetch.calls[0].url, /\/repos\//);
});

test('action metadata has no expressions, which GitHub would evaluate and fail to load', () => {
  const actions = path.join(path.dirname(new URL(import.meta.url).pathname), '../../.github/actions');
  for (const name of fs.readdirSync(actions)) {
    const file = path.join(actions, name, 'action.yml');
    if (!fs.existsSync(file)) continue;
    const text = fs.readFileSync(file, 'utf8');
    const metadata = text.slice(0, text.search(/^runs:/m) === -1 ? text.length : text.search(/^runs:/m));
    for (const line of metadata.split('\n')) {
      if (/^\s+value:/.test(line)) continue;
      assert.doesNotMatch(line, /\$\{\{/, `${name}/action.yml: ${line.trim()}`);
    }
  }
});
