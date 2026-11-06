"""Evaluate freshly generated admission policies without asserting artifact trust."""

import json
import os
import subprocess
import tempfile
from pathlib import Path

from validation_fixture import deployment_fixture


def main():
    with tempfile.TemporaryDirectory(prefix="spire-policy-") as directory:
        fixture_path = Path(directory) / "fixture.json"
        descriptor = os.open(fixture_path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        with os.fdopen(descriptor, "w") as output:
            json.dump(deployment_fixture(), output)
        environment = os.environ | {
            "SPIRE_POLICY_FIXTURE": str(fixture_path),
            "GOTOOLCHAIN": "local",
            "GOFLAGS": "-mod=readonly",
        }
        result = subprocess.run([
            "go", "-C", str(Path(__file__).resolve().parent / "celtest"),
            "test", "-count=1", "./...",
        ], env=environment)
    return result.returncode if result.returncode >= 0 else 128 - result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
