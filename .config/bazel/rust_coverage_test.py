"""Exercise the Rust collector with profiles from a real subprocess."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


class RustCoverageTest(unittest.TestCase):
    def collect(self, objects=None, *, split=False, profiles=True):
        with tempfile.TemporaryDirectory(dir=os.environ.get("TEST_TMPDIR")) as directory:
            root = Path(directory)
            runfiles = root / "runfiles"
            workspace = runfiles / "workspace"
            bin_dir = root / "bazel-out" / "config" / "bin"
            coverage = root / "bazel-out" / "config" / "testlogs" / "collector"
            coverage.mkdir(parents=True)
            for directory in (workspace, bin_dir):
                (directory / "fixtures").mkdir(parents=True)
                (directory / "fixtures/coverage runner").symlink_to(RUNNER)
                (directory / "fixtures/coverage child").symlink_to(CHILD)

            env = os.environ.copy()
            env.pop("RUNFILES_DIR", None)
            env.pop("RUST_COVERAGE_OBJECTS", None)
            if profiles:
                result = subprocess.run(
                    [str(RUNNER), str(CHILD)],
                    env=env | {"LLVM_PROFILE_FILE": str(coverage / "%p-%m.profraw")},
                    capture_output=True,
                    text=True,
                    check=True,
                )
                self.assertIn("child exercised", result.stdout)
                self.assertEqual(len(list(coverage.glob("*.profraw"))), 2)

            env.update({
                "ROOT": str(root),
                "COVERAGE_DIR": str(coverage),
                "TEST_WORKSPACE": "workspace",
                "TEST_BINARY": "fixtures/coverage runner",
                "RUST_LLVM_COV": TOOLS["llvm_cov"],
                "RUST_LLVM_PROFDATA": TOOLS["llvm_profdata"],
            })
            if not split:
                env["RUNFILES_DIR"] = str(runfiles)
            if objects is not None:
                env["RUST_COVERAGE_OBJECTS"] = objects
            subprocess.run([str(COLLECTOR)], env=env, capture_output=True, text=True, check=True)
            return (coverage / "coverage.dat").read_text()

    def assert_covered(self, report, source):
        records = [record for record in report.split("end_of_record") if f"/{source}\n" in record]
        self.assertEqual(len(records), 1, f"missing coverage for {source}")
        hits = [int(line.split(",")[1]) for line in records[0].splitlines() if line.startswith("DA:")]
        self.assertTrue(any(hits), f"no executed lines for {source}")

    def test_default_collects_only_the_test_object(self):
        report = self.collect()
        self.assert_covered(report, "coverage_test_runner.rs")
        self.assertNotIn("coverage_test_child.rs", report)

    def test_additional_object_collects_child_execution(self):
        report = self.collect("fixtures/coverage child")
        self.assert_covered(report, "coverage_test_runner.rs")
        self.assert_covered(report, "coverage_test_child.rs")

    def test_additional_object_without_runfiles(self):
        report = self.collect("fixtures/coverage child", split=True)
        self.assert_covered(report, "coverage_test_child.rs")

    def test_multiple_objects_preserve_spaces_in_paths(self):
        report = self.collect("fixtures/coverage child\nfixtures/coverage runner")
        self.assert_covered(report, "coverage_test_runner.rs")
        self.assert_covered(report, "coverage_test_child.rs")

    def test_no_profiles_retains_empty_report(self):
        self.assertEqual(self.collect("fixtures/coverage child", profiles=False), "")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    for argument in ("collector", "runner", "child", "tools"):
        parser.add_argument(f"--{argument}", required=True, type=Path)
    args, remaining = parser.parse_known_args()
    COLLECTOR = args.collector.resolve()
    RUNNER = args.runner.resolve()
    CHILD = args.child.resolve()
    TOOLS = {key: str(Path(value).resolve()) for key, value in json.loads(args.tools.read_text()).items()}
    unittest.main(argv=[__file__, *remaining])
