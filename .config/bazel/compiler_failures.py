"""Compile UI fixtures with Bazel's rustc and check their trybuild goldens."""

import argparse
import difflib
import json
from pathlib import Path
import re
import subprocess


def normalize(rendered, package):
    rendered = rendered.replace(package + "/", "")
    rendered = rendered.replace("rsworkspace/", "$WORKSPACE/")
    rendered = re.sub(r"and \d+ others", "and $N others", rendered)
    lines = []
    external_source = False
    for line in rendered.splitlines():
        # trybuild omits generated long-type filenames. The hint only points at that artifact.
        if re.match(r"\s*= note: (the full (type name|name for the type) has been written to|consider using `--verbose`)", line):
            continue
        if re.match(r"\s*(-->|:::) \$WORKSPACE/", line):
            line = re.sub(r":\d+:\d+$", "", line)
            external_source = True
        elif external_source and re.match(r"\s*[\d|.]", line):
            line = re.sub(r"^(\s*)\d+", lambda match: " " * len(match[0]), line)
        else:
            external_source = False
        lines.append(line)
    return "\n".join(lines).rstrip() + "\n"


def check(fixture, golden, result):
    compiled = json.loads(Path(result).read_text())
    if compiled["exit"] != 1:
        raise AssertionError(f"{fixture}: expected rustc exit 1, got {compiled['exit']}\n{compiled['stderr']}")
    if compiled["stdout"]:
        raise AssertionError(f"{fixture}: unexpected compiler stdout: {compiled['stdout']}")
    diagnostics = [json.loads(line) for line in compiled["stderr"].splitlines()]
    rendered = "".join(
        diagnostic["rendered"]
        for diagnostic in diagnostics
        if diagnostic["level"] != "failure-note"
        and not (diagnostic["message"].startswith("aborting due to ") and not diagnostic["spans"])
    )
    actual = normalize(rendered, str(Path(fixture).parent.parent.parent))
    expected = Path(golden).read_text()
    if actual != expected:
        raise AssertionError("".join(difflib.unified_diff(
            expected.splitlines(keepends=True), actual.splitlines(keepends=True),
            fromfile=str(golden), tofile=str(result),
        )))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", action="append", default=[])
    parser.add_argument("--case", nargs=3, action="append", default=[])
    parser.add_argument("--capture")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.capture:
        compiled = subprocess.run(args.command[1:], capture_output=True, text=True, check=False)
        Path(args.capture).write_text(json.dumps({
            "exit": compiled.returncode, "stdout": compiled.stdout, "stderr": compiled.stderr,
        }))
        return
    missing = set(args.fixture) - {case[0] for case in args.case}
    if missing:
        raise AssertionError("No Bazel compiler result for fixtures: " + ", ".join(sorted(missing)))
    if not args.fixture:
        raise AssertionError("No compile-fail fixtures discovered")
    for case in args.case:
        check(*case)
        print(f"PASS {case[0]} (full diagnostic golden)")


if __name__ == "__main__":
    main()
