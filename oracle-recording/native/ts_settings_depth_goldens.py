#!/usr/bin/env python3
"""Records how the pinned TypeScript `resolveSettings` reads and merges deeply nested settings files on Node, so
the stack profile the TypeScript dialect of the settings engine follows is checked against Node and the pinned
package instead of against a reading of the bundle.

usage: python3 -I ts_settings_depth_goldens.py NODE PACKAGE_DIR OUT_FILE [--check | --observe RUNS HOST]

NODE runs `resolve_settings_oracle.mjs` against PACKAGE_DIR, the unpacked pinned `@anthropic-ai/claude-agent-sdk`.
The stack V8 has, and so every boundary here, depends on the Node release and the platform: `PROFILES` holds the
boundaries of each platform a `NodeStack` profile names, and the recorder records the cases of the platform it runs
on, so record each golden file with Node 24.13.1 on its own platform (the linux ones in the `node:24.13.1-bookworm`
image). On macOS the stack also depends on how much of it the command line and environment fill when Node starts:
each argument and each `NAME=value` entry takes its UTF-8 length plus 9 bytes, and the path of the executable once
more. Node runs here with only a `PAD` variable, sized so they fill `STARTUP_BYTES`, the startup `NodeStack` assumes
by default. On Linux V8 sets its limit below them, so they move no boundary.

A profile names a boundary by the last level on its near side, and the cases record that level and the next. V8
compiles functions on background threads and installs the code at times that vary from run to run, which moves where
a deep merge runs out. `NodeStack` assumes no background compile lands while a merge runs, so each case records what
Node answers with `REFERENCE_FLAGS`, which keep every function in the interpreter, and the recorder stops on any case
where Node with `CROSS_CHECK_FLAGS`, which compile on the main thread only, answers otherwise. `flags` and
`cross_check_flags` hold them, and `node` the Node release, platform and machine, so each golden can be recorded
again. With `--observe RUNS HOST` the recorder runs every case RUNS times with the default flags, two runs per core
at once, and replaces the `observed` entry of HOST, which lists as `bands` each case some run answered otherwise,
with every answer and how many runs gave it. It still writes them when no run gave the recorded answer of a case,
then names that case and fails.

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

import concurrent.futures
import json
import os
import platform
import re
import subprocess
import sys
import tempfile

args = sys.argv[1:]
if len(args) < 3 or any(arg.startswith("-") for arg in args[:3]) or (args[3:4] == ["--observe"] and len(args) != 6):
    sys.stderr.write("usage: ts_settings_depth_goldens.py NODE PACKAGE_DIR OUT_FILE [--check | --observe RUNS HOST]\n")
    sys.exit(2)
node, package, out, flags = args[0], args[1], os.path.abspath(args[2]), args[3:]
ORACLE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "resolve_settings_oracle.mjs")
FOLDER = tempfile.mkdtemp()
sys.setrecursionlimit(20000)
STARTUP_BYTES = 4384
REFERENCE_FLAGS = ["--no-opt", "--no-maglev", "--no-sparkplug"]
CROSS_CHECK_FLAGS = ["--no-concurrent-recompilation", "--no-concurrent-osr", "--no-concurrent-sparkplug"]
ENSURE_ASCII = False

PLACES = {
    "userSettings": "home/.claude/settings.json",
    "projectSettings": "work/.claude/settings.json",
    "localSettings": "work/.claude/settings.local.json",
}

U, P = "userSettings", "projectSettings"
HELD_TWICE, ARRAY_LIKE = '"a":[1],"b":[1],', '"a":{"length":1},"b":{"length":1},'


def levels(level):
    """A profile names a boundary by the last level on its near side: the cases record it and the next."""
    return [level, level + 1]


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
        "merge": 1307, "read_objects": 1649, "merge_arrays": 1306, "read_arrays": 3297, "mixed_100": 3198,
        "mixed_999": 1202, "mixed_1447": 1001, "copy": 1565, "copy_104": 1466, "union_objects": 1646,
        "leaf_empty_object": 1648, "leaf_empty_array": 1648, "leaf_true_array": 1648, "leaf_true": 1649,
        "leaf_object_in_arrays": 3296, "copy_empty_object": 1565, "copy_scalar_object": 1564, "copy_inner": 104,
        "merge_empty_object": 1307, "merge_empty_array": 1306, "merge_held_array": 1305, "merge_true": 1307,
        "merge_after_array": 1307, "merge_after_scalar": 1308, "merge_after_held_arrays": 1307,
        "merge_after_held_array": 1305, "merge_after_nested_held_arrays": 1307, "merge_after_held_arrays_of_source": 1307,
        "merge_after_array_like_copies": 1307, "merge_after_empty_array": 1306, "index_compile_inside": 1333,
        "index_compile": 1362, "index_compile_onto": 1362,
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


ANSWERS = ("effective", "error")


def answer(result):
    return {"effective": json.dumps(result["effective"], ensure_ascii=False, separators=(",", ":")), "error": None}


def rejection(error):
    return {"effective": None, "error": error}


def entry_bytes(text):
    return len(text.encode("utf-8")) + 9


def pad(command):
    """The length of the `PAD` value that makes the command line and environment fill `STARTUP_BYTES`."""
    used = sum(entry_bytes(arg) for arg in command) + entry_bytes(f"executable_path={command[0]}") + entry_bytes("PAD=")
    if used > STARTUP_BYTES:
        sys.stderr.write(f"the command line takes {used} bytes, more than {STARTUP_BYTES}: use shorter paths\n")
        sys.exit(1)
    return STARTUP_BYTES - used


def prepare(name, files):
    """Writes the files and the oracle spec of the case `name`, and returns the path of the spec."""
    root = os.path.join(FOLDER, name)
    os.makedirs(os.path.join(root, "home/.claude"))
    os.makedirs(os.path.join(root, "work/.claude"))
    for source, *shape in files:
        with open(os.path.join(root, PLACES[source]), "w", encoding="utf-8") as handle:
            handle.write(text(*shape))
    spec = {
        "root": root,
        "platform": sys.platform,
        "env": {"HOME": os.path.join(root, "home")},
        "deep": True,
        "cases": [{"name": name, "options": {"cwd": os.path.join(root, "work")}}],
    }
    spec_path = os.path.join(FOLDER, f"{name}.spec.json")
    with open(spec_path, "w", encoding="utf-8") as handle:
        json.dump(spec, handle)
    return spec_path


def run(spec_path, node_flags):
    """What `resolveSettings` answers for the case `spec_path` names, on Node started with `node_flags`."""
    command = [node, *node_flags, ORACLE, package, spec_path]
    # Node can exit on an uncaught error before a pipe drains the long source line it prints, so stderr goes to a file.
    with tempfile.TemporaryFile() as errors:
        process = subprocess.run(command, stdout=subprocess.PIPE, stderr=errors, env={"PAD": "x" * pad(command)})
        errors.seek(0)
        stderr = errors.read().decode("utf-8")
    if process.returncode == 0:
        return answer(json.loads(process.stdout.decode("utf-8"))[0]["result"])
    found = re.search(r"^(\w*Error): (.*)$", stderr, re.M)
    if not found:
        sys.stderr.write(stderr)
        sys.exit(1)
    return rejection({"name": found.group(1), "message": found.group(2)})


def reference(name, files):
    """What Node answers with `REFERENCE_FLAGS`, once Node with `CROSS_CHECK_FLAGS` answered the same."""
    spec_path = prepare(name, files)
    answered = run(spec_path, REFERENCE_FLAGS)
    checked = run(spec_path, CROSS_CHECK_FLAGS)
    if checked != answered:
        sys.stderr.write(f"{name}: {json.dumps(answered)} with {REFERENCE_FLAGS}, {json.dumps(checked)} with {CROSS_CHECK_FLAGS}\n")
        sys.exit(1)
    return {"name": name, "files": [list(file) for file in files], **answered}


def observe(entries, runs, host):
    """The `observed` entry of `host`: every case `runs` times with the default flags, two runs per core at once."""
    specs = [prepare(name, files) for name, files in CASES]
    with concurrent.futures.ThreadPoolExecutor(max_workers=2 * (os.cpu_count() or 1)) as pool:
        pending = [[pool.submit(run, spec_path, []) for _ in range(runs)] for spec_path in specs]
        answers = [[future.result() for future in futures] for futures in pending]
    bands, missed = [], []
    for entry, answered in zip(entries, answers):
        recorded = {key: entry[key] for key in ANSWERS}
        if recorded not in answered:
            missed.append(entry["name"])
        distinct = []
        for one in answered:
            if one not in distinct:
                distinct.append(one)
        if distinct != [recorded]:
            bands.append({"name": entry["name"], "outcomes": [{**one, "runs": answered.count(one)} for one in distinct]})
    return {"host": host, "flags": [], "runs": runs, "bands": bands}, missed


def observed_text(observed):
    hosts = []
    for seen in observed:
        head = json.dumps({key: seen[key] for key in ("host", "flags", "runs")})[:-1]
        bands = ",\n".join(f"        {json.dumps(band, ensure_ascii=ENSURE_ASCII)}" for band in seen["bands"])
        hosts.append(f'    {head}, "bands": [' + (f"\n{bands}\n    ]}}" if bands else "]}"))
    return "[\n" + ",\n".join(hosts) + "\n  ]" if hosts else "[]"


def body(entries, observed):
    return (
        "{\n"
        + f'  "node": {json.dumps(LABEL)},\n'
        + f'  "flags": {json.dumps(REFERENCE_FLAGS)},\n'
        + f'  "cross_check_flags": {json.dumps(CROSS_CHECK_FLAGS)},\n'
        + f'  "startup_bytes": {STARTUP_BYTES},\n'
        + '  "cases": [\n'
        + ",\n".join(f"    {json.dumps(entry, ensure_ascii=ENSURE_ASCII)}" for entry in entries)
        + "\n  ],\n"
        + f'  "observed": {observed_text(observed)}\n'
        + "}\n"
    )


version = subprocess.run([node, "--version"], capture_output=True, check=True, env={}).stdout.decode().strip()
LABEL = f"{version} {sys.platform} {platform.machine()}"
previous = None
if os.path.exists(out):
    with open(out, encoding="utf-8") as handle:
        previous = json.load(handle)

if flags[:1] == ["--observe"]:
    names = [name for name, _ in CASES]
    if previous is None or previous["node"] != LABEL or [entry["name"] for entry in previous["cases"]] != names:
        sys.stderr.write(f"{out} holds no goldens of these cases from {LABEL}: record them without --observe first\n")
        sys.exit(1)
    seen, missed = observe(previous["cases"], int(flags[1]), flags[2])
    observed = [entry for entry in previous.get("observed", []) if entry["host"] != seen["host"]] + [seen]
    with open(out, "w", encoding="utf-8") as handle:
        handle.write(body(previous["cases"], observed))
    if missed:
        sys.stderr.write(f"no run with the default flags gave the recorded answer of {', '.join(missed)}\n")
        sys.exit(1)
    sys.exit(0)

text_out = body([reference(name, files) for name, files in CASES], previous.get("observed", []) if previous else [])
if "--check" in flags:
    stale = previous is None
    if not stale:
        with open(out, encoding="utf-8") as handle:
            stale = handle.read() != text_out
    if stale:
        sys.stderr.write(f"{out} is stale: run ts_settings_depth_goldens.py without --check\n")
        sys.exit(1)
else:
    with open(out, "w", encoding="utf-8") as handle:
        handle.write(text_out)
