#!/usr/bin/env python3
"""Download and verify the Python source release pinned by baseline.json."""

from __future__ import annotations

import shutil
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import check  # noqa: E402


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print("usage: fetch_pinned_python.py DESTINATION", file=sys.stderr)
        return 2
    destination = Path(argv[1])
    baseline = check.read_json(HERE / "baseline.json")
    pinned = baseline["sources"]["python"]
    with tempfile.TemporaryDirectory() as scratch:
        package, version, artifact = check.fetch_python(Path(scratch), pinned["version"])
        if version != pinned["version"] or artifact["integrity"] != pinned["artifact"]["integrity"]:
            raise check.FetchError("Fetched Python package does not match the reviewed baseline artifact")
        shutil.move(str(package), str(destination))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv))
    except check.CheckError as error:
        print(f"fetch_pinned_python: {error}", file=sys.stderr)
        sys.exit(2)
