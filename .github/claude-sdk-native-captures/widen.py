"""Writes a copy of a golden recorder whose profile for the running platform records a window of levels around each
boundary, so a boundary that sits elsewhere on this host shows where it sits.

usage: python3 -I widen.py RECORDER OUT KEEP

KEEP lists, comma separated, the profile keys the recorder uses as single levels; they keep their value.
"""

import re
import sys

recorder, out, keep = sys.argv[1], sys.argv[2], sys.argv[3]
source = open(recorder, encoding="utf-8").read()
anchor = 'TARGET = f"{sys.platform} {platform.machine()}"\n'
if source.count(anchor) != 1:
    sys.stderr.write(f"{recorder} does not pick its profile as expected\n")
    sys.exit(1)
widened = anchor + f"""_KEEP = {sorted(filter(None, keep.split(",")))!r}


def _widen(value):
    low, high = (value[0], value[-1]) if isinstance(value, list) else (value, value + 1)
    return list(range(low - 3, high + 4))


if TARGET in PROFILES:
    PROFILES[TARGET] = {{key: value if key in _KEEP else _widen(value) for key, value in PROFILES[TARGET].items()}}
"""
source = source.replace(anchor, widened)
# Neighbouring boundaries share case names, so a window can name a case twice: record it once.
cases = re.search(r"^CASES = .*\n", source, re.M)
if cases is None:
    sys.stderr.write(f"{recorder} has no CASES line\n")
    sys.exit(1)
unique = "CASES = list({name: (name, files) for name, files in CASES}.values())\n"
source = source[: cases.end()] + unique + source[cases.end() :]
open(out, "w", encoding="utf-8").write(source)
