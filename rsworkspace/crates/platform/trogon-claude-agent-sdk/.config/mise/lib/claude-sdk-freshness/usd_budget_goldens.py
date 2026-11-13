#!/usr/bin/env python3
"""Records what Python's `str(float)` prints for a fixed corpus, so the `--max-budget-usd` text the Python dialect
sends is checked against CPython instead of against a reading of its source.

usage: python3 -I usd_budget_goldens.py OUT_FILE [--check]

`ties` pairs the bit pattern of a double (16 hex digits) whose shortest digits sit exactly halfway between two
candidates, such as 2**-25, with `str(value)`: CPython keeps the even digit there, where rounding half up would
not. `values` pairs every other double of the corpus with `str(value)`: zeros, the non-finite values, budget-like
amounts, every power of two, and powers of ten around the points where `repr` switches notation. The corpus is
drawn from a seeded generator, so the output only changes when this script or CPython's number printing does.
"""

import json
import math
import struct
import sys
from decimal import Decimal

args = sys.argv[1:]
if not args or args[0].startswith("-"):
    sys.stderr.write("usage: usd_budget_goldens.py OUT_FILE [--check]\n")
    sys.exit(2)
out, flags = args[0], args[1:]

seed = 0x9E3779B9


def next32():
    """mulberry32, the generator the js_stringify and python_json goldens use."""
    global seed
    seed = (seed + 0x6D2B79F5) & 0xFFFFFFFF
    t = seed
    t = ((t ^ (t >> 15)) * (t | 1)) & 0xFFFFFFFF
    t ^= (t + (((t ^ (t >> 7)) * (t | 61)) & 0xFFFFFFFF)) & 0xFFFFFFFF
    return (t ^ (t >> 14)) & 0xFFFFFFFF


def bits_of(value):
    return struct.unpack(">Q", struct.pack(">d", value))[0]


def from_bits(bits):
    return struct.unpack(">d", struct.pack(">Q", bits & 0xFFFFFFFFFFFFFFFF))[0]


def is_tie(value):
    """Whether another candidate with as many digits as `repr` printed reads back as `value` just as closely."""
    printed = Decimal(repr(value))
    exact = Decimal(value)
    step = Decimal(1).scaleb(printed.as_tuple().exponent)
    return any(
        float(other) == value and abs(other - exact) == abs(printed - exact)
        for other in (printed + step, printed - step)
    )


ties = {}
values = {}


def add(value):
    for item in (value, -value):
        key = f"{bits_of(item):016x}"
        if math.isfinite(item) and item != 0 and is_tie(item):
            ties.setdefault(key, str(item))
        else:
            values.setdefault(key, str(item))


for value in [0.0, math.inf, math.nan, 5e-324, sys.float_info.max, sys.float_info.min]:
    add(value)
for value in [0.01, 0.05, 0.1, 0.25, 0.3, 0.5, 1.0, 1.005, 2.5, 5.0, 9.99, 10.0, 12.34, 19.99, 99.99, 100.0, 1e3]:
    add(value)
for exponent in range(-1074, 1024):
    add(2.0**exponent)
for exponent in range(-324, 309):
    add(float(f"1e{exponent}"))
# repr switches to scientific notation below 1e-4 and from 1e16 on.
for exponent in range(-7, 19):
    bits = bits_of(float(f"1e{exponent}"))
    add(from_bits(bits - 1))
    add(from_bits(bits + 1))
# Odd multiples of powers of two, where the shortest digits land exactly halfway far more often than elsewhere.
for exponent in range(-1074, 1018):
    for multiple in range(1, 64, 2):
        value = math.ldexp(multiple, exponent)
        if is_tie(value):
            add(value)
for exponent in range(-40, 30):
    for multiple in range(1, 1 << 10, 2):
        value = math.ldexp(multiple, exponent)
        if is_tie(value):
            add(value)
for _ in range(500):
    value = from_bits((next32() << 32) | next32())
    if math.isfinite(value):
        add(value)


def lines(entries):
    return ",\n".join(f"    {json.dumps(pair)}" for pair in sorted(entries.items()))


body = f'{{\n  "ties": [\n{lines(ties)}\n  ],\n  "values": [\n{lines(values)}\n  ]\n}}\n'

if "--check" in flags:
    with open(out, encoding="utf-8") as handle:
        if handle.read() != body:
            sys.stderr.write(f"{out} is stale: run usd_budget_goldens.py without --check\n")
            sys.exit(1)
else:
    with open(out, "w", encoding="utf-8") as handle:
        handle.write(body)
