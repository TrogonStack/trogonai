import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


PUBLISH = Path(__file__).with_name("publish.py")
BRANCH = "_xml_coverage_reports"


class CoveragePublicationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.remote = self.root / "remote.git"
        self.git(self.root, "init", "--bare", str(self.remote))
        self.git(self.remote, "config", "core.hooksPath", str(self.remote / "hooks"))
        self.seed = self.root / "seed"
        self.git(self.root, "clone", str(self.remote), str(self.seed))
        self.configure(self.seed)
        self.git(self.seed, "switch", "--orphan", BRANCH)
        self.commit(self.seed, "README.md", b"coverage storage\n")
        self.git(self.seed, "push", "origin", f"HEAD:refs/heads/{BRANCH}")

    def git(self, directory, *arguments, check=True, input=None):
        return subprocess.run(
            ["git", "-C", str(directory), *arguments],
            check=check,
            input=input,
            capture_output=True,
        )

    def configure(self, directory):
        self.git(directory, "config", "user.name", "Coverage Fixture")
        self.git(directory, "config", "user.email", "coverage@example.test")
        self.git(directory, "config", "commit.gpgsign", "false")
        self.git(directory, "config", "core.hooksPath", os.devnull)

    def clone(self, name):
        directory = self.root / name
        self.git(self.root, "clone", "--branch", BRANCH, str(self.remote), str(directory))
        self.configure(directory)
        return directory

    def commit(self, directory, filename, data):
        path = directory / filename
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        self.git(directory, "add", filename)
        self.git(directory, "commit", "-m", "Store fixture report")

    def publish(self, directory, *arguments):
        return subprocess.run(
            [sys.executable, str(PUBLISH), "--directory", str(directory), "--branch", BRANCH, *arguments],
            capture_output=True,
        )

    def stored(self, filename):
        return self.git(self.remote, "show", f"refs/heads/{BRANCH}:{filename}").stdout

    def test_disjoint_concurrent_reports_both_survive_a_stale_writer(self):
        first = self.clone("first")
        second = self.clone("second")
        first_report = b'<coverage line-rate="1"><!-- first exact bytes --></coverage>\n'
        second_report = b'<coverage line-rate="0.99"><!-- second exact bytes --></coverage>\n'
        self.commit(first, "data/first/coverage.xml", first_report)
        self.commit(second, "data/second/coverage.xml", second_report)
        self.git(first, "push", "--atomic", "origin", f"HEAD:refs/heads/{BRANCH}")

        result = self.publish(second)

        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertEqual(self.stored("data/first/coverage.xml"), first_report)
        self.assertEqual(self.stored("data/second/coverage.xml"), second_report)
        self.assertEqual(self.git(second, "status", "--porcelain").stdout, b"")

    def test_conflicting_report_is_refused_without_rewriting_the_winner(self):
        first = self.clone("first")
        second = self.clone("second")
        self.commit(first, "data/shared/coverage.xml", b"winning report\n")
        self.commit(second, "data/shared/coverage.xml", b"conflicting report\n")
        self.git(first, "push", "origin", f"HEAD:refs/heads/{BRANCH}")
        winner = self.git(self.remote, "rev-parse", f"refs/heads/{BRANCH}").stdout
        local = self.git(second, "rev-parse", "HEAD").stdout

        result = self.publish(second)

        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.git(self.remote, "rev-parse", f"refs/heads/{BRANCH}").stdout, winner)
        self.assertEqual(self.stored("data/shared/coverage.xml"), b"winning report\n")
        self.assertEqual(self.git(second, "rev-parse", "HEAD").stdout, local)
        self.assertEqual(self.git(second, "status", "--porcelain").stdout, b"")

    def test_repeated_server_refusal_has_a_finite_attempt_budget(self):
        writer = self.clone("writer")
        self.commit(writer, "data/writer/coverage.xml", b"unpublished\n")
        hook = self.remote / "hooks" / "pre-receive"
        hook.write_text("#!/bin/sh\nprintf 'refused\\n' >> rejections\nexit 1\n")
        hook.chmod(0o755)

        result = self.publish(writer, "--attempts", "3")

        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((self.remote / "rejections").read_text().splitlines(), ["refused"] * 3)
        self.assertNotEqual(self.git(self.remote, "show", f"refs/heads/{BRANCH}:data/writer/coverage.xml", check=False).returncode, 0)

    def test_current_storage_tip_is_a_successful_no_op(self):
        writer = self.clone("writer")
        old = self.git(self.remote, "rev-parse", f"refs/heads/{BRANCH}").stdout

        result = self.publish(writer)

        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertEqual(self.git(self.remote, "rev-parse", f"refs/heads/{BRANCH}").stdout, old)

    def test_uncommitted_report_is_refused_without_changing_storage(self):
        writer = self.clone("writer")
        old = self.git(self.remote, "rev-parse", f"refs/heads/{BRANCH}").stdout
        report = writer / "uncommitted.xml"
        report.write_bytes(b"uncommitted report\n")

        result = self.publish(writer)

        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.git(self.remote, "rev-parse", f"refs/heads/{BRANCH}").stdout, old)
        self.assertEqual(report.read_bytes(), b"uncommitted report\n")

    def test_missing_storage_ref_can_be_created_without_force(self):
        self.git(self.remote, "update-ref", "-d", f"refs/heads/{BRANCH}")
        self.commit(self.seed, "data/main/coverage.xml", b"initial report\n")

        result = self.publish(self.seed)

        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertEqual(self.stored("data/main/coverage.xml"), b"initial report\n")


if __name__ == "__main__":
    unittest.main()
