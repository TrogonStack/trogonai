import os
import json
from pathlib import Path
import base64
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import test_publish as publication
import publish
import storage
from sources import GitObjects

ROOT = Path(__file__).resolve().parents[4]
TASKS = ROOT / '.config/mise/tasks/github/actions/coverage-report'
ACTION = ROOT / '.github/actions/coverage-report/action.yml'


class StorageCheckoutTests(publication.PublicationTests):
    def setUp(self):
        self.primaries = {}
        super().setUp()
        self.change(self.seed, 'data/pr/a/child/coverage.xml', 'unrelated descendant\n' * 8192)
        publication.run(self.seed, 'git', 'push', '--quiet', 'origin', 'HEAD')

    def environment(self, branch, subdirectory='.', trusted_main=False):
        environment = dict(os.environ, MISE_PROJECT_ROOT=str(ROOT),
                    COVERAGE_STORAGE_BRANCH='_xml_coverage_reports',
                    COVERAGE_STORAGE_SUBDIRECTORY=subdirectory,
                    COVERAGE_DIFF_BRANCH='main', COVERAGE_DIFF_ENABLED='true',
                    GITHUB_HEAD_REF=branch,
                    GITHUB_REF='refs/heads/main' if trusted_main else 'refs/pull/1/merge',
                    GITHUB_EVENT_NAME='push' if trusted_main else 'pull_request',
                    GITHUB_SHA='fixture-source', GITHUB_RUN_ID='1', GITHUB_RUN_ATTEMPT='1')
        environment.pop('COVERAGE_TOKEN', None)
        return environment

    def configure_checkout(self, directory, env, check=True):
        try:
            with patch.dict(os.environ, env):
                _, _, paths, _ = publish.context()
                patterns = storage.path_patterns(paths)
        except (ValueError, RuntimeError) as error:
            if check:
                raise
            return subprocess.CompletedProcess([], 1, '', str(error))
        result = subprocess.run(['git', 'sparse-checkout', 'set', '--no-cone', '--stdin'],
                                cwd=directory, text=True, input='\n'.join(patterns) + '\n',
                                capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def clone(self, name):
        if name == 'seed':
            return super().clone(name)
        primary = self.base / name
        publication.run(self.base, 'git', 'clone', '--quiet', str(self.remote), str(primary))
        publication.run(primary, 'git', 'checkout', '--quiet', '--detach')
        for key, value in [('user.name', 'Coverage Fixture'), ('user.email', 'coverage@example.invalid'),
                           ('commit.gpgsign', 'false'), ('core.hooksPath', str(primary / '.git/hooks'))]:
            publication.run(primary, 'git', 'config', key, value)
        env = dict(self.environment('pr/' + name), GITHUB_WORKSPACE=str(primary),
                   GITHUB_SHA=publication.run(primary, 'git', 'rev-parse', 'HEAD').stdout.strip())
        self.assertIn('run: mise run github:actions:coverage-report:storage-prepare', ACTION.read_text())
        self.assertNotIn('uses: actions/checkout', ACTION.read_text())
        self.task(primary, 'storage-prepare', env)
        directory = primary / '_xml_coverage_reports'
        self.primaries[directory] = (primary, env)
        return directory

    def invoke(self, repo, branch, operation, check=True, trusted_main=False):
        selected = self.configure_checkout(repo, self.environment(branch, trusted_main=trusted_main), check=check)
        if selected is not None:
            return selected
        result = super().invoke(repo, branch, operation, check=check, trusted_main=trusted_main)
        if not result.returncode and branch == 'pr/a':
            self.assert_unrelated_absent(repo)
        return result

    def change(self, repo, path, value):
        if repo != self.seed and path.startswith('data/pr/a/'):
            self.configure_checkout(repo, self.environment('pr/a'))
        if (repo / '.git').is_file() and publication.run(repo, 'git', 'symbolic-ref', '--quiet', 'HEAD', check=False).returncode:
            publication.run(repo, 'git', 'switch', '--quiet', '-C', '_xml_coverage_reports')
        super().change(repo, path, value)

    def assert_unrelated_absent(self, repo):
        for name in ('data/pr/b/coverage.xml', 'data/pr/a/child/coverage.xml'):
            self.assertFalse((repo / name).exists(), f'unrelated report materialized: {name}')
            self.assertNotEqual(publication.run(repo, 'git', 'cat-file', '-s', 'HEAD:' + name).stdout.strip(), '0')
        self.assertEqual(publication.run(repo, 'git', 'rev-parse', '--is-shallow-repository').stdout.strip(), 'false')
        self.assertEqual(publication.run(repo, 'git', 'status', '--porcelain').stdout, '')

    def test_declared_checkout_excludes_unrelated_reports_with_full_history(self):
        repo = self.clone('a')
        self.assert_unrelated_absent(repo)
        self.assertEqual((repo / 'data/main/coverage.xml').read_text(), 'immutable baseline')
        self.assertEqual((repo / 'data/main/sources.json').read_text(), 'immutable source binding')
        self.assertEqual((repo / 'data/pr/a/coverage.xml').read_text(), 'old a')
        self.assertEqual(publication.run(repo, 'git', 'rev-list', '--count', 'HEAD').stdout,
                         publication.run(self.seed, 'git', 'rev-list', '--count', 'HEAD').stdout)

    def test_literal_subdirectory_and_hierarchical_branch_select_exact_files(self):
        subdirectory = 'sub[ab]*? #!/leaf] '
        expected = {'data/.gitkeep', 'README.md'}
        for branch in ('main', 'pr/a'):
            for name in ('coverage.xml', 'sources.json', 'badge.svg'):
                path = f'data/{branch}/{subdirectory}/{name}'
                self.change(self.seed, path, path)
                if branch != 'main' or name != 'badge.svg':
                    expected.add(path)
        self.change(self.seed, 'data/pr/a/subaXYZ #!/leaf] /coverage.xml', 'unrelated wildcard match')
        self.change(self.seed, 'data/pr/a/sub[ab]*? #!/leaf] /child/coverage.xml', 'unrelated child')
        publication.run(self.seed, 'git', 'push', '--quiet', 'origin', 'HEAD')
        repo = self.clone('a')
        self.configure_checkout(repo, self.environment('pr/a', subdirectory))
        actual = {path for path in publication.run(repo, 'git', 'ls-files').stdout.splitlines()
                  if (repo / path).is_file()}
        self.assertEqual(actual, expected - {'data/.gitkeep', 'README.md'})
        self.assertEqual(publication.run(repo, 'git', 'status', '--porcelain').stdout, '')

    def test_control_character_path_is_rejected_before_checkout(self):
        repo = self.clone('a')
        rejected = self.configure_checkout(repo, self.environment('pr/a', 'bad\npath'), check=False)
        self.assertEqual(rejected.returncode, 1)
        self.assert_unrelated_absent(repo)

    def production_fixture(self, branch='pr/a'):
        source = self.base / 'source'
        source.mkdir()
        publication.run(source, 'git', 'init', '--quiet')
        publication.run(source, 'git', 'config', 'user.name', 'Coverage Fixture')
        publication.run(source, 'git', 'config', 'user.email', 'coverage@example.invalid')
        publication.run(source, 'git', 'config', 'commit.gpgsign', 'false')
        publication.run(source, 'git', 'config', 'core.hooksPath', str(source / '.git/hooks'))
        self.change(source, 'handler.rs', 'pub fn answer() -> u32 { 42 }\n')
        head = publication.run(source, 'git', 'rev-parse', 'HEAD').stdout.strip()
        raw = source / 'raw.xml'
        raw.write_text('<coverage lines-valid="1" lines-covered="1" line-rate="1"><sources><source>.</source></sources><packages><package line-rate="1"><classes><class filename="handler.rs" line-rate="1"><lines><line number="1" hits="1"/></lines></class></classes></package></packages></coverage>')
        env = dict(self.environment(branch, trusted_main=not branch), GITHUB_WORKSPACE=str(source),
                   GITHUB_SHA=head, GITHUB_REPOSITORY='coverage/fixture',
                   COVERAGE_PROVENANCE_REPORT=str(raw), COVERAGE_PATH=str(raw),
                   COVERAGE_REPORT_BRANCH=branch or 'main', GITHUB_OUTPUT=str(self.base / 'task-output'),
                   GITHUB_ENV=str(self.base / 'task-env'), GITHUB_JOB='test', GITHUB_RUN_ID='1', GITHUB_RUN_ATTEMPT='1')
        self.task(source, 'capture', env)
        pair = source / 'coverage-action/verified'
        baseline = {name: (pair / name).read_text() for name in ('coverage.xml', 'sources.json')}
        if not branch:
            self.change(source, 'handler.rs', 'pub fn answer() -> u32 {\n    42\n}\n')
            env['GITHUB_SHA'] = publication.run(source, 'git', 'rev-parse', 'HEAD').stdout.strip()
            head = env['GITHUB_SHA']
            raw.write_text('<coverage lines-valid="3" lines-covered="3" line-rate="1"><sources><source>.</source></sources><packages><package line-rate="1"><classes><class filename="handler.rs" line-rate="1"><lines><line number="1" hits="1"/><line number="2" hits="1"/><line number="3" hits="1"/></lines></class></classes></package></packages></coverage>')
            self.task(source, 'capture', env)
        for name in ('coverage.xml', 'sources.json'):
            self.change(self.seed, 'data/main/' + name, baseline[name])
        publication.run(self.seed, 'git', 'push', '--quiet', 'origin', 'HEAD')
        publication.run(source, 'git', 'remote', 'add', 'origin', str(self.remote))
        publication.run(source, 'git', 'fetch', '--quiet', 'origin',
                        '+refs/heads/_xml_coverage_reports:refs/remotes/origin/_xml_coverage_reports')
        self.source_head = head
        self.source_index = (source / '.git/index').read_bytes()
        self.source_files = {name: (source / name).read_bytes()
                             for name in publication.run(source, 'git', 'ls-files').stdout.splitlines()}
        self.task(source, 'storage-prepare', env)
        repo = source / '_xml_coverage_reports'
        self.primaries[repo] = (source, env)
        return source, repo, pair, env

    def task(self, cwd, name, env, check=True):
        interpreter = sys.executable if name == 'threshold' else 'bash'
        return publication.run(cwd, interpreter, str(TASKS / name), env=env, check=check)

    def publish_production_pair(self, branch):
        source, repo, pair, env = self.production_fixture(branch)
        baseline = {name: self.remote_value('data/main/' + name) for name in ('coverage.xml', 'sources.json')}
        self.task(repo, 'initialize', env)
        self.task(repo, 'push-initial', env)
        (source / '.coverage-total').write_text('100')
        self.task(source, 'store-percent', env)
        destination = repo / 'data' / (branch or 'main')
        (destination / 'badge.svg').write_text('<svg/>')
        self.task(repo / 'data', 'commit-badge', env)
        self.task(repo, 'push-badge', env)
        self.task(repo, 'commit-report', env)
        self.task(repo, 'push-report', env)
        for name in ('coverage.xml', 'sources.json'):
            self.assertEqual(self.remote_value(f'data/{branch or "main"}/{name}'), (pair / name).read_text())
            if branch:
                self.assertEqual(self.remote_value('data/main/' + name), baseline[name])
            else:
                self.assertNotEqual(self.remote_value('data/main/' + name), baseline[name])
        self.assertEqual(self.remote_value(f'data/{branch or "main"}/badge.svg'), '<svg/>')
        self.assert_unrelated_absent(repo)
        self.assertEqual(json.loads((destination / 'sources.json').read_text())['commit'], env['GITHUB_SHA'])
        self.assert_primary_unchanged(source)
        receipt = Path(publication.run(repo, 'git', 'rev-parse', '--git-path', 'coverage-publication-receipt.json').stdout.strip())
        self.assertTrue(receipt.is_relative_to(source / '.git/worktrees'))
        self.assertFalse((source / '.git/coverage-publication-receipt.json').exists())
        self.task(source, 'cleanup', env)
        self.task(source, 'cleanup', env)
        self.assertFalse(repo.exists())
        self.assert_primary_unchanged(source)

    def assert_primary_unchanged(self, source):
        self.assertEqual(publication.run(source, 'git', 'rev-parse', 'HEAD').stdout.strip(), self.source_head)
        self.assertEqual((source / '.git/index').read_bytes(), self.source_index)
        self.assertEqual({name: (source / name).read_bytes() for name in self.source_files}, self.source_files)
        self.assertNotEqual(publication.run(source, 'git', 'sparse-checkout', 'list', check=False).returncode, 0)
        GitObjects(source, self.source_head)

    def test_production_tasks_publish_existing_verified_pair(self):
        self.publish_production_pair('pr/a')

    def test_production_tasks_publish_new_hierarchical_verified_pair(self):
        self.publish_production_pair('pr/new/name]')

    def test_production_tasks_publish_trusted_main_verified_pair(self):
        self.publish_production_pair('')

    def test_production_initialize_rejects_mismatched_baseline_pair(self):
        _, repo, _, env = self.production_fixture()
        (repo / 'data/main/coverage.xml').write_text('<coverage/>')
        publication.run(repo, 'git', 'add', '--', 'data/main/coverage.xml')
        publication.run(repo, 'git', 'commit', '--quiet', '-m', 'fixture corrupt baseline')
        publication.run(repo, 'git', 'push', '--quiet', 'origin', 'HEAD:refs/heads/_xml_coverage_reports')
        before = self.remote_value('data/pr/a/coverage.xml')
        rejected = self.task(repo, 'initialize', env, check=False)
        self.assertEqual(rejected.returncode, 1)
        self.assertIn('Coverage: manifest-binding.', rejected.stderr)
        self.assertEqual(self.remote_value('data/pr/a/coverage.xml'), before)

    def test_cleanup_preserves_unrelated_worktree_and_refuses_unowned_directory(self):
        repo = self.clone('a')
        primary, env = self.primaries[repo]
        other = primary / 'other'
        publication.run(primary, 'git', 'worktree', 'add', '--quiet', '--detach', str(other), 'HEAD')
        self.task(primary, 'cleanup', env)
        self.assertTrue(other.is_dir())
        self.assertFalse(repo.exists())
        repo.mkdir()
        marker = repo / 'keep'
        marker.write_text('unowned')
        self.assertNotEqual(self.task(primary, 'storage-prepare', env, check=False).returncode, 0)
        self.assertNotEqual(self.task(primary, 'cleanup', env, check=False).returncode, 0)
        self.assertEqual(marker.read_text(), 'unowned')

    def test_cleanup_rejects_symlink_and_mismatched_run_receipt(self):
        repo = self.clone('a')
        primary, env = self.primaries[repo]
        self.assertNotEqual(self.task(primary, 'cleanup', dict(env, GITHUB_RUN_ATTEMPT='2'), check=False).returncode, 0)
        self.assertTrue(repo.is_dir())
        self.task(primary, 'cleanup', env)
        target = primary / 'keep'
        target.mkdir()
        repo.symlink_to(target, target_is_directory=True)
        self.assertNotEqual(self.task(primary, 'storage-prepare', env, check=False).returncode, 0)
        self.assertNotEqual(self.task(primary, 'cleanup', env, check=False).returncode, 0)
        self.assertTrue(repo.is_symlink())
        self.assertTrue(target.is_dir())

    def test_preparation_refreshes_absent_storage_ref_without_changing_source(self):
        repo = self.clone('a')
        primary, env = self.primaries[repo]
        self.task(primary, 'cleanup', env)
        publication.run(primary, 'git', 'update-ref', '-d', 'refs/remotes/origin/_xml_coverage_reports')
        self.change(self.seed, 'data/unrelated/coverage.xml', 'new unrelated report')
        publication.run(self.seed, 'git', 'push', '--quiet', 'origin', 'HEAD')
        self.task(primary, 'storage-prepare', env)
        self.assertEqual(publication.run(repo, 'git', 'show', 'HEAD:data/unrelated/coverage.xml').stdout, 'new unrelated report')
        self.assertFalse((repo / 'data/unrelated/coverage.xml').exists())
        self.assertEqual(publication.run(primary, 'git', 'rev-parse', 'HEAD').stdout.strip(), env['GITHUB_SHA'])

    def test_shallow_source_is_rejected_before_preparation_changes_it(self):
        primary = self.base / 'shallow'
        publication.run(self.base, 'git', 'clone', '--quiet', '--depth=1', self.remote.as_uri(), str(primary))
        env = dict(self.environment('pr/a'), GITHUB_WORKSPACE=str(primary),
                   GITHUB_SHA=publication.run(primary, 'git', 'rev-parse', 'HEAD').stdout.strip())
        boundary = (primary / '.git/shallow').read_bytes()
        rejected = self.task(primary, 'storage-prepare', env, check=False)
        self.assertEqual(rejected.returncode, 1)
        self.assertIn('complete Git history', rejected.stderr)
        self.assertEqual((primary / '.git/shallow').read_bytes(), boundary)
        self.assertFalse((primary / '_xml_coverage_reports').exists())

    def test_promisor_source_is_rejected_before_preparation_creates_a_worktree(self):
        repo = self.clone('a')
        primary, env = self.primaries[repo]
        self.task(primary, 'cleanup', env)
        publication.run(primary, 'git', 'config', 'remote.origin.promisor', 'true')
        rejected = self.task(primary, 'storage-prepare', env, check=False)
        self.assertEqual(rejected.returncode, 1)
        self.assertIn('Partial clone object stores are not supported', rejected.stderr)
        self.assertFalse(repo.exists())


class StorageCredentialTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='ci-storage-auth-', dir=Path(__file__).parent)
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        publication.run(self.root, 'git', 'init', '--quiet')
        publication.run(self.root, 'git', 'remote', 'add', 'origin', 'https://github.com/coverage/fixture')
        self.env = dict(os.environ, COVERAGE_TOKEN='nonsecret-action-fixture',
                        GITHUB_SERVER_URL='https://github.com', GITHUB_REPOSITORY='coverage/fixture',
                        GIT_ASKPASS='fixture-askpass', SSH_ASKPASS='fixture-ssh-askpass')

    def test_supplied_token_overrides_generic_host_and_repository_headers_without_writing_them(self):
        for scope in ('http.extraheader', 'http.https://github.com/.extraheader',
                      'http.https://github.com/coverage/fixture.extraheader'):
            for value in ('first-nonsecret-fixture', 'second-nonsecret-fixture'):
                publication.run(self.root, 'git', 'config', '--add', scope, value)
        publication.run(self.root, 'git', 'config', 'credential.helper', 'fixture-helper')
        publication.run(self.root, 'git', 'config', 'core.askPass', 'fixture-askpass')
        before = (self.root / '.git/config').read_bytes()
        env = dict(self.env, GIT_CONFIG_COUNT='1', GIT_CONFIG_KEY_0='core.abbrev', GIT_CONFIG_VALUE_0='12')
        with patch.dict(os.environ, env):
            child = publish.git_environment(self.root)
        actual = publication.run(self.root, 'git', 'config', '--get-urlmatch', 'http.extraheader',
                                 'https://github.com/coverage/fixture', env=child).stdout
        expected = 'AUTHORIZATION: basic ' + base64.b64encode(b'x-access-token:nonsecret-action-fixture').decode()
        self.assertTrue(actual.strip() == expected)
        self.assertEqual(publication.run(self.root, 'git', 'config', '--get', 'core.abbrev', env=child).stdout.strip(), '12')
        self.assertEqual(publication.run(self.root, 'git', 'config', '--get', 'credential.helper', env=child).stdout.strip(), '')
        self.assertEqual(publication.run(self.root, 'git', 'config', '--get', 'core.askPass', env=child).stdout.strip(), '')
        self.assertEqual(child['GIT_ASKPASS'], '')
        self.assertEqual(child['SSH_ASKPASS'], '')
        self.assertEqual(child['GIT_TERMINAL_PROMPT'], '0')
        self.assertNotIn('COVERAGE_TOKEN', child)
        self.assertEqual((self.root / '.git/config').read_bytes(), before)

    def test_canonical_git_suffix_and_distinct_push_route_use_the_supplied_token(self):
        publication.run(self.root, 'git', 'config', 'remote.origin.pushurl', 'https://github.com/coverage/fixture.git')
        publication.run(self.root, 'git', 'config', 'http.https://github.com/coverage/fixture.git.extraheader', 'nonsecret-inherited')
        with patch.dict(os.environ, self.env):
            child = publish.git_environment(self.root)
        expected = 'AUTHORIZATION: basic ' + base64.b64encode(b'x-access-token:nonsecret-action-fixture').decode()
        for url in ('https://github.com/coverage/fixture', 'https://github.com/coverage/fixture.git'):
            actual = publication.run(self.root, 'git', 'config', '--get-urlmatch', 'http.extraheader', url, env=child).stdout
            self.assertTrue(actual.strip() == expected)

    def test_empty_token_and_untrusted_fetch_or_push_routes_fail_closed(self):
        with patch.dict(os.environ, dict(self.env, COVERAGE_TOKEN='')):
            with self.assertRaises(RuntimeError):
                publish.git_environment(self.root)
        for key in ('remote.origin.url', 'remote.origin.pushurl'):
            for value in ('https://other.invalid/coverage/fixture', 'https://github.com/other/fixture',
                          'ssh://github.com/coverage/fixture', 'https://fixture@github.com/coverage/fixture'):
                with self.subTest(key=key, route=value):
                    publication.run(self.root, 'git', 'config', key, value)
                    with patch.dict(os.environ, self.env):
                        with self.assertRaises(RuntimeError):
                            publish.git_environment(self.root)
                    publication.run(self.root, 'git', 'config', '--unset-all', key)
                    if key == 'remote.origin.url':
                        publication.run(self.root, 'git', 'config', key, 'https://github.com/coverage/fixture')


if __name__ == '__main__':
    unittest.main(verbosity=2)
