import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

HELPER = Path(__file__).with_name('provenance.py')


class SourcePairTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='coverage-pair-', dir=Path(__file__).parent)
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        self.run_git('init', '--quiet')
        self.run_git('config', 'user.name', 'Coverage Fixture')
        self.run_git('config', 'user.email', 'coverage@example.invalid')
        self.run_git('config', 'commit.gpgsign', 'false')
        self.run_git('config', 'core.hooksPath', str(self.root / '.git/hooks'))
        (self.root / 'handler.rs').write_text('pub fn answer() -> u32 { 42 }\n')
        self.run_git('add', '--', 'handler.rs')
        self.run_git('commit', '--quiet', '-m', 'fixture')
        self.head = self.run_git('rev-parse', 'HEAD').stdout.strip()
        self.raw = self.root / 'raw.xml'
        self.raw.write_text('<coverage lines-valid="1" lines-covered="1" line-rate="1"><sources><source>.</source></sources><packages><package><classes><class filename="handler.rs"><lines><line number="1" hits="1"/></lines></class></classes></package></packages></coverage>')
        self.env = dict(os.environ, GITHUB_WORKSPACE=str(self.root), GITHUB_REPOSITORY='coverage/fixture', GITHUB_SHA=self.head, COVERAGE_PROVENANCE_REPORT=str(self.raw), COVERAGE_DIFF_ENABLED='true', COVERAGE_DIFF_BRANCH='main', COVERAGE_STORAGE_SUBDIRECTORY='.')

    def run_git(self, *arguments):
        return subprocess.run(['git', '-C', str(self.root), *arguments], capture_output=True, text=True, check=True)

    def invoke(self, command, cwd=None):
        return subprocess.run([sys.executable, str(HELPER), command], cwd=cwd or self.root, env=self.env, capture_output=True, text=True)

    def test_capture_keeps_raw_comparison_input_and_binds_published_pair(self):
        original = self.raw.read_bytes()
        result = self.invoke('capture')
        self.assertEqual(result.returncode, 0, result.stderr)
        pair = self.root / 'coverage-action/verified'
        manifest = json.loads((pair / 'sources.json').read_text())
        self.assertEqual(self.raw.read_bytes(), original)
        self.assertEqual(manifest['version'], 2)
        self.assertEqual(manifest['commit'], self.head)
        self.assertEqual(manifest['report_sha256'], hashlib.sha256((pair / 'coverage.xml').read_bytes()).hexdigest())
        self.assertEqual(manifest['sources']['handler.rs']['sha256'], hashlib.sha256((self.root / 'handler.rs').read_bytes()).hexdigest())

    def test_dirty_source_capture_cannot_replace_an_existing_valid_pair(self):
        self.assertEqual(self.invoke('capture').returncode, 0)
        pair = self.root / 'coverage-action/verified'
        old = {name:(pair / name).read_bytes() for name in ('coverage.xml','sources.json')}
        (self.root / 'handler.rs').write_text('pub fn answer() -> u32 { 0 }\n')
        self.assertNotEqual(self.invoke('capture').returncode, 0)
        self.assertEqual({name:(pair / name).read_bytes() for name in old}, old)

    def test_xml_only_baseline_update_is_rejected_without_rewriting_sidecar(self):
        self.assertEqual(self.invoke('capture').returncode, 0)
        directory = self.root / 'data/main'
        directory.mkdir(parents=True)
        for name in ('coverage.xml','sources.json'):
            (directory / name).write_bytes((self.root / 'coverage-action/verified' / name).read_bytes())
        self.assertEqual(self.invoke('baseline').returncode, 0)
        old = (directory / 'sources.json').read_bytes()
        (directory / 'coverage.xml').write_bytes(self.raw.read_bytes())
        self.assertNotEqual(self.invoke('baseline').returncode, 0)
        self.assertEqual((directory / 'sources.json').read_bytes(), old)


    def install_baseline(self):
        result = self.invoke('capture')
        self.assertEqual(result.returncode, 0, result.stderr)
        baseline = self.root / 'data/main'
        baseline.mkdir(parents=True)
        for name in ('coverage.xml', 'sources.json'):
            (baseline / name).write_bytes((self.root / 'coverage-action/verified' / name).read_bytes())
        return baseline

    def test_missing_baseline_report_is_rejected_without_rewriting_sources(self):
        baseline = self.install_baseline()
        old = (baseline / 'sources.json').read_bytes()
        (baseline / 'coverage.xml').unlink()
        rejected = self.invoke('baseline')
        self.assertEqual(rejected.returncode, 1)
        self.assertEqual((baseline / 'sources.json').read_bytes(), old)
        self.assertFalse((baseline / 'coverage.xml').exists())

    def test_missing_baseline_manifest_is_rejected_without_seeding_sources(self):
        baseline = self.install_baseline()
        old = (baseline / 'coverage.xml').read_bytes()
        (baseline / 'sources.json').unlink()
        rejected = self.invoke('baseline')
        self.assertEqual(rejected.returncode, 1)
        self.assertEqual((baseline / 'coverage.xml').read_bytes(), old)
        self.assertFalse((baseline / 'sources.json').exists())

    def test_malformed_baseline_manifest_is_rejected_without_rewriting_it(self):
        baseline = self.install_baseline()
        (baseline / 'sources.json').write_text('{broken')
        old = {name:(baseline / name).read_bytes() for name in ('coverage.xml', 'sources.json')}
        self.assertEqual(self.invoke('baseline').returncode, 1)
        self.assertEqual({name:(baseline / name).read_bytes() for name in old}, old)

    def test_duplicate_baseline_manifest_keys_are_rejected(self):
        baseline = self.install_baseline()
        path = baseline / 'sources.json'
        source = path.read_text().replace('"version": 2', '"version": 2, "version": 2')
        path.write_text(source)
        self.assertEqual(self.invoke('baseline').returncode, 1)
        self.assertEqual(path.read_text(), source)

    def test_baseline_source_blob_must_match_the_named_commit(self):
        baseline = self.install_baseline()
        path = baseline / 'sources.json'
        manifest = json.loads(path.read_text())
        manifest['sources']['handler.rs']['blob'] = '0' * 40
        path.write_text(json.dumps(manifest))
        before = path.read_bytes()
        self.assertEqual(self.invoke('baseline').returncode, 1)
        self.assertEqual(path.read_bytes(), before)

    def test_baseline_commit_must_exist_as_a_real_commit(self):
        baseline = self.install_baseline()
        path = baseline / 'sources.json'
        manifest = json.loads(path.read_text())
        manifest['commit'] = '0' * 40
        path.write_text(json.dumps(manifest))
        before = path.read_bytes()
        self.assertEqual(self.invoke('baseline').returncode, 1)
        self.assertEqual(path.read_bytes(), before)

    def test_baseline_repository_must_match_the_action_context(self):
        baseline = self.install_baseline()
        path = baseline / 'sources.json'
        manifest = json.loads(path.read_text())
        manifest['repository'] = 'other/fixture'
        path.write_text(json.dumps(manifest))
        self.assertEqual(self.invoke('baseline').returncode, 1)

    def test_report_line_must_exist_in_its_immutable_source_even_with_a_matching_report_hash(self):
        baseline = self.install_baseline()
        report = baseline / 'coverage.xml'
        changed = report.read_bytes().replace(b'number="1"', b'number="2"')
        self.assertNotEqual(changed, report.read_bytes())
        report.write_bytes(changed)
        path = baseline / 'sources.json'
        manifest = json.loads(path.read_text())
        manifest['report_sha256'] = hashlib.sha256(changed).hexdigest()
        path.write_text(json.dumps(manifest))
        self.assertEqual(self.invoke('baseline').returncode, 1)
        self.assertEqual(report.read_bytes(), changed)

    def test_capture_rejects_internal_xml_entities_before_replacing_a_valid_pair(self):
        self.assertEqual(self.invoke('capture').returncode, 0)
        pair = self.root / 'coverage-action/verified'
        old = {name:(pair / name).read_bytes() for name in ('coverage.xml','sources.json')}
        self.raw.write_text('<!DOCTYPE coverage [<!ENTITY alias "payload">]><coverage><sources><source>.</source></sources></coverage>')
        self.assertEqual(self.invoke('capture').returncode, 1)
        self.assertEqual({name:(pair / name).read_bytes() for name in old}, old)


if __name__ == '__main__':
    unittest.main(verbosity=2)
