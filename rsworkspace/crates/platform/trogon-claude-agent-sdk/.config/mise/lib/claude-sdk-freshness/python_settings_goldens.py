#!/usr/bin/env python3
"""Records what the Python SDK's `_build_settings_value` passes as `--settings` when `sandbox` is set, so the Rust
Python dialect is checked against CPython and the pinned `claude_agent_sdk` instead of against a reading of their
source.

usage: python3 -I python_settings_goldens.py OUT_FILE [--check]

Run it with an interpreter that has the pinned `claude_agent_sdk` installed.

`depth` holds one entry per caller of `_build_settings_value`: the deepest nesting of objects `json.loads` reads
there, the `RecursionError` message one level deeper, and the SHA-256 and length of the `--settings` value at the
deepest nesting. CPython 3.12 checks its C recursion limit of 10000 once per object the scanner enters, and the
checks already open where `json.loads` runs count against it: `module` calls `_build_settings_value` from a plain
function, `client_connect` awaits `ClaudeSDKClient.connect` under `asyncio.run`, and `query` iterates `query()`
under `asyncio.run`. Each depth is searched with a fresh event loop, then checked once more after a first call, so
the caches the first call fills cannot move it. `surrogates` pairs settings text holding lone surrogate escapes
with the `--settings` value, as `json.loads` keeps a lone surrogate in the `str` and `json.dumps` escapes it.
"""

import asyncio
import hashlib
import json
import platform
import sys

from claude_agent_sdk import ClaudeAgentOptions, ClaudeSDKClient, query
from claude_agent_sdk._internal.transport.subprocess_cli import SubprocessCLITransport

args = sys.argv[1:]
if not args or args[0].startswith("-"):
    sys.stderr.write("usage: python_settings_goldens.py OUT_FILE [--check]\n")
    sys.exit(2)
out, flags = args[0], args[1:]

SANDBOX = {"enabled": True}


def nested(depth):
    return '{"a":' * depth + "null" + "}" * depth


def options(settings):
    return ClaudeAgentOptions(cli_path="/nonexistent/claude", sandbox=SANDBOX, settings=settings)


def outcome(error):
    if isinstance(error, RecursionError):
        return str(error)
    return None


def module(depth):
    try:
        SubprocessCLITransport(prompt="hi", options=options(nested(depth)))._build_settings_value()
    except BaseException as error:
        return outcome(error)
    return None


async def connect(depth):
    client = ClaudeSDKClient(options(nested(depth)))
    try:
        await client.connect()
    except BaseException as error:
        return outcome(error)
    return None


async def run_query(depth):
    try:
        async for _ in query(prompt="hi", options=options(nested(depth))):
            pass
    except BaseException as error:
        return outcome(error)
    return None


def warmed(probe):
    async def twice(depth):
        await probe(1)
        return await probe(depth)

    return twice


CALLERS = [
    ("module", module, lambda depth: (module(1), module(depth))[1]),
    ("client_connect", lambda depth: asyncio.run(connect(depth)), lambda depth: asyncio.run(warmed(connect)(depth))),
    ("query", lambda depth: asyncio.run(run_query(depth)), lambda depth: asyncio.run(warmed(run_query)(depth))),
]


def deepest(probe):
    low, high = 1, 20000
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
    value = SubprocessCLITransport(prompt="hi", options=options(nested(loads)))._build_settings_value()
    digest = hashlib.sha256(value.encode("ascii")).hexdigest()
    return {"caller": name, "loads": loads, "error": error, "value_sha256": digest, "value_length": len(value)}


depth = [depth_entry(name, cold, warm) for name, cold, warm in CALLERS]

surrogate_texts = [
    '{"x": "\\udc00"}',
    '{"\\ud800k": "a\\ud83d"}',
    '{"x": "\\udc00\\ud800", "y": ["\\udfff"]}',
    '{"x": "\\ud83d\\ude00"}',
]
surrogates = [
    [text, SubprocessCLITransport(prompt="hi", options=options(text))._build_settings_value()]
    for text in surrogate_texts
]


def lines(items):
    return ",\n".join(f"    {json.dumps(item)}" for item in items)


body = (
    "{\n"
    f'  "python": {json.dumps(platform.python_version())},\n'
    f'  "depth": [\n{lines(depth)}\n  ],\n'
    f'  "surrogates": [\n{lines(surrogates)}\n  ]\n'
    "}\n"
)

if "--check" in flags:
    with open(out, encoding="utf-8") as handle:
        if handle.read() != body:
            sys.stderr.write(f"{out} is stale: run python_settings_goldens.py without --check\n")
            sys.exit(1)
else:
    with open(out, "w", encoding="utf-8") as handle:
        handle.write(body)
