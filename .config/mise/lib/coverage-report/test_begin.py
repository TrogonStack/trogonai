import json
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[4]
ACTION = ROOT / '.github/actions/coverage-report/action.yml'


class FreshGateEvidenceTests(unittest.TestCase):
    def setUp(self):
        fixtures = ROOT / '.trogonai'
        fixtures.mkdir(exist_ok=True)
        self.tmp = tempfile.TemporaryDirectory(prefix='coverage-begin-', dir=fixtures)
        self.addCleanup(self.tmp.cleanup)
        self.directory = Path(self.tmp.name)
        self.env = dict(os.environ, MISE_AUTO_INSTALL='false', GITHUB_WORKSPACE=str(self.directory),
                        GITHUB_REPOSITORY='coverage/fixture', GITHUB_SHA='a' * 40,
                        GITHUB_ENV=str(self.directory / 'environment'), COVERAGE_THRESHOLD='95',
                        COVERAGE_PROVENANCE_REPORT='missing.xml', COVERAGE_DIFF_ENABLED='true',
                        COVERAGE_SKIP_STORAGE='false', COVERAGE_PYCOBERTURA_STATUS='0',
                        COVERAGE_INCREASE_FAILURE='true', COVERAGE_NEW_FAILURE='true',
                        COVERAGE_EXCEPTION_FAILURE='true', COVERAGE_RATE_FAILURE='true')
        for name, data in {'.coverage-comparison-status': b'0\n', '.coverage-total': b'100\n',
                           '.coverage-total.diff-branch': b'0\n'}.items():
            (self.directory / name).write_bytes(data)

    def invoke(self, *command):
        result = subprocess.run(command, cwd=self.directory, env=self.env, capture_output=True, text=True)
        print(json.dumps({'command': command, 'exit': result.returncode,
                          'stdout': result.stdout, 'stderr': result.stderr}), flush=True)
        return result

    def begin(self):
        command = next(line.strip().split('run: ', 1)[1] for line in ACTION.read_text().splitlines()
                       if line.strip().startswith('run: mise run '))
        result = self.invoke(*shlex.split(command))
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_failed_capture_cannot_reuse_prior_comparison_or_percentage(self):
        self.begin()
        capture = self.invoke('mise', 'run', 'github:actions:coverage-report:capture')
        self.assertNotEqual(capture.returncode, 0, 'missing report must fail source capture')
        comparison = self.invoke('mise', 'run', 'github:actions:coverage-report:diff-gate')
        threshold = self.invoke('mise', 'run', 'github:actions:coverage-report:threshold')
        self.assertNotEqual(comparison.returncode, 0, 'failed capture cannot reuse an earlier successful comparison')
        self.assertNotEqual(threshold.returncode, 0, 'failed capture cannot reuse an earlier measured percentage')

    def test_begin_preserves_complete_coverage_evidence(self):
        expected = {'raw.xml': b'<coverage/>', '.coverage-output.final': b'full report\n',
                    '.coverage-output.diff': b'complete comparison\n'}
        for name, data in expected.items():
            (self.directory / name).write_bytes(data)
        self.begin()
        self.assertEqual({name: (self.directory / name).read_bytes() for name in expected}, expected)


if __name__ == '__main__':
    unittest.main(verbosity=2)
