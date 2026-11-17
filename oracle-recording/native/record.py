#!/usr/bin/env python3
"""Runs one golden recorder of this folder natively: every oracle run it starts goes through the gate of `gate.py`
(host platform, real clock, the call position R3_CALL names, every spawned program ended), and what the runs reported
is written to TALLY_FILE.

usage: python3 -I record.py RECORDER TALLY_FILE NODE PACKAGE_DIR OUT_FILE [RECORDER FLAGS...]

The recorder runs as its own `__main__` with its own file name, so it finds `resolve_settings_oracle.mjs` beside it,
and with every argument after TALLY_FILE as its command line. Its `subprocess` is the standard module except `run`,
which checks each oracle run and counts the bytes the command line and environment take (`inherited` when the
recorder passes no environment). The organization cases of `ts_managed_settings_goldens.py` write `proc/version`,
which a native run cannot stand in for, so they are left out and listed under `diagnostic_only`. PYTHON_SDK, when set,
names a folder holding the pinned `claude_agent_sdk` for `settings_stand_in_goldens.py`.
"""

import json
import os
import subprocess as real_subprocess
import sys
import traceback
import types

HERE = os.path.dirname(os.path.abspath(__file__))
args = sys.argv[1:]
if len(args) < 5:
    sys.stderr.write("usage: record.py RECORDER TALLY_FILE NODE PACKAGE_DIR OUT_FILE [RECORDER FLAGS...]\n")
    sys.exit(2)
recorder = os.path.join(HERE, os.path.basename(args[0]))
tally_file = os.path.abspath(args[1])
GATE = {"__file__": os.path.join(HERE, "gate.py"), "__name__": "gate"}
with open(GATE["__file__"], encoding="utf-8") as handle:
    exec(compile(handle.read(), GATE["__file__"], "exec"), GATE)
if not GATE["NATIVE"]:
    sys.stderr.write("record.py records natively only: set R3_NATIVE=1\n")
    sys.exit(2)

STARTUP = {}
DIAGNOSTIC = []


def entry_bytes(text):
    return len(str(text).encode("utf-8")) + 9


def run(command, *positional, **named):
    """`subprocess.run`, with each oracle run checked by the gate and its startup bytes counted."""
    context = GATE["before"](command)
    if context is not None:
        environment = named.get("env")
        used = sum(entry_bytes(arg) for arg in command) + entry_bytes(f"executable_path={command[0]}")
        if environment is None:
            key = "inherited"
        else:
            key = str(used + sum(entry_bytes(f"{name}={value}") for name, value in environment.items()))
        with GATE["LOCK"]:
            STARTUP[key] = STARTUP.get(key, 0) + 1
    try:
        process = real_subprocess.run(command, *positional, **named)
    except real_subprocess.CalledProcessError:
        GATE["after"](context)
        raise
    GATE["after"](context)
    return process


def leave_out(cases):
    """Lists the organization cases as diagnostic only and leaves them out of the native record."""
    DIAGNOSTIC.extend(case[0] for case in cases)
    return []


shim = types.ModuleType("subprocess")
shim.__dict__.update({name: getattr(real_subprocess, name) for name in dir(real_subprocess) if not name.startswith("__")})
shim.run = run

with open(recorder, encoding="utf-8") as handle:
    source = handle.read()
if source.count("\nimport subprocess\n") != 1:
    sys.stderr.write(f"{recorder}: expected one `import subprocess` line\n")
    sys.exit(2)
source = source.replace("\nimport subprocess\n", "\n", 1)
if os.path.basename(recorder) == "ts_managed_settings_goldens.py":
    if source.count(" + FIXED + org_cases()\n") != 1:
        sys.stderr.write(f"{recorder}: expected one `+ FIXED + org_cases()`\n")
        sys.exit(2)
    source = source.replace(" + FIXED + org_cases()\n", " + FIXED + LEAVE_OUT(org_cases())\n", 1)
if os.environ.get("PYTHON_SDK"):
    sys.path.insert(0, os.environ["PYTHON_SDK"])

sys.argv = [recorder, *args[2:]]
scope = {"__name__": "__main__", "__file__": recorder, "subprocess": shim, "LEAVE_OUT": leave_out}
outcome = {"exit": 0, "error": None}
try:
    exec(compile(source, recorder, "exec"), scope)
except SystemExit as stop:
    outcome["exit"] = stop.code if isinstance(stop.code, int) else (0 if stop.code is None else 1)
    if stop.code not in (None, 0) and not isinstance(stop.code, int):
        outcome["error"] = str(stop.code)
except BaseException as error:  # noqa: BLE001 - the tally records why the recorder stopped
    outcome = {"exit": 1, "error": f"{type(error).__name__}: {error}", "trace": traceback.format_exc()[-4000:]}

report = {
    "recorder": os.path.basename(recorder),
    "arguments": args[2:],
    "platform": sys.platform,
    "call": GATE["CALL"],
    **outcome,
    "diagnostic_only": DIAGNOSTIC,
    "startup_bytes": STARTUP,
    "native": GATE["snapshot"](),
}
with open(tally_file, "w", encoding="utf-8") as handle:
    json.dump(report, handle, indent=1)
if outcome["error"]:
    sys.stderr.write(f"{report['recorder']}: {outcome['error']}\n")
sys.exit(outcome["exit"] or 0)
