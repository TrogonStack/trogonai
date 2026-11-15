import pathlib
import subprocess
import sys
import tempfile
import unittest

def run(cwd, *args, check=True, env=None):
    result = subprocess.run(list(args), cwd=cwd, text=True, capture_output=True, env=env)
    if check and result.returncode:
        raise AssertionError(f'{args[:2]} exit {result.returncode}: {result.stderr}')
    return result


class PublicationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='ci-publication-', dir=pathlib.Path(__file__).parent)
        self.addCleanup(self.tmp.cleanup)
        self.base = pathlib.Path(self.tmp.name)
        self.remote = self.base / 'remote.git'
        self.helper = pathlib.Path(__file__).with_name('publish.py').resolve()
        run(self.base, 'git', 'init', '--bare', '--initial-branch=_xml_coverage_reports', str(self.remote))
        self.seed = self.clone('seed')
        self.change(self.seed, 'data/main/coverage.xml', 'immutable baseline')
        self.change(self.seed, 'data/main/sources.json', 'immutable source binding')
        self.change(self.seed, 'data/pr/a/coverage.xml', 'old a')
        self.change(self.seed, 'data/pr/b/coverage.xml', 'old b')
        run(self.seed, 'git', 'push', '-u', 'origin', 'HEAD')

    def clone(self, name):
        directory = self.base / name
        run(self.base, 'git', 'clone', '--quiet', str(self.remote), str(directory))
        run(directory, 'git', 'config', 'user.name', 'Coverage Fixture')
        run(directory, 'git', 'config', 'user.email', 'coverage@example.invalid')
        run(directory, 'git', 'config', 'commit.gpgsign', 'false')
        run(directory, 'git', 'config', 'core.hooksPath', str(directory / '.git/hooks'))
        return directory

    def change(self, repo, path, value):
        target = repo / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(value)
        run(repo, 'git', 'add', '--', path)
        run(repo, 'git', 'commit', '--quiet', '-m', 'fixture')

    def invoke(self, repo, branch, operation, check=True, trusted_main=False):
        import os
        env = os.environ.copy()
        env.update(COVERAGE_STORAGE_BRANCH='_xml_coverage_reports', COVERAGE_STORAGE_SUBDIRECTORY='.', COVERAGE_DIFF_BRANCH='main', COVERAGE_DIFF_ENABLED='true', GITHUB_HEAD_REF=branch, GITHUB_REF='refs/heads/main' if trusted_main else 'refs/pull/1/merge', GITHUB_EVENT_NAME='push' if trusted_main else 'pull_request', GITHUB_SHA='fixture-source')
        return run(repo, sys.executable, str(self.helper), operation, check=check, env=env)

    def remote_value(self, path):
        return run(self.base, 'git', '--git-dir', str(self.remote), 'show', f'HEAD:{path}').stdout

    def test_disjoint_snapshots_preserve_both_reports_and_baseline(self):
        a, b = self.clone('a'), self.clone('b')
        self.invoke(a, 'pr/a', 'init'); self.invoke(b, 'pr/b', 'init')
        self.change(a, 'data/pr/a/coverage.xml', 'new a'); self.change(b, 'data/pr/b/coverage.xml', 'new b')
        self.invoke(a, 'pr/a', 'push'); self.invoke(b, 'pr/b', 'push')
        self.assertEqual(self.remote_value('data/pr/a/coverage.xml'), 'new a')
        self.assertEqual(self.remote_value('data/pr/b/coverage.xml'), 'new b')
        self.assertEqual(self.remote_value('data/main/coverage.xml'), 'immutable baseline')

    def test_competing_destination_rejects_stale_report(self):
        a, b = self.clone('a'), self.clone('b')
        self.invoke(a, 'pr/a', 'init'); self.invoke(b, 'pr/a', 'init')
        self.change(a, 'data/pr/a/coverage.xml', 'new winner'); self.change(b, 'data/pr/a/coverage.xml', 'stale loser')
        self.invoke(a, 'pr/a', 'push')
        rejected = self.invoke(b, 'pr/a', 'push', check=False)
        self.assertEqual(rejected.returncode, 1)
        self.assertIn('destination changed after comparison', rejected.stderr)
        self.assertEqual(self.remote_value('data/pr/a/coverage.xml'), 'new winner')

    def test_changed_baseline_rejects_report_compared_to_old_baseline(self):
        a, b = self.clone('a'), self.clone('b')
        self.invoke(a, 'pr/a', 'init')
        self.change(a, 'data/pr/a/coverage.xml', 'new a')
        self.change(b, 'data/main/coverage.xml', 'new baseline')
        run(b, 'git', 'push', '--quiet', 'origin', 'HEAD')
        rejected = self.invoke(a, 'pr/a', 'push', check=False)
        self.assertEqual(rejected.returncode, 1)
        self.assertIn('data/main/coverage.xml', rejected.stderr)
        self.assertEqual(self.remote_value('data/pr/a/coverage.xml'), 'old a')

    def test_own_badge_then_xml_publications_advance_expected_identity(self):
        a = self.clone('a')
        self.invoke(a, 'pr/a', 'init')
        self.invoke(a, 'pr/a', 'push')
        self.change(a, 'data/pr/a/badge.svg', 'new badge')
        self.invoke(a, 'pr/a', 'push')
        self.change(a, 'data/pr/a/coverage.xml', 'new a')
        self.invoke(a, 'pr/a', 'push')
        self.assertEqual(self.remote_value('data/pr/a/badge.svg'), 'new badge')
        self.assertEqual(self.remote_value('data/pr/a/coverage.xml'), 'new a')

    def test_local_baseline_mutation_is_rejected(self):
        a = self.clone('a')
        self.invoke(a, 'pr/a', 'init')
        self.change(a, 'data/main/coverage.xml', 'unapproved local baseline')
        rejected = self.invoke(a, 'pr/a', 'push', check=False)
        self.assertEqual(rejected.returncode, 1)
        self.assertIn('local coverage comparison baseline changed', rejected.stderr)
        self.assertEqual(self.remote_value('data/main/coverage.xml'), 'immutable baseline')

    def test_trusted_main_updates_baseline_and_rejects_stale_main_writer(self):
        a, b = self.clone('a'), self.clone('b')
        self.invoke(a, '', 'init', trusted_main=True); self.invoke(b, '', 'init', trusted_main=True)
        self.change(a, 'data/main/coverage.xml', 'new accepted baseline')
        self.change(b, 'data/main/coverage.xml', 'stale baseline')
        self.invoke(a, '', 'push', trusted_main=True)
        rejected = self.invoke(b, '', 'push', check=False, trusted_main=True)
        self.assertEqual(rejected.returncode, 1)
        self.assertIn('destination changed after comparison', rejected.stderr)
        self.assertEqual(self.remote_value('data/main/coverage.xml'), 'new accepted baseline')

    def test_pull_request_cannot_claim_main_baseline_destination(self):
        a = self.clone('a')
        rejected = self.invoke(a, 'main', 'init', check=False)
        self.assertEqual(rejected.returncode, 1)
        self.assertIn('cannot overwrite the comparison baseline', rejected.stderr)

    def test_missing_baseline_fails_without_seeding_one(self):
        a = self.clone('a')
        run(a, 'git', 'rm', 'data/main/coverage.xml')
        run(a, 'git', 'commit', '--quiet', '-m', 'fixture removal')
        rejected = self.invoke(a, 'pr/a', 'init', check=False)
        self.assertEqual(rejected.returncode, 1)
        self.assertIn('comparison baseline is missing', rejected.stderr)
        self.assertEqual(self.remote_value('data/main/coverage.xml'), 'immutable baseline')

    def test_legacy_force_publication_reproduces_remote_ref_race(self):
        import shlex
        a, b = self.clone('a'), self.clone('b')
        self.change(a, 'README.md', 'initialized')
        self.change(b, 'data/pr/b/coverage.xml', 'racing b')
        hook = a / run(a, 'git', 'rev-parse', '--git-path', 'hooks/pre-push').stdout.strip()
        hook.write_text('#!/bin/sh\nrm -- "$0"\nunset $(git rev-parse --local-env-vars)\ngit -C ' + shlex.quote(str(b)) + ' push --quiet origin HEAD\n')
        hook.chmod(0o755)
        result = run(a, 'git', 'push', '--force', 'origin', 'HEAD:refs/heads/_xml_coverage_reports', check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('cannot lock ref', result.stderr)
        print('Legacy pinned-force subprocess exit:', result.returncode)
        print(result.stderr)
        self.assertEqual(self.remote_value('data/pr/b/coverage.xml'), 'racing b')

    def test_retry_after_disjoint_push_between_fetch_and_push(self):
        import shlex
        a, b = self.clone('a'), self.clone('b')
        self.invoke(a, 'pr/a', 'init')
        self.change(a, 'data/pr/a/coverage.xml', 'new a')
        self.change(b, 'data/pr/b/coverage.xml', 'racing b')
        hook = a / run(a, 'git', 'rev-parse', '--git-path', 'hooks/pre-push').stdout.strip()
        hook.write_text('#!/bin/sh\nrm -- "$0"\nunset $(git rev-parse --local-env-vars)\ngit -C ' + shlex.quote(str(b)) + ' push --quiet origin HEAD\n')
        hook.chmod(0o755)
        result = self.invoke(a, 'pr/a', 'push')
        self.assertIn('attempt 2', result.stdout)
        self.assertEqual(self.remote_value('data/pr/a/coverage.xml'), 'new a')
        self.assertEqual(self.remote_value('data/pr/b/coverage.xml'), 'racing b')


    def test_changed_baseline_source_sidecar_rejects_stale_publication(self):
        a, b = self.clone('a'), self.clone('b')
        self.invoke(a, 'pr/a', 'init')
        self.change(a, 'data/pr/a/coverage.xml', 'new a')
        self.change(b, 'data/main/sources.json', 'new source binding')
        run(b, 'git', 'push', '--quiet', 'origin', 'HEAD')
        rejected = self.invoke(a, 'pr/a', 'push', check=False)
        self.assertEqual(rejected.returncode, 1)
        self.assertIn('data/main/sources.json', rejected.stderr)
        self.assertEqual(self.remote_value('data/pr/a/coverage.xml'), 'old a')

    def test_competing_destination_sidecar_rejects_stale_publication(self):
        a, b = self.clone('a'), self.clone('b')
        self.invoke(a, 'pr/a', 'init')
        self.change(a, 'data/pr/a/coverage.xml', 'new a')
        self.change(b, 'data/pr/a/sources.json', 'racing source binding')
        run(b, 'git', 'push', '--quiet', 'origin', 'HEAD')
        rejected = self.invoke(a, 'pr/a', 'push', check=False)
        self.assertEqual(rejected.returncode, 1)
        self.assertIn('data/pr/a/sources.json', rejected.stderr)
        self.assertEqual(self.remote_value('data/pr/a/coverage.xml'), 'old a')

    def test_local_baseline_source_mutation_is_rejected(self):
        a = self.clone('a')
        self.invoke(a, 'pr/a', 'init')
        self.change(a, 'data/main/sources.json', 'unapproved local source binding')
        rejected = self.invoke(a, 'pr/a', 'push', check=False)
        self.assertEqual(rejected.returncode, 1)
        self.assertIn('local coverage comparison baseline changed', rejected.stderr)

    def test_xml_source_pair_is_published_together(self):
        a = self.clone('a')
        self.invoke(a, 'pr/a', 'init')
        self.change(a, 'data/pr/a/coverage.xml', 'new a')
        self.change(a, 'data/pr/a/sources.json', 'new a source binding')
        self.invoke(a, 'pr/a', 'push')
        self.assertEqual(self.remote_value('data/pr/a/coverage.xml'), 'new a')
        self.assertEqual(self.remote_value('data/pr/a/sources.json'), 'new a source binding')



    def test_missing_baseline_source_sidecar_refuses_initialization(self):
        a = self.clone('a')
        run(a, 'git', 'rm', 'data/main/sources.json')
        run(a, 'git', 'commit', '--quiet', '-m', 'fixture source removal')
        rejected = self.invoke(a, 'pr/a', 'init', check=False)
        self.assertEqual(rejected.returncode, 1)
        self.assertIn('comparison baseline is missing', rejected.stderr)
        self.assertEqual(self.remote_value('data/main/sources.json'), 'immutable source binding')

    def test_baseline_source_change_between_fetch_and_push_rejects_retry(self):
        import shlex
        a, b = self.clone('a'), self.clone('b')
        self.invoke(a, 'pr/a', 'init')
        self.change(a, 'data/pr/a/coverage.xml', 'new a')
        self.change(b, 'data/main/sources.json', 'new baseline binding during push')
        hook = a / run(a, 'git', 'rev-parse', '--git-path', 'hooks/pre-push').stdout.strip()
        hook.write_text('#!/bin/sh\nrm -- "$0"\nunset $(git rev-parse --local-env-vars)\ngit -C ' + shlex.quote(str(b)) + ' push --quiet origin HEAD\n')
        hook.chmod(0o755)
        rejected = self.invoke(a, 'pr/a', 'push', check=False)
        self.assertEqual(rejected.returncode, 1)
        self.assertIn('data/main/sources.json', rejected.stderr)
        self.assertEqual(self.remote_value('data/pr/a/coverage.xml'), 'old a')
        self.assertEqual(self.remote_value('data/main/sources.json'), 'new baseline binding during push')

    def test_destination_source_change_between_fetch_and_push_rejects_retry(self):
        import shlex
        a, b = self.clone('a'), self.clone('b')
        self.invoke(a, 'pr/a', 'init')
        self.change(a, 'data/pr/a/coverage.xml', 'new a')
        self.change(b, 'data/pr/a/sources.json', 'new destination binding during push')
        hook = a / run(a, 'git', 'rev-parse', '--git-path', 'hooks/pre-push').stdout.strip()
        hook.write_text('#!/bin/sh\nrm -- "$0"\nunset $(git rev-parse --local-env-vars)\ngit -C ' + shlex.quote(str(b)) + ' push --quiet origin HEAD\n')
        hook.chmod(0o755)
        rejected = self.invoke(a, 'pr/a', 'push', check=False)
        self.assertEqual(rejected.returncode, 1)
        self.assertIn('data/pr/a/sources.json', rejected.stderr)
        self.assertEqual(self.remote_value('data/pr/a/coverage.xml'), 'old a')
        self.assertEqual(self.remote_value('data/pr/a/sources.json'), 'new destination binding during push')


if __name__ == "__main__":
    unittest.main(verbosity=2)
