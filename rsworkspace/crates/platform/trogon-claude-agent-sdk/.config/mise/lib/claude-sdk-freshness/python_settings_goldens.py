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

`files` holds settings files `_build_settings_value` reads with `Path.read_text(encoding="utf-8-sig")`: the bytes
of each, as hex before and after a run of `a` bytes of the given length, and either the `--settings` value or the
`CLIConnectionError` message. They cover invalid UTF-8, which raises `UnicodeDecodeError` with positions counted
after a leading byte order mark, a byte order mark and the prefixes of one, which decode to nothing, and carriage
returns, which universal newlines turn into line feeds before `json.loads` counts lines. `missing_paths` pairs the
bytes of a relative `cwd` that holds no settings file with the message, whose path `repr` shows each byte that is
not UTF-8 as the lone surrogate `os.fsdecode` gives it.

`raised` holds settings files the `utf-8-sig` codec cannot decode, as hex, with what each caller raises for one:
the caller as in `depth`, the class name of the exception, whether it is a `ClaudeSDKError`, its message, and the
class name, `encoding`, `object` as hex, `start`, `end` and `reason` of its `__cause__`. Every caller names a CLI that
does not exist, so an exception raised while the command is built comes before any attempt to start it.
"""

import asyncio
import hashlib
import json
import os
import platform
import sys
import tempfile

from claude_agent_sdk import ClaudeAgentOptions, ClaudeSDKClient, ClaudeSDKError, query
from claude_agent_sdk._internal.transport.subprocess_cli import SubprocessCLITransport

args = sys.argv[1:]
if not args or args[0].startswith("-"):
    sys.stderr.write("usage: python_settings_goldens.py OUT_FILE [--check]\n")
    sys.exit(2)
out, flags = os.path.abspath(args[0]), args[1:]

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

FOLDER = tempfile.mkdtemp()

FILES = [
    ("", 0, ""),
    ("ef", 0, ""),
    ("efbb", 0, ""),
    ("efbbbf", 0, ""),
    ("efbbbfefbbbf7b7d", 0, ""),
    ("efbbbf7b2278223a22efbbbf227d", 0, ""),
    ("20efbbbf7b7d", 0, ""),
    ("ef80", 0, ""),
    ("efbb80", 0, ""),
    ("efbbbfef", 0, ""),
    ("7b2278223a22eda080227d", 0, ""),
    ("7b2278223a22edbfbf227d", 0, ""),
    ("7b2278223a2280227d", 0, ""),
    ("7b2278223a22c0af227d", 0, ""),
    ("7b2278223a22ff227d", 0, ""),
    ("7b2278223a22f5227d", 0, ""),
    ("7b2278223a22f490227d", 0, ""),
    ("7b2278223a22e080227d", 0, ""),
    ("7b2278223a22c3227d", 0, ""),
    ("7b2278223a22e282227d", 0, ""),
    ("7b2278223a22f09f98227d", 0, ""),
    ("7b2278223a22e282", 0, ""),
    ("7b2278223a317dff", 0, ""),
    ("efbbbf7b2278223a2280227d", 0, ""),
    ("7b2278223a22efbfbe227d", 0, ""),
    ("7b2278223a22", 10000, "80227d"),
    ("7b0d0a2278223a312c7d", 0, ""),
    ("7b0d2278223a312c7d", 0, ""),
    ("0d0a0d0a7b2278223a5d", 0, ""),
    ("7b2278223a22610d62227d", 0, ""),
    ("7b2278223a22610a62227d", 0, ""),
]


def file_entry(index, head, pad, tail):
    path = os.path.join(FOLDER, f"settings-{index}.json")
    with open(path, "wb") as handle:
        handle.write(bytes.fromhex(head) + b"a" * pad + bytes.fromhex(tail))
    try:
        found = ["value", SubprocessCLITransport(prompt="hi", options=options(path))._build_settings_value()]
    except Exception as error:
        found = ["error", str(error)]
    return [head, pad, tail, found]


files = [file_entry(index, *entry) for index, entry in enumerate(FILES)]

RAISED_FILES = [
    "7b2278223a22ff227d",
    "efbbbf7b2278223a2280227d",
    "7b2278223a22e282",
    "7b2278223a22eda080227d",
    "7b2278223a22f09f98227d",
]


async def connect_file(path):
    await ClaudeSDKClient(options(path)).connect()


async def query_file(path):
    async for _ in query(prompt="hi", options=options(path)):
        pass


RAISING_CALLERS = [
    ("module", lambda path: SubprocessCLITransport(prompt="hi", options=options(path))._build_settings_value()),
    ("client_connect", lambda path: asyncio.run(connect_file(path))),
    ("query", lambda path: asyncio.run(query_file(path))),
]


def raised_by(caller, path):
    try:
        caller(path)
    except Exception as error:
        cause = error.__cause__
        return [
            type(error).__name__,
            isinstance(error, ClaudeSDKError),
            str(error),
            type(cause).__name__,
            cause.encoding,
            cause.object.hex(),
            cause.start,
            cause.end,
            cause.reason,
        ]
    raise SystemExit(f"{path}: an undecodable settings file was read")


def raised_entry(index, head):
    path = os.path.join(FOLDER, f"raised-{index}.json")
    with open(path, "wb") as handle:
        handle.write(bytes.fromhex(head))
    return [head, [[name, *raised_by(caller, path)] for name, caller in RAISING_CALLERS]]


raised = [raised_entry(index, head) for index, head in enumerate(RAISED_FILES)]

MISSING_CWDS = [
    "73ff78",
    "69742773ff",
    "712227fe",
    "636166c3a9",
    "ee8080",
    "7409627f5c",
    "c3",
    "610a620d85c285e280a8",
    "eda080",
    "e282",
]


def missing_entry(cwd):
    found = ClaudeAgentOptions(
        cli_path="/nonexistent/claude", sandbox=SANDBOX, settings="none.json", cwd=os.fsdecode(bytes.fromhex(cwd))
    )
    try:
        SubprocessCLITransport(prompt="hi", options=found)._build_settings_value()
    except Exception as error:
        return [cwd, str(error)]
    raise SystemExit(f"{cwd}: a missing settings file was read")


os.chdir(FOLDER)
missing_paths = [missing_entry(cwd) for cwd in MISSING_CWDS]


def lines(items):
    return ",\n".join(f"    {json.dumps(item)}" for item in items)


body = (
    "{\n"
    f'  "python": {json.dumps(platform.python_version())},\n'
    f'  "depth": [\n{lines(depth)}\n  ],\n'
    f'  "surrogates": [\n{lines(surrogates)}\n  ],\n'
    f'  "files": [\n{lines(files)}\n  ],\n'
    f'  "raised": [\n{lines(raised)}\n  ],\n'
    f'  "missing_paths": [\n{lines(missing_paths)}\n  ]\n'
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
