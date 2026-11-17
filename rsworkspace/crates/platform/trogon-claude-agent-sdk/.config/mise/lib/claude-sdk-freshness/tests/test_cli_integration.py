import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


HERE = Path(__file__).resolve().parents[1]
ROOT = HERE.parents[3]


class CliIntegrationTests(unittest.TestCase):
    def test_freshness_task_validates_explicit_report_mode_before_running_checker(self):
        task = HERE.parents[1] / "tasks/claude-sdk/freshness"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = root / "python3"
            executable.write_text('#!/bin/sh\nprintf \'%s\\n\' "$@"\n')
            executable.chmod(0o755)
            environment = dict(os.environ, PATH=f"{root}{os.pathsep}{os.defpath}", MISE_PROJECT_ROOT=str(root))
            environment.pop("CLAUDE_SDK_FRESHNESS_REPORT_MODE", None)
            environment.pop("CLAUDE_SDK_FRESHNESS_FILE_ISSUE", None)
            for mode, valid in [(None, True), ("local", True), ("issue", True), ("typo", False), ("", False)]:
                with self.subTest(mode=mode):
                    current = environment.copy()
                    if mode is not None:
                        current["CLAUDE_SDK_FRESHNESS_REPORT_MODE"] = mode
                    result = subprocess.run(["bash", str(task)], env=current, capture_output=True, text=True, timeout=10)
                    if valid:
                        self.assertEqual(result.returncode, 0, result.stderr)
                        self.assertEqual("--file-issue" in result.stdout.splitlines(), mode == "issue")
                        self.assertIn("--out", result.stdout.splitlines())
                    else:
                        self.assertNotEqual(result.returncode, 0)
                        self.assertEqual(result.stdout, "")
                        self.assertIn("CLAUDE_SDK_FRESHNESS_REPORT_MODE", result.stderr)

    def test_source_changes_reach_a_rust_mapped_report_without_running_upstream(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            python = root / "python"
            package = python / "src/claude_agent_sdk"
            package.mkdir(parents=True)
            (python / "pyproject.toml").write_text('[project]\nversion = "1.0.0"\n')
            source = package / "__init__.py"
            source.write_text(
                "__all__ = ['Options']\nclass Options:\n    budget: float | None = None\n"
                "raise RuntimeError('upstream code must never run')\n"
            )
            typescript = root / "typescript"
            typescript.mkdir()
            (typescript / "package.json").write_text(json.dumps({
                "name": "fixture-sdk", "version": "1.0.0", "type": "module",
                "types": "sdk.d.ts", "exports": {".": {"types": "./sdk.d.ts"}},
            }))
            declarations = typescript / "sdk.d.ts"
            declarations.write_text("export interface Options { budget?: number; }\n")
            (typescript / "sdk.mjs").write_text("throw new Error('upstream code must never run');\n")
            baseline = root / "baseline.json"
            command = [sys.executable, str(HERE / "check.py"), "--python-source", str(python),
                       "--typescript-package", str(typescript)]
            candidate = subprocess.run(
                [*command, "--write-baseline", str(baseline), "--out", str(root / "candidate")],
                capture_output=True, text=True, timeout=120,
            )
            self.assertEqual(candidate.returncode, 0, candidate.stderr)
            accepted = baseline.read_bytes()
            mapping = root / "mapping.json"
            mapping.write_text(json.dumps({"schema_version": 1, "features": [{
                "id": "budget",
                "upstream": {language: {"symbols": ["Options.budget"], "prefixes": []}
                             for language in ("python", "typescript")},
                "rust_api": ["ClaudeAgentOptions.max_budget_usd"],
                "tests": [{
                    "path": "tests/runtime_features.rs",
                    "name": "structured_input_and_declarative_configuration_reach_the_cli",
                }],
                "status": "supported", "rationale": "Typed Rust budget configuration has public fixture evidence.",
            }]}))
            source.write_text(
                "__all__ = ['Options']\nclass Options:\n    budget: float\n    new_limit: int = 1\n"
                "raise RuntimeError('upstream code must never run')\n"
            )
            declarations.write_text("export interface Options { budget: number; newLimit?: number; }\n")
            output = root / "report"
            completed = subprocess.run(
                [*command, "--baseline", str(baseline), "--mapping", str(mapping), "--out", str(output)],
                cwd=ROOT, capture_output=True, text=True, timeout=120,
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            report = json.loads((output / "report.json").read_text())
            self.assertTrue(report["upstream_drift"])
            for language in ("python", "typescript"):
                changes = {item["symbol"]: item for item in report["sources"][language]["api_changes"]}
                budget = changes["Options.budget"]
                self.assertEqual(budget["change"], "changed")
                self.assertTrue(budget["needs_review"])
                self.assertEqual(budget["rust_evidence"]["rust_api"], ["ClaudeAgentOptions.max_budget_usd"])
            self.assertIn("Options.new_limit", report["unreviewed_rust_mappings"]["python"]["observed"])
            self.assertIn("Options.newLimit", report["unreviewed_rust_mappings"]["typescript"]["observed"])
            self.assertEqual(baseline.read_bytes(), accepted)
            self.assertIn("ClaudeAgentOptions.max_budget_usd", (output / "report.md").read_text())
            self.assertNotIn(str(root), (output / "report.md").read_text())


if __name__ == "__main__":
    unittest.main()
