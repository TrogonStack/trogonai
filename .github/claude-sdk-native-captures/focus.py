"""Writes a copy of a widened scan that records only the cases whose names match PATTERN, so a level where Node
gives different results on different runs can be run many times.

usage: python3 -I focus.py SCAN OUT PATTERN
"""

import re
import sys

scan, out, pattern = sys.argv[1], sys.argv[2], sys.argv[3]
source = open(scan, encoding="utf-8").read()
anchor = "CASES = list({name: (name, files) for name, files in CASES}.values())\n"
if source.count(anchor) != 1:
    sys.stderr.write(f"{scan} is not a widened scan\n")
    sys.exit(1)
focus = f"CASES = [case for case in CASES if re.fullmatch({pattern!r}, case[0])]\n"
open(out, "w", encoding="utf-8").write(source.replace(anchor, anchor + focus))
