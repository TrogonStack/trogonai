#!/usr/bin/env python3
"""Records how the pinned TypeScript `resolveSettings` reads deeply nested managed settings documents and managed
documents holding lone surrogates on Node, so the managed reader of the TypeScript dialect of the settings engine is
checked against Node and the pinned package instead of against a reading of the bundle.

usage: python3 -I ts_managed_settings_goldens.py NODE PACKAGE_DIR OUT_FILE [--check | --observe RUNS HOST]

NODE runs `resolve_settings_oracle.mjs` against PACKAGE_DIR, the unpacked pinned `@anthropic-ai/claude-agent-sdk`,
on linux, with the command line and environment filling `STARTUP_BYTES` as `ts_settings_depth_goldens.py` explains.
Every boundary depends on the Node release and the platform: `PROFILES` holds the boundaries of each platform a
`NodeStack` profile names, the recorder records the cases of the platform it runs on, and each golden file comes
from Node 24.13.1 on its own platform, as `ts_settings_depth_goldens.py` explains. A profile names a boundary by the
last level on its near side, and `partial_drop` by how deep `z` nests. Each case records what Node answers with
`REFERENCE_FLAGS` once Node with `CROSS_CHECK_FLAGS` answered the same, and `--observe RUNS HOST` records under
`observed` the cases that answer otherwise with the default flags, as `ts_settings_depth_goldens.py` explains.

A managed document takes other passes than a settings file: the reader parses it inside the try that turns an
overflow into an unreadable document, the file cache copies it, the main file merges into its folder outside that
try, each drop-in merges inside it (an overflow there gives up on the rest of the folder), and the admin settings
merge onto a fresh object outside it. Every case reads a user settings file first, which the boundaries were
measured with. A merge writes as it goes, so a drop-in that runs out keeps what it merged before.

`resolveSettings` evaluates the managed settings four times: for the settings sources, for the effective settings, for
their origin and for whether the machine policy loaded, each on a stack of its own, while the reader caches each
document from the first evaluation that reads it. A drop-in that runs out leaves the file tier unloaded, and the
evaluations for the origin and the machine policy then reach drop-ins the earlier ones never read: the `origin_read` and
`machine_read` cases read one first in each of them, and `origin_read_arrays` and `machine_read_arrays` one holding
nested arrays, which the reader's clone runs out on first. `origin_copy` nests arrays under `__proto__` in a
drop-in, which the file cache copies and the merge skips: a profile names the last level that copy fits in the origin
evaluation. Those evaluations run after the sources merge, so the arrays holding members those merges entered count
toward the two that V8 spends more on, as the `machine_after_held_*` cases record. `machine_merge` nests objects in a
drop-in whose merge into the folder runs out in the sources evaluation, and a profile names the last level that merge
fits in the machine policy evaluation. The origin evaluation's merge shows only in what it reads next: `origin_merge`
puts the same drop-in before one the origin evaluation's walk runs out on, whose overflow the reader caches once that
evaluation reaches it, so the machine policy evaluation reads it afresh only when the origin evaluation's merge ran out
first, and a profile names the last level that merge fits.

The `org_*` cases read the managed settings as an organization session (`CLAUDE_CODE_ENTRYPOINT` is `remote_cowork`)
run by root, with no settings sources, so the managed settings file is writable and only its policy members load,
next to a machine policy in the registry that a write check holds once the session runs under WSL. Each resolves
twice in one process, `ORG_STEPS` apart, the second under WSL, and records one answer per resolve. The reader then
reads the managed settings twice: once keeping what each document may hold, and once more restricting the copies
the first pass cached once the held registry document asks it to. `org_<path>_<level>` puts objects nested `level`
deep under each path the restriction copies, and `org_copy` nested arrays under a member nothing reads,
which the first pass copies as deep as the clone allows: a profile names the last level that copy fits. The first
resolve starts as soon as the package loads, as every other case does, and the second once the clock reaches its
`at`.

The settings and the effective settings are two evaluations: the sources first, then the effective settings on a
stack with less room, reading the documents the first one cached. The `effective_*` cases read as an organization
session with the settings sources left as they are. `effective_copy` puts nested arrays under a member nothing
reads in the managed settings file, whose copy in the effective evaluation runs out while the one for the sources
still fits, and `effective_ask` puts them next to a permission a drop-in asks for, which the effective settings lose
once that copy runs out while the machine policy still loads it: a profile names the last level each one fits.

A case that records a boundary is named after the profile key that names it, then the level it reads, so the
boundaries Node measured can be read back from the goldens.

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
import shutil
import subprocess
import sys
import tempfile
import threading

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

# Where upstream reads the machine-wide managed settings on this host (MYe, sdk.mjs 0.3.323 line 146 col 23907).
MANAGED_FOLDER = {"darwin": "Library/Application Support/ClaudeCode", "win32": "Program Files/ClaudeCode"}.get(
    sys.platform, "etc/claude-code")
PLACES = {
    "user": "home/.claude/settings.json",
    "managed": f"{MANAGED_FOLDER}/managed-settings.json",
    "drop_a": f"{MANAGED_FOLDER}/managed-settings.d/a.json",
    "drop_b": f"{MANAGED_FOLDER}/managed-settings.d/b.json",
    "hklm": "mnt/c/Windows/System32/hklm.txt",
}
USER = ["user", '{"u":[null]}']
HELD_TWICE = ["user", '{"u":[1],"v":[1]}']
HELD = ["managed", '{"x":[null]}']
LATER = ["drop_b", '{"y":1}']
SERVERS = ["managed", '{"managedMcpServers":{}}']
# The managed settings the parent process hands over, which the policy origin names once no managed file loads.
PARENT = ["parent", '{"permissions":{"deny":["Bash(p:*)"]}}']
# What closes the nested value of a `proto_*` case: an array holding `null`, an empty array, an object holding
# `null` or an empty object.
LEAVES = {"null_array": "[null]", "empty_array": "[]", "null_object": '{"x":null}', "empty_object": "{}"}

# The organization cases: a stand-in for reg.exe under the root answers the machine policy from `hklm.txt`. A Windows
# host cannot run it, so there `REGISTRY_STAND_IN` names `registry_stand_in.rs` built, which answers the same.
REGISTRY_TOOL = "mnt/c/Windows/System32/reg.exe"
REGISTRY_STAND_IN = """#!/bin/sh
d=$(dirname "$0")
case "$2" in
HKLM*) f=hklm ;;
HKCU*) f=hkcu ;;
*) f=none ;;
esac
if [ -f "$d/$f.txt" ]; then cat "$d/$f.txt"; exit 0; fi
echo "ERROR: The system was unable to find the specified registry key or value." >&2
exit 1
"""
ORG_ENV = {"CLAUDE_CODE_ENTRYPOINT": "remote_cowork"}
ORG_PROCESS = {"euid": 0, "uid": 0, "egid": 0, "gid": 0, "groups": [0]}
ORG_STEPS = [
    {"files": {}},
    {"at": 1800000, "files": {"proc/version": "Linux version 5.15.153.1-microsoft-standard-WSL2\n"}},
]
HELD_REGISTRY = ["hklm", '{"permissions":{"deny":["Bash(curl:*)"]},"allowedMcpServers":[{"serverName":"h"}]}']
ORG_LEVELS = (1300, 2300)
# Each path the restriction of a held document copies, with `%s` where the nested value goes.
ORG_PATHS = {
    "allowed_mcp_servers": '"allowedMcpServers":[{"serverName":"a","z":%s}]',
    "denied_mcp_servers": '"deniedMcpServers":[{"serverName":"a","z":%s}]',
    "strict_known_marketplaces": '"strictKnownMarketplaces":[{"source":"github","repo":"a/b","z":%s}]',
    "allowed_marketplaces": '"allowedMarketplaces":[{"source":"github","repo":"a/b","z":%s}]',
    "blocked_marketplaces": '"blockedMarketplaces":[{"source":"github","repo":"a/b","z":%s}]',
    "strict_plugin_only_customization": '"strictPluginOnlyCustomization":%s',
    "available_models_match": '"availableModelsMatch":%s',
    "denied_models": '"deniedModels":%s',
    "allowed_providers": '"allowedProviders":%s',
    "enforce_available_models": '"enforceAvailableModels":%s',
    "disabled_mcpjson_servers": '"disabledMcpjsonServers":%s',
    "allowed_http_hook_urls": '"allowedHttpHookUrls":%s',
    "sigv4": '"sandbox":{"credentials":{"sigv4":%s}}',
    "aws_pairs": '"sandbox":{"credentials":{"awsPairs":%s}}',
    "unknown_member": '"disableFoo":%s',
    "deny_read": '"sandbox":{"filesystem":{"denyRead":%s}}',
    "deny_read_member": '"sandbox":{"filesystem":{"denyRead":["a",%s]}}',
    "denied_domains": '"sandbox":{"network":{"deniedDomains":%s}}',
    "credential_files": '"sandbox":{"credentials":{"files":%s}}',
    "credential_env_vars": '"sandbox":{"credentials":{"envVars":%s}}',
    "strict_allowlist": '"sandbox":{"network":{"strictAllowlist":%s}}',
    "plugin_options": '"pluginConfigs":{"security-guidance@claude-plugins-official":{"options":{"allowManagedModsOnly":%s}}}',
    "permissions_ask": '"permissions":{"deny":["Bash(rm:*)"],"ask":%s}',
    "aws_pair_member": '"sandbox":{"credentials":{"awsPairs":[{"accessKeyIdVar":"A","z":%s}]}}',
    "sigv4_member": '"sandbox":{"credentials":{"sigv4":[{"service":"s3","z":%s}]}}',
}


def org_managed(path, value):
    return ["managed", '{"permissions":{"deny":["Bash(rm:*)"]},' + ORG_PATHS[path] % value + "}"]


def nested_objects(level):
    return '{"x":' * (level - 1) + "null" + "}" * (level - 1)


def nested_arrays(level):
    return "[" * level + "null" + "]" * level


def proto(objects, arrays, leaf):
    """A drop-in holding, under `__proto__` inside `x`, `objects` nested objects, then `arrays` nested arrays, then
    the leaf `leaf` names: the file cache copy enters `__proto__`, while lodash `mergeWith` reads it as undefined."""
    return '{"x":{"__proto__":' + '{"x":' * objects + "[" * arrays + LEAVES[leaf] + "]" * arrays + "}" * objects + "}}"


def proto_cases(name):
    """The two cases a profile names for `name`: each `[objects, arrays, leaf, startup_bytes]`."""
    return lambda first, second: [
        (f"{name}_{objects}_{arrays}_{leaf}_{startup}", [USER, PARENT, ["drop_a", proto(objects, arrays, leaf)]], startup)
        for objects, arrays, leaf, startup in (first, second)
    ]


def effective_ask(level):
    """A drop-in asking for a permission next to `level` nested arrays under a member nothing reads."""
    return [
        USER,
        ["managed", '{"permissions":{"deny":["Bash(rm:*)"]}}'],
        ["drop_a", '{"permissions":{"ask":["Bash(x:*)"]},"disableFoo":' + nested_arrays(level) + "}"],
    ]


def org_cases():
    return [
        (f"org_{path}_{level}", [org_managed(path, nested_objects(level)), HELD_REGISTRY])
        for path in ORG_PATHS
        for level in ORG_LEVELS
    ]


def levels(level):
    """A profile names a boundary by the last level on its near side: the cases record it and the next."""
    return [level, level + 1]


def pair(name, files):
    return lambda level: [(f"{name}_{n}", files(n)) for n in levels(level)]


def partial(objects):
    """A drop-in whose merge runs out inside `z`, `objects` nested objects that read but do not merge."""
    return '{"r":1,"p":{"q":{"k":1},"z":' + '{"x":' * objects + "null" + "}" * objects + ',"w":2},"s":1}'


def later_read(name, arrays=False):
    """A drop-in that runs out at `drop`, then one `level` deep under `managedMcpServers`: nested objects, which the
    reader's walk runs out on first, or with `arrays` one object holding nested arrays, which its clone runs out on
    first."""
    shape = (lambda n: [1, n, 0]) if arrays else (lambda n: [n, 0, 0])
    return lambda drop, level: [
        (f"{name}_{n}", [USER, SERVERS, ["drop_a", drop, 0, 0], ["drop_b", *shape(n), "managedMcpServers"]])
        for n in levels(level)
    ]


def later_merge(name):
    """A drop-in `level` deep, then one under `managedMcpServers` one object deeper than the origin evaluation's walk
    reaches, which the machine policy evaluation's walk still reaches."""
    return lambda level, read: [
        (f"{name}_{n}", [USER, SERVERS, ["drop_a", n, 0, 0], ["drop_b", read + 1, 0, 0, "managedMcpServers"]])
        for n in levels(level)
    ]


GROUPS = [
    ("main_merge", pair("main_merge", lambda n: [USER, ["managed", n, 0, 0]])),
    ("main_merge_arrays", pair("main_merge_arrays", lambda n: [USER, ["managed", 1, n, 0]])),
    ("main_read", pair("main_read", lambda n: [USER, ["managed", n, 0, 0]])),
    ("main_read_arrays", pair("main_read_arrays", lambda n: [USER, ["managed", 1, n, 0]])),
    ("drop_merge", pair("drop_merge", lambda n: [USER, ["drop_a", n, 0, 0], LATER])),
    ("drop_merge_arrays", pair("drop_merge_arrays", lambda n: [USER, ["drop_a", 1, n, 0], LATER])),
    ("drop_read", pair("drop_read", lambda n: [USER, ["drop_a", n, 0, 0], LATER])),
    ("drop_read_arrays", pair("drop_read_arrays", lambda n: [USER, ["drop_a", 1, n, 0], LATER])),
    ("admin_arrays", pair("admin_arrays", lambda n: [USER, HELD, ["drop_a", 1, n, 0]])),
    ("admin_inner", pair("admin_inner", lambda n: [USER, HELD, ["drop_a", 1, 1, n]])),
    ("sources_copy", pair("sources_copy", lambda n: [USER, HELD, ["drop_a", 1, n, 0], LATER])),
    ("origin_merge origin_read", later_merge("origin_merge")),
    ("machine_merge", pair("machine_merge", lambda n: [USER, ["drop_a", n, 0, 0], LATER])),
    ("machine_copy", pair("machine_copy", lambda n: [USER, HELD, ["drop_a", 1, n, 0], LATER])),
    ("origin_drop origin_read", later_read("origin_read")),
    ("machine_drop machine_read", later_read("machine_read")),
    ("origin_drop origin_read_arrays", later_read("origin_read_arrays", arrays=True)),
    ("machine_drop machine_read_arrays", later_read("machine_read_arrays", arrays=True)),
    ("origin_copy", pair("origin_copy", lambda n: [USER, PARENT, ["drop_a", proto(0, n, "null_array")]])),
    ("machine_after_held_array", pair("machine_after_held_array", lambda n: [USER, ["drop_a", n, 1, 0], LATER])),
    ("machine_after_held_arrays", pair("machine_after_held_arrays", lambda n: [HELD_TWICE, ["drop_a", n, 1, 0], LATER])),
    (
        "partial_drop",
        lambda n: [("partial_drop", [USER, ["managed", '{"p":{"a":1}}'], ["drop_a", partial(n)], LATER])],
    ),
    (
        "org_copy",
        lambda n: [(f"org_copy_{m}", [org_managed("unknown_member", nested_arrays(m)), HELD_REGISTRY]) for m in levels(n)],
    ),
    (
        "effective_copy",
        lambda n: [(f"effective_copy_{m}", [USER, org_managed("unknown_member", nested_arrays(m))]) for m in levels(n)],
    ),
    (
        "effective_ask effective_ask_machine",
        lambda ask, machine: [(f"effective_ask_{m}", effective_ask(m)) for m in levels(ask)]
        + [(f"effective_ask_machine_{m}", effective_ask(m)) for m in levels(machine)],
    ),
    ("proto_effective", lambda pair: proto_cases("proto_effective")(*pair)),
    ("proto_sources", lambda pair: proto_cases("proto_sources")(*pair)),
    ("proto_origin", lambda pair: proto_cases("proto_origin")(*pair)),
    ("proto_machine", lambda pair: proto_cases("proto_machine")(*pair)),
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
        "origin_read_arrays": 3271, "machine_read_arrays": 3274, "origin_copy": 1942, "origin_merge": 1243,
        "machine_after_held_array": 1241, "machine_after_held_arrays": 1243, "partial_drop": 1300,
        "org_copy": 1873, "effective_copy": 1940, "effective_ask": 1940, "effective_ask_machine": 1945,
        "proto_effective": [[0, 1939, "null_array", 4384], [0, 1940, "null_array", 4384]],
        "proto_sources": [[0, 1940, "null_array", 4384], [0, 1941, "null_array", 4384]],
        "proto_origin": [[0, 1942, "null_array", 4384], [0, 1943, "null_array", 4384]],
        "proto_machine": [[0, 1943, "null_array", 4384], [0, 1944, "null_array", 4384]],
    },
    "linux aarch64": {
        "main_merge": 1249, "main_merge_arrays": 1248, "main_read": 1952, "main_read_arrays": 3123,
        "drop_merge": 1249, "drop_merge_arrays": 1248, "drop_read": 1952, "drop_read_arrays": 3123,
        "admin_arrays": 1251, "admin_inner": 1250, "sources_copy": 1784, "machine_merge": 1252, "machine_copy": 1788,
        "origin_drop": 1250, "origin_read": 1954, "machine_drop": 1252, "machine_read": 1956,
        "origin_read_arrays": 3127, "machine_read_arrays": 3130, "origin_copy": 1784, "origin_merge": 1251,
        "machine_after_held_array": 1249, "machine_after_held_arrays": 1251, "partial_drop": 1300,
        "org_copy": 1744, "effective_copy": 1784, "effective_ask": 1784, "effective_ask_machine": 1788,
    },
    "linux x86_64": {
        "main_merge": 1304, "main_merge_arrays": 1303, "main_read": 1644, "main_read_arrays": 3289,
        "drop_merge": 1304, "drop_merge_arrays": 1303, "drop_read": 1644, "drop_read_arrays": 3289,
        "admin_arrays": 1306, "admin_inner": 1305, "sources_copy": 1562, "machine_merge": 1307, "machine_copy": 1565,
        "origin_drop": 1305, "origin_read": 1646, "machine_drop": 1307, "machine_read": 1648,
        "origin_read_arrays": 3293, "machine_read_arrays": 3295, "origin_copy": 1561, "origin_merge": 1306,
        "machine_after_held_array": 1304, "machine_after_held_arrays": 1306, "partial_drop": 1360,
    },
}

TARGET = f"{sys.platform} {platform.machine()}"
if TARGET not in PROFILES:
    sys.stderr.write(f"no profile records {TARGET}: add its boundaries to PROFILES\n")
    sys.exit(1)
CASES = [case for keys, make in GROUPS for case in make(*(PROFILES[TARGET][key] for key in keys.split()))] + FIXED + org_cases()


def text(objects, arrays, inner, key="x"):
    opening = f'{{"{key}":' + '{"x":' * (objects - 1) + "[" * arrays + '{"x":' * inner
    return opening + "null" + "}" * inner + "]" * arrays + "}" * objects


def content(file):
    return file[1] if len(file) == 2 else text(*file[1:])


def case_startup(case):
    """The command line and environment one case starts Node with: its own when it names one."""
    return case[2] if len(case) > 2 else STARTUP_BYTES


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


ANSWERS = ("effective", "exact", "machine_policy", "managed_source", "error")


def answer(results):
    """What one case answers: each member as one value, or for an organization case as one per resolve."""
    answers = [single(entry["result"]) for entry in results]
    if len(answers) == 1:
        return answers[0]
    return {key: [one[key] for one in answers] for key in ANSWERS[:4]} | {"error": None}


def single(result):
    managed = next((source for source in result["sources"] if source["source"] == "managed"), None)
    return {
        "effective": stringify(well_formed(result["effective"]), False),
        "exact": exact(result["effective"]),
        "machine_policy": stringify(result["machinePolicy"], False),
        "managed_source": stringify(well_formed(managed), False),
        "error": None,
    }


def rejection(error):
    return {"effective": None, "exact": None, "machine_policy": None, "managed_source": None, "error": error}


def entry_bytes(text):
    return len(text.encode("utf-8")) + 9


def pad(command, startup=STARTUP_BYTES):
    """The length of the `PAD` value that makes the command line and environment fill `startup` bytes."""
    used = sum(entry_bytes(arg) for arg in command) + entry_bytes(f"executable_path={command[0]}") + entry_bytes("PAD=")
    if used > startup:
        sys.stderr.write(f"the command line takes {used} bytes, more than {startup}: use shorter paths\n")
        sys.exit(1)
    return startup - used


def organization(name):
    return name.startswith("org_")


def effective(name):
    return name.startswith("effective_")


def registry_text(document):
    """What reg.exe prints for the `Settings` value holding `document`."""
    return "\n    Settings    REG_SZ    " + document + "\n\n"


def write(location, text):
    os.makedirs(os.path.dirname(location), exist_ok=True)
    with open(location, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)


def prepare(name, files, folder=None):
    """Writes the files and the oracle spec of the case `name` under `folder` (`FOLDER` by default), and returns the
    path of the spec."""
    folder = folder or FOLDER
    root = os.path.join(folder, name)
    os.makedirs(os.path.join(root, "home/.claude"))
    os.makedirs(os.path.join(root, "work/.claude"))
    options = {"cwd": os.path.join(root, "work")}
    for file in files:
        text = content(file)
        if file[0] == "parent":
            options["managedSettings"] = json.loads(text)
        else:
            write(os.path.join(root, PLACES[file[0]]), registry_text(text) if file[0] == "hklm" else text)
    spec = {
        "root": root,
        "platform": sys.platform,
        "env": {"HOME": os.path.join(root, "home")},
        "deep": True,
        "cases": [{"name": name, "options": options}],
    }
    if effective(name):
        spec["env"].update(ORG_ENV)
    if organization(name):
        tool = os.path.join(root, REGISTRY_TOOL)
        if sys.platform == "win32":
            os.makedirs(os.path.dirname(tool), exist_ok=True)
            shutil.copyfile(os.environ["REGISTRY_STAND_IN"], tool)
        else:
            write(tool, REGISTRY_STAND_IN)
            os.chmod(tool, 0o755)
        options["settingSources"] = []
        spec["env"].update(ORG_ENV)
        spec["process"] = ORG_PROCESS
        spec["cases"] = [{"name": f"{name}/{index}", "options": options, **step} for index, step in enumerate(ORG_STEPS)]
    spec_path = os.path.join(folder, f"{name}.spec.json")
    with open(spec_path, "w", encoding="utf-8") as handle:
        json.dump(spec, handle)
    return spec_path


# The working folders of the runs going on now.
RUNNING = set()
RUNNING_LOCK = threading.Lock()


def run(spec_path, node_flags, startup=STARTUP_BYTES):
    """What `resolveSettings` answers for the case `spec_path` names, on Node started with `node_flags` and a command
    line and environment of `startup` bytes."""
    command = [node, *node_flags, ORACLE, package, spec_path]
    # The steps of an organization case write their files as the resolves go, so each run starts without them.
    with open(spec_path, encoding="utf-8") as handle:
        spec = json.load(handle)
    # Runs that shared a folder would read and delete each other's step files.
    with RUNNING_LOCK:
        if spec["root"] in RUNNING:
            raise RuntimeError(f"two runs at once would share the working folder {spec['root']}")
        RUNNING.add(spec["root"])
    try:
        for case in spec["cases"]:
            for step_file in case.get("files", {}):
                if os.path.exists(os.path.join(spec["root"], step_file)):
                    os.remove(os.path.join(spec["root"], step_file))
        # Node can exit on an uncaught error before a pipe drains the long source line it prints, so stderr goes to a
        # file.
        with tempfile.TemporaryFile() as errors:
            process = subprocess.run(command, stdout=subprocess.PIPE, stderr=errors,
                                     env={"PAD": "x" * pad(command, startup)})
            errors.seek(0)
            stderr = errors.read().decode("utf-8")
    finally:
        with RUNNING_LOCK:
            RUNNING.discard(spec["root"])
    if process.returncode == 0:
        return answer(json.loads(process.stdout.decode("utf-8")))
    found = re.search(r"^(\w*Error): (.*)$", stderr, re.M)
    if not found:
        sys.stderr.write(stderr)
        sys.exit(1)
    return rejection({"name": found.group(1), "message": found.group(2)})


def reference(name, files, startup=STARTUP_BYTES):
    """What Node answers with `REFERENCE_FLAGS`, once Node with `CROSS_CHECK_FLAGS` answered the same."""
    spec_path = prepare(name, files)
    answered = run(spec_path, REFERENCE_FLAGS, startup)
    checked = run(spec_path, CROSS_CHECK_FLAGS, startup)
    if checked != answered:
        sys.stderr.write(f"{name}: {json.dumps(answered)} with {REFERENCE_FLAGS}, {json.dumps(checked)} with {CROSS_CHECK_FLAGS}\n")
        sys.exit(1)
    own = {"startup_bytes": startup} if startup != STARTUP_BYTES else {}
    return {"name": name, "files": [list(file) for file in files], **own, **answered}


def observe(entries, runs, host):
    """The `observed` entry of `host`: every case `runs` times with the default flags, two runs per core at once,
    each run in a folder of its own. The folder name has the length of `FOLDER`'s, so every path a run sees has the
    length it has in the reference run."""

    def once(case):
        folder = tempfile.mkdtemp()
        try:
            return run(prepare(case[0], case[1], folder), [], case_startup(case))
        finally:
            shutil.rmtree(folder, ignore_errors=True)

    with concurrent.futures.ThreadPoolExecutor(max_workers=2 * (os.cpu_count() or 1)) as pool:
        pending = [[pool.submit(once, case) for _ in range(runs)] for case in CASES]
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
    names = [case[0] for case in CASES]
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

text_out = body([reference(*case) for case in CASES], previous.get("observed", []) if previous else [])
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
