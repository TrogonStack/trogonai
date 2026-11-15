import os
from pathlib import Path
import subprocess
import tempfile
import unittest


TASK = (
    Path(__file__).resolve().parents[2]
    / ".config/mise/tasks/github/workflows/canary-container-images/canary-is-branch-tip"
)
CURRENT_SHA = "a" * 40
NEWER_SHA = "b" * 40


class CanaryBranchTipTests(unittest.TestCase):
    def run_task(self, *, branch="main", tip=CURRENT_SHA, api_failure=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "output"
            output.touch()
            commands = root / "bin"
            commands.mkdir()
            git = commands / "git"
            git.write_text(
                "#!/bin/sh\n"
                "echo \"fatal: could not read Username for 'https://github.com': "
                "No such device or address\" >&2\n"
                "exit 128\n"
            )
            gh = commands / "gh"
            gh.write_text(
                "#!/usr/bin/env python3\n"
                "import os, sys\n"
                "args = sys.argv[1:]\n"
                "assert args[0] == 'api'\n"
                "assert 'repos/example/project/commits' in args\n"
                "assert 'sha=refs/heads/' + os.environ['GITHUB_REF_NAME'] in args\n"
                "assert 'per_page=1' in args\n"
                "assert args[args.index('--method') + 1] == 'GET'\n"
                "assert args[args.index('--jq') + 1] == '.[0].sha'\n"
                "assert os.environ.get('GITHUB_TOKEN')\n"
                "if os.environ['TEST_API_FAILURE'] == 'true':\n"
                "    sys.exit(1)\n"
                "print(os.environ['TEST_TIP'])\n"
            )
            git.chmod(0o755)
            gh.chmod(0o755)
            env = dict(os.environ)
            env.update(
                PATH=str(commands) + os.pathsep + env["PATH"],
                GITHUB_OUTPUT=str(output),
                GITHUB_REF_NAME=branch,
                GITHUB_SHA=CURRENT_SHA,
                GITHUB_TOKEN="non-secret-test-fixture",
                GITHUB_REPOSITORY="example/project",
                TEST_TIP=tip,
                TEST_API_FAILURE=str(api_failure).lower(),
            )
            result = subprocess.run(
                ["bash", str(TASK)],
                cwd=root,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            return result, output.read_text()

    def test_current_commit_without_checkout_git_credentials(self):
        result, output = self.run_task()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output, "current=true\n")

    def test_stale_commit_does_not_publish(self):
        result, output = self.run_task(tip=NEWER_SHA)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output, "current=false\n")

    def test_branch_name_is_passed_without_losing_url_characters(self):
        result, output = self.run_task(branch="feature/topic#review&ready")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output, "current=true\n")

    def test_api_failure_does_not_authorize_publication(self):
        result, output = self.run_task(api_failure=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(output, "")

    def test_missing_or_malformed_head_does_not_authorize_publication(self):
        for tip in ["", "null", "not-a-sha", "a" * 39, "a" * 41]:
            with self.subTest(tip=tip):
                result, output = self.run_task(tip=tip)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(output, "")


if __name__ == "__main__":
    unittest.main()
