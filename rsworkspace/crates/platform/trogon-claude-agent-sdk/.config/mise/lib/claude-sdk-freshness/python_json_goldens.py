#!/usr/bin/env python3
"""Records what Python's json module reads and prints for a fixed corpus, so the Rust `python_json_loads` and
`python_json_dumps` are checked against CPython instead of against a reading of its source.

usage: python3 -I python_json_goldens.py OUT_FILE [--check]

`numbers` pairs the bit pattern of a finite double (16 hex digits) with `json.dumps(value)`. `documents` pairs
JSON source text with `json.dumps(json.loads(text))`, which covers exact integers, float spellings, the NaN and
Infinity constants, duplicate keys and string escapes. `refused` pairs text `json.loads` refuses with the kind of
error and, for the errors whose text the Rust port reproduces, the message. The corpus is drawn from a seeded
generator, so the output only changes when this script or CPython's number printing does.
"""

import json
import struct
import sys

args = sys.argv[1:]
if not args or args[0].startswith("-"):
    sys.stderr.write("usage: python_json_goldens.py OUT_FILE [--check]\n")
    sys.exit(2)
out, flags = args[0], args[1:]

seed = 0x9E3779B9


def next32():
    """mulberry32, the generator the js_stringify goldens use."""
    global seed
    seed = (seed + 0x6D2B79F5) & 0xFFFFFFFF
    t = seed
    t = ((t ^ (t >> 15)) * (t | 1)) & 0xFFFFFFFF
    t ^= (t + (((t ^ (t >> 7)) * (t | 61)) & 0xFFFFFFFF)) & 0xFFFFFFFF
    return (t ^ (t >> 14)) & 0xFFFFFFFF


def below(n):
    return next32() % n


def pick(items):
    return items[below(len(items))]


def bits_of(value):
    return struct.unpack(">Q", struct.pack(">d", value))[0]


def from_bits(bits):
    return struct.unpack(">d", struct.pack(">Q", bits & 0xFFFFFFFFFFFFFFFF))[0]


def finite(value):
    return value == value and value not in (float("inf"), float("-inf"))


numbers = {}


def add(value):
    for item in [value, -value] if below(4) == 0 else [value]:
        if finite(item):
            numbers.setdefault(f"{bits_of(item):016x}", json.dumps(item))


def neighbors(value):
    if value == 0:
        return [value]
    bits = bits_of(value)
    return [from_bits(bits - 1), value, from_bits(bits + 1)]


for value in [0.0, 5e-324, 1e-323, 2.225073858507201e-308, 2.2250738585072014e-308, sys.float_info.max]:
    for item in neighbors(value):
        add(item)
for value in [-0.0, -5e-324, -sys.float_info.max]:
    numbers.setdefault(f"{bits_of(value):016x}", json.dumps(value))
# Every power of ten and its neighbors: repr switches notation at 1e-4 and 1e16.
for exponent in range(-324, 309):
    for item in neighbors(float(f"1e{exponent}")):
        add(item)
for exponent in range(-1074, 1024):
    value = 2.0**exponent
    if -24 <= exponent <= 72:
        for item in neighbors(value):
            add(item)
    else:
        add(value)
for exponent in [-7, -6, -5, -4, -3, 14, 15, 16, 17]:
    for mantissa in range(1, 26):
        add(float(f"{mantissa}e{exponent}"))
        add(float(f"{mantissa / 10}e{exponent}"))
for _ in range(2000):
    value = from_bits((next32() << 32) | next32())
    if finite(value):
        add(value)
for _ in range(1000):
    digits = 1 + below(17)
    text = str(1 + below(9)) + "".join(str(below(10)) for _ in range(digits - 1))
    add(float(f"{text}e{below(80) - 40}"))

hand_written = [
    "0", "-0", "0.0", "-0.0", "1.0", "1E5", "1e16", "1e15", "0.00001", "0.0001", "1e-400", "-1e-400", "1e400",
    "-1e400", "1E400", "NaN", "Infinity", "-Infinity", "[NaN, Infinity, -Infinity]",
    "9007199254740993", "-9007199254740993", "18446744073709551615", "18446744073709551616",
    "-9223372036854775808", "-9223372036854775809", "123456789012345678901234567890",
    "-98765432109876543210987654321", "1" + "0" * 400, "-" + "9" * 4300, "1.7976931348623157e308",
    "[]", "{}", "[[],{},[{}]]", ' { "a" : [ 1 , 2 ] } ', "\t\n\r [1]\r\n", "true", "false", "null", '""',
    '{"b":1,"a":2,"1":3,"0":4}',
    '{"a":1,"b":2,"a":3}',
    '{"a":1e400,"b":NaN,"a":-Infinity}',
    '["\\u0000\\u0001\\u001f\\u007f\\u0080\\u2028\\u2029\\ufeff"]',
    '"\\"\\\\\\/\\b\\f\\n\\r\\t"',
    '"\\ud83d\\ude00 é 日本 \\u00e9 \U0001f600"',
    '{"\\u0000":"\\u0000","\\n":"\\t","\\"":"\\\\"}',
    '{"a":[1.0,2.50,-0.0,1e21,1e+21,1E-7,0.1e1,10e-1,1e-5,1.5e-5]}',
]

refused_texts = [
    "1" * 4301, "-" + "1" * 4301, "[" + "0" * 4301 + "]", '{"a":' + "9" * 5000 + "}", "﻿{}",
    "-NaN", "nan", "infinity", "Infinityx", "[01]", "[1.]", "[1e]", "[+1]", "[.5]", "[1,]", '{"a":1,}', "[1] 2",
    "", " ", "[", '{"a"}', "'a'", '"\\x41"', '"a\nb"', "[1 2]", "- 1", "[-]", "{1:2}",
]

key_pool = ["a", "b", "model", "0", "1", "10", "01", "-1", "é", "\n", '"', "", "\U0001f600"]
string_pool = ["", "a", '"', "\\", "/", "\u0000", "\u0008", "\u000b", "\u001f", "\u007f", " ", "é", "日本",
               "\U0001f600", " "]


def token():
    choice = below(12)
    if choice == 0:
        return pick(["NaN", "Infinity", "-Infinity"])
    sign = "-" if below(4) == 0 else ""
    int_digits = 1 + below(40)
    text = "0" if below(10) == 0 else str(1 + below(9))
    if text != "0":
        text += "".join(str(below(10)) for _ in range(int_digits - 1))
    if below(3) == 0:
        text += "." + "".join(str(below(10)) for _ in range(1 + below(12)))
    if below(3) == 0:
        text += pick(["e", "E"]) + pick(["", "+", "-"]) + str(below(400))
    return sign + text


def document(depth):
    kind = below(4) if depth >= 4 else below(6)
    if kind in (0, 3):
        return token()
    if kind == 1:
        return json.dumps("".join(pick(string_pool) for _ in range(below(3) + 1)), ensure_ascii=below(2) == 0)
    if kind == 2:
        return pick(["true", "false", "null"])
    if kind == 4:
        return "[" + ",".join(document(depth + 1) for _ in range(below(5))) + "]"
    entries = [f"{json.dumps(pick(key_pool), ensure_ascii=False)}:{document(depth + 1)}" for _ in range(below(6))]
    return "{" + ",".join(entries) + "}"


generated = [document(0) for _ in range(400)]

documents = [[source, json.dumps(json.loads(source))] for source in hand_written + generated]


def refusal(source):
    try:
        json.loads(source)
    except json.JSONDecodeError as error:
        if str(error).startswith("Unexpected UTF-8 BOM"):
            return [source, "bom", str(error)]
        return [source, "syntax", None]
    except ValueError as error:
        return [source, "integer", str(error)]
    raise SystemExit(f"json.loads accepted {source!r}")


refused = [refusal(source) for source in refused_texts]


def lines(pairs):
    return ",\n".join(f"    {json.dumps(pair)}" for pair in pairs)


body = (
    "{\n"
    f'  "numbers": [\n{lines(sorted(numbers.items(), key=lambda item: item[0]))}\n  ],\n'
    f'  "documents": [\n{lines(documents)}\n  ],\n'
    f'  "refused": [\n{lines(refused)}\n  ]\n'
    "}\n"
)

if "--check" in flags:
    with open(out, encoding="utf-8") as handle:
        if handle.read() != body:
            sys.stderr.write(f"{out} is stale: run python_json_goldens.py without --check\n")
            sys.exit(1)
else:
    with open(out, "w", encoding="utf-8") as handle:
        handle.write(body)
