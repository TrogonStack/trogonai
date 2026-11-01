#!/usr/bin/env python3

import json
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
FIXTURES = Path(__file__).parent / "fixtures"


def main() -> int:
    failures: list[str] = []
    total = 0
    for fixture_path in sorted(FIXTURES.glob("*.json")):
        cases = json.loads(fixture_path.read_text())
        for case in cases:
            total += 1
            result = subprocess.run(
                [
                    "buf",
                    "convert",
                    ".",
                    "--type",
                    case["type"],
                    "--from=-#format=json",
                    "--to=/dev/null",
                    "--validate",
                ],
                cwd=ROOT,
                input=json.dumps(case["message"]),
                text=True,
                capture_output=True,
                check=False,
            )
            accepted = result.returncode == 0
            if accepted != case["valid"]:
                disposition = "accepted" if accepted else "rejected"
                failures.append(f'{fixture_path.name}: {case["name"]}: unexpectedly {disposition}')

    for failure in failures:
        print(failure)
    print(f"{total - len(failures)}/{total} key custody fixtures matched")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
