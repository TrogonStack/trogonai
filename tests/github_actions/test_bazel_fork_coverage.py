"""Exercise the workflow task CLIs with real Git blobs and the pinned coverage helper."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import zipfile
from xml.etree import ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
TASKS = ROOT / '.config/mise/tasks/github/workflows/ci-bazel-fork-coverage'
TOOLS = Path(os.environ.get('COVERAGE_TOOLS', ROOT / '.coverage-tools/actions/coverage-report')).resolve()


class ForkCoverageTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not all((TOOLS / name).is_file() for name in ('report.py', 'sources.py', 'diagnostics.py')):
            raise RuntimeError('Set COVERAGE_TOOLS to the pinned shared coverage helper checkout')

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='fork-coverage-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.env = {key: value for key, value in os.environ.items() if not key.startswith(('GIT_', 'GITHUB_', 'COVERAGE_'))}
        self.env.update(GITHUB_REPOSITORY='example/project', GITHUB_REPOSITORY_ID='1',
                        GITHUB_RUN_ID='200', GITHUB_RUN_ATTEMPT='1', GITHUB_EVENT_NAME='workflow_run',
                        GITHUB_WORKSPACE=str(self.root), MISE_PROJECT_ROOT=str(ROOT), COVERAGE_TOOLS=str(TOOLS),
                        GITHUB_EVENT_PATH=str(self.root / 'event.json'), GITHUB_OUTPUT=str(self.root / 'output'),
                        GITHUB_STEP_SUMMARY=str(self.root / 'summary'))
        self.git('init', '--quiet', '-b', 'main')
        self.write('rsworkspace/src/lib.rs', ''.join(f'pub fn f{i}() {{}}\n' for i in range(100)))
        self.write('rsworkspace/src/other.rs', ''.join(f'pub fn other{i}() {{}}\n' for i in range(100)))
        self.base = self.commit()
        self.write('rsworkspace/src/lib.rs', ''.join(f'pub fn f{i}() {{ let _x = 1; }}\n' for i in range(100)))
        self.head = self.commit()
        self.merge = self.git('commit-tree', self.git('rev-parse', 'HEAD^{tree}'), '-p', self.base, '-p', self.head, '-m', 'merge fixture')
        self.git('checkout', '--quiet', self.base)
        self.repo = {'id': 1, 'full_name': 'example/project'}
        self.fork = {'id': 2, 'full_name': 'contributor/project'}
        self.pr = {'id': 70, 'number': 7, 'state': 'open', 'merged': False, 'merge_commit_sha': self.merge,
                   'head': {'sha': self.head, 'ref': 'change', 'repo': self.fork},
                   'base': {'sha': self.base, 'ref': 'main', 'repo': self.repo}, 'labels': []}
        self.run = {'id': 100, 'run_attempt': 1, 'run_number': 10, 'workflow_id': 3,
                    'path': '.github/workflows/ci-bazel.yml', 'name': 'CI Bazel', 'event': 'pull_request',
                    'status': 'completed', 'conclusion': 'success', 'head_sha': self.head,
                    'head_branch': 'change', 'repository': self.repo, 'head_repository': self.fork,
                    'pull_requests': []}
        self.context = {'version': 1, 'repository': 'example/project', 'repository_id': 1,
                        'run_id': 100, 'run_attempt': 1, 'pr': 7, 'head_sha': self.head,
                        'base_sha': self.base, 'base_ref': 'main', 'measured_sha': self.merge}
        self.artifact = {'id': 90, 'name': 'bazel-coverage-1', 'expired': False, 'size_in_bytes': 4096,
                         'workflow_run': {'id': 100, 'repository_id': 1, 'head_repository_id': 2,
                                          'head_sha': self.head, 'head_branch': 'change'}}
        self.responses = {}
        self.write('bin/gh', '#!' + sys.executable + '\n' + 'fixture_root = ' + repr(str(self.root)) + '\n' + '''import json, pathlib, sys
p = pathlib.Path(fixture_root)
args = sys.argv[1:]
calls = json.loads((p / 'calls.json').read_text())
calls.append(args)
(p / 'calls.json').write_text(json.dumps(calls))
endpoint = args[-1]
routes = json.loads((p / 'routes.json').read_text())
route = routes.get(endpoint)
if route is None:
    sys.stderr.write('unexpected fixture endpoint')
    sys.exit(1)
if '--method' in args and args[args.index('--method') + 1] in ('POST', 'PATCH'):
    (p / 'request.json').write_text(sys.stdin.read())
if isinstance(route, dict) and '$sequence' in route:
    index = sum(c[-1] == endpoint for c in calls) - 1
    route = route['$sequence'][min(index, len(route['$sequence']) - 1)]
if isinstance(route, dict) and '$file' in route:
    sys.stdout.buffer.write((p / route['$file']).read_bytes())
elif isinstance(route, dict) and '$error' in route:
    sys.exit(1)
else:
    sys.stdout.write(json.dumps(route))
''')
        (self.root / 'bin/gh').chmod(0o755)
        self.env['PATH'] = str(self.root / 'bin') + os.pathsep + self.env['PATH']
        self.baseline()
        self.archive()

    def write(self, name, value):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(value)

    def git(self, *args):
        env = {**self.env, 'GIT_AUTHOR_NAME': 'Fixture', 'GIT_AUTHOR_EMAIL': 'fixture',
               'GIT_COMMITTER_NAME': 'Fixture', 'GIT_COMMITTER_EMAIL': 'fixture'}
        return subprocess.check_output(['git', '-c', 'core.hooksPath=/dev/null', '-c', 'commit.gpgsign=false', *args], cwd=self.root, env=env, text=True).strip()

    def commit(self):
        self.git('add', '--all')
        self.git('commit', '--quiet', '-m', 'fixture')
        return self.git('rev-parse', 'HEAD')

    def xml(self, misses=0, lines=100, filename='src/lib.rs'):
        entries = ''.join(f'<line number="{i}" hits="{int(i > misses)}"/>' for i in range(1, lines + 1))
        return f'<coverage lines-valid="{lines}" lines-covered="{lines - misses}" line-rate="{(lines - misses) / lines}"><packages><package name="src"><classes><class filename="{filename}" name="lib" line-rate="{(lines - misses) / lines}"><lines>{entries}</lines></class></classes></package></packages></coverage>'

    def baseline(self, xml=None, commit=None):
        self.write('old.xml', xml or self.xml())
        code = "import sys; from pathlib import Path; sys.path.insert(0, sys.argv[1]); from sources import make_pair, GitObjects, xml_tree; tree = xml_tree(Path('old.xml').read_bytes()); make_pair('old.xml', 'baseline/sources.json', 'example/project', GitObjects(Path('.'), sys.argv[2]), {node.attrib['filename']: 'rsworkspace/' + node.attrib['filename'] for node in tree.findall('.//class')})"
        subprocess.check_call([sys.executable, '-c', code, str(TOOLS), commit or self.base], cwd=self.root, env=self.env)

    def aggregate_xml(self, first, other):
        root = ET.fromstring(self.xml(misses=first))
        root.find('.//classes').append(ET.fromstring(self.xml(misses=other, filename='src/other.rs')).find('.//class'))
        root.attrib.update({'lines-valid': '200', 'lines-covered': str(200 - first - other), 'line-rate': str((200 - first - other) / 200)})
        return ET.tostring(root, encoding='unicode')

    def archive(self, misses=4, files=None):
        files = files or {'coverage-bazel.xml': self.xml(misses), 'coverage-context.json': json.dumps(self.context)}
        with zipfile.ZipFile(self.root / 'artifact.zip', 'w', zipfile.ZIP_DEFLATED) as archive:
            for name, value in files.items():
                archive.writestr(name, value)
        data = (self.root / 'artifact.zip').read_bytes()
        self.artifact['digest'] = 'sha256:' + hashlib.sha256(data).hexdigest()

    def routes(self):
        prefix = 'repos/example/project'
        return {
            prefix + '/actions/workflows/ci-bazel.yml': {'id': 3, 'path': '.github/workflows/ci-bazel.yml', 'name': 'CI Bazel'},
            prefix + '/actions/runs/100': self.run,
            prefix + '/actions/runs/100/attempts/1': self.run,
            prefix + '/actions/runs/200': {'id': 200, 'run_attempt': 1, 'repository': self.repo,
                                         'path': '.github/workflows/ci-bazel-fork-coverage.yml', 'event': 'workflow_run', 'status': 'in_progress'},
            prefix + '/actions/runs/100/attempts/1/jobs?per_page=100&page=1': {'total_count': 1, 'jobs': [{'id': 99, 'name': 'verify', 'conclusion': 'success', 'status': 'completed'}]},
            prefix + f'/commits/{self.head}/pulls?per_page=100&page=1': [self.pr],
            prefix + '/pulls/7': self.pr,
            prefix + f'/actions/workflows/3/runs?event=pull_request&head_sha={self.head}&per_page=100&page=1': {'total_count': 1, 'workflow_runs': [self.run]},
            prefix + f'/git/commits/{self.merge}': {'sha': self.merge, 'parents': [{'sha': self.base}, {'sha': self.head}]},
            prefix + '/actions/runs/100/artifacts?per_page=100&page=1': {'total_count': 1, 'artifacts': [self.artifact]},
            prefix + '/actions/artifacts/90/zip': {'$file': 'artifact.zip'},
            prefix + '/git/ref/heads/_xml_coverage_reports_bazel': {'object': {'sha': self.base, 'type': 'commit'}},
            prefix + f'/contents/data/main/coverage.xml?ref={self.base}': {'$file': 'baseline/coverage.xml'},
            prefix + f'/contents/data/main/sources.json?ref={self.base}': {'$file': 'baseline/sources.json'},
            prefix + '/issues/7/comments?per_page=100&page=1': [],
            prefix + '/issues/7/comments': {'id': 500},
        }

    def invoke(self, task='compare', overrides=None, comparison=None):
        event = {'action': 'completed', 'repository': self.repo, 'workflow_run': self.run}
        self.write('event.json', json.dumps(event))
        self.write('routes.json', json.dumps({**self.routes(), **(overrides or {})}))
        self.write('calls.json', '[]')
        self.write('output', '')
        env = dict(self.env)
        if comparison is not None:
            env['COVERAGE_COMPARISON'] = json.dumps(comparison)
        result = subprocess.run([sys.executable, str(TASKS / task)], cwd=self.root, env=env, capture_output=True, text=True)
        result.calls = json.loads((self.root / 'calls.json').read_text())
        result.outputs = dict(line.split('=', 1) for line in (self.root / 'output').read_text().splitlines())
        result.comparison = json.loads(result.outputs['comparison']) if 'comparison' in result.outputs else None
        return result

    def no_writes(self, result):
        self.assertFalse(any('--method' in call and call[call.index('--method') + 1] != 'GET' for call in result.calls))

    def refused(self, result):
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIsNone(result.comparison)
        self.no_writes(result)

    def test_new_misses_above_threshold_fail_and_publish_only_in_separate_task(self):
        result = self.invoke()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIsNotNone(result.comparison, result.stderr)
        self.assertFalse(result.comparison['policy_passed'])
        self.assertEqual(result.comparison['current'], {'statements': 100, 'misses': 4})
        self.no_writes(result)
        published = self.invoke('publish', comparison=result.comparison)
        self.assertEqual(published.returncode, 0, published.stderr)
        body = json.loads((self.root / 'request.json').read_text())['body']
        self.assertIn('96.00000000%', body)
        self.assertIn(self.head, body)
        self.assertIn('FAIL', body)

    def test_covered_change_passes_without_remote_writes(self):
        self.archive(misses=0)
        result = self.invoke()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(result.comparison['policy_passed'])
        self.no_writes(result)
        self.assertEqual(self.git('rev-parse', 'HEAD'), self.base)

    def local_gate(self, misses=4, overrides=None):
        self.write('event.json', json.dumps({'repository': self.repo, 'number': 7, 'pull_request': self.pr}))
        self.write('routes.json', json.dumps({**self.routes(), **(overrides or {})}))
        self.write('calls.json', '[]')
        self.write('rsworkspace/coverage-bazel.xml', self.xml(misses))
        env = {**self.env, 'GITHUB_EVENT_NAME': 'pull_request', 'GITHUB_SHA': self.merge, 'GITHUB_RUN_ID': '100'}
        result = subprocess.run([sys.executable, str(ROOT / '.config/mise/tasks/github/workflows/ci-bazel/coverage-fork')],
                                cwd=self.root, env=env, capture_output=True, text=True)
        result.calls = json.loads((self.root / 'calls.json').read_text())
        return result

    def test_original_fork_cli_gates_new_misses_and_accepts_a_covered_change(self):
        regression = self.local_gate()
        self.assertEqual(regression.returncode, 1, regression.stderr)
        self.assertIn('FAIL', (self.root / 'summary').read_text())
        self.no_writes(regression)
        covered = self.local_gate(misses=0)
        self.assertEqual(covered.returncode, 0, covered.stderr)
        self.no_writes(covered)
        self.assertEqual(self.git('rev-parse', 'HEAD'), self.base)

    def test_producer_context_cli_records_the_actual_merge_and_attempt(self):
        self.write('event.json', json.dumps({'repository': self.repo, 'number': 7, 'pull_request': self.pr}))
        env = {**self.env, 'MISE_PROJECT_ROOT': str(self.root), 'GITHUB_SHA': self.merge, 'GITHUB_RUN_ID': '100'}
        result = subprocess.run([sys.executable, str(ROOT / '.config/mise/tasks/github/workflows/ci-bazel/coverage-context')],
                                cwd=self.root, env=env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads((self.root / 'rsworkspace/coverage-context.json').read_text()), self.context)

    def test_equal_misses_on_changed_source_still_reject_new_uncovered_lines(self):
        self.baseline(self.xml(misses=4))
        result = self.invoke()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(result.comparison['diff_status'], 3)
        self.assertEqual(result.comparison['current']['misses'], result.comparison['baseline']['misses'])

    def test_rate_reduction_fails_even_when_diff_finds_no_new_misses(self):
        self.head = self.git('commit-tree', self.git('rev-parse', self.base + '^{tree}'), '-p', self.base, '-m', 'unchanged source fixture')
        self.merge = self.git('commit-tree', self.git('rev-parse', self.base + '^{tree}'), '-p', self.base, '-p', self.head, '-m', 'merge fixture')
        self.pr['head']['sha'] = self.run['head_sha'] = self.context['head_sha'] = self.artifact['workflow_run']['head_sha'] = self.head
        self.pr['merge_commit_sha'] = self.context['measured_sha'] = self.merge
        self.baseline(self.xml(misses=4))
        self.archive(files={'coverage-bazel.xml': self.xml(misses=4, lines=99), 'coverage-context.json': json.dumps(self.context)})
        result = self.invoke()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(result.comparison['diff_status'], 0)
        self.assertFalse(result.comparison['policy_passed'])

    def test_aggregate_improvement_does_not_hide_a_per_file_regression(self):
        self.baseline(self.aggregate_xml(0, 10))
        self.archive(files={'coverage-bazel.xml': self.aggregate_xml(4, 0), 'coverage-context.json': json.dumps(self.context)})
        result = self.invoke()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(result.comparison['diff_status'], 2)
        self.assertLess(result.comparison['current']['misses'], result.comparison['baseline']['misses'])

    def test_failed_source_coverage_can_be_published_but_failed_verify_cannot(self):
        self.run['conclusion'] = 'failure'
        comparison = self.invoke()
        self.assertIsNotNone(comparison.comparison, comparison.stderr)
        published = self.invoke('publish', comparison=comparison.comparison)
        self.assertEqual(published.returncode, 0, published.stderr)
        endpoint = 'repos/example/project/actions/runs/100/attempts/1/jobs?per_page=100&page=1'
        self.refused(self.invoke(overrides={endpoint: {'total_count': 1, 'jobs': [{'id': 99, 'name': 'verify', 'status': 'completed', 'conclusion': 'failure'}]}}))

    def test_reset_label_waives_diff_policy_but_never_threshold(self):
        self.pr['labels'] = [{'name': 'rust:coverage-baseline-reset'}]
        self.assertEqual(self.invoke().returncode, 0)
        self.archive(misses=6)
        result = self.invoke()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIsNotNone(result.comparison)
        self.assertFalse(result.comparison['policy_passed'])

    def test_wrong_repository_workflow_attempt_head_and_closed_pr_fail_closed(self):
        prefix = 'repos/example/project'
        for field, value in [('workflow_id', 4), ('path', '.github/workflows/other.yml'),
                             ('run_attempt', 2), ('head_sha', 'f' * 40), ('conclusion', 'cancelled')]:
            with self.subTest(field=field):
                changed = {**self.run, field: value}
                self.refused(self.invoke(overrides={prefix + '/actions/runs/100': changed}))
        for patch in [{'state': 'closed'}, {'head': {**self.pr['head'], 'sha': 'f' * 40}},
                      {'head': {**self.pr['head'], 'repo': {'id': 9, 'full_name': 'stranger/project'}}},
                      {'base': {**self.pr['base'], 'repo': {'id': 9, 'full_name': 'stranger/project'}}}]:
            self.refused(self.invoke(overrides={prefix + '/pulls/7': {**self.pr, **patch}}))

    def test_ambiguous_association_duplicate_artifacts_and_newer_runs_fail_closed(self):
        prefix = 'repos/example/project'
        endpoints = {
            prefix + f'/commits/{self.head}/pulls?per_page=100&page=1': [self.pr, {**self.pr, 'number': 8, 'id': 80}],
            prefix + '/actions/runs/100/artifacts?per_page=100&page=1': {'total_count': 2, 'artifacts': [self.artifact, {**self.artifact, 'id': 91}]},
            prefix + f'/actions/workflows/3/runs?event=pull_request&head_sha={self.head}&per_page=100&page=1': {'total_count': 2, 'workflow_runs': [self.run, {**self.run, 'id': 101, 'run_number': 11, 'status': 'in_progress'}]},
        }
        for endpoint, value in endpoints.items():
            with self.subTest(endpoint=endpoint):
                self.refused(self.invoke(overrides={endpoint: value}))

    def test_archive_paths_context_entities_and_manifest_tampering_fail_closed(self):
        for files in [
            {'../coverage-bazel.xml': self.xml(), 'coverage-context.json': json.dumps(self.context)},
            {'coverage-bazel.xml': self.xml(), 'coverage-context.json': json.dumps({**self.context, 'run_attempt': 2})},
            {'coverage-bazel.xml': '<!DOCTYPE coverage [<!ENTITY x "bad">]><coverage>&x;</coverage>', 'coverage-context.json': json.dumps(self.context)},
            {'coverage-bazel.xml': self.xml(), 'coverage-context.json': json.dumps({**self.context, 'command': 'anything'})},
            {'coverage-bazel.xml': self.xml(), 'coverage-context.json': json.dumps({**self.context, 'version': True})},
            {'coverage-bazel.xml': self.xml(), 'coverage-context.json': json.dumps(self.context).replace('"version": 1', '"version": 1, "version": 1')},
            {'coverage-bazel.xml': self.xml(lines=101), 'coverage-context.json': json.dumps(self.context)},
            {'coverage-bazel.xml': self.xml().replace('src/lib.rs', '../outside'), 'coverage-context.json': json.dumps(self.context)},
        ]:
            self.archive(files=files)
            self.refused(self.invoke())
        self.archive()
        manifest = json.loads((self.root / 'baseline/sources.json').read_text())
        manifest['sources']['rsworkspace/src/lib.rs']['sha256'] = '0' * 64
        self.write('baseline/sources.json', json.dumps(manifest))
        self.refused(self.invoke())

    def test_missing_baseline_api_failures_digest_mismatch_and_expiration_fail_closed(self):
        prefix = 'repos/example/project'
        self.refused(self.invoke(overrides={prefix + '/git/ref/heads/_xml_coverage_reports_bazel': {'$error': True}}))
        for patch in [{'digest': 'sha256:' + '0' * 64}, {'expired': True}, {'size_in_bytes': 128 * 1024 * 1024},
                      {'workflow_run': {**self.artifact['workflow_run'], 'head_repository_id': 9}}]:
            artifact = {**self.artifact, **patch}
            self.refused(self.invoke(overrides={prefix + '/actions/runs/100/artifacts?per_page=100&page=1': {'total_count': 1, 'artifacts': [artifact]}}))

    def test_a_valid_manifest_from_an_unrelated_source_commit_is_not_a_main_baseline(self):
        unrelated = self.git('commit-tree', self.git('rev-parse', self.base + '^{tree}'), '-m', 'unrelated fixture')
        self.baseline(commit=unrelated)
        self.refused(self.invoke())

    def test_symlink_and_duplicate_zip_entries_are_never_extracted(self):
        for symlink in (True, False):
            with zipfile.ZipFile(self.root / 'artifact.zip', 'w') as archive:
                info = zipfile.ZipInfo('coverage-bazel.xml')
                if symlink:
                    info.create_system = 3
                    info.external_attr = 0o120777 << 16
                archive.writestr(info, self.xml())
                archive.writestr('coverage-context.json', json.dumps(self.context))
                if not symlink:
                    archive.writestr('coverage-bazel.xml', self.xml())
            self.artifact['digest'] = 'sha256:' + hashlib.sha256((self.root / 'artifact.zip').read_bytes()).hexdigest()
            self.refused(self.invoke())

    def test_pagination_must_be_complete_and_includes_later_pages(self):
        endpoint = f'repos/example/project/actions/workflows/3/runs?event=pull_request&head_sha={self.head}&per_page=100&page='
        older = [{**self.run, 'id': i + 1000, 'run_number': i + 1, 'head_branch': 'other'} for i in range(100)]
        result = self.invoke(overrides={endpoint + '1': {'total_count': 101, 'workflow_runs': older},
                                        endpoint + '2': {'total_count': 101, 'workflow_runs': [self.run]}})
        self.assertIsNotNone(result.comparison, result.stderr)
        self.refused(self.invoke(overrides={endpoint + '1': {'total_count': 2, 'workflow_runs': [self.run]}}))

    def test_state_change_during_comparison_or_before_publication_never_writes(self):
        changed = {**self.pr, 'state': 'closed'}
        endpoint = 'repos/example/project/pulls/7'
        self.refused(self.invoke(overrides={endpoint: {'$sequence': [self.pr, changed]}}))
        result = self.invoke()
        self.assertIsNotNone(result.comparison, result.stderr)
        published = self.invoke('publish', overrides={endpoint: changed}, comparison=result.comparison)
        self.assertNotEqual(published.returncode, 0)
        self.no_writes(published)

    def test_publisher_rejects_supplied_destination_or_forged_pass(self):
        comparison = self.invoke().comparison
        self.assertIsNotNone(comparison)
        for changed in [{**comparison, 'destination': 'elsewhere'}, {**comparison, 'policy_passed': True},
                        {**comparison, 'binding': {**comparison['binding'], 'pr': 8}}]:
            published = self.invoke('publish', comparison=changed)
            self.assertNotEqual(published.returncode, 0)
            self.no_writes(published)

    def test_publisher_escapes_untrusted_text_and_updates_only_an_unambiguous_bot_comment(self):
        comparison = self.invoke().comparison
        comparison['detail'] = '<script>bad()</script> ``` @everyone\n::error::untrusted'
        published = self.invoke('publish', comparison=comparison)
        self.assertEqual(published.returncode, 0, published.stderr)
        body = json.loads((self.root / 'request.json').read_text())['body']
        self.assertIn('&lt;script&gt;', body)
        self.assertIn('&#64;everyone', body)
        self.assertNotIn('<script>', body)
        comment = {'id': 400, 'body': body, 'user': {'login': 'github-actions[bot]', 'type': 'Bot'}}
        endpoint = 'repos/example/project/issues/7/comments?per_page=100&page=1'
        patch = 'repos/example/project/issues/comments/400'
        updated = self.invoke('publish', comparison=comparison, overrides={endpoint: [comment], patch: {'id': 400}})
        self.assertEqual(updated.returncode, 0, updated.stderr)
        self.assertTrue(any('--method' in row and row[row.index('--method') + 1] == 'PATCH' and row[-1] == patch for row in updated.calls))
        for comments in [[comment, {**comment, 'id': 401}], [{**comment, 'body': body.replace('source:10:1', 'source:11:1')}]]:
            result = self.invoke('publish', comparison=comparison, overrides={endpoint: comments})
            self.assertNotEqual(result.returncode, 0)
            self.no_writes(result)

    def test_pr_or_source_attempt_changing_at_final_publish_guard_prevents_a_write(self):
        comparison = self.invoke().comparison
        self.assertIsNotNone(comparison)
        for endpoint, value in [
            ('repos/example/project/pulls/7', {**self.pr, 'state': 'closed'}),
            ('repos/example/project/actions/runs/100', {**self.run, 'run_attempt': 2}),
        ]:
            initial = self.pr if endpoint.endswith('/pulls/7') else self.run
            result = self.invoke('publish', comparison=comparison, overrides={endpoint: {'$sequence': [initial, value]}})
            self.assertNotEqual(result.returncode, 0)
            self.no_writes(result)

    def test_base_ref_change_with_the_same_sha_still_invalidates_publication(self):
        comparison = self.invoke().comparison
        self.assertIsNotNone(comparison)
        changed = {**self.pr, 'base': {**self.pr['base'], 'ref': 'different-target'}}
        result = self.invoke('publish', comparison=comparison, overrides={
            'repos/example/project/pulls/7': {'$sequence': [self.pr, changed]},
        })
        self.assertNotEqual(result.returncode, 0)
        self.no_writes(result)

    def test_superseded_consumer_attempt_cannot_publish(self):
        comparison = self.invoke().comparison
        self.assertIsNotNone(comparison)
        endpoint = 'repos/example/project/actions/runs/200'
        consumer = self.routes()[endpoint]
        result = self.invoke('publish', comparison=comparison, overrides={
            endpoint: {'$sequence': [consumer, {**consumer, 'run_attempt': 2}]},
        })
        self.assertNotEqual(result.returncode, 0)
        self.no_writes(result)


if __name__ == '__main__':
    unittest.main()
