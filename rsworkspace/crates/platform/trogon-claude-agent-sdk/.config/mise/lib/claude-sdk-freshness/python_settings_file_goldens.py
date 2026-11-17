#!/usr/bin/env python3
"""Records how the Python SDK reads a settings file, so the Python dialect of the settings engine is checked against
CPython and the pinned `claude_agent_sdk` instead of against a reading of their source.

usage: python3 -I python_settings_file_goldens.py OUT_FILE [--check]

Run it with an interpreter that has the pinned `claude_agent_sdk` installed.

The Python SDK reads a settings file in one place: `_build_settings_value` runs `json.loads` on the file that
`settings` names when a sandbox is set. Each file holds an object whose `x` member nests arrays, and `depth` holds
one entry per caller of `_build_settings_value`: the deepest file `json.loads` reads there, counting the outer
object, the `RecursionError` message one level deeper, and the SHA-256 and length of `json.dumps` of the `x` member
at the deepest nesting. CPython 3.12 checks its C recursion limit of 10000 once per array and object the scanner
enters, and the checks already open where `json.loads` runs count against it: `module` calls
`_build_settings_value` from a plain function, `client_connect` awaits `ClaudeSDKClient.connect` under
`asyncio.run`, and `query` iterates `query()` under `asyncio.run`. Each depth is searched with a fresh event loop,
then checked once more after a first call, so the caches the first call fills cannot move it. `surrogates` pairs
the text of a settings file holding lone surrogate escapes with `json.dumps` of the `x` member `json.loads` reads
from it, which keeps every lone surrogate. Each `collisions` group lists settings files one resolve reads, as the
source that reads it, its text and `json.dumps` of the object `json.loads` reads from it. The Python SDK reads each
file on its own, so a lone surrogate in one file and a private-use code point in another both keep their own value.
"""

import asyncio
import hashlib
import json
import os
import platform
import sys
import tempfile

from claude_agent_sdk import ClaudeAgentOptions, ClaudeSDKClient, query
from claude_agent_sdk._internal.transport.subprocess_cli import SubprocessCLITransport

args = sys.argv[1:]
if not args or args[0].startswith("-"):
    sys.stderr.write("usage: python_settings_file_goldens.py OUT_FILE [--check]\n")
    sys.exit(2)
out, flags = args[0], args[1:]

SANDBOX = {"enabled": True}
FOLDER = tempfile.mkdtemp()


def nested(depth):
    return '{"x":' + "[" * (depth - 1) + "null" + "]" * (depth - 1) + "}"


def settings_file(name, text):
    path = os.path.join(FOLDER, f"settings-{name}.json")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)
    return path


def options(depth):
    return ClaudeAgentOptions(
        cli_path="/nonexistent/claude", sandbox=SANDBOX, settings=settings_file(depth, nested(depth))
    )


def outcome(error):
    if isinstance(error, RecursionError):
        return str(error)
    return None


def module(depth):
    try:
        SubprocessCLITransport(prompt="hi", options=options(depth))._build_settings_value()
    except BaseException as error:
        return outcome(error)
    return None


async def connect(depth):
    client = ClaudeSDKClient(options(depth))
    try:
        await client.connect()
    except BaseException as error:
        return outcome(error)
    return None


async def run_query(depth):
    try:
        async for _ in query(prompt="hi", options=options(depth)):
            pass
    except BaseException as error:
        return outcome(error)
    return None


def warmed(probe):
    async def twice(depth):
        await probe(2)
        return await probe(depth)

    return twice


CALLERS = [
    ("module", module, lambda depth: (module(2), module(depth))[1]),
    ("client_connect", lambda depth: asyncio.run(connect(depth)), lambda depth: asyncio.run(warmed(connect)(depth))),
    ("query", lambda depth: asyncio.run(run_query(depth)), lambda depth: asyncio.run(warmed(run_query)(depth))),
]


def deepest(probe):
    low, high = 2, 20000
    if probe(low) is not None or probe(high) is None:
        raise SystemExit("the search range does not hold the boundary")
    while high - low > 1:
        middle = (low + high) // 2
        if probe(middle) is None:
            low = middle
        else:
            high = middle
    return low


def depth_entry(name, cold, warm):
    loads = deepest(cold)
    error = cold(loads + 1)
    if warm(loads) is not None or warm(loads + 1) != error:
        raise SystemExit(f"{name}: a first call moves the boundary")
    value = SubprocessCLITransport(prompt="hi", options=options(loads))._build_settings_value()
    member = json.dumps(json.loads(value)["x"])
    digest = hashlib.sha256(member.encode("ascii")).hexdigest()
    return {"caller": name, "loads": loads, "error": error, "x_sha256": digest, "x_length": len(member)}


depth = [depth_entry(name, cold, warm) for name, cold, warm in CALLERS]

SURROGATE_FILES = [
    r'{"x":"\ud800"}',
    r'{"x":"a\udc00b"}',
    r'{"x":"\ude00\ud83d"}',
    r'{"x":"\ud83d\ude00"}',
    r'{"x":["\udbff",{"k\ud83d":"\ude00","\ud800":null}]}',
    r'{"x":{"\udfff":"\udfff","\udfff ":1}}',
    r'{"x":["\ud800","\ue000","\ue800\udc00"]}',
    r'{"x":"\ue000"}',
]


def surrogate_entry(index, text):
    found = ClaudeAgentOptions(
        cli_path="/nonexistent/claude", sandbox=SANDBOX, settings=settings_file(f"surrogates-{index}", text)
    )
    value = SubprocessCLITransport(prompt="hi", options=found)._build_settings_value()
    return [text, json.dumps(json.loads(value)["x"])]


surrogates = [surrogate_entry(index, text) for index, text in enumerate(SURROGATE_FILES)]

COLLISIONS = [
    [("userSettings", r'{"x":"\ud800"}'), ("projectSettings", r'{"y":"\ue000"}')],
    [("userSettings", r'{"x":"\ue000"}'), ("projectSettings", r'{"y":"\ud800"}')],
    [("userSettings", r'{"x":"\ud800"}'), ("projectSettings", '{"y":"\ue000"}')],
    [("userSettings", r'{"x":["\ud800","\ue000x"]}')],
    [
        ("userSettings", r'{"x":"\ue800"}'),
        ("projectSettings", r'{"y":"\uf000\udbff"}'),
        ("localSettings", r'{"z":"\ue000\ud800"}'),
    ],
    [("policySettings", r'{"p":"\ue000"}'), ("userSettings", r'{"x":"\ud800"}')],
]


def collision_file(group, index, source, text):
    found = ClaudeAgentOptions(
        cli_path="/nonexistent/claude", sandbox=SANDBOX, settings=settings_file(f"collision-{group}-{index}", text)
    )
    value = SubprocessCLITransport(prompt="hi", options=found)._build_settings_value()
    return [source, text, json.dumps(json.loads(value))]


collisions = [
    [collision_file(group, index, source, text) for index, (source, text) in enumerate(files)]
    for group, files in enumerate(COLLISIONS)
]


def lines(items):
    return ",\n".join(f"    {json.dumps(item)}" for item in items)


body = (
    "{\n"
    f'  "python": {json.dumps(platform.python_version())},\n'
    f'  "depth": [\n{lines(depth)}\n  ],\n'
    f'  "surrogates": [\n{lines(surrogates)}\n  ],\n'
    f'  "collisions": [\n{lines(collisions)}\n  ]\n'
    "}\n"
)

if "--check" in flags:
    with open(out, encoding="utf-8") as handle:
        if handle.read() != body:
            sys.stderr.write(f"{out} is stale: run python_settings_file_goldens.py without --check\n")
            sys.exit(1)
else:
    with open(out, "w", encoding="utf-8") as handle:
        handle.write(body)
