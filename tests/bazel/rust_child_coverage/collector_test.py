import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


class CollectorTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.collector, cls.subject, cls.helper, cls.llvm_cov, cls.llvm_profdata = [
            Path(argument).absolute() for argument in sys.argv[1:]
        ]
        cls.temporary = tempfile.TemporaryDirectory(dir=os.environ["TEST_TMPDIR"])
        cls.directory = Path(cls.temporary.name)
        cls.raw = cls.directory / "raw"
        cls.raw.mkdir()
        environment = os.environ.copy()
        environment["LLVM_PROFILE_FILE"] = str(cls.raw / "child-%p-%m.profraw")
        subject = subprocess.run([cls.subject, "--probe"], env=environment, capture_output=True, timeout=30)
        if subject.returncode not in (0, 78):
            raise AssertionError("real subject capability probe failed")
        result = subprocess.run([cls.helper, "--probe"], env=environment, capture_output=True, timeout=30)
        cls.probe = json.loads(result.stdout)
        if result.returncode != (0 if cls.probe["supported"] else 78):
            raise AssertionError("real helper capability probe did not exit with its reported support")

    @classmethod
    def tearDownClass(cls):
        cls.temporary.cleanup()

    def collect(self, objects):
        directory = self.directory / self.id().rsplit(".", 1)[-1]
        directory.mkdir()
        profiles = list(self.raw.glob("*.profraw"))
        self.assertGreaterEqual(len(profiles), 2, "real subject and child must both write profiles")
        for profile in profiles:
            shutil.copyfile(profile, directory / profile.name)
        runfiles = Path(os.environ["TEST_SRCDIR"])
        workspace = runfiles / os.environ["TEST_WORKSPACE"]
        environment = os.environ.copy()
        environment.update({
            "COVERAGE_DIR": str(directory),
            "ROOT": str(workspace),
            "RUNFILES_DIR": str(runfiles),
            "TEST_BINARY": str(self.subject.relative_to(workspace)),
            "RUST_LLVM_COV": str(self.llvm_cov),
            "RUST_LLVM_PROFDATA": str(self.llvm_profdata),
            "RUST_COVERAGE_OBJECTS": objects,
        })
        result = subprocess.run([self.collector], env=environment, capture_output=True, timeout=30)
        report = directory / "coverage.dat"
        return result, report.read_text() if report.exists() else ""

    @staticmethod
    def line_hits(report, suffix):
        source = ""
        hits = 0
        for line in report.splitlines():
            if line.startswith("SF:"):
                source = line[3:]
            elif line.startswith("DA:") and source.endswith(suffix):
                hits += int(line.split(",")[1])
        return hits

    def test_declared_child_objects_export_real_main_execution(self):
        workspace = Path(os.environ["TEST_SRCDIR"]) / os.environ["TEST_WORKSPACE"]
        declared = str(self.helper.relative_to(workspace))
        result, report = self.collect(declared + "\n\n" + declared)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertGreater(self.line_hits(report, "sandbox-command-supervisor/src/main.rs"), 0)
        self.assertEqual(self.probe["kind"], "probe")

    def test_empty_objects_keep_test_binary_coverage(self):
        result, report = self.collect("")
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertGreater(self.line_hits(report, "sandbox-command-supervisor/src/bin/overflow_fixture.rs"), 0)

    def test_missing_objects_are_rejected(self):
        result, _ = self.collect("missing-coverage-child")
        self.assertNotEqual(result.returncode, 0)

    def test_parent_traversal_is_rejected(self):
        result, _ = self.collect("../coverage-child")
        self.assertNotEqual(result.returncode, 0)

    def test_absolute_objects_are_rejected(self):
        result, _ = self.collect(str(self.helper))
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main(argv=[sys.argv[0]])
