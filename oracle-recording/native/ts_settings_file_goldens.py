#!/usr/bin/env python3
"""Records how the pinned TypeScript `resolveSettings` reads settings files holding lone surrogates, so the
TypeScript dialect of the settings engine is checked against Node and the pinned package instead of against a
reading of the bundle.

usage: python3 -I ts_settings_file_goldens.py NODE PACKAGE_DIR OUT_FILE [--check]

NODE runs `resolve_settings_oracle.mjs` against PACKAGE_DIR, the unpacked pinned `@anthropic-ai/claude-agent-sdk`.

The settings reader picks UTF-16LE for a file that starts with FF FE and UTF-8 otherwise, and `JSON.parse` keeps
each lone surrogate, escaped or raw, in the JavaScript string it returns. Each `files` entry names a case, the
settings files of one resolve as the source that reads it and the hex of its bytes, and `effective`: the
`effective` member of what `resolveSettings` returns, with `toWellFormed` applied to every string and key as the
typed Rust result holds them, printed as `JSON.stringify` prints it (keys that become equal keep the first place and
the last value), plus `exact`, the same member as `JSON.stringify` prints it with every lone surrogate escaped, and
`result`, the whole object `resolveSettings` returns as `JSON.stringify` prints it, with the sandbox root taken off
every path so the home and project folders read as `/home/fixture` and `/work/project`.

lodash `mergeWith` merges the files into `effective`: it reads each member through `safeGet`, so an array or object
under `__proto__` merges nothing while a scalar there is written, and walks members as `for...in` does, array
indices first, which the `proto_merge` case records. Before that the settings schema builds the document, each
schema object and each record by assignment, which drops every `__proto__` member there while a value the schema
passes through whole keeps it, which the `proto_schema` case records.
"""

import json
import os
import re
import subprocess
import sys
import tempfile

args = sys.argv[1:]
if len(args) < 3 or any(arg.startswith("-") for arg in args[:3]):
    sys.stderr.write("usage: ts_settings_file_goldens.py NODE PACKAGE_DIR OUT_FILE [--check]\n")
    sys.exit(2)
node, package, out, flags = args[0], args[1], os.path.abspath(args[2]), args[3:]
ORACLE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "resolve_settings_oracle.mjs")
FOLDER = tempfile.mkdtemp()

PLACES = {
    "userSettings": "home/fixture/.claude/settings.json",
    "projectSettings": "work/project/.claude/settings.json",
    "localSettings": "work/project/.claude/settings.local.json",
}


def utf16(text):
    return text.encode("utf-16-le", "surrogatepass")


CASES = [
    ("escapes", [("userSettings", rb'{"x":"\ud800","y":["\udc00","\ud83d\ude00","\ufffd","\ude00\ud83d"]}')]),
    ("keys", [("userSettings", rb'{"x":{"\ud800":"a","\udc00":"b","k":"c","\ud800 ":"d"}}')]),
    (
        "union",
        [
            ("userSettings", rb'{"permissions":{"allow":["Bash(\ud800)"]}}'),
            ("projectSettings", rb'{"permissions":{"allow":["Bash(\udc00)","Bash(\ud800)"]}}'),
        ],
    ),
    ("collision", [("userSettings", rb'{"x":"\ud800"}'), ("projectSettings", rb'{"y":"\ue000"}')]),
    ("collision_raw", [("userSettings", rb'{"x":"\ud800"}'), ("projectSettings", '{"y":"\ue000"}'.encode())]),
    ("raw_utf8", [("userSettings", b'{"x":"a\xed\xa0\x80b","y":"\xff\xc0\xaf\xe2\x82"}')]),
    ("utf16", [("userSettings", b"\xff\xfe" + utf16('{"x":"\ud800","y":"\ud83d\ude00","z":["\udc00a"]}'))]),
    ("utf16_odd", [("userSettings", b"\xff\xfe" + utf16('{"x":"\udbff"}') + b"\x7b")]),
    ("utf16_split_pair", [("userSettings", b"\xff\xfe" + utf16('{"x":"\ud83d') + utf16('\\ude00"}'))]),
    ("utf16_escape_then_unit", [("userSettings", b"\xff\xfe" + utf16('{"x":"\\ud83d\ude00"}'))]),
    ("utf16_after_backslash", [("userSettings", b"\xff\xfe" + utf16('{"x":"\\\ud800"}'))]),
    ("utf16_after_two_backslashes", [("userSettings", b"\xff\xfe" + utf16('{"x":"\\\\\ud800"}'))]),
    ("utf16_outside_string", [("userSettings", b"\xff\xfe" + utf16('{"x":1\ud800}'))]),
    ("utf16_no_bom", [("userSettings", utf16('{"x":"a"}'))]),
    ("two_boms", [("userSettings", b'\xef\xbb\xbf\xef\xbb\xbf{"x":1}')]),
    ("bom_only", [("userSettings", b"\xef\xbb\xbf")]),
    (
        "proto_merge",
        [
            ("userSettings", b'{"__proto__":{"a":1},"x":{"__proto__":{"b":1},"k":1},"1":{"2":1,"0":2}}'),
            ("projectSettings", b'{"x":{"__proto__":[{"d":1}],"k":2},"y":{"__proto__":7},"1":{"1":3}}'),
        ],
    ),
    (
        "proto_schema",
        [
            (
                "userSettings",
                b'{"__proto__":7,"a":1,"env":{"__proto__":"v","A":"1"},'
                b'"permissions":{"__proto__":{"x":1},"allow":["A"]},"x":{"__proto__":7}}',
            ),
        ],
    ),
]


def well_formed(value):
    if isinstance(value, str):
        return re.sub("[\ud800-\udfff]", "\ufffd", value)
    if isinstance(value, list):
        return [well_formed(item) for item in value]
    if isinstance(value, dict):
        kept = {}
        for key, item in value.items():
            kept[well_formed(key)] = well_formed(item)
        return kept
    return value


def stringify(value, ensure_ascii):
    return json.dumps(value, ensure_ascii=ensure_ascii, separators=(",", ":"))


def exact(value):
    """`JSON.stringify` of `value`, which escapes each lone surrogate and leaves every other code point as is."""
    text = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    return re.sub("[\ud800-\udfff]", lambda found: f"\\u{ord(found.group()):04x}", text)


def record(name, files):
    root = os.path.join(FOLDER, name)
    os.makedirs(os.path.join(root, "home/fixture/.claude"))
    os.makedirs(os.path.join(root, "work/project/.claude"))
    for source, data in files:
        with open(os.path.join(root, PLACES[source]), "wb") as handle:
            handle.write(data)
    spec = {
        "root": root,
        "platform": sys.platform,
        "env": {"HOME": os.path.join(root, "home/fixture")},
        "cases": [{"name": name, "options": {"cwd": os.path.join(root, "work/project")}}],
    }
    spec_path = os.path.join(FOLDER, f"{name}.spec.json")
    with open(spec_path, "w", encoding="utf-8") as handle:
        json.dump(spec, handle)
    run = subprocess.run([node, ORACLE, package, spec_path], capture_output=True, check=True)
    result = json.loads(run.stdout.decode("utf-8"))[0]["result"]
    effective = result["effective"]
    return {
        "name": name,
        "files": [[source, data.hex()] for source, data in files],
        "effective": stringify(well_formed(effective), False),
        "exact": exact(effective),
        "result": exact(result).replace(json.dumps(root)[1:-1], ""),
    }


entries = [record(name, files) for name, files in CASES]
body = "{\n" + '  "files": [\n' + ",\n".join(f"    {json.dumps(entry)}" for entry in entries) + "\n  ]\n}\n"

if "--check" in flags:
    with open(out, encoding="utf-8") as handle:
        if handle.read() != body:
            sys.stderr.write(f"{out} is stale: run ts_settings_file_goldens.py without --check\n")
            sys.exit(1)
else:
    with open(out, "w", encoding="utf-8") as handle:
        handle.write(body)
