#!/usr/bin/env python3
"""Records what the pinned TypeScript `resolveSettings` and the pinned Python `claude_agent_sdk` give for settings
files that hold lone surrogates once every private-use code point of the Basic Multilingual Plane is taken, so the
stand-ins the settings engine takes from planes 15 and 16 for those surrogates are checked against Node, CPython and
the pinned packages: each must count as the one UTF-16 unit of its surrogate in every length and depth. A zod
length check counts the code points of a string (upstream `Bg`) once its UTF-16 length passes the bound, so a
character past the Basic Multilingual Plane counts once there and twice in a plain `e.length` check.

usage: python3 -I settings_stand_in_goldens.py NODE PACKAGE_DIR OUT_FILE [--check]

Run it with a Python that has the pinned `claude_agent_sdk` installed. NODE runs `resolve_settings_oracle.mjs`
against PACKAGE_DIR, the unpacked pinned `@anthropic-ai/claude-agent-sdk`, on linux.

In each file text `<private-use>` stands for the 6400 code points U+E000 to U+F8FF, written as is. A local settings
file holding only that string is ignored, as every settings file that holds no object is, but it takes every
private-use code point of the Basic Multilingual Plane.

Each `cases` entry names a resolve and its files as `[place, text]`. `result` is the whole object `resolveSettings`
returns as `JSON.stringify` prints it, with the sandbox root taken off every path so the home and project folders
read as `/home/fixture` and `/work/project`, or `null` when it rejects; `error` is the error it rejects with, or
`null`. Each `depth` entry is a case of `ts_settings_depth_goldens.py`, `[source, objects, arrays, inner, leaf]`,
with the filler beside it: Node runs with only a `PAD` variable, sized so the command line and environment fill
`STARTUP_BYTES`, and the oracle runs with `deep`. `effective` is the `effective` member of the result as
`JSON.stringify` prints it, or `null` when it rejects. `python` is the deepest file the Python SDK reads where a plain
function calls `_build_settings_value`, a file that holds the filler under `f` and arrays nested under `x` around two
lone surrogates, with `json.dumps` of its `x` member as a SHA-256 and a length.
"""

import hashlib
import json
import os
import platform
import re
import subprocess
import sys
import tempfile

from claude_agent_sdk import ClaudeAgentOptions
from claude_agent_sdk._internal.transport.subprocess_cli import SubprocessCLITransport

args = sys.argv[1:]
if len(args) < 3 or any(arg.startswith("-") for arg in args[:3]):
    sys.stderr.write("usage: settings_stand_in_goldens.py NODE PACKAGE_DIR OUT_FILE [--check]\n")
    sys.exit(2)
node, package, out, flags = args[0], args[1], os.path.abspath(args[2]), args[3:]
ORACLE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "resolve_settings_oracle.mjs")
FOLDER = tempfile.mkdtemp()
sys.setrecursionlimit(20000)
STARTUP_BYTES = 4384
FILLER = "<private-use>"
PRIVATE_USE = "".join(chr(c) for c in range(0xE000, 0xF900))

PLACES = {
    "userSettings": "home/fixture/.claude/settings.json",
    "projectSettings": "work/project/.claude/settings.json",
    "localSettings": "work/project/.claude/settings.local.json",
    "managed": "etc/claude-code/managed-settings.json",
}
U, P, L, M = PLACES
FILL = [L, json.dumps(FILLER)]


def metadata(url):
    server = {"type": "http", "url": "https://e.com", "oauth": {"authServerMetadataUrl": url}}
    return json.dumps({"managedMcpServers": {"s": server}}).replace("@", "\\ud800")


def helper_path(path):
    return json.dumps({"policyHelpers": {"linux": {"path": path}}}).replace("@", "\\ud800")


CASES = [
    ("escape", [[U, '{"x":["\\ud800","\\udbff\\udbff","k"],"\\udc00":1}'], FILL]),
    ("escape_without_filler", [[U, '{"x":["\\ud800","\\udbff\\udbff","k"],"\\udc00":1}']]),
    ("path_1024", [[M, helper_path("/" + "a" * 1022 + "@")], FILL]),
    ("path_1025", [[M, helper_path("/" + "a" * 1023 + "@")], FILL]),
    ("metadata_4096", [[M, metadata("https://e.com/" + "a" * 4081 + "@")], FILL]),
    ("metadata_4097", [[M, metadata("https://e.com/" + "a" * 4082 + "@")], FILL]),
    ("path_astral_1024", [[M, helper_path("/" + "a" * 1022 + "\U0001f600")]]),
    ("path_astral_1025", [[M, helper_path("/" + "a" * 1023 + "\U0001f600")]]),
    ("path_astral_pairs", [[M, helper_path("/" + "\U0001f600" * 1023)]]),
    ("metadata_astral_4096", [[M, metadata("https://e.com/" + "a" * 4080 + "\U0001f600")]]),
    ("metadata_astral_4097", [[M, metadata("https://e.com/" + "a" * 4081 + "\U0001f600")]]),
    ("length_join", [[U, '{"x":[1]}'], [P, '{"x":{"length":["\\ud800"]}}'], FILL]),
    ("length_text", [[U, '{"x":[1]}'], [P, '{"x":{"length":"\\ud800"}}'], FILL]),
]

LEAF = '"\\ud800\\udbff"'
DEPTH = [
    ("objects_2223", ("userSettings", 2223, 0, 0, LEAF)),
    ("objects_2224", ("userSettings", 2224, 0, 0, LEAF)),
    ("arrays_3276", ("userSettings", 1, 3276, 0, LEAF)),
    ("arrays_3277", ("userSettings", 1, 3277, 0, LEAF)),
    ("merge_1244", ("userSettings", 1244, 0, 0, LEAF)),
    ("merge_1245", ("userSettings", 1245, 0, 0, LEAF)),
]


def exact(value):
    """`JSON.stringify` of `value`, which escapes each lone surrogate and leaves every other code point as is."""
    text = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    return re.sub("[\ud800-\udfff]", lambda found: f"\\u{ord(found.group()):04x}", text)


def entry_bytes(text):
    return len(text.encode("utf-8")) + 9


def pad(command):
    """The length of the `PAD` value that makes the command line and environment fill `STARTUP_BYTES`."""
    used = sum(entry_bytes(arg) for arg in command) + entry_bytes(f"executable_path={command[0]}") + entry_bytes("PAD=")
    if used > STARTUP_BYTES:
        sys.stderr.write(f"the command line takes {used} bytes, more than {STARTUP_BYTES}: use shorter paths\n")
        sys.exit(1)
    return STARTUP_BYTES - used


def nested(objects, arrays, inner, leaf):
    opening = '{"x":' + '{"x":' * (objects - 1) + "[" * arrays + '{"x":' * inner
    return opening + leaf + "}" * inner + "]" * arrays + "}" * objects


def run(name, files, deep):
    """What the oracle prints for `files`, or the error `resolveSettings` rejects with."""
    root = os.path.join(FOLDER, name)
    for place, text in files:
        location = os.path.join(root, PLACES[place])
        os.makedirs(os.path.dirname(location), exist_ok=True)
        with open(location, "w", encoding="utf-8") as handle:
            handle.write(text.replace(FILLER, PRIVATE_USE))
    os.makedirs(os.path.join(root, "home/fixture/.claude"), exist_ok=True)
    os.makedirs(os.path.join(root, "work/project/.claude"), exist_ok=True)
    spec = {
        "root": root,
        "platform": "linux",
        "env": {"HOME": os.path.join(root, "home/fixture")},
        "deep": deep,
        "cases": [{"name": name, "options": {"cwd": os.path.join(root, "work/project")}}],
    }
    spec_path = os.path.join(FOLDER, f"{name}.spec.json")
    with open(spec_path, "w", encoding="utf-8") as handle:
        json.dump(spec, handle)
    command = [node, ORACLE, package, spec_path]
    environment = {"PAD": "x" * pad(command)} if deep else None
    found = subprocess.run(command, capture_output=True, env=environment)
    if found.returncode == 0:
        return root, json.loads(found.stdout.decode("utf-8"))[0]["result"], None
    error = re.search(r"^(\w*Error): (.*)$", found.stderr.decode("utf-8"), re.M)
    if not error:
        sys.stderr.write(found.stderr.decode("utf-8"))
        sys.exit(1)
    return root, None, {"name": error.group(1), "message": error.group(2)}


def record(name, files):
    root, result, error = run(name, files, False)
    text = None if result is None else exact(result).replace(json.dumps(root)[1:-1], "")
    return {"name": name, "files": files, "result": text, "error": error}


def record_depth(name, shape):
    source, *rest = shape
    files = [[source, nested(*rest)], FILL]
    _, result, error = run(name, files, True)
    effective = None if result is None else exact(result["effective"])
    return {"name": name, "files": [list(shape), FILL], "effective": effective, "error": error}


def python_file(depth):
    text = '{"f":"' + PRIVATE_USE + '","x":' + "[" * (depth - 1) + LEAF + "]" * (depth - 1) + "}"
    path = os.path.join(FOLDER, f"python-{depth}.json")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)
    return path


def python_value(depth):
    """What `_build_settings_value` returns for the file at `depth`, or the error it raises."""
    options = ClaudeAgentOptions(cli_path="/nonexistent/claude", sandbox={"enabled": True}, settings=python_file(depth))
    try:
        return SubprocessCLITransport(prompt="hi", options=options)._build_settings_value(), None
    except RecursionError as error:
        return None, str(error)


def python_depth():
    low, high = 2, 20000
    if python_value(low)[1] is not None or python_value(high)[1] is None:
        raise SystemExit("the search range does not hold the boundary")
    while high - low > 1:
        middle = (low + high) // 2
        if python_value(middle)[1] is None:
            low = middle
        else:
            high = middle
    value, _ = python_value(low)
    member = json.dumps(json.loads(value)["x"])
    digest = hashlib.sha256(member.encode("ascii")).hexdigest()
    error = python_value(low + 1)[1]
    return {"loads": low, "error": error, "x_sha256": digest, "x_length": len(member)}


version = subprocess.run([node, "--version"], capture_output=True, check=True, env={}).stdout.decode().strip()
cases = [record(name, files) for name, files in CASES]
depth = [record_depth(name, shape) for name, shape in DEPTH]
body = (
    "{\n"
    + f'  "node": {json.dumps(f"{version} {sys.platform} {platform.machine()}")},\n'
    + f'  "startup_bytes": {STARTUP_BYTES},\n'
    + f'  "filler": {json.dumps(FILLER)},\n'
    + '  "cases": [\n'
    + ",\n".join(f"    {json.dumps(entry)}" for entry in cases)
    + "\n  ],\n"
    + '  "depth": [\n'
    + ",\n".join(f"    {json.dumps(entry)}" for entry in depth)
    + "\n  ],\n"
    + f'  "python": {json.dumps(python_depth())}\n'
    + "}\n"
)

if "--check" in flags:
    with open(out, encoding="utf-8") as handle:
        if handle.read() != body:
            sys.stderr.write(f"{out} is stale: run settings_stand_in_goldens.py without --check\n")
            sys.exit(1)
else:
    with open(out, "w", encoding="utf-8") as handle:
        handle.write(body)
