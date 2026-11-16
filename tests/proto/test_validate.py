"""Exercise the validator CLI through a recording wrapper around pinned Buf."""

import copy
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest


ROOT = (
    Path(os.environ["TEST_SRCDIR"]) / os.environ["TEST_WORKSPACE"]
    if "TEST_SRCDIR" in os.environ
    else Path(__file__).resolve().parents[2]
)
FIXTURES = ROOT / "tests/proto/connector_mcp/fixtures/discovery-pins.json"


class ValidatorTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory(prefix="validator-cli-")
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.root = Path(cls.temporary.name)
        cls.env = os.environ.copy()
        pinned = tomllib.loads((ROOT / ".config/mise/config.toml").read_text())["tools"]["buf"]
        cls.buf = str(Path(os.environ["VALIDATOR_TEST_BUF"]).resolve()) if "VALIDATOR_TEST_BUF" in os.environ else subprocess.check_output(
            ["mise", "which", "buf", "--tool", f"buf@{pinned}"], text=True
        ).strip()
        if "VALIDATOR_TEST_BUF_CACHE" in os.environ:
            cache = cls.root / "buf-cache"
            cache.mkdir()
            subprocess.run(["tar", "-xf", str(Path(os.environ["VALIDATOR_TEST_BUF_CACHE"]).resolve()), "-C", str(cache)], check=True)
            cls.env.update(BUF_CACHE_DIR=str(cache), HTTPS_PROXY="http://127.0.0.1:9",
                           HTTP_PROXY="http://127.0.0.1:9", NO_PROXY="")
        for name in ("buf.yaml", "buf.lock", ".config/mise/config.toml", "tests/proto/validate.py"):
            target = cls.root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, target)
        shutil.copytree(ROOT / "proto", cls.root / "proto")
        cls.fixtures = cls.root / "tests/proto/cli/fixtures/cases.json"
        cls.fixtures.parent.mkdir(parents=True)
        cls.cases = json.loads(FIXTURES.read_text())
        cls.wrapper = cls.root / "buf-wrapper"
        # Bazel's interpreter path can exceed the kernel's shebang limit.
        launcher = "#!/bin/sh\n'''exec' {} \"$0\" \"$@\"\n' '''\n".format(shlex.quote(sys.executable))
        cls.wrapper.write_text(launcher + '''
import json, os, pathlib, subprocess, sys
args = sys.argv[1:]
payload = sys.stdin.buffer.read() if args[0] == "convert" else None
actual = args.copy()
if args[0] == "convert" and "--validate" in args and os.environ.get("BUF_FATAL"):
    actual[actual.index("--type") + 1] = "missing.Message"
result = subprocess.run([os.environ["REAL_BUF"], *actual], input=payload, capture_output=True)
entry = {"args": args, "payload": json.loads(payload) if payload else None,
         "returncode": result.returncode, "stderr": result.stderr.decode()}
pathlib.Path(os.environ["BUF_CALLS"], str(os.getpid()) + ".json").write_text(json.dumps(entry))
sys.stdout.buffer.write(result.stdout)
sys.stderr.buffer.write(result.stderr)
sys.exit(result.returncode)
''')
        cls.wrapper.chmod(0o755)

    def invoke(self, cases, fatal=False):
        self.fixtures.write_text(json.dumps(cases))
        with tempfile.TemporaryDirectory(dir=self.root) as calls:
            result = subprocess.run(
                [sys.executable, str(self.root / "tests/proto/validate.py"), "--buf", str(self.wrapper)],
                cwd=self.root, text=True, capture_output=True,
                env={**self.env, "REAL_BUF": self.buf, "BUF_CALLS": calls, "BUF_FATAL": "1" if fatal else ""},
            )
            recorded = [json.loads(path.read_text()) for path in Path(calls).glob("*.json")]
        return result, recorded

    def test_successful_validation_needs_no_separate_parse(self):
        result, calls = self.invoke(self.cases)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "Protobuf contract fixtures: 3 valid, 3 invalid, 0 failures\n")
        self.assertEqual(sum(call["args"][0] == "build" for call in calls), 1)
        for case in self.cases:
            with self.subTest(case=case["name"]):
                conversions = [call for call in calls if call["args"][0] == "convert" and call["payload"] == case["message"]]
                validated = [call for call in conversions if "--validate" in call["args"]]
                self.assertEqual(len(validated), 1)
                self.assertEqual(validated[0]["returncode"], 0 if case["valid"] else 1)
                self.assertEqual(len(conversions), 1 if case["valid"] else 2)

    def test_parse_unknown_fields_and_diagnostic_failures_remain_distinct(self):
        valid = next(case for case in self.cases if case["valid"])
        invalid = next(case for case in self.cases if not case["valid"])
        cases = []

        def case(name, original):
            item = copy.deepcopy(original)
            item["name"] = name
            cases.append(item)
            return item

        case("valid-rejected", invalid)["valid"] = True
        case("invalid-accepted", valid)["valid"] = False
        wrong = case("wrong-diagnostic", invalid)
        wrong["violations"] = ["this diagnostic must be present verbatim"]
        for expected in (True, False):
            malformed = case(f"parse-{expected}", invalid)
            malformed["valid"] = expected
            malformed["message"]["responseIssuerRequired"] = {}
        unknown = case("unknown", valid)
        unknown["message"]["discovery"]["unexpected"] = True

        result, calls = self.invoke(cases)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, "Protobuf contract fixtures: 1 valid, 2 invalid, 6 failures\n")
        conversions = [call for call in calls if call["args"][0] == "convert"]

        def diagnostic(item, validate):
            return next(call["stderr"] for call in conversions
                        if call["payload"] == item["message"] and ("--validate" in call["args"]) == validate)

        expected = [
            f"cli/valid-rejected: valid payload rejected: {diagnostic(cases[0], True)}",
            "cli/invalid-accepted: invalid payload accepted",
            f"cli/wrong-diagnostic: expected {wrong['violations'][0]!r}: {diagnostic(wrong, True)}",
            *[f"cli/{item['name']}: fixture cannot parse: {diagnostic(item, False)}" for item in cases[3:5]],
            "cli/unknown: unknown fixture fields: /discovery/unexpected",
        ]
        self.assertEqual(result.stderr, "".join(line + "\n" for line in expected))
        self.assertFalse(any(call["payload"] == unknown["message"] for call in conversions))

    def test_fatal_buf_failure_is_not_a_validation_violation(self):
        invalid = next(case for case in self.cases if not case["valid"])
        result, calls = self.invoke([invalid], fatal=True)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, "Protobuf contract fixtures: 0 valid, 1 invalid, 1 failures\n")
        conversions = [call for call in calls if call["args"][0] == "convert"]
        validated = next(call for call in conversions if "--validate" in call["args"])
        parsed = next(call for call in conversions if "--validate" not in call["args"])
        self.assertNotEqual(validated["returncode"], 0)
        self.assertEqual(parsed["returncode"], 0)
        self.assertEqual(result.stderr, f"cli/{invalid['name']}: validation did not run: {validated['stderr']}\n")


if __name__ == "__main__":
    unittest.main()
