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
      '.config/mise/tasks/github-actions/canary-is-branch-tip',
      'buf.gen.yaml',
      'tests/github_actions/test_canary_is_branch_tip.py',
    ];
    assert.equal(selected('rust', files, event), false);
    assert.equal(selected('go', files, event), false);
    assert.equal(selected('proto', files, event), true);
    assert.deepEqual(selected('codeql', files, event), []);
  });

  test(`${event}: canary helper and tests need only automation validation`, () => {
    const files = ['.config/mise/tasks/github-actions/canary-is-branch-tip', 'tests/github_actions/test_canary_is_branch_tip.py'];
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
  '.config/mise/tasks/github-actions/rust-components',
  '.config/mise/tasks/github-actions/dylint-tests',
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
  'proto/trogonai/scheduler/schedules/v1/schedule_completed.proto', 'buf.yaml', 'buf.lock',
  '.config/mise/config.toml', '.config/mise/mise.lock',
]) test(`Go dependency: ${file}`, () => assert.equal(selected('go', [file], 'pull_request'), true));

for (const file of [
  'buf.gen.yaml', 'proto/trogonai/policy/subjects/v1alpha1/subject.proto',
  '.config/mise/tasks/proto/generate', '.config/mise/tasks/proto/format/rust',
  '.config/mise/lib/proto-plugin', '.config/mise/tasks/github-actions/assert-proto-generated',
  '.config/mise/tasks/github-actions/assert-key-custody-proto-generated',
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
  const files = ['.config/mise/tasks/github-actions/canary-is-branch-tip', 'rsworkspace/crates/platform/trogonai-proto/src/gen/mod.rs'];
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
      if (id === 'changes' || (suite === 'codeql' && id === 'result')) continue;
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

test('empty CodeQL matrices are skipped and selected build modes are preserved', async () => {
  const job = workflows.codeql.jobs.analyze;
  const init = job.steps.find((step) => step.name === 'Initialize CodeQL');
  assert.equal(init.with.queries, 'security-and-quality');
  assert.equal(job.steps.find((step) => step.name === 'Perform CodeQL analysis').with.category, '/language:${{ matrix.language }}');
  for (const [language, expected] of [['go', 'autobuild'], ['rust', 'none'], ['javascript-typescript', 'none']]) {
    assert.equal((await evaluate(init.with['build-mode'], { matrix: { language } })).coerceString(), expected);
  }
  const value = await evaluate(job.strategy.matrix.language, { needs: { changes: { outputs: { changes: '' } } } });
  assert.deepEqual(value.values().map((item) => item.coerceString()), ['go', 'javascript-typescript', 'rust']);
});

test('CodeQL result reports every outcome so branch rules can require it', () => {
  const job = workflows.codeql.jobs.result;
  assert.equal(job.name, 'CodeQL result');
  assert.deepEqual(job.needs, ['changes', 'analyze']);
  assert.equal(job.if, '${{ always() }}');
  const step = job.steps.at(-1);
  assert.deepEqual(step.env, {
    SELECTION_RESULT: '${{ needs.changes.result }}',
    SELECTED_LANGUAGES: '${{ needs.changes.outputs.changes }}',
    ANALYZE_RESULT: '${{ needs.analyze.result }}',
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
    const env = { ...process.env, SELECTION_RESULT: selection, SELECTED_LANGUAGES: languages, ANALYZE_RESULT: analyze };
    const run = spawnSync('bash', ['-c', step.run], { env, encoding: 'utf8' });
    assert.equal(run.status, status, `selection ${selection} ${languages}, analyze ${analyze}`);
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
  assert.equal(filter.run, 'mise exec node@26.3.0 -- mise run --skip-tools github:workflows:ci-selection:filter');
  assert.equal(filter.env.GH_TOKEN, '${{ github.token }}');
});

test('actionlint installs through the supported mise backend', () => {
  const job = load('.github/workflows/ci-automation.yml').jobs.verify;
  const installer = job.steps.find((step) => step.uses?.startsWith('jdx/mise-action@'));
  assert.match(installer.with.install_args, /github:rhysd\/actionlint/);
  const tools = fs.readFileSync(path.join(root, '.config/mise/config.toml'), 'utf8');
  assert.match(tools, /^"github:rhysd\/actionlint" = "\d+\.\d+\.\d+"$/m);
  assert.equal(job.steps.some((step) => step.uses?.startsWith('taiki-e/install-action@')), false);
});
