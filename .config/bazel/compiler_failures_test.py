"""Exercise rejection using the real compiler results and diagnostic goldens."""

import argparse
import json
import os
from pathlib import Path
import tempfile

from compiler_failures import check


def rejects(fixture, golden, result):
    try:
        check(fixture, golden, result)
    except AssertionError:
        return
    raise AssertionError(f"Accepted an altered compiler contract: {fixture}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", action="append")
    parser.add_argument("--case", nargs=3, action="append", required=True)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(dir=os.environ["TEST_TMPDIR"]) as directory:
        altered_golden = Path(directory) / "altered.stderr"
        altered_result = Path(directory) / "altered.json"
        for fixture, golden, result in args.case:
            check(fixture, golden, result)
            lines = Path(golden).read_text().splitlines(keepends=True)
            checked = 0
            for index, line in enumerate(lines):
                if line.strip():
                    altered_golden.write_text("".join(lines[:index] + lines[index + 1:]))
                    rejects(fixture, altered_golden, result)
                    checked += 1
            compiled = json.loads(Path(result).read_text())
            for status in [0, 101, -11]:
                altered_result.write_text(json.dumps(compiled | {"exit": status}))
                rejects(fixture, golden, altered_result)
            unrelated = {"level": "error", "message": "unrelated failure", "spans": [], "rendered": "error: unrelated failure\n"}
            for stderr in [json.dumps(unrelated), compiled["stderr"] + json.dumps(unrelated) + "\n"]:
                altered_result.write_text(json.dumps(compiled | {"stderr": stderr}))
                rejects(fixture, golden, altered_result)
            print(f"PASS {fixture}: {checked} diagnostic lines, success, crash, and unrelated failures rejected")


if __name__ == "__main__":
    main()
