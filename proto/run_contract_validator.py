"""Runs the contract validator at the path given first.

The validator finds the repository root and its fixture suites relative to its
own file, so it runs from the test's private copy of the workspace instead of
the runfiles, whose links lead back to the source tree.
"""

import runpy
import sys

script = sys.argv.pop(1)
sys.argv[0] = script
runpy.run_path(script, run_name="__main__")
