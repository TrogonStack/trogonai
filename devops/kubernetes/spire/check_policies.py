"""Evaluate freshly generated admission policies without asserting artifact trust."""

import argparse
import json
import os
import subprocess
import tempfile
from pathlib import Path

from validation_fixture import deployment_fixture


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write-fixture", action="store_true", help="regenerate the reviewed Go test fixture")
    args = parser.parse_args()
    workspace = Path(__file__).resolve().parents[3] / "goworkspace"
    reviewed_fixture = workspace / "internal/spirepolicy/testdata/deployment.json"
    expected = (json.dumps(deployment_fixture(), indent=2, sort_keys=True) + "\n").encode()
    if args.write_fixture:
        reviewed_fixture.write_bytes(expected)
        return 0
    try:
        current = reviewed_fixture.read_bytes()
    except OSError:
        current = None
    if current != expected:
        parser.exit(1, "Go policy fixture is missing or stale; regenerate with check_policies.py --write-fixture and review the diff\n")
    with tempfile.TemporaryDirectory(prefix="spire-policy-") as directory:
        fixture_path = Path(directory) / "fixture.json"
        descriptor = os.open(fixture_path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        with os.fdopen(descriptor, "wb") as output:
            output.write(expected)
        environment = os.environ | {
            "SPIRE_POLICY_FIXTURE": str(fixture_path),
            "GOTOOLCHAIN": "local",
            "GOFLAGS": "-mod=readonly",
        }
        result = subprocess.run([
            "go", "-C", str(workspace),
            "test", "-count=1", "./internal/spirepolicy",
        ], env=environment)
    return result.returncode if result.returncode >= 0 else 128 - result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
