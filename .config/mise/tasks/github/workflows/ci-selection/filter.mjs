#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';

const root = fileURLToPath(new URL('../../../../../../', import.meta.url));
const toolkit = createRequire(path.join(root, 'tests/github_actions/package.json'));
const { default: yaml } = await import(pathToFileURL(toolkit.resolve('js-yaml')));
const { default: picomatch } = await import(pathToFileURL(toolkit.resolve('picomatch')));
const env = process.env;
const workspace = env.GITHUB_WORKSPACE || root;
const NULL_COMMIT = '0'.repeat(40);
const isInitialPush = (before) => before == null || before === '' || before === NULL_COMMIT;
const sha = (value) => typeof value === 'string' && /^[0-9a-f]{40}$/.test(value) && value !== NULL_COMMIT;
const run = (command, args) => execFileSync(command, args, {
  cwd: workspace, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
  timeout: 60000, maxBuffer: 32 * 1024 * 1024,
});
const api = (endpoint, options = []) => JSON.parse(run('gh', ['api', ...options, endpoint]));
const requireValue = (condition) => { if (!condition) throw new Error('Invalid selection input'); };
const filePath = (value) => typeof value === 'string' && value.length > 0 && !value.includes('\0')
  && !value.split('/').some((part) => ['', '.', '..'].includes(part));

try {
  const event = JSON.parse(fs.readFileSync(env.GITHUB_EVENT_PATH, 'utf8'));
  const repo = env.GITHUB_REPOSITORY;
  requireValue(typeof repo === 'string' && /^[A-Za-z0-9_-]+\/[A-Za-z0-9_.-]+$/.test(repo));
  requireValue(!['.', '..'].includes(repo.split('/')[1]));
  requireValue(event.repository?.full_name?.toLowerCase() === repo.toLowerCase());
  requireValue(sha(env.GITHUB_SHA) && run('git', ['rev-parse', '--verify', 'HEAD']).trim() === env.GITHUB_SHA);
  requireValue(env.GITHUB_OUTPUT);

  const groups = env.CI_FILTERS === '.github/filters/ci.yml' ? ['rust', 'go', 'proto']
    : env.CI_FILTERS === '.github/filters/codeql.yml' ? ['go', 'javascript-typescript', 'rust'] : null;
  requireValue(groups);
  const filters = yaml.load(fs.readFileSync(path.join(workspace, env.CI_FILTERS), 'utf8'));
  requireValue(filters && !Array.isArray(filters) && typeof filters === 'object');
  requireValue(Object.keys(filters).sort().join(',') === [...groups].sort().join(','));
  const matchers = Object.entries(filters).map(([name, patterns]) => {
    requireValue(Array.isArray(patterns) && patterns.length > 0);
    requireValue(patterns.every((pattern) => typeof pattern === 'string' && pattern.length > 0 && !pattern.startsWith('!')));
    return [name, patterns.map((pattern) => picomatch(pattern, { dot: true, flags: 's' }))];
  });

  let files = [];
  let selectAll = false;
  if (env.GITHUB_EVENT_NAME === 'push') {
    requireValue(event.after === env.GITHUB_SHA);
    selectAll = isInitialPush(event.before);
    if (!selectAll) {
      requireValue(sha(event.before));
      run('git', ['rev-parse', '--verify', `${event.before}^{commit}`]);
      run('git', ['rev-parse', '--verify', `${env.GITHUB_SHA}^{commit}`]);
      files = run('git', ['diff', '--no-renames', '--name-only', '-z', event.before, env.GITHUB_SHA, '--'])
        .split('\0').filter(Boolean);
    }
  } else if (env.GITHUB_EVENT_NAME === 'pull_request') {
    const number = event.number;
    const expectedHead = event.pull_request?.head?.sha;
    const expectedBase = event.pull_request?.base?.sha;
    requireValue(Number.isSafeInteger(number) && number > 0 && sha(expectedHead));
    requireValue(expectedBase === undefined || sha(expectedBase));
    const endpoint = `repos/${repo}/pulls/${number}`;
    const check = (metadata) => {
      requireValue(metadata?.head?.sha === expectedHead && sha(metadata?.base?.sha));
      requireValue(expectedBase === undefined || metadata.base.sha === expectedBase);
      requireValue(Number.isSafeInteger(metadata.changed_files) && metadata.changed_files >= 0);
      return metadata;
    };
    const before = check(api(endpoint));
    const pages = api(`${endpoint}/files?per_page=100`, ['--paginate', '--slurp']);
    requireValue(Array.isArray(pages) && pages.every(Array.isArray));
    const rows = pages.flat();
    const after = check(api(endpoint));
    requireValue(before.base.sha === after.base.sha && rows.length === before.changed_files && rows.length === after.changed_files);
    requireValue(new Set(rows.map((row) => row?.filename)).size === rows.length);
    files = rows.flatMap((row) => {
      requireValue(row && ['added', 'removed', 'modified', 'renamed', 'copied', 'changed', 'unchanged'].includes(row.status));
      requireValue(row.status !== 'renamed' || typeof row.previous_filename === 'string');
      const names = row.previous_filename === undefined ? [row.filename] : [row.filename, row.previous_filename];
      requireValue(names.every(filePath));
      return names;
    });
  } else if (env.GITHUB_EVENT_NAME === 'workflow_dispatch') {
    selectAll = true;
  } else {
    throw new Error('Unsupported selection event');
  }

  const selected = selectAll ? groups : matchers.filter(([, patterns]) => files.some((file) => patterns.some((match) => match(file))))
    .map(([name]) => name);
  const output = matchers.map(([name]) => `${name}=${selected.includes(name)}\n`).join('')
    + `changes=${JSON.stringify(selected)}\n`;
  fs.appendFileSync(env.GITHUB_OUTPUT, output);
} catch {
  console.error('CI selection failed; retaining full-check defaults');
  process.exitCode = 1;
}
