"""Bazel entry point that runs these scripts the way the mise tasks run them.

The modules here import each other by bare name, and one of them is named
`profile`, as is a standard library module. `python3 script.py` and
`python3 -m unittest discover -s <dir>` put this directory ahead of the
standard library on sys.path; rules_python does not, so this restores that
order before anything here is imported.

Tests are loaded from the test_*.py files in this directory only, not through
unittest discovery, which would also walk the directories rules_python places
beside them and fail to import those as test packages.

Usage: bazel_entry.py --discover | bazel_entry.py <script.py> [args...]
"""

import os
import runpy
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))


def main(argv):
    sys.path.insert(0, HERE)
    if argv == ["--discover"]:
        names = sorted(
            entry.name.removesuffix(".py")
            for entry in os.scandir(HERE)
            if entry.name.startswith("test_") and entry.name.endswith(".py")
        )
        suite = unittest.defaultTestLoader.loadTestsFromNames(names)
        if not suite.countTestCases():
            raise SystemExit(f"no test_*.py tests found in {HERE}")
        return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1
    script, *arguments = argv
    sys.argv = [os.path.join(HERE, script), *arguments]
    runpy.run_path(sys.argv[0], run_name="__main__")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
