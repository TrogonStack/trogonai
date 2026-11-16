import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { spawnSync } from 'node:child_process';
import yaml from 'js-yaml';
import { Lexer, Parser, Evaluator, data } from '@actions/expressions';

const root = fileURLToPath(new URL('../../', import.meta.url));
const workflows = fs.readdirSync(path.join(root, '.github/workflows'))
  .filter((file) => file.endsWith('.yml'))
  .map((file) => yaml.load(fs.readFileSync(path.join(root, '.github/workflows', file), 'utf8')));
const bazel = workflows.find((workflow) => workflow.name === 'CI Bazel');
const continuations = workflows.filter((workflow) => workflow.on.workflow_run?.workflows?.includes(bazel.name));

function evaluate(value, context) {
  if (typeof value !== 'string' || !value.startsWith('${{')) return value;
  const expression = new Parser(new Lexer(value.slice(3, -2)).lex().tokens, Object.keys(context), []).parse();
  const convert = (v) => {
    if (v === null) return new data.Null();
    if (Array.isArray(v)) return new data.Array(...v.map(convert));
    if (typeof v === 'object') return new data.Dictionary(...Object.entries(v).map(([key, val]) => ({ key, value: convert(val) })));
    if (typeof v === 'boolean') return new data.BooleanData(v);
    return new data.StringData(v);
  };
  return new Evaluator(expression, convert(context)).evaluate().coerceString();
}

test('a fork above 95% still reaches a baseline comparison and trusted publication', () => {
  const context = {
    github: { event_name: 'pull_request', repository: 'example/project', event: { pull_request: { head: { repo: { full_name: 'contributor/project' } } } } },
    steps: { baseline: { outputs: { ready: 'false' } } },
  };
  context.env = { COVERAGE_FORK: evaluate(bazel.jobs.coverage.env.COVERAGE_FORK, context) };
  const action = bazel.jobs.coverage.steps.find((step) => step.uses?.startsWith('TrogonStack/github-actions/actions/coverage-report@'));
  assert.equal(action.with.threshold, 95);
  const directDiff = evaluate(action.with.diff, context) === 'true';
  const directPublish = evaluate(action.with.publish, context) === 'true';
  // A 96% report with new misses passes the threshold. The workflow contract
  // must schedule the missing comparison even though the fork token cannot write.
  assert.ok(directDiff || continuations.some((workflow) => workflow.jobs.compare),
    '96% fork coverage with new misses passes: no baseline comparison is scheduled');
  assert.ok(directPublish || continuations.some((workflow) => workflow.jobs.publish),
    'fork coverage has no trusted publishing continuation');
});

test('the fork continuation separates read-only comparison from comment authority', () => {
  assert.equal(continuations.length, 1, 'expected an unambiguous CI Bazel continuation');
  const workflow = continuations[0];
  assert.deepEqual(workflow.on.workflow_run.types, ['completed']);
  assert.deepEqual(workflow.permissions, {});
  assert.deepEqual(workflow.jobs.compare.permissions, { actions: 'read', contents: 'read', 'pull-requests': 'read' });
  assert.deepEqual(workflow.jobs.publish.permissions, { actions: 'read', contents: 'read', 'pull-requests': 'write' });
  assert.equal(workflow.jobs.publish.needs, 'compare');
  for (const job of Object.values(workflow.jobs)) {
    const checkout = job.steps.find((step) => step.uses?.startsWith('actions/checkout@') && !step.with?.repository);
    assert.equal(checkout.with.ref, '${{ github.workflow_sha }}');
    assert.equal(checkout.with['persist-credentials'], false);
    assert.ok(!job.steps.some((step) => step.uses?.startsWith('actions/download-artifact@')));
    assert.ok(!Object.values(job.permissions).includes('write') || !job.steps.some((step) => step.name === 'Checkout coverage tools'));
  }
});

test('the original fork check gates Bazel result with read-only comparison', () => {
  const fork = bazel.jobs['fork-coverage'];
  assert.ok(fork, 'Bazel result still accepts a fork after checking only the threshold');
  assert.deepEqual(fork.permissions, { contents: 'read', 'pull-requests': 'read' });
  assert.ok(fork.steps.some((step) => step.run === 'mise run --skip-tools github:workflows:ci-bazel:coverage-fork'));
  assert.ok(bazel.jobs.result.needs.includes('fork-coverage'));
  const results = bazel.jobs.result.steps.find((step) => step.env?.JOB_RESULTS).env.JOB_RESULTS;
  assert.match(results, /needs\['fork-coverage'\]\.result/);
});

test('Bazel result requires the selected coverage job and refuses failure or cancellation', () => {
  const template = bazel.jobs.result.steps.find((step) => step.env?.JOB_RESULTS).env.JOB_RESULTS;
  for (const fork of [true, false]) {
    for (const outcome of ['success', 'failure', 'cancelled', 'skipped']) {
      const context = {
        github: { event_name: 'pull_request', repository: 'example/project', event: { pull_request: { head: { repo: { full_name: fork ? 'contributor/project' : 'example/project' } } } } },
        needs: { verify: { result: 'success' }, coverage: { result: fork ? 'skipped' : outcome }, 'fork-coverage': { result: fork ? outcome : 'skipped' } },
      };
      assert.equal(evaluate(bazel.jobs.coverage.if, context), String(!fork));
      assert.equal(evaluate(bazel.jobs['fork-coverage'].if, context), String(fork));
      const results = template.replace(/\$\{\{[\s\S]*?\}\}/g, (expression) => evaluate(expression, context));
      const result = spawnSync(path.join(root, '.config/mise/tasks/github/workflows/ci-bazel/result'), [], {
        cwd: root, encoding: 'utf8', env: { ...process.env, MISE_PROJECT_ROOT: root, SELECTION_RESULT: 'success', SELECTED: 'true', JOB_RESULTS: results },
      });
      assert.equal(result.status === 0, outcome === 'success', `${fork}: ${outcome}: ${result.stderr}`);
    }
  }
});

test('coverage producers, consumers and helper dependencies remain aligned', () => {
  const producer = bazel.jobs.verify.steps.find((step) => step.name === 'Upload coverage report');
  assert.equal(producer.with.name, 'bazel-coverage-${{ github.run_attempt }}');
  assert.match(producer.with.path, /coverage-context\.json/);
  for (const name of ['coverage', 'fork-coverage']) {
    const download = bazel.jobs[name].steps.find((step) => step.uses?.startsWith('actions/download-artifact@'));
    assert.equal(download.with.name, producer.with.name);
  }
  const action = bazel.jobs.coverage.steps.find((step) => step.uses?.startsWith('TrogonStack/github-actions/actions/coverage-report@'));
  const pin = action.uses.split('@')[1];
  const jobs = [bazel.jobs.coverage, bazel.jobs['fork-coverage'], continuations[0].jobs.compare,
    workflows.find((workflow) => workflow.name === 'CI Automation').jobs.verify];
  for (const job of jobs) {
    const checkout = job.steps.find((step) => step.with?.repository === 'TrogonStack/github-actions');
    assert.equal(checkout.with.ref, pin);
    for (const step of job.steps.filter((step) => step.uses)) assert.match(step.uses, /@[0-9a-f]{40}$/);
  }
});
