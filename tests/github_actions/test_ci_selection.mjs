import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import yaml from 'js-yaml';
import picomatch from 'picomatch';
import { Lexer, Parser, Evaluator, data } from '@actions/expressions';

const root = fileURLToPath(new URL('../../', import.meta.url));
const load = (file) => yaml.load(fs.readFileSync(path.join(root, file), 'utf8'));
const workflows = {
  codeql: load('.github/workflows/codeql.yml'),
};

function matches(patterns, files) {
  return patterns.flat(Infinity).some((pattern) => {
    const match = picomatch(pattern, { dot: true, flags: 's' });
    return files.some((file) => match(file));
  });
}

function selected(name, files) {
  const filters = load(workflows[name].jobs.changes.with.filters);
  return Object.keys(filters).filter((language) => matches(filters[language], files));
}

test('canary and automation changes skip CodeQL', () => {
  const files = [
    '.config/mise/tasks/github/workflows/canary-container-images/canary-is-branch-tip',
    'tests/github_actions/test_canary_is_branch_tip.py', '.config/buf/buf.gen.yaml',
    '.github/workflows/codeql.yml', '.github/workflows/ci-selection.yml',
    '.github/filters/codeql.yml', 'tests/github_actions/test_ci_selection.mjs',
  ];
  assert.deepEqual(selected('codeql', files), []);
});

for (const [file, language] of [
  ['rsworkspace/crates/platform/trogon-gateway/src/lib.rs', 'rust'],
  ['rsworkspace/Cargo.lock', 'rust'],
  ['goworkspace/internal/protowit/generate.go', 'go'],
  ['tsworkspace/apps/console/src/index.ts', 'javascript-typescript'],
]) test(`CodeQL language: ${file}`, () => assert.deepEqual(selected('codeql', [file]), [language]));

test('automation changes cannot hide Rust changes from CodeQL', () => {
  const files = ['.config/mise/tasks/github/workflows/canary-container-images/canary-is-branch-tip', 'rsworkspace/crates/platform/trogonai-proto/src/gen/mod.rs'];
  assert.deepEqual(selected('codeql', files), ['rust']);
});

async function evaluate(source, context, cancelled = false) {
  function convert(value) {
    if (typeof value === 'string') return new data.StringData(value);
    if (typeof value === 'boolean') return new data.BooleanData(value);
    if (value === null || value === undefined) return new data.Null();
    return new data.Dictionary(...Object.entries(value).map(([key, item]) => ({ key, value: convert(item) })));
  }
  const functions = [{ name: 'cancelled', minArgs: 0, maxArgs: 0, call: () => new data.BooleanData(cancelled) }];
  const lexer = new Lexer(source.replace(/^\$\{\{\s*/, '').replace(/\s*\}\}$/, ''));
  const parser = new Parser(lexer.lex().tokens, Object.keys(context), functions);
  return new Evaluator(parser.parse(), convert(context), new Map(functions.map((f) => [f.name, f]))).evaluate();
}

test('every heavy job is gated, including comparison failures and cancellation', async () => {
  for (const [suite, workflow] of Object.entries(workflows)) {
    assert.equal(workflow.jobs.changes.uses, './.github/workflows/ci-selection.yml');
    assert.equal(workflow.jobs.changes.permissions['pull-requests'], 'read');
    for (const [id, job] of Object.entries(workflow.jobs)) {
      if (id === 'changes' || id === 'result') continue;
      assert.equal(job.needs, 'changes');
      for (const result of ['success', 'failure']) {
        const outputs = { changes: '[]' };
        const context = { needs: { changes: { result, outputs } }, github: { event_name: 'pull_request' } };
        assert.equal((await evaluate(job.if, context)).coerceString(), String(result !== 'success'), `${suite}/${id}`);
        assert.equal((await evaluate(job.if, context, true)).coerceString(), 'false', `${suite}/${id} cancellation`);
        outputs.changes = '["rust"]';
        assert.equal((await evaluate(job.if, context)).coerceString(), 'true', `${suite}/${id} relevant change`);
      }
    }
  }
});

test('selection always runs with conservative fallback defaults', async () => {
  const job = load('.github/workflows/ci-selection.yml').jobs.filter;
  const filter = job.steps.find((step) => step.id === 'filter');
  assert.equal(filter.if, undefined);
  assert.deepEqual(Object.keys(job.outputs), ['changes']);
  assert.equal((await evaluate(job.outputs.changes, { steps: { filter: { outputs: {} } } })).coerceString(), '["go","javascript-typescript","rust"]');
  const checkout = job.steps.find((step) => step.uses?.startsWith('actions/checkout@'));
  assert.equal((await evaluate(checkout.with['fetch-depth'], { github: { event_name: 'push' } })).coerceString(), '0');
});

test('CodeQL preserves the baseline configuration set for each selected language', async () => {
  const matrix = workflows.codeql.jobs.analyze.strategy.matrix.language;
  const baseline = ['go', 'javascript-typescript', 'rust'];
  for (const language of baseline) {
    const context = { needs: { changes: { outputs: { changes: JSON.stringify([language]) } } } };
    const actual = Array.isArray(matrix) ? matrix
      : (await evaluate(matrix, context)).values().map((item) => item.coerceString());
    assert.deepEqual(actual, baseline, `${language} changes retain every baseline configuration`);
  }
});

test('empty CodeQL matrices are skipped and selected build modes are preserved', async () => {
  const job = workflows.codeql.jobs.analyze;
  const init = job.steps.find((step) => step.name === 'Initialize CodeQL');
  assert.equal(init.with.queries, 'security-and-quality');
  assert.equal(job.steps.find((step) => step.name === 'Perform CodeQL analysis').with.category, '/language:${{ matrix.language }}');
  for (const [language, expected] of [['go', 'autobuild'], ['rust', 'none'], ['javascript-typescript', 'none']]) {
    assert.equal((await evaluate(init.with['build-mode'], { matrix: { language } })).coerceString(), expected);
  }
  assert.deepEqual(job.strategy.matrix.language, ['go', 'javascript-typescript', 'rust']);
});

function runResult(workflow, step, env) {
  assert.equal(step.run, `mise run --skip-tools github:workflows:${workflow}:result`);
  const task = path.join(root, `.config/mise/tasks/github/workflows/${workflow}/result`);
  return spawnSync(task, [], { env: { ...process.env, MISE_PROJECT_ROOT: root, ...env }, encoding: 'utf8' });
}

test('CodeQL result reports every outcome so branch rules can require it', async () => {
  const job = workflows.codeql.jobs.result;
  assert.equal(job.name, 'CodeQL result');
  assert.deepEqual(job.needs, ['changes', 'analyze']);
  assert.equal(job.if, '${{ always() }}');
  const step = job.steps.at(-1);
  assert.deepEqual(step.env, {
    SELECTION_RESULT: '${{ needs.changes.result }}',
    SELECTED: "${{ needs.changes.outputs.changes != '[]' }}",
    JOB_RESULTS: '${{ needs.analyze.result }}',
  });
  for (const [selection, languages, analyze, status] of [
    ['success', '["rust"]', 'success', 0],
    ['failure', '', 'success', 0],
    ['success', '[]', 'skipped', 0],
    ['cancelled', '', 'skipped', 1],
    ['success', '["rust"]', 'skipped', 1],
    ['success', '["rust"]', 'failure', 1],
    ['success', '["rust"]', 'cancelled', 1],
  ]) {
    const context = { needs: { changes: { outputs: { changes: languages } } } };
    const selected = (await evaluate(step.env.SELECTED, context)).coerceString();
    const run = runResult('codeql', step, { SELECTION_RESULT: selection, SELECTED: selected, JOB_RESULTS: analyze });
    assert.equal(run.status, status, `selection ${selection} ${languages}, analyze ${analyze}: ${run.stderr}`);
  }
});

test('automation changes exercise the selection tests, canary regression, and task syntax', () => {
  const workflow = load('.github/workflows/ci-automation.yml');
  const files = ['tests/github_actions/test_canary_is_branch_tip.py', '.github/workflows/ci-selection.yml', '.github/filters/codeql.yml', '.config/mise/tasks/rust/coverage'];
  for (const event of ['push', 'pull_request']) assert.equal(matches(workflow.on[event].paths, files), true);
  assert.equal(workflow.jobs.verify.steps.at(-1).run, 'mise run --skip-tools github:workflows:ci-automation:verify');
  const task = fs.readFileSync(path.join(root, '.config/mise/tasks/github/workflows/ci-automation/verify'), 'utf8');
  assert.match(task, /node --test tests\/github_actions\/test_ci_selection.mjs/);
  assert.match(task, /python3 -m unittest discover -s tests\/github_actions/);
  assert.match(task, /bash -n "\$task"/);
  assert.equal(JSON.parse(fs.readFileSync(new URL('./package.json', import.meta.url))).devDependencies.picomatch, '4.0.7');
});

test('no workflow reports the retired Rust, Go or Proto result checks', () => {
  for (const entry of fs.readdirSync(path.join(root, '.github/workflows'))) {
    for (const [id, job] of Object.entries(load(`.github/workflows/${entry}`).jobs)) {
      assert.ok(!['Rust result', 'Go result', 'Proto result'].includes(job.name), `${entry}/${id}`);
    }
  }
});

test('selection starts with approved actions and the repo-owned CLI', () => {
  const job = load('.github/workflows/ci-selection.yml').jobs.filter;
  const filter = job.steps.find((step) => step.id === 'filter');
  assert.equal(filter.uses, undefined);
  assert.equal(filter.run, 'mise run --skip-tools github:workflows:ci-selection:filter');
  assert.equal(filter.env.GH_TOKEN, '${{ github.token }}');
});

// Python tasks keep the language they were written in; their parsing logic has
// no reasonable bash equivalent.
const pythonTasks = new Set([
  '.config/mise/tasks/github/workflows/ci-bazel/coverage-report',
  '.config/mise/tasks/github/workflows/ci-bazel/coverage-context',
  '.config/mise/tasks/github/workflows/ci-bazel/coverage-fork',
  '.config/mise/tasks/github/workflows/ci-bazel-fork-coverage/compare',
  '.config/mise/tasks/github/workflows/ci-bazel-fork-coverage/publish',
  '.config/mise/tasks/github/workflows/ci-cargo/bazel-manual-tests',
  '.config/mise/tasks/github/workflows/ci-sandbox-ratchets/file-sizes',
  '.config/mise/tasks/github/workflows/ci-sandbox-ratchets/ignored-tests',
]);

function assertOwnedTask(run, kind, owner, where) {
  const pattern = new RegExp(`^mise run (?:--skip-tools )?(github:${kind}:${owner.replace(/[.-]/g, '\\$&')}:[a-z0-9-]+)$`);
  const match = run.match(pattern);
  assert.ok(match, `${where} must run one github:${kind}:${owner} task, got ${JSON.stringify(run)}`);
  const file = `.config/mise/tasks/${match[1].replaceAll(':', '/')}`;
  assert.ok(fs.existsSync(path.join(root, file)), `${where} runs missing task ${file}`);
  assert.notEqual(fs.statSync(path.join(root, file)).mode & 0o111, 0, `${file} is executable`);
  const source = fs.readFileSync(path.join(root, file), 'utf8');
  if (pythonTasks.has(file)) {
    assert.match(source, /^#!\/usr\/bin\/env python3\n#MISE description="/, file);
  } else {
    assert.match(source, /^#!\/usr\/bin\/env bash\n#MISE description="[^"]+"\n(?:#MISE .*\n)*set -euo pipefail\n/, file);
  }
}

test('workflow and action steps each run one task owned by that workflow or action', () => {
  for (const entry of fs.readdirSync(path.join(root, '.github/workflows'))) {
    const owner = entry.replace(/\.ya?ml$/, '');
    for (const [id, job] of Object.entries(load(`.github/workflows/${entry}`).jobs)) {
      for (const step of job.steps ?? []) {
        if (step.run !== undefined) assertOwnedTask(step.run, 'workflows', owner, `${entry}/${id}`);
      }
    }
  }
  for (const owner of fs.readdirSync(path.join(root, '.github/actions'))) {
    const action = load(`.github/actions/${owner}/action.yml`);
    for (const step of action.runs.steps ?? []) {
      if (step.run !== undefined) assertOwnedTask(step.run, 'actions', owner, `actions/${owner}`);
    }
  }
  assert.equal(fs.existsSync(path.join(root, '.config/mise/tasks/github-actions')), false);
});
