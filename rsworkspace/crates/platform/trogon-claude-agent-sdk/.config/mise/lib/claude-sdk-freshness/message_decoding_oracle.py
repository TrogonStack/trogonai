#!/usr/bin/env python3
"""Stream each corpus message through the pinned Python query() and print what it yields.

usage: message_decoding_oracle.py SOURCE_DIR CASES_JSON

SOURCE_DIR is the pinned source release: ``claude_agent_sdk`` is imported from its ``src`` directory, so its
dependencies must be installed. Each case runs against an in-memory
transport that answers the initialize request, sends the case message and closes. The output maps every
case name to the messages query() yielded (class name and fields) and the error that ended the stream.
"""

from __future__ import annotations

import dataclasses
import json
import sys
from pathlib import Path
from typing import Any


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        print("usage: message_decoding_oracle.py SOURCE_DIR CASES_JSON", file=sys.stderr)
        return 2
    sys.path.insert(0, str(Path(argv[1]).resolve() / "src"))

    import anyio
    from claude_agent_sdk import ClaudeAgentOptions, query
    from claude_agent_sdk._internal.transport import Transport

    class OneMessage(Transport):
        def __init__(self, message: dict[str, Any]) -> None:
            self.message = message
            self.send, self.receive = anyio.create_memory_object_stream[dict[str, Any]](16)

        async def connect(self) -> None:
            return None

        async def write(self, data: str) -> None:
            frame = json.loads(data)
            if frame.get("type") != "control_request":
                return
            await self.send.send(
                {
                    "type": "control_response",
                    "response": {"subtype": "success", "request_id": frame["request_id"], "response": {}},
                }
            )
            if frame["request"].get("subtype") == "initialize":
                await self.send.send(self.message)
                await self.send.aclose()

        async def read_messages(self):  # type: ignore[override]
            async with self.receive:
                async for frame in self.receive:
                    yield frame

        async def close(self) -> None:
            return None

        def is_ready(self) -> bool:
            return True

        async def end_input(self) -> None:
            return None

    async def nothing():
        return
        yield

    async def run(message: dict[str, Any]) -> dict[str, Any]:
        yielded: list[dict[str, Any]] = []
        error = None
        try:
            async for item in query(prompt=nothing(), options=ClaudeAgentOptions(), transport=OneMessage(message)):
                yielded.append({"class": type(item).__name__, "fields": dataclasses.asdict(item)})
        except Exception as caught:  # noqa: BLE001 - every failure is part of the recorded behavior
            error = {"class": type(caught).__name__, "message": str(caught)}
        return {"yielded": yielded, "error": error}

    cases = json.loads(Path(argv[2]).read_text())["cases"]
    results = {case["name"]: anyio.run(run, case["message"]) for case in cases}
    json.dump(results, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
