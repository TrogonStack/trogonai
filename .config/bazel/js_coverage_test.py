"""Exercise the actual JavaScript coverage bootstrap with selected manifests."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class JavaScriptCoverageTest(unittest.TestCase):
    def collect(self, selected):
        with tempfile.TemporaryDirectory(dir=os.environ.get("TEST_TMPDIR")) as directory:
            root = Path(directory)
            runfiles = root / "runfiles"
            workspace = runfiles / "workspace"
            fixtures = workspace / "fixtures"
            fixtures.mkdir(parents=True)
            parent = fixtures / "coverage_test_parent.cjs"
            child = fixtures / "coverage_test_child.cjs"
            shutil.copyfile(PARENT, parent)
            shutil.copyfile(CHILD, child)
            coverage = root / "coverage"
            coverage.mkdir()
            manifest = root / "selected-files"
            if selected is not None:
                manifest.write_text("\n".join(selected))
            env = os.environ.copy()
            for key in ("NODE_V8_COVERAGE", "NODE_OPTIONS", "COVERAGE_OUTPUT_FILE", "SPLIT_COVERAGE_POST_PROCESSING"):
                env.pop(key, None)
            env.update({
                "COVERAGE_DIR": str(coverage),
                "COVERAGE_MANIFEST": str(manifest),
                "JS_BINARY__COVERAGE_REPORT": str(REPORTER),
                "JS_BINARY__NODE_BINARY": str(NODE),
                "JS_BINARY__RUNFILES": str(runfiles),
                "TEST_WORKSPACE": "workspace",
            })
            result = subprocess.run(
                [str(NODE), "--require", str(BOOTSTRAP), str(parent), str(child)],
                cwd=workspace,
                env=env,
                capture_output=True,
                text=True,
            )
            effects = [json.loads(line) for line in result.stdout.splitlines()]
            self.assertEqual([item["subject"] for item in effects], ["parent executed", "child executed"])
            report = coverage / "_rules_js_report.lcov"
            return {
                "result": result,
                "effects": effects,
                "profiles": len(list(coverage.glob("coverage-*.json"))),
                "report": report.read_text() if report.exists() else None,
            }

    def assert_covered(self, report, source):
        self.assertIsNotNone(report)
        sources = [line for line in report.splitlines() if line.startswith("SF:")]
        self.assertEqual(sources, [f"SF:fixtures/{source}"])
        records = [record for record in report.split("end_of_record") if f"SF:fixtures/{source}\n" in record]
        self.assertEqual(len(records), 1, f"missing selected source {source}")
        hits = [int(line.split(",")[1]) for line in records[0].splitlines() if line.startswith("DA:")]
        self.assertTrue(any(hits), f"selected source {source} has no execution hits")

    def test_empty_selection_runs_subjects_without_enabling_coverage(self):
        actual = self.collect([])
        self.assertEqual(actual["result"].returncode, 0, actual["result"].stderr)
        self.assertEqual(actual["profiles"], 0)
        self.assertTrue(all(not item["coverage"] for item in actual["effects"]))
        self.assertEqual(actual["report"], "")

    def test_selected_parent_retains_real_execution_coverage(self):
        actual = self.collect(["fixtures/coverage_test_parent.cjs"])
        self.assertEqual(actual["result"].returncode, 0, actual["result"].stderr)
        self.assertGreaterEqual(actual["profiles"], 2)
        self.assertTrue(all(item["coverage"] for item in actual["effects"]))
        self.assert_covered(actual["report"], "coverage_test_parent.cjs")

    def test_selected_child_retains_transitive_execution_coverage(self):
        actual = self.collect(["fixtures/coverage_test_child.cjs"])
        self.assertEqual(actual["result"].returncode, 0, actual["result"].stderr)
        self.assertGreaterEqual(actual["profiles"], 2)
        self.assertTrue(all(item["coverage"] for item in actual["effects"]))
        self.assert_covered(actual["report"], "coverage_test_child.cjs")

    def test_unknown_selection_retains_collection_and_reports_the_error(self):
        actual = self.collect(None)
        self.assertNotEqual(actual["result"].returncode, 0)
        self.assertIn("selected-files", actual["result"].stderr)
        self.assertGreaterEqual(actual["profiles"], 2)
        self.assertTrue(all(item["coverage"] for item in actual["effects"]))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    for argument in ("bootstrap", "reporter", "parent", "child", "tools"):
        parser.add_argument(f"--{argument}", required=True, type=Path)
    args, remaining = parser.parse_known_args()
    BOOTSTRAP = args.bootstrap.resolve()
    REPORTER = args.reporter.resolve()
    PARENT = args.parent.resolve()
    CHILD = args.child.resolve()
    NODE = Path(json.loads(args.tools.read_text())["node"]).resolve()
    unittest.main(argv=[__file__, *remaining])
