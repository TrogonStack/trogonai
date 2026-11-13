import os
from pathlib import Path
import subprocess
import tempfile
import unittest


LAUNCHER = Path(__file__).resolve().parents[1] / ".config/mise/lib/verify"


class HydroLauncherTests(unittest.TestCase):
    def setUp(self):
        self.fixture = tempfile.TemporaryDirectory()
        self.addCleanup(self.fixture.cleanup)
        self.root = Path(self.fixture.name)
        self.log = self.root / "mise-arguments"
        mise = self.root / "mise"
        mise.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$HYDRO_LAUNCH_LOG"\n')
        mise.chmod(0o755)
        self.environment = os.environ.copy()
        for name in ("CI", "GITHUB_ACTIONS", "BUILD_WORKSPACE_DIRECTORY", "MISE_PROJECT_ROOT"):
            self.environment.pop(name, None)
        self.environment.update(
            PATH=str(self.root) + os.pathsep + self.environment["PATH"],
            HYDRO_LAUNCH_LOG=str(self.log),
        )

    def launch(self, variables, arguments=()):
        return subprocess.run(
            [str(LAUNCHER), *arguments],
            env={**self.environment, **variables},
            capture_output=True,
            text=True,
            check=False,
        )

    def test_ci_refuses_before_launching_mise(self):
        for name in ("CI", "GITHUB_ACTIONS"):
            for value in ("true", "false"):
                with self.subTest(name=name, value=value):
                    result = self.launch({name: value})
                    self.assertEqual(result.returncode, 2)
                    self.assertIn("disabled in CI", result.stderr)
                    self.assertFalse(self.log.exists())

    def test_mise_root_selects_locked_command_and_forwards_filter(self):
        result = self.launch({"MISE_PROJECT_ROOT": str(self.root)}, ["removal_stays_terminal"])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            self.log.read_text().splitlines(),
            [
                "-C", str(self.root / "projects/hydro-verification"), "exec", "--",
                "cargo", "test", "--locked", "--test", "decisions", "--", "--nocapture",
                "removal_stays_terminal",
            ],
        )

    def test_bazel_workspace_takes_precedence(self):
        result = self.launch({
            "BUILD_WORKSPACE_DIRECTORY": str(self.root),
            "MISE_PROJECT_ROOT": str(self.root / "nested"),
        })
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.log.read_text().splitlines()[1], str(self.root / "projects/hydro-verification"))

    def test_missing_workspace_refuses_before_launching_mise(self):
        result = self.launch({})
        self.assertEqual(result.returncode, 2)
        self.assertIn("through mise or bazel run", result.stderr)
        self.assertFalse(self.log.exists())


if __name__ == "__main__":
    unittest.main()
