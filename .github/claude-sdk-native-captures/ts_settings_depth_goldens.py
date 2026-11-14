#!/usr/bin/env python3
"""Records how the pinned TypeScript `resolveSettings` reads and merges deeply nested settings files on Node, so
the stack profile the TypeScript dialect of the settings engine follows is checked against Node and the pinned
package instead of against a reading of the bundle.

usage: python3 -I ts_settings_depth_goldens.py NODE PACKAGE_DIR OUT_FILE [--check]

NODE runs `resolve_settings_oracle.mjs` against PACKAGE_DIR, the unpacked pinned `@anthropic-ai/claude-agent-sdk`.
The stack V8 has, and so every boundary here, depends on the Node release and the platform: `PROFILES` holds the
boundaries of each platform a `NodeStack` profile names, and the recorder records the cases of the platform it runs
on, so record each golden file with Node 24.13.1 on its own platform (the linux ones in the `node:24.13.1-bookworm`
image). On macOS the stack also depends on how much of it the command line and environment fill when Node starts:
each argument and each `NAME=value` entry takes its UTF-8 length plus 9 bytes, and the path of the executable once
more. Node runs here with only a `PAD` variable, sized so they fill `STARTUP_BYTES`, the startup `NodeStack` assumes
by default. On Linux V8 sets its limit below them, so they move no boundary.

A profile names a boundary by the last level on its near side, and the cases record that level and the next. Where
a boundary moves between runs, as the stack the reader starts with varies by a few bytes, the profile names the
levels to record outright, one on each side of it: the linux x86_64 profile, recorded under emulation on arm64, does
that for the reader's walk of nested objects and for the compile of the `isIndex` pattern at the end of the merge's
reach.

Each `cases` entry names a resolve and its settings files as `[source, objects, arrays, inner]`: a root object
holding `objects - 1` nested objects under `x`, then `arrays` nested arrays, then `inner` nested objects around
`null`. A fifth member is the JSON text that takes the place of `null`, and a sixth the members the root object holds
before `x`. V8 compiles the regular expression lodash `isIndex` tests array indexes with on its first two tests in
the process, wherever the array sits, which the `merge_after_*` and `merge_index_compile_*` cases record: running out
of stack there rejects with a `SyntaxError`, and lodash reaches such a test past the levels the merge otherwise
fits. The oracle runs with `deep`, so it prints what `resolveSettings` returns without recursing and a result nested
past the stack `JSON.stringify` has (about 2100 levels) still records. `effective` is the `effective` member of what
`resolveSettings` returns, printed as `JSON.stringify` prints it, or `null` when it rejects, and `error` is the error
it rejects with, or `null`.
"""

import json
import os
import platform
import re
import subprocess
import sys
import tempfile

args = sys.argv[1:]
if len(args) < 3 or any(arg.startswith("-") for arg in args[:3]):
    sys.stderr.write("usage: ts_settings_depth_goldens.py NODE PACKAGE_DIR OUT_FILE [--check]\n")
    sys.exit(2)
node, package, out, flags = args[0], args[1], os.path.abspath(args[2]), args[3:]
ORACLE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "resolve_settings_oracle.mjs")
FOLDER = tempfile.mkdtemp()
sys.setrecursionlimit(20000)
STARTUP_BYTES = 4384

PLACES = {
    "userSettings": "home/.claude/settings.json",
    "projectSettings": "work/.claude/settings.json",
    "localSettings": "work/.claude/settings.local.json",
}

U, P = "userSettings", "projectSettings"
HELD_TWICE, ARRAY_LIKE = '"a":[1],"b":[1],', '"a":{"length":1},"b":{"length":1},'


def levels(level):
    """A profile names a boundary by the last level on its near side, or names the levels to record outright."""
    return level if isinstance(level, list) else [level, level + 1]


def pair(name, files):
    return lambda level: [(f"{name}_{n}", files(n)) for n in levels(level)]


GROUPS = [
    ("merge", lambda n: [(f"objects_{n}", [(U, n, 0, 0)]), (f"objects_{n + 1}", [(U, n + 1, 0, 0)]),
                         (f"objects_{n + 1}_project", [(P, n + 1, 0, 0)])]),
    ("read_objects", pair("objects", lambda n: [(U, n, 0, 0)])),
    ("merge_arrays", pair("arrays", lambda n: [(U, 1, n, 0)])),
    ("read_arrays", pair("arrays", lambda n: [(U, 1, n, 0)])),
    ("mixed_100", lambda level: [(f"mixed_100_{n}", [(U, 100, n, 0)]) for n in levels(level)]),
    ("mixed_999", lambda level: [(f"mixed_{n}_999", [(U, n, 999, 0)]) for n in levels(level)]),
    ("mixed_1447", lambda level: [(f"mixed_{n}_1447", [(U, n, 1447, 0)]) for n in levels(level)]),
    ("copy", pair("union", lambda n: [(U, 1, 1, 0), (P, 1, n, 0)])),
    ("copy_104", lambda level: [(f"union_{n}_104", [(U, 1, 1, 0), (P, 1, n, 104)]) for n in levels(level)]),
    ("read_arrays", lambda level: [(f"union_skipped_{levels(level)[-1]}", [(U, 1, 1, 0), (P, 1, levels(level)[-1], 0)])]),
    ("merge", lambda n: [(f"merged_onto_{n}", [(U, n, 0, 0), (P, n, 0, 0)])]),
    ("merge", lambda n: [(f"array_onto_object_{n}", [(U, n, 0, 0), (P, 1, n - 1, 0)])]),
    ("read_objects merge", lambda read, n: [("skipped_then_merged", [(U, levels(read)[-1], 0, 0), (P, n, 0, 0)])]),
    ("union_objects", pair("union_objects", lambda n: [(U, 1, 1, 0), (P, 1, 1, n)])),
    ("leaf_empty_object", pair("leaf_empty_object", lambda n: [(U, n, 0, 0, "{}")])),
    ("leaf_empty_array", pair("leaf_empty_array", lambda n: [(U, n, 0, 0, "[]")])),
    ("leaf_true_array", pair("leaf_true_array", lambda n: [(U, n, 0, 0, "[true]")])),
    ("leaf_true", pair("leaf_true", lambda n: [(U, n, 0, 0, "true")])),
    ("leaf_object_in_arrays", pair("leaf_object_in_arrays", lambda n: [(U, 1, n, 0, '{"a":true}')])),
    ("copy_empty_object", pair("copy_empty_object", lambda n: [(U, 1, 1, 0), (P, 1, n, 0, "{}")])),
    ("copy_scalar_object", pair("copy_scalar_object", lambda n: [(U, 1, 1, 0), (P, 1, n, 0, '{"a":null}')])),
    ("copy_104 copy_inner", lambda arrays, inner: [(f"copy_empty_object_{arrays}_{n}", [(U, 1, 1, 0), (P, 1, arrays, n, "{}")])
                                                   for n in levels(inner)]),
    ("merge_empty_object", pair("merge_empty_object", lambda n: [(U, n, 0, 0, "{}")])),
    ("merge_empty_array", pair("merge_empty_array", lambda n: [(U, n, 0, 0, "[]")])),
    ("merge_held_array", pair("merge_held_array", lambda n: [(U, n, 0, 0, "[null]")])),
    ("merge_true", pair("merge_true", lambda n: [(U, n, 0, 0, "true")])),
    ("merge_after_array", pair("merge_after_array", lambda n: [(U, n, 0, 0, "[]", '"a":[],')])),
    ("merge_after_scalar", pair("merge_after_scalar", lambda n: [(U, n, 0, 0, "null", '"a":1,')])),
    ("merge_after_held_arrays", pair("merge_after_held_arrays", lambda n: [(U, n, 1, 0, "null", HELD_TWICE)])),
    ("merge_after_held_array", pair("merge_after_held_array", lambda n: [(U, n, 2, 0, "null", '"a":[1],')])),
    ("merge_after_nested_held_arrays", pair("merge_after_nested_held_arrays", lambda n: [(U, n, 1, 0, "null", '"a":[[1]],')])),
    ("merge_after_held_arrays_of_source", pair("merge_after_held_arrays_of_source",
                                               lambda n: [(U, 1, 0, 0, "null", HELD_TWICE), (P, n, 1, 0)])),
    ("merge_after_array_like_copies", pair("merge_after_array_like_copies",
                                           lambda n: [(U, 1, 0, 0, "null", ARRAY_LIKE), (P, n, 1, 0, "null", HELD_TWICE)])),
    ("merge_after_empty_array", pair("merge_after_empty_array", lambda n: [(U, n, 0, 0, "[null]", '"a":[],')])),
    ("index_compile_inside", lambda n: [(f"merge_index_compile_{n}", [(U, n, 1, 0, "null", '"a":[1],')])]),
    ("index_compile", pair("merge_index_compile", lambda n: [(U, n, 1, 0, "null", '"a":[1],')])),
    ("index_compile_onto", pair("merge_index_compile_onto", lambda n: [(U, 1240, 0, 0, "null", '"a":[1],'), (P, n, 1, 0)])),
]

PROFILES = {
    "darwin arm64": {
        "merge": 1244, "read_objects": 2223, "merge_arrays": 1243, "read_arrays": 3276, "mixed_100": 3182,
        "mixed_999": 1688, "mixed_1447": 1448, "copy": 1945, "copy_104": 1886, "union_objects": 2221,
        "leaf_empty_object": 2223, "leaf_empty_array": 2222, "leaf_true_array": 2222, "leaf_true": 2223,
        "leaf_object_in_arrays": 3275, "copy_empty_object": 1944, "copy_scalar_object": 1944, "copy_inner": 104,
        "merge_empty_object": 1244, "merge_empty_array": 1243, "merge_held_array": 1242, "merge_true": 1244,
        "merge_after_array": 1244, "merge_after_scalar": 1245, "merge_after_held_arrays": 1244,
        "merge_after_held_array": 1242, "merge_after_nested_held_arrays": 1244, "merge_after_held_arrays_of_source": 1244,
        "merge_after_array_like_copies": 1244, "merge_after_empty_array": 1243, "index_compile_inside": 1270,
        "index_compile": 1296, "index_compile_onto": 1296,
    },
    "linux aarch64": {
        "merge": 1252, "read_objects": 1957, "merge_arrays": 1251, "read_arrays": 3131, "mixed_100": 3032,
        "mixed_999": 1426, "mixed_1447": 1188, "copy": 1788, "copy_104": 1693, "union_objects": 1955,
        "leaf_empty_object": 1957, "leaf_empty_array": 1957, "leaf_true_array": 1956, "leaf_true": 1957,
        "leaf_object_in_arrays": 3130, "copy_empty_object": 1788, "copy_scalar_object": 1787, "copy_inner": 104,
        "merge_empty_object": 1252, "merge_empty_array": 1251, "merge_held_array": 1250, "merge_true": 1252,
        "merge_after_array": 1252, "merge_after_scalar": 1253, "merge_after_held_arrays": 1252,
        "merge_after_held_array": 1250, "merge_after_nested_held_arrays": 1252, "merge_after_held_arrays_of_source": 1252,
        "merge_after_array_like_copies": 1252, "merge_after_empty_array": 1251, "index_compile_inside": 1277,
        "index_compile": 1304, "index_compile_onto": 1304,
    },
    "linux x86_64": {
        "merge": 1307, "read_objects": [1648, 1650], "merge_arrays": 1306, "read_arrays": 3297, "mixed_100": 3198,
        "mixed_999": 1202, "mixed_1447": 1001, "copy": 1565, "copy_104": 1466, "union_objects": 1646,
        "leaf_empty_object": 1648, "leaf_empty_array": 1648, "leaf_true_array": 1648, "leaf_true": [1648, 1650],
        "leaf_object_in_arrays": 3296, "copy_empty_object": 1565, "copy_scalar_object": 1564, "copy_inner": 104,
        "merge_empty_object": 1307, "merge_empty_array": 1306, "merge_held_array": 1305, "merge_true": 1307,
        "merge_after_array": 1307, "merge_after_scalar": 1308, "merge_after_held_arrays": 1307,
        "merge_after_held_array": 1305, "merge_after_nested_held_arrays": 1307, "merge_after_held_arrays_of_source": 1307,
        "merge_after_array_like_copies": 1307, "merge_after_empty_array": 1306, "index_compile_inside": 1333,
        "index_compile": [1361, 1363], "index_compile_onto": [1361, 1363],
    },
}

TARGET = f"{sys.platform} {platform.machine()}"
if TARGET not in PROFILES:
    sys.stderr.write(f"no profile records {TARGET}: add its boundaries to PROFILES\n")
    sys.exit(1)
CASES = [case for keys, make in GROUPS for case in make(*(PROFILES[TARGET][key] for key in keys.split()))]


def text(objects, arrays, inner, leaf="null", first=""):
    opening = "{" + first + '"x":' + '{"x":' * (objects - 1) + "[" * arrays + '{"x":' * inner
    return opening + leaf + "}" * inner + "]" * arrays + "}" * objects


def entry_bytes(text):
    return len(text.encode("utf-8")) + 9


def pad(command):
    """The length of the `PAD` value that makes the command line and environment fill `STARTUP_BYTES`."""
    used = sum(entry_bytes(arg) for arg in command) + entry_bytes(f"executable_path={command[0]}") + entry_bytes("PAD=")
    if used > STARTUP_BYTES:
        sys.stderr.write(f"the command line takes {used} bytes, more than {STARTUP_BYTES}: use shorter paths\n")
        sys.exit(1)
    return STARTUP_BYTES - used


def record(name, files):
    root = os.path.join(FOLDER, name)
    os.makedirs(os.path.join(root, "home/.claude"))
    os.makedirs(os.path.join(root, "work/.claude"))
    for source, *shape in files:
        with open(os.path.join(root, PLACES[source]), "w", encoding="utf-8") as handle:
            handle.write(text(*shape))
    spec = {
        "root": root,
        "platform": "linux",
        "env": {"HOME": os.path.join(root, "home")},
        "deep": True,
        "cases": [{"name": name, "options": {"cwd": os.path.join(root, "work")}}],
    }
    spec_path = os.path.join(FOLDER, f"{name}.spec.json")
    with open(spec_path, "w", encoding="utf-8") as handle:
        json.dump(spec, handle)
    command = [node, ORACLE, package, spec_path]
    # Node can exit on an uncaught error before a pipe drains the long source line it prints, so stderr goes to a file.
    with tempfile.TemporaryFile() as errors:
        run = subprocess.run(command, stdout=subprocess.PIPE, stderr=errors, env={"PAD": "x" * pad(command)})
        errors.seek(0)
        stderr = errors.read().decode("utf-8")
    entry = {"name": name, "files": [list(file) for file in files]}
    if run.returncode == 0:
        effective = json.loads(run.stdout.decode("utf-8"))[0]["result"]["effective"]
        entry["effective"] = json.dumps(effective, ensure_ascii=False, separators=(",", ":"))
        entry["error"] = None
    else:
        found = re.search(r"^(\w*Error): (.*)$", stderr, re.M)
        if not found:
            sys.stderr.write(stderr)
            sys.exit(1)
        entry["effective"] = None
        entry["error"] = {"name": found.group(1), "message": found.group(2)}
    return entry


version = subprocess.run([node, "--version"], capture_output=True, check=True, env={}).stdout.decode().strip()
entries = [record(name, files) for name, files in CASES]
body = (
    "{\n"
    + f'  "node": {json.dumps(f"{version} {sys.platform} {platform.machine()}")},\n'
    + f'  "startup_bytes": {STARTUP_BYTES},\n'
    + '  "cases": [\n'
    + ",\n".join(f"    {json.dumps(entry, ensure_ascii=False)}" for entry in entries)
    + "\n  ]\n}\n"
)

if "--check" in flags:
    with open(out, encoding="utf-8") as handle:
        if handle.read() != body:
            sys.stderr.write(f"{out} is stale: run ts_settings_depth_goldens.py without --check\n")
            sys.exit(1)
else:
    with open(out, "w", encoding="utf-8") as handle:
        handle.write(body)
