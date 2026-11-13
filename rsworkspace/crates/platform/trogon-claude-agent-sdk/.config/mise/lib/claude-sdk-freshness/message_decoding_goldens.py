#!/usr/bin/env python3
"""Combine the TypeScript and Python oracle outputs into the message decoding goldens.

usage: message_decoding_goldens.py TYPESCRIPT_JSON PYTHON_JSON EXPECTED_JSON [--check]

With --check the goldens are compared with EXPECTED_JSON instead of written to it.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path


def main(argv: list[str]) -> int:
    arguments = [argument for argument in argv[1:] if argument != "--check"]
    if len(arguments) != 3:
        print(__doc__.strip().splitlines()[2], file=sys.stderr)
        return 2
    typescript, python, expected = (Path(argument) for argument in arguments)
    typescript_results = json.loads(typescript.read_text())
    python_results = json.loads(python.read_text())
    if typescript_results.keys() != python_results.keys():
        print("The oracles ran different cases", file=sys.stderr)
        return 1
    goldens = {
        name: {"typescript": typescript_results[name], "python": python_results[name]} for name in typescript_results
    }
    rendered = json.dumps(goldens, indent=2, ensure_ascii=False) + "\n"
    if "--check" in argv:
        if expected.read_text() != rendered:
            print(f"{expected.name} does not match the pinned SDKs; rerun without --check", file=sys.stderr)
            return 1
        return 0
    expected.write_text(rendered)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
