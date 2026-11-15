#!/usr/bin/env python3
"""Records when the Python `query()` closes the CLI's stdin relative to what the CLI sent.

usage: run_end_machine_goldens.py PYTHON_SRC OUT_FILE

Runs the pinned `claude_agent_sdk` from PYTHON_SRC against `run_end_machine_cli.mjs` for the session-state,
delegated-task, queued-turn and `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS` scenarios, and writes the `_py` entries of
`tests/fixtures/auth_callbacks/run_machine.json` into OUT_FILE, keeping what `run_end_machine_goldens.mjs` wrote.

Each scenario gets 3.5 seconds before the recorder cancels the query itself. A `harness_close` line goes into the
log first, so the close that the cancellation causes is never counted as the SDK ending the run: a run the SDK never
ended records `never`.
"""

from __future__ import annotations

import json
import os
import shutil
import sys
import tempfile
from pathlib import Path

if len(sys.argv) != 3:
    print(__doc__.strip().splitlines()[2], file=sys.stderr)
    sys.exit(2)
PYTHON_SRC, OUT = sys.argv[1], Path(sys.argv[2])
sys.path.insert(0, PYTHON_SRC)

import anyio  # noqa: E402
from claude_agent_sdk import ClaudeAgentOptions, HookMatcher, SystemMessage, query  # noqa: E402
from claude_agent_sdk._version import __version__  # noqa: E402

CLI = Path(__file__).resolve().parent / "run_end_machine_cli.mjs"


def uuid(n):
    """A placeholder message UUID ending in `n`."""
    return "00000000-0000-4000-8000-" + str(n).zfill(12)


def state(s):
    return {"type": "system", "subtype": "session_state_changed", "state": s, "session_id": "s"}


def said(t):
    return {
        "type": "assistant",
        "session_id": "s",
        "parent_tool_use_id": None,
        "uuid": uuid(len(t)),
        "message": {
            "id": "m" + t,
            "type": "message",
            "role": "assistant",
            "model": "m",
            "content": [{"type": "text", "text": t}],
            "stop_reason": "end_turn",
            "stop_sequence": None,
            "usage": {"input_tokens": 1, "output_tokens": 1},
        },
    }


def task(subtype):
    extra = {"status": "completed", "output_file": "f", "summary": "x"} if subtype == "task_notification" else {}
    return {
        "type": "system",
        "subtype": subtype,
        "task_id": "t1",
        "task_type": "local_agent",
        "description": "d",
        "uuid": uuid(1),
        "session_id": "s",
        **extra,
    }


def wait(ms):
    return {"__wait": ms}


RESULT = {
    "type": "result",
    "subtype": "success",
    "is_error": False,
    "duration_ms": 1,
    "duration_api_ms": 1,
    "num_turns": 1,
    "session_id": "s",
    "result": "done",
    "queued_turn_count": 0,
}
RESULT_QUEUED = {**RESULT, "queued_turn_count": 1}
CEILING_STEPS = [wait(100), state("running"), wait(100), said("A"), RESULT, wait(300), state("running"), wait(900), state("requires_action")]
SCENARIOS = {
    "task_open_at_idle": [wait(100), state("running"), wait(100), task("task_started"), said("A"), RESULT, wait(300), state("idle"), wait(300), said("CCC")],
    "task_then_notified": [wait(100), state("running"), wait(100), task("task_started"), said("A"), RESULT, wait(300), task("task_notification"), wait(100), state("idle"), wait(300), said("CCC")],
    "continuation_reported": [wait(100), state("running"), wait(100), said("A"), RESULT_QUEUED, wait(300), state("idle"), wait(300), state("running"), said("BB"), RESULT, wait(300), said("CCC")],
    "running_then_idle": [wait(100), state("running"), wait(100), said("A"), RESULT, wait(300), state("idle"), wait(300), said("CCC")],
    "ceiling": CEILING_STEPS,
    "ceiling_zero": CEILING_STEPS,
    "ceiling_exponent": CEILING_STEPS,
}
CEILINGS = {"ceiling": "500", "ceiling_zero": "0", "ceiling_exponent": "5e2"}


def label(frame):
    if frame["type"] == "result":
        return "result"
    if frame["type"] == "assistant":
        return frame["message"]["content"][0]["text"]
    if frame["type"] == "system" and "state" in frame:
        return frame["state"]
    return None


def label_message(message):
    name = type(message).__name__
    if name == "ResultMessage":
        return "result"
    if name == "AssistantMessage":
        return message.content[0].text
    if isinstance(message, SystemMessage):
        if isinstance(message.data, dict) and "state" in message.data:
            return message.data["state"]
        return "system"
    return name


async def hook(_input, _tool_use_id, _context):
    return {}


async def nothing():
    return
    yield


async def scenario(logs, cli, name, steps, env_extra, empty=False):
    log = logs / (name + ".log")
    log.write_text("")
    script = {("after_init" if empty else "on_user"): steps, "stay_open": True}
    options = ClaudeAgentOptions(
        cli_path=str(cli),
        env={**env_extra, "FAKE_LOG": str(log), "FAKE_SCRIPT": json.dumps(script)},
        hooks={"PreToolUse": [HookMatcher(matcher=None, hooks=[hook])]},
    )
    yielded = []
    ended = False
    async with anyio.create_task_group() as group:

        async def stop():
            await anyio.sleep(3.5)
            with log.open("a") as handle:
                handle.write(json.dumps({"kind": "harness_close"}) + "\n")
            group.cancel_scope.cancel()

        group.start_soon(stop)
        async for message in query(prompt=nothing() if empty else "hi", options=options):
            yielded.append(label_message(message))
        ended = True
        group.cancel_scope.cancel()
    await anyio.sleep(0.1)
    events = [json.loads(line) for line in log.read_text().strip().split("\n") if line]
    mark = next((i for i, e in enumerate(events) if e["kind"] == "harness_close"), len(events))
    closed = next((e for e in events[:mark] if e["kind"] == "stdin_closed"), None)
    sent = [
        (label(e["frame"]), closed is not None and e["seq"] < closed["seq"])
        for e in events
        if e["kind"] == "cli_out" and label(e["frame"])
    ]
    before = [name for name, early in sent if early]
    return {
        "stdin_closed_after": (before[-1] if before else "nothing") if closed else "never",
        "cli_sent": [name for name, _ in sent],
        "yielded": yielded,
        "ended": ended,
    }


async def main():
    logs = Path(tempfile.mkdtemp(prefix="run-end-machine-"))
    cli = logs / "cli.sh"
    cli.write_text('#!/bin/sh\nexec node "%s" "$@"\n' % CLI)
    cli.chmod(0o755)
    existing = json.loads(OUT.read_text()) if OUT.exists() else {}
    fixture = {
        "captured_from": {**existing.get("captured_from", {}), "python": "claude_agent_sdk " + __version__},
        "scenarios": dict(existing.get("scenarios", {})),
    }
    for name, steps in SCENARIOS.items():
        env = {"CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS": CEILINGS[name]} if name in CEILINGS else {}
        fixture["scenarios"]["m_%s_py" % name] = await scenario(logs, cli, "m_%s_py" % name, steps, env)
    fixture["scenarios"]["m_empty_stream_py"] = await scenario(
        logs, cli, "m_empty_stream_py", CEILING_STEPS, {"CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS": "500"}, True
    )
    shutil.rmtree(logs, ignore_errors=True)
    OUT.write_text(json.dumps(fixture, indent=2) + "\n")


anyio.run(main)
