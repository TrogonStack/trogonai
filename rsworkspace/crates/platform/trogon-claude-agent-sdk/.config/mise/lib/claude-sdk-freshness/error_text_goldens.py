#!/usr/bin/env python3
"""Print the exception texts the pinned Python SDK builds in the `__init__` of its error classes.

usage: python3 -I error_text_goldens.py PYTHON_SDK_DIRECTORY

The output is the JSON the Rust `error_tests.rs` goldens were copied from.
"""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path


def load_errors(directory: Path):
    spec = importlib.util.spec_from_file_location("sdk_errors", directory / "_errors.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(__doc__.strip().splitlines()[2], file=sys.stderr)
        return 2
    errors = load_errors(Path(argv[1]))
    long_line = "x" * 120
    results = {
        "not_found_default": str(errors.CLINotFoundError()),
        "not_found_path": str(errors.CLINotFoundError(cli_path="/opt/claude")),
        "not_found_message": str(errors.CLINotFoundError("custom")),
        "not_found_message_path": str(errors.CLINotFoundError("custom", "/opt/claude")),
        "not_found_empty_path": str(errors.CLINotFoundError(cli_path="")),
        "process_bare": str(errors.ProcessError("failed")),
        "process_exit": str(errors.ProcessError("failed", exit_code=7)),
        "process_zero_exit": str(errors.ProcessError("failed", exit_code=0)),
        "process_stderr": str(errors.ProcessError("failed", stderr="boom")),
        "process_empty_stderr": str(errors.ProcessError("failed", stderr="")),
        "process_both": str(errors.ProcessError("failed", 7, "boom")),
        "result_bare": str(errors.ResultError("failed")),
        "result_exit": str(errors.ResultError("failed", {"subtype": "error_max_turns"}, 1)),
        "result_non_dict": str(errors.ResultError("failed", "text", 2)),
        "decode_short": str(errors.CLIJSONDecodeError("{oops", ValueError("bad"))),
        "decode_long": str(errors.CLIJSONDecodeError(long_line, ValueError("bad"))),
        "decode_exact": str(errors.CLIJSONDecodeError("y" * 100, ValueError("bad"))),
    }
    result = errors.ResultError(
        "failed",
        {
            "subtype": "error_max_turns",
            "errors": ["  a  ", "", 3, "b"],
            "result": "text",
            "api_error_status": 529,
            "terminal_reason": "max_turns",
            "session_id": "s",
        },
    )
    results["result_fields"] = {
        "subtype": result.subtype,
        "errors": result.errors,
        "result": result.result,
        "api_error_status": result.api_error_status,
        "terminal_reason": result.terminal_reason,
        "session_id": result.session_id,
    }
    print(json.dumps(results, indent=2, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
