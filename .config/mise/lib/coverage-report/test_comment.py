import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

HELPER = Path(__file__).with_name('comment.py')


class CommentTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='coverage-comment-', dir=HELPER.parent)
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.env = dict(os.environ, COVERAGE_ARTIFACT_URL='https://github.com/coverage/fixture/actions/runs/1/artifacts/2', COVERAGE_THRESHOLD='95', GITHUB_SHA='a' * 40, COVERAGE_HEAD_SHA='b' * 40)
        (self.root / '.coverage-total').write_text('98.49000000')
        (self.root / 'raw.xml').write_bytes(b'<coverage><class filename="crates/handler.rs"/></coverage>')
        (self.root / '.coverage-output.diff').write_text('unchanged complete comparison evidence\n' * 12000)

    def invoke(self):
        return subprocess.run([sys.executable, str(HELPER)], cwd=self.root, env=self.env, capture_output=True, text=True)

    def snapshot(self):
        return {name:hashlib.sha256((self.root / name).read_bytes()).hexdigest() for name in ('raw.xml', '.coverage-output.diff', '.coverage-output.final')}

    def test_oversized_full_report_becomes_bounded_comment_with_complete_artifact_link(self):
        (self.root / '.coverage-output.final').write_text('🧪 full report row\n' * 12000)
        before = self.snapshot()
        self.assertGreater((self.root / '.coverage-output.final').stat().st_size, 65536)
        result = self.invoke()
        self.assertEqual(result.returncode, 0, result.stderr)
        comment = (self.root / '.coverage-output.comment').read_bytes()
        self.assertLessEqual(len(comment), 60000)
        self.assertLessEqual(len(comment.decode().encode('utf-16-le')) // 2, 60000)
        self.assertIn(self.env['COVERAGE_ARTIFACT_URL'], comment.decode())
        self.assertIn('98.49000000%', comment.decode())
        self.assertIn('95%', comment.decode())
        self.assertIn('b' * 40, comment.decode())
        self.assertEqual(self.snapshot(), before)

    def test_small_comment_is_preserved_byte_for_byte(self):
        original = b'# Coverage\n\nDetailed full report\n'
        (self.root / '.coverage-output.final').write_bytes(original)
        self.assertEqual(self.invoke().returncode, 0)
        self.assertEqual((self.root / '.coverage-output.comment').read_bytes(), original)

    def test_missing_artifact_link_cannot_claim_a_full_report_exists(self):
        (self.root / '.coverage-output.final').write_text('row\n' * 20000)
        self.env['COVERAGE_ARTIFACT_URL'] = ''
        before = self.snapshot()
        self.assertNotEqual(self.invoke().returncode, 0)
        self.assertFalse((self.root / '.coverage-output.comment').exists())
        self.assertEqual(self.snapshot(), before)

    def test_artifact_link_cannot_inject_another_markdown_destination(self):
        (self.root / '.coverage-output.final').write_text('row\n' * 20000)
        self.env['COVERAGE_ARTIFACT_URL'] = 'https://github.com/coverage/fixture>\nforged'
        self.assertNotEqual(self.invoke().returncode, 0)
        self.assertFalse((self.root / '.coverage-output.comment').exists())


    def test_a_finite_minimum_retains_the_existing_threshold_input_semantics(self):
        (self.root / '.coverage-output.final').write_text('row\n' * 20000)
        self.env['COVERAGE_THRESHOLD'] = '-1'
        result = self.invoke()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('Minimum required coverage: -1%', (self.root / '.coverage-output.comment').read_text())


if __name__ == '__main__':
    unittest.main(verbosity=2)
