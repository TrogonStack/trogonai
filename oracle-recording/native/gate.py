"""The per-run check of the native oracle, as the flag and PAD checks run on every probe.

With R3_NATIVE=1 every run of resolve_settings_oracle.mjs gets the call position R3_CALL names (`first` or `second`)
written into its spec, and after the run its `.native.json` report must show this host's platform, a real clock that
started inside the run, every spawned program ended, and no refusal. A refusal raises NativeRefusal, any other
failed check raises NativeViolation; both stop the unit, which records why. TALLY counts what every run reported.
"""

import json
import os
import sys
import threading
import time

NATIVE = os.environ.get("R3_NATIVE") == "1"
CALL = os.environ.get("R3_CALL") or "first"
ORACLE_NAME = "resolve_settings_oracle.mjs"
LOCK = threading.Lock()


class NativeRefusal(RuntimeError):
    """The native oracle refused a run whose spec needs another platform, a fake /proc or a simulated clock."""


class NativeViolation(RuntimeError):
    """A native run whose report does not hold the platform, clock or program checks."""


def fresh():
    return {"runs": 0, "platforms": {}, "calls": {}, "clock_first_start": None, "clock_offsets": {},
            "spawns": {}, "exits": {}, "unfinished_at_settle": 0, "ignored": {}, "refusals": {}, "violations": []}


TALLY = fresh()


def snapshot():
    with LOCK:
        return json.loads(json.dumps(TALLY))


def spec_of(command):
    """The spec path of an oracle command line (the argument after the oracle and the package), or None."""
    for index, arg in enumerate(command):
        if os.path.basename(str(arg)) == ORACLE_NAME:
            return str(command[index + 2]) if len(command) > index + 2 else None
    return None


def before(command):
    """Writes the call position into the spec of an oracle run and returns what `after` checks, or None."""
    if not NATIVE:
        return None
    spec = spec_of(command)
    if spec is None:
        return None
    with open(spec, encoding="utf-8") as handle:
        text = handle.read()
    head = '{"call":' + json.dumps(CALL)
    if not text.startswith(head):
        if not text.startswith("{"):
            raise NativeViolation(f"{spec}: a spec that is not a JSON object")
        text = head + ("" if text[1:].lstrip().startswith("}") else ",") + text[1:]
        with open(spec, "w", encoding="utf-8", newline="\n") as handle:
            handle.write(text)
    report = spec + ".native.json"
    if os.path.exists(report):
        os.remove(report)
    return {"spec": spec, "report": report, "started": int(time.time() * 1000)}


def count(table, key, by=1):
    table[key] = table.get(key, 0) + by


def after(context):
    """Checks the report of the run `before` prepared; raises on a refusal or a failed check."""
    if context is None:
        return
    ended = int(time.time() * 1000) + 1
    try:
        with open(context["report"], encoding="utf-8") as handle:
            report = json.load(handle)
        os.remove(context["report"])
    except (OSError, ValueError) as error:
        with LOCK:
            TALLY["runs"] += 1
            TALLY["violations"].append(f"{context['spec']}: no report ({error!r})")
        raise NativeViolation(f"{context['spec']}: the native oracle wrote no report") from error
    problems = []
    clock = report.get("clock") or {}
    if report.get("platform") != sys.platform:
        problems.append(f"platform {report.get('platform')!r}, not {sys.platform!r}")
    if clock.get("source") != "real":
        problems.append(f"clock {clock.get('source')!r}")
    start = clock.get("start")
    if not isinstance(start, int) or not context["started"] <= start <= ended:
        problems.append(f"clock start {start!r} outside the run [{context['started']}, {ended}]")
    if report.get("call") != CALL:
        problems.append(f"call {report.get('call')!r}, not {CALL!r}")
    if report.get("spawns") != report.get("exits"):
        problems.append(f"spawned {report.get('spawns')} but ended {report.get('exits')}")
    refused = report.get("refused")
    with LOCK:
        TALLY["runs"] += 1
        count(TALLY["platforms"], str(report.get("platform")))
        count(TALLY["calls"], str(report.get("call")))
        if isinstance(start, int) and (TALLY["clock_first_start"] is None or start < TALLY["clock_first_start"]):
            TALLY["clock_first_start"] = start
        for offset in clock.get("offsets") or []:
            count(TALLY["clock_offsets"], str(offset))
        for name, number in (report.get("spawns") or {}).items():
            count(TALLY["spawns"], name, number)
        for name, number in (report.get("exits") or {}).items():
            count(TALLY["exits"], name, number)
        for case in report.get("cases") or []:
            if case.get("running") or case.get("spawns") != case.get("exits"):
                TALLY["unfinished_at_settle"] += 1
        for item in report.get("ignored") or []:
            count(TALLY["ignored"], item.split(":", 1)[0])
        if refused:
            count(TALLY["refusals"], refused)
        elif problems:
            TALLY["violations"].append(f"{context['spec']}: {'; '.join(problems)}")
    if refused:
        raise NativeRefusal(f"native refusal: {refused}")
    if problems:
        raise NativeViolation(f"{context['spec']}: {'; '.join(problems)}")


def delta(before_tally, after_tally):
    """What the runs between two snapshots reported."""
    out = {}
    for key, value in after_tally.items():
        old = before_tally.get(key)
        if key == "clock_first_start":
            out[key] = value
        elif isinstance(value, int) and not isinstance(value, bool):
            out[key] = value - (old or 0)
        elif isinstance(value, dict):
            out[key] = {name: number - (old or {}).get(name, 0) for name, number in value.items()
                        if number != (old or {}).get(name, 0)}
        elif isinstance(value, list):
            out[key] = value[len(old or []):]
        else:
            out[key] = value
    return out
