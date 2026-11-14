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
  rust: load('.github/workflows/ci-rust.yml'),
  go: load('.github/workflows/ci-go.yml'),
  proto: load('.github/workflows/ci-proto.yml'),
  codeql: load('.github/workflows/codeql.yml'),
};

function matches(patterns, files) {
  return patterns.flat(Infinity).some((pattern) => {
    const match = picomatch(pattern, { dot: true, flags: 's' });
    return files.some((file) => match(file));
  });
}

function selected(name, files, event) {
  const workflow = workflows[name];
  if (workflow.jobs.changes) {
    const filters = load(workflow.jobs.changes.with.filters);
    if (name === 'codeql') {
      return Object.keys(filters).filter((language) => matches(filters[language], files));
    }
    return matches(filters[name], files);
  }
  const trigger = workflow.on[event];
  const runs = trigger['paths-ignore']
    ? files.some((file) => !matches(trigger['paths-ignore'], [file]))
    : !trigger.paths || matches(trigger.paths, files);
  return name === 'codeql' ? (runs ? ['go', 'javascript-typescript', 'rust'] : []) : runs;
}

for (const event of ['pull_request', 'push']) {
  test(`${event}: canary fix plus proto configuration avoids unrelated suites`, () => {
    const files = [
      '.config/mise/tasks/github/workflows/canary-container-images/canary-is-branch-tip',
      'buf.gen.yaml',
      'tests/github_actions/test_canary_is_branch_tip.py',
    ];
    assert.equal(selected('rust', files, event), false);
    assert.equal(selected('go', files, event), false);
    assert.equal(selected('proto', files, event), true);
    assert.deepEqual(selected('codeql', files, event), []);
  });

  test(`${event}: canary helper and tests need only automation validation`, () => {
    const files = ['.config/mise/tasks/github/workflows/canary-container-images/canary-is-branch-tip', 'tests/github_actions/test_canary_is_branch_tip.py'];
    for (const suite of ['rust', 'go', 'proto']) assert.equal(selected(suite, files, event), false);
    assert.deepEqual(selected('codeql', files, event), []);
  });

  test(`${event}: workflow and selection maintenance remain lightweight`, () => {
    const files = [
      '.github/workflows/ci-rust.yml', '.github/workflows/ci-go.yml',
      '.github/workflows/ci-proto.yml', '.github/workflows/codeql.yml',
      '.github/workflows/ci-selection.yml', '.github/filters/ci.yml',
      '.github/filters/codeql.yml', 'tests/github_actions/test_ci_selection.mjs',
    ];
    for (const suite of ['rust', 'go', 'proto']) assert.equal(selected(suite, files, event), false);
    assert.deepEqual(selected('codeql', files, event), []);
  });
}

const rustInputs = [
  'rsworkspace/crates/platform/trogon-gateway/src/lib.rs',
  'rsworkspace/Cargo.lock', 'rsworkspace/rust-toolchain.toml',
  '.github/actions/setup-rust/action.yml', '.config/mise/config.toml', '.config/mise/mise.lock',
  '.config/mise/tasks/github/actions/setup-rust/rust-components',
  '.config/mise/tasks/github/workflows/ci-rust/dylint-tests',
  '.config/mise/tasks/github/workflows/ci-rust/result',
  '.config/mise/tasks/rust/coverage',
  '.config/mise/tasks/artifacts/wasm-components',
  '.config/mise/tasks/proto/schedules-descriptor-set',
  '.config/mise/tasks/rust/wasm-target', '.config/mise/lib/shared.sh',
  'proto/trogonai/scheduler/schedules/v1/schedule.proto', 'buf.yaml', 'buf.lock',
  'devops/docker/compose/compose.yml',
  'devops/docker/compose/services/nats/gateway-env.test.sh',
  'devops/docker/compose/services/a2a-gateway/config.json',
];
for (const file of rustInputs) test(`Rust dependency: ${file}`, () => assert.equal(selected('rust', [file], 'pull_request'), true));

for (const file of [
  'goworkspace/internal/protowit/generate.go', 'goworkspace/go.mod',
  '.config/mise/tasks/check/go', '.config/mise/tasks/test/go/race',
  '.config/mise/tasks/build/go', '.config/mise/lib/shared.sh',
  '.config/mise/tasks/github/workflows/ci-go/result',
  'proto/trogonai/scheduler/schedules/v1/schedule_completed.proto', 'buf.yaml', 'buf.lock',
  '.config/mise/config.toml', '.config/mise/mise.lock',
]) test(`Go dependency: ${file}`, () => assert.equal(selected('go', [file], 'pull_request'), true));

for (const file of [
  'buf.gen.yaml', 'proto/trogonai/policy/subjects/v1alpha1/subject.proto',
  '.config/mise/tasks/proto/generate', '.config/mise/tasks/proto/format/rust',
  '.config/mise/lib/proto-plugin', '.config/mise/tasks/github/workflows/ci-proto/assert-proto-generated',
  '.config/mise/tasks/github/workflows/ci-proto/assert-key-custody-proto-generated',
  'tests/proto/validate.py', 'tsworkspace/pnpm-lock.yaml',
  'rsworkspace/crates/platform/trogonai-proto/src/gen/mod.rs',
]) test(`Proto dependency: ${file}`, () => assert.equal(selected('proto', [file], 'pull_request'), true));

for (const [file, language] of [
  ['rsworkspace/crates/platform/trogon-gateway/src/lib.rs', 'rust'],
  ['rsworkspace/Cargo.lock', 'rust'],
  ['goworkspace/internal/protowit/generate.go', 'go'],
  ['tsworkspace/apps/console/src/index.ts', 'javascript-typescript'],
]) test(`CodeQL language: ${file}`, () => assert.deepEqual(selected('codeql', [file], 'pull_request'), [language]));

test('automation changes cannot hide generated Rust changes', () => {
  const files = ['.config/mise/tasks/github/workflows/canary-container-images/canary-is-branch-tip', 'rsworkspace/crates/platform/trogonai-proto/src/gen/mod.rs'];
  assert.equal(selected('rust', files, 'pull_request'), true);
  assert.equal(selected('proto', files, 'pull_request'), true);
  assert.deepEqual(selected('codeql', files, 'pull_request'), ['rust']);
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
        const outputs = { rust: 'false', go: 'false', proto: 'false', changes: '[]' };
        const context = { needs: { changes: { result, outputs } }, github: { event_name: 'pull_request' } };
        assert.equal((await evaluate(job.if, context)).coerceString(), String(result !== 'success'), `${suite}/${id}`);
        assert.equal((await evaluate(job.if, context, true)).coerceString(), 'false', `${suite}/${id} cancellation`);
        outputs[suite === 'codeql' ? 'changes' : suite] = suite === 'codeql' ? '["rust"]' : 'true';
        assert.equal((await evaluate(job.if, context)).coerceString(), 'true', `${suite}/${id} relevant change`);
      }
    }
  }
});

test('selection always runs with conservative fallback defaults', async () => {
  const job = load('.github/workflows/ci-selection.yml').jobs.filter;
  const filter = job.steps.find((step) => step.id === 'filter');
  assert.equal(filter.if, undefined);
  for (const name of ['rust', 'go', 'proto']) {
    assert.equal((await evaluate(job.outputs[name], { steps: { filter: { outputs: {} } } })).coerceString(), 'true');
    assert.equal((await evaluate(job.outputs[name], { steps: { filter: { outputs: { [name]: 'false' } } } })).coerceString(), 'false');
  }
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

test('Rust, Go and Proto results cover every job so branch rules can require them', () => {
  for (const [suite, name] of [['rust', 'Rust'], ['go', 'Go'], ['proto', 'Proto']]) {
    const { result, ...jobs } = workflows[suite].jobs;
    assert.equal(result.name, `${name} result`);
    assert.deepEqual(result.needs, Object.keys(jobs), `${suite} result needs every job`);
    assert.equal(result.if, '${{ always() }}');
    const step = result.steps.at(-1);
    assert.equal(step.env.SELECTION_RESULT, '${{ needs.changes.result }}');
    assert.equal(step.env.SELECTED, `\${{ needs.changes.outputs.${suite} }}`);
    for (const id of Object.keys(jobs).filter((id) => id !== 'changes')) {
      assert.ok(step.env.JOB_RESULTS.includes(`needs.${id}.result`), `${suite} result checks ${id}`);
    }
    for (const [selection, selected, results, status] of [
      ['success', 'true', 'success success', 0],
      ['failure', '', 'success success', 0],
      ['success', 'false', 'skipped skipped', 0],
      ['cancelled', '', 'skipped skipped', 1],
      ['success', 'true', 'success skipped', 1],
      ['success', 'true', 'success failure', 1],
      ['success', 'true', 'cancelled success', 1],
    ]) {
      const run = runResult(`ci-${suite}`, step, { SELECTION_RESULT: selection, SELECTED: selected, JOB_RESULTS: results });
      assert.equal(run.status, status, `${suite}: selection ${selection} ${selected}, jobs ${results}: ${run.stderr}`);
    }
  }
});

test('actual direct and transitive mise dependencies remain selected', () => {
  for (const [suite, name] of [['rust', 'ci-rust'], ['go', 'ci-go'], ['proto', 'ci-proto']]) {
    const documents = [fs.readFileSync(path.join(root, `.github/workflows/${name}.yml`), 'utf8')];
    if (suite === 'rust') documents.push(fs.readFileSync(path.join(root, '.github/actions/setup-rust/action.yml'), 'utf8'));
    const pending = documents.flatMap((source) => [...source.matchAll(/mise run(?: --[\w-]+)* ([\w:-]+)/g)].map((match) => match[1]));
    const seen = new Set();
    while (pending.length) {
      const name = pending.pop();
      if (seen.has(name)) continue;
      seen.add(name);
      const file = `.config/mise/tasks/${name.replaceAll(':', '/')}`;
      const source = fs.readFileSync(path.join(root, file), 'utf8');
      assert.equal(selected(suite, [file], 'pull_request'), true, `${suite} depends on ${file}`);
      for (const [, list] of source.matchAll(/^#MISE depends(?:_post)?=(\[.*\])/gm)) pending.push(...JSON.parse(list));
      for (const [, task] of source.matchAll(/mise run(?: --[\w-]+)* ([\w:-]+)/g)) pending.push(task);
      for (const [lib] of source.matchAll(/\.config\/mise\/lib\/[\w.-]+/g)) {
        assert.equal(selected(suite, [lib], 'pull_request'), true, `${suite} depends on ${lib}`);
      }
    }
  }
});

test('automation changes exercise the same filters, GitHub expressions, and canary regression', () => {
  const workflow = load('.github/workflows/ci-automation.yml');
  const files = ['tests/github_actions/test_canary_is_branch_tip.py', '.github/workflows/ci-rust.yml', '.github/filters/ci.yml'];
  for (const event of ['push', 'pull_request']) assert.equal(matches(workflow.on[event].paths, files), true);
  assert.equal(workflow.jobs.verify.steps.at(-1).run, 'mise run --skip-tools github:workflows:ci-automation:verify');
  const task = fs.readFileSync(path.join(root, '.config/mise/tasks/github/workflows/ci-automation/verify'), 'utf8');
  assert.match(task, /node --test tests\/github_actions\/test_ci_selection.mjs/);
  assert.match(task, /python3 -m unittest discover -s tests\/github_actions/);
  assert.match(task, /actionlint/);
  assert.equal(JSON.parse(fs.readFileSync(new URL('./package.json', import.meta.url))).devDependencies.picomatch, '4.0.7');
});

test('selection starts with approved actions and the repo-owned CLI', () => {
  const job = load('.github/workflows/ci-selection.yml').jobs.filter;
  const filter = job.steps.find((step) => step.id === 'filter');
  assert.equal(filter.uses, undefined);
  assert.equal(filter.run, 'mise run --skip-tools github:workflows:ci-selection:filter');
  assert.equal(filter.env.GH_TOKEN, '${{ github.token }}');
});

test('Rust heavy jobs read their own entry of the selected runners', async () => {
  const names = ['test', 'build', 'wasm-decider', 'lint'];
  assert.equal(workflows.rust.jobs.changes.with['runner-jobs'], names.join(' '));
  const labels = async (id, outputs) => {
    const inner = workflows.rust.jobs[id]['runs-on'].replace(/^\$\{\{\s*/, '').replace(/\s*\}\}$/, '');
    const context = { needs: { changes: { outputs } } };
    return JSON.parse((await evaluate(`toJSON(${inner})`, context)).coerceString());
  };
  const hosted = Object.fromEntries(names.map((name) => [name, ['ubuntu-latest']]));
  const mixed = { ...hosted, test: ['homelab'], build: ['homelab'] };
  for (const name of names) {
    assert.equal(workflows.rust.jobs[name].needs, 'changes');
    assert.doesNotMatch(workflows.rust.jobs[name]['runs-on'], /\|\|/);
    assert.deepEqual(await labels(name, { runners: JSON.stringify(hosted) }), ['ubuntu-latest'], name);
    assert.deepEqual(await labels(name, { runners: JSON.stringify(mixed) }), mixed[name], name);
  }
  for (const [suite, workflow] of Object.entries(workflows)) {
    if (suite === 'rust') continue;
    for (const [id, job] of Object.entries(workflow.jobs)) if (id !== 'changes') assert.equal(job['runs-on'], 'ubuntu-latest', `${suite}/${id}`);
  }
});

test('runner selection runs in its own job and fails loudly', async () => {
  const selection = load('.github/workflows/ci-selection.yml');
  const job = selection.jobs.runner;
  const step = job.steps.find((item) => item.id === 'runner');
  assert.equal(step.uses, './.github/actions/runs-on-fallback');
  assert.equal(step['continue-on-error'], undefined);
  assert.ok(job.steps[0].uses.startsWith('actions/checkout@'));
  assert.equal(job.steps.at(-1), step);
  assert.equal(step.with.scope, 'organization');
  assert.equal(step.with.jobs, '${{ inputs.runner-jobs }}');
  assert.equal(step.with['github-token'], '${{ steps.baton.outputs.token }}');
  assert.equal(selection.on.workflow_call.secrets, undefined);
  assert.equal(selection.on.workflow_call.inputs['runner-jobs'].default, '');
  assert.equal(selection.on.workflow_call.inputs['runner-jobs'].required, false);
  assert.equal(selection.on.workflow_call.outputs.runners.value, '${{ jobs.runner.outputs.runners }}');
  assert.equal(job.outputs.runners, '${{ steps.runner.outputs.runners }}');
  assert.equal(selection.jobs.filter.steps.some((item) => item.id === 'runner'), false);
  assert.equal(selection.jobs.filter.outputs.runners, undefined);
  for (const [suite, workflow] of Object.entries(workflows)) {
    // A caller below what any job of the called workflow requests fails to
    // start, even when that job would be skipped.
    assert.equal(workflow.jobs.changes.permissions['id-token'], 'write', suite);
    assert.equal(workflow.jobs.changes.secrets, undefined, suite);
  }
});

test('only main pushes and dispatches can read the Baton key', async () => {
  const job = load('.github/workflows/ci-selection.yml').jobs.runner;
  assert.equal(job.permissions['id-token'], 'write');
  const auth = job.steps.find((item) => item.id === 'gcp');
  const cases = [
    ['push', 'refs/heads/main', true],
    ['workflow_dispatch', 'refs/heads/main', true],
    ['pull_request', 'refs/pull/1/merge', false],
    ['pull_request_target', 'refs/heads/main', false],
    ['merge_group', 'refs/heads/gh-readonly-queue/main/pr-1', false],
    ['push', 'refs/heads/feature', false],
  ];
  for (const [event_name, ref, trusted] of cases) {
    const context = { github: { event_name, ref } };
    assert.equal((await evaluate(job.environment, context)).coerceString(), trusted ? 'github-app-baton' : '', `${event_name} ${ref}`);
    assert.equal((await evaluate(auth.if, context)).coerceString(), String(trusted), `${event_name} ${ref}`);
  }
  for (const id of ['gcp', 'baton_key', 'baton']) {
    assert.equal(job.steps.find((item) => item.id === id)['continue-on-error'], true, id);
  }
  const token = job.steps.find((item) => item.id === 'baton');
  assert.equal(token.with.owner, '${{ github.repository_owner }}');
  assert.equal(token.with['permission-organization-self-hosted-runners'], 'read');
  assert.deepEqual(Object.keys(token.with).filter((key) => key.startsWith('permission-')), ['permission-organization-self-hosted-runners']);
});

test('actionlint installs through the supported mise backend', () => {
  const job = load('.github/workflows/ci-automation.yml').jobs.verify;
  const installer = job.steps.find((step) => step.uses?.startsWith('jdx/mise-action@'));
  assert.match(installer.with.install_args, /github:rhysd\/actionlint/);
  const tools = fs.readFileSync(path.join(root, '.config/mise/config.toml'), 'utf8');
  assert.match(tools, /^"github:rhysd\/actionlint" = "\d+\.\d+\.\d+"$/m);
  assert.equal(job.steps.some((step) => step.uses?.startsWith('taiki-e/install-action@')), false);
});

// Python tasks keep the language they were written in; their parsing logic has
// no reasonable bash equivalent.
const pythonTasks = new Set([
  '.config/mise/tasks/github/workflows/ci-adr/lint-adr-refs',
  '.config/mise/tasks/github/workflows/ci-rust/decider-test-suites',
  '.config/mise/tasks/github/workflows/ci-rust/enforce-cargo-license',
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
