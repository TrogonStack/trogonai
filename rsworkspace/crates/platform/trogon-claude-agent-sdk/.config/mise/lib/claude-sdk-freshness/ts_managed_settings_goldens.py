#!/usr/bin/env python3
"""Records how the pinned TypeScript `resolveSettings` reads deeply nested managed settings documents and managed
documents holding lone surrogates on Node, so the managed reader of the TypeScript dialect of the settings engine is
checked against Node and the pinned package instead of against a reading of the bundle.

usage: python3 -I ts_managed_settings_goldens.py NODE PACKAGE_DIR OUT_FILE [--check | --observe RUNS HOST]

NODE runs `resolve_settings_oracle.mjs` against PACKAGE_DIR, the unpacked pinned `@anthropic-ai/claude-agent-sdk`,
on linux, with the command line and environment filling `STARTUP_BYTES` as `ts_settings_depth_goldens.py` explains.
Every boundary depends on the Node release and the platform: `PROFILES` holds the boundaries Node showed on each platform, the recorder records the cases of the platform it runs on, and each golden file comes
from Node 24.13.1 on its own platform, as `ts_settings_depth_goldens.py` explains. A `PROFILES` entry names a boundary by the
last level on its near side, and `partial_drop` by how deep `z` nests. Each case records what Node answers with
`REFERENCE_FLAGS` once Node with `CROSS_CHECK_FLAGS` answered the same, and `--observe RUNS HOST` records under
`observed` the cases that answer otherwise with the default flags, as `ts_settings_depth_goldens.py` explains.

A managed document takes other passes than a settings file: the reader parses it inside the try that turns an
overflow into an unreadable document, the file cache copies it, the main file merges into its folder outside that
try, each drop-in merges inside it (an overflow there gives up on the rest of the folder), and the admin settings
merge onto a fresh object outside it. Every case reads a user settings file first, which the boundaries were
measured with. A merge writes as it goes, so a drop-in that runs out keeps what it merged before.

`resolveSettings` evaluates the managed settings three times: for the settings, for their origin and for whether the
machine policy loaded, each on a stack of its own, while the reader caches each document from the first evaluation
that reads it. A drop-in that runs out leaves the file tier unloaded, and the later evaluations then reach drop-ins
the first one never read: the `origin_read` and `machine_read` cases read one first in the second and the third. The
later evaluations run after the sources merge, so the arrays holding members those merges entered count toward the
two that V8 spends more on, as the `machine_after_held_*` cases record.

Each `cases` entry names a resolve and its files as `[place, objects, arrays, inner]`, a root object holding
`objects - 1` nested objects under `x`, then `arrays` nested arrays, then `inner` nested objects around `null`, as
`[place, objects, arrays, inner, key]`, the same under `key` at the root, or as `[place, text]`. `effective` is the `effective` member of what `resolveSettings` returns, printed by `JSON.stringify`
after `toWellFormed`, `exact` is the same member with every lone surrogate escaped, as `JSON.stringify` prints it,
and `machine_policy` is the `machinePolicy` member, each `null` when it rejects; `error` is the error it rejects
with, or `null`.
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
    sys.stderr.write("usage: ts_managed_settings_goldens.py NODE PACKAGE_DIR OUT_FILE [--check | --observe RUNS HOST]\n")
    sys.exit(2)
node, package, out, flags = args[0], args[1], os.path.abspath(args[2]), args[3:]
ORACLE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "resolve_settings_oracle.mjs")
FOLDER = tempfile.mkdtemp()
sys.setrecursionlimit(20000)
STARTUP_BYTES = 4384
REFERENCE_FLAGS = ["--no-opt", "--no-maglev", "--no-sparkplug"]
CROSS_CHECK_FLAGS = ["--no-concurrent-recompilation", "--no-concurrent-osr", "--no-concurrent-sparkplug"]
ENSURE_ASCII = True

PLACES = {
    "user": "home/.claude/settings.json",
    "managed": "etc/claude-code/managed-settings.json",
    "drop_a": "etc/claude-code/managed-settings.d/a.json",
    "drop_b": "etc/claude-code/managed-settings.d/b.json",
}
USER = ["user", '{"u":[null]}']
HELD_TWICE = ["user", '{"u":[1],"v":[1]}']
HELD = ["managed", '{"x":[null]}']
LATER = ["drop_b", '{"y":1}']
SERVERS = ["managed", '{"managedMcpServers":{}}']


def levels(level):
    """A `PROFILES` entry names a boundary by the last level on its near side: the cases record it and the next."""
    return [level, level + 1]


def pair(name, files):
    return lambda level: [(f"{name}_{n}", files(n)) for n in levels(level)]


def partial(objects):
    """A drop-in whose merge runs out inside `z`, `objects` nested objects that read but do not merge."""
    return '{"r":1,"p":{"q":{"k":1},"z":' + '{"x":' * objects + "null" + "}" * objects + ',"w":2},"s":1}'


def later_read(name):
    return lambda drop, level: [
        (f"{name}_{n}", [USER, SERVERS, ["drop_a", drop, 0, 0], ["drop_b", n, 0, 0, "managedMcpServers"]])
        for n in levels(level)
    ]


GROUPS = [
    ("main_merge", pair("main_objects", lambda n: [USER, ["managed", n, 0, 0]])),
    ("main_merge_arrays", pair("main_arrays", lambda n: [USER, ["managed", 1, n, 0]])),
    ("main_read", pair("main_objects", lambda n: [USER, ["managed", n, 0, 0]])),
    ("main_read_arrays", pair("main_arrays", lambda n: [USER, ["managed", 1, n, 0]])),
    ("drop_merge", pair("drop_objects", lambda n: [USER, ["drop_a", n, 0, 0], LATER])),
    ("drop_merge_arrays", pair("drop_arrays", lambda n: [USER, ["drop_a", 1, n, 0], LATER])),
    ("drop_read", pair("drop_objects", lambda n: [USER, ["drop_a", n, 0, 0], LATER])),
    ("drop_read_arrays", pair("drop_arrays", lambda n: [USER, ["drop_a", 1, n, 0], LATER])),
    ("admin_arrays", pair("admin_arrays", lambda n: [USER, HELD, ["drop_a", 1, n, 0]])),
    ("admin_inner", pair("admin_inner", lambda n: [USER, HELD, ["drop_a", 1, 1, n]])),
    ("sources_copy", pair("copy_arrays", lambda n: [USER, HELD, ["drop_a", 1, n, 0], LATER])),
    ("machine_merge", pair("drop_objects", lambda n: [USER, ["drop_a", n, 0, 0], LATER])),
    ("machine_copy", pair("copy_arrays", lambda n: [USER, HELD, ["drop_a", 1, n, 0], LATER])),
    ("origin_drop origin_read", later_read("origin_read")),
    ("machine_drop machine_read", later_read("machine_read")),
    ("machine_after_held_array", pair("machine_after_held_array", lambda n: [USER, ["drop_a", n, 1, 0], LATER])),
    ("machine_after_held_arrays", pair("machine_after_held_arrays", lambda n: [HELD_TWICE, ["drop_a", n, 1, 0], LATER])),
    (
        "partial_drop",
        lambda n: [("partial_drop", [USER, ["managed", '{"p":{"a":1}}'], ["drop_a", partial(n)], LATER])],
    ),
]

FIXED = [
    ("surrogates_main", [USER, ["managed", '{"s":"\\ud800a","t":["\\udfff","\\ud83d\\ude00"]}']]),
    ("surrogates_drop", [USER, ["drop_a", '{"s":"\\udc00b"}'], ["drop_b", '{"t":"\\ud83d"}']]),
    (
        "surrogates_beside_private_use",
        [["user", '{"u":"\ue800"}'], ["managed", '{"s":"\\ud800\ue000"}'], ["drop_a", '{"t":"\\ud800\uf000"}']],
    ),
]

PROFILES = {
    "darwin arm64": {
        "main_merge": 1241, "main_merge_arrays": 1240, "main_read": 2217, "main_read_arrays": 3268,
        "drop_merge": 1241, "drop_merge_arrays": 1240, "drop_read": 2217, "drop_read_arrays": 3268,
        "admin_arrays": 1243, "admin_inner": 1242, "sources_copy": 1941, "machine_merge": 1244, "machine_copy": 1945,
        "origin_drop": 1242, "origin_read": 2220, "machine_drop": 1244, "machine_read": 2222,
        "machine_after_held_array": 1241, "machine_after_held_arrays": 1243, "partial_drop": 1300,
    },
    "linux aarch64": {
        "main_merge": 1249, "main_merge_arrays": 1248, "main_read": 1952, "main_read_arrays": 3123,
        "drop_merge": 1249, "drop_merge_arrays": 1248, "drop_read": 1952, "drop_read_arrays": 3123,
        "admin_arrays": 1251, "admin_inner": 1250, "sources_copy": 1784, "machine_merge": 1252, "machine_copy": 1788,
        "origin_drop": 1250, "origin_read": 1954, "machine_drop": 1252, "machine_read": 1956,
        "machine_after_held_array": 1249, "machine_after_held_arrays": 1251, "partial_drop": 1300,
    },
    "linux x86_64": {
        "main_merge": 1304, "main_merge_arrays": 1303, "main_read": 1644, "main_read_arrays": 3289,
        "drop_merge": 1304, "drop_merge_arrays": 1303, "drop_read": 1644, "drop_read_arrays": 3289,
        "admin_arrays": 1306, "admin_inner": 1305, "sources_copy": 1562, "machine_merge": 1307, "machine_copy": 1565,
        "origin_drop": 1305, "origin_read": 1646, "machine_drop": 1307, "machine_read": 1648,
        "machine_after_held_array": 1304, "machine_after_held_arrays": 1306, "partial_drop": 1360,
    },
}

TARGET = f"{sys.platform} {platform.machine()}"
if TARGET not in PROFILES:
    sys.stderr.write(f"no profile records {TARGET}: add its boundaries to PROFILES\n")
    sys.exit(1)
CASES = [case for keys, make in GROUPS for case in make(*(PROFILES[TARGET][key] for key in keys.split()))] + FIXED


def text(objects, arrays, inner, key="x"):
    opening = f'{{"{key}":' + '{"x":' * (objects - 1) + "[" * arrays + '{"x":' * inner
    return opening + "null" + "}" * inner + "]" * arrays + "}" * objects


def content(file):
    return file[1] if len(file) == 2 else text(*file[1:])


def well_formed(value):
    if isinstance(value, str):
        return re.sub("[\ud800-\udfff]", "\ufffd", value)
    if isinstance(value, list):
        return [well_formed(item) for item in value]
    if isinstance(value, dict):
        return {well_formed(key): well_formed(item) for key, item in value.items()}
    return value


def stringify(value, ensure_ascii):
    return json.dumps(value, ensure_ascii=ensure_ascii, separators=(",", ":"))


def exact(value):
    """`JSON.stringify` of `value`, which escapes each lone surrogate and leaves every other code point as is."""
    return re.sub("[\ud800-\udfff]", lambda found: f"\\u{ord(found.group()):04x}", stringify(value, False))


ANSWERS = ("effective", "exact", "machine_policy", "error")


def answer(result):
    return {
        "effective": stringify(well_formed(result["effective"]), False),
        "exact": exact(result["effective"]),
        "machine_policy": stringify(result["machinePolicy"], False),
        "error": None,
    }


def rejection(error):
    return {"effective": None, "exact": None, "machine_policy": None, "error": error}


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
    for file in files:
        location = os.path.join(root, PLACES[file[0]])
        os.makedirs(os.path.dirname(location), exist_ok=True)
        with open(location, "w", encoding="utf-8") as handle:
            handle.write(content(file))
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
        sys.stderr.write(f"{out} is stale: run ts_managed_settings_goldens.py without --check\n")
        sys.exit(1)
else:
    with open(out, "w", encoding="utf-8") as handle:
        handle.write(text_out)
