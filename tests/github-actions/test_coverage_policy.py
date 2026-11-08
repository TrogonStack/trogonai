import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
POLICY = ROOT / ".config/mise/tasks/github-actions/coverage-policy"
WORKFLOW = ROOT / ".github/workflows/ci-rust.yml"


class CoveragePolicyTests(unittest.TestCase):
    def policy(self, event, ref, requested="false", payload=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            event_file = root / "event.json"
            event_file.write_text(json.dumps(payload or {}))
            output = root / "output"
            result = subprocess.run(
                [sys.executable, str(POLICY)],
                env=dict(
                    os.environ,
                    GITHUB_EVENT_NAME=event,
                    GITHUB_REF=ref,
                    GITHUB_EVENT_PATH=str(event_file),
                    GITHUB_OUTPUT=str(output),
                    COVERAGE_BASELINE_RESET_REQUESTED=requested,
                ),
                capture_output=True,
                text=True,
            )
            return result, output.read_text() if output.exists() else ""

    def test_explicit_main_dispatch_can_publish_an_approved_baseline(self):
        result, output = self.policy("workflow_dispatch", "refs/heads/main", "true")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output, "enforce_regressions=false\n")

    def test_reset_request_outside_main_is_rejected_without_outputs(self):
        for ref in ("refs/heads/feature", "refs/tags/main", "refs/pull/1/merge", ""):
            with self.subTest(ref=ref):
                result, output = self.policy("workflow_dispatch", ref, "true")
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(output, "")

    def test_dispatch_without_explicit_reset_keeps_gates(self):
        for requested in ("false", ""):
            with self.subTest(requested=requested):
                result, output = self.policy("workflow_dispatch", "refs/heads/main", requested)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(output, "enforce_regressions=true\n")

    def test_malformed_dispatch_boolean_is_rejected_without_outputs(self):
        for requested in ("TRUE", "yes", "1", "true\nfalse"):
            with self.subTest(requested=requested):
                result, output = self.policy("workflow_dispatch", "refs/heads/main", requested)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(output, "")

    def test_ordinary_push_does_not_inherit_a_pr_label_or_reset_input(self):
        payload = {
            "inputs": {"coverage_baseline_reset": "true"},
            "pull_request": {"labels": [{"name": "rust:coverage-baseline-reset"}]},
        }
        result, output = self.policy("push", "refs/heads/main", "true", payload)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output, "enforce_regressions=true\n")

    def test_pr_label_remains_the_only_pr_reset_authority(self):
        for labels, expected in (
            ([], "true"),
            ([{"name": "rust:coverage-baseline-reset-suffix"}], "true"),
            ([{"name": "rust:coverage-baseline-reset"}], "false"),
        ):
            with self.subTest(labels=labels):
                result, output = self.policy(
                    "pull_request", "refs/pull/1/merge", "true", {"pull_request": {"labels": labels}}
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(output, f"enforce_regressions={expected}\n")

    def test_other_event_types_cannot_waive_gates(self):
        for event in ("pull_request_target", "schedule", "repository_dispatch"):
            with self.subTest(event=event):
                result, output = self.policy(
                    event,
                    "refs/heads/main",
                    "true",
                    {"pull_request": {"labels": [{"name": "rust:coverage-baseline-reset"}]}},
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(output, "enforce_regressions=true\n")


class WorkflowContractTests(unittest.TestCase):
    def test_dispatch_is_explicit_boolean_and_defaults_to_strict(self):
        workflow = WORKFLOW.read_text()
        dispatch = re.search(r"^  workflow_dispatch:\n(?:(?:    .*)?\n)*", workflow, re.M)
        self.assertIsNotNone(dispatch, "CI has no trusted main baseline-update entry point")
        self.assertIn("coverage_baseline_reset:", dispatch[0])
        self.assertIn("type: boolean", dispatch[0])
        self.assertIn("default: false", dispatch[0])

    def test_workflow_consumes_policy_for_each_regression_gate(self):
        workflow = WORKFLOW.read_text()
        self.assertIn("run: python3 tests/github-actions/test_coverage_policy.py", workflow)
        self.assertIn("- 'tests/github-actions/**'", workflow)
        self.assertIn("id: coverage-policy", workflow)
        self.assertIn("run: python3 .config/mise/tasks/github-actions/coverage-policy", workflow)
        self.assertIn("COVERAGE_BASELINE_RESET_REQUESTED: ${{ inputs.coverage_baseline_reset || false }}", workflow)
        for policy in (
            "uncovered-statements-increase-failure",
            "new-uncovered-statements-failure",
            "coverage-rate-reduction-failure",
        ):
            self.assertRegex(
                workflow,
                rf"{policy}: \$\{{\{{ steps\.coverage-policy\.outputs\.enforce_regressions == 'true' \}}\}}",
            )
        for unchanged in ("threshold: 95", "fail: true", "diff: true", "source-root: rsworkspace"):
            self.assertIn(unchanged, workflow)


if __name__ == "__main__":
    unittest.main()
