#!/usr/bin/env python3
"""Replay of the job 54 band40 process for the drop_merge pair (Ruling 45 step 2, registration in the job log header).

usage: python3 -I replay54.py LIB NODE PACKAGE OUT.json WORKTREE

WORKTREE is the checkout of the commit job 54 ran (cbe37448cc): this driver execs that tree's own r3.py (the text before its
R3_LIST branch) with `__file__` set to the tree's path, so the harness, the recorder port, the oracle and the process shape are
job 54's. The call position is R3_CALL of the job, as in job 54. Each case runs in a Node process of its own, as
`Band39Runs.once` runs it. Two legs per level: U (the job 54 process, REF flags) and G (the same process with the V8 function
event log flags appended after the REF flags; PAD shrinks by what they add).
"""

import glob
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

LIB, NODE, PACKAGE, OUT, TREE = sys.argv[1:6]
REPEATS = int(os.environ.get("REPLAY_REPEATS", "3"))
BELOW, ABOVE = 6, 10
BUDGET_SECONDS = int(os.environ.get("REPLAY_BUDGET_SECONDS", str(150 * 60)))
LOG_FLAGS = ["--log-function-events", "--no-logfile-per-isolate"]
STARTED = time.time()

HERE = os.path.join(TREE, "oracle-recording")
R3_PATH = os.path.join(HERE, "r3.py")
with open(R3_PATH, encoding="utf-8") as handle:
    SOURCE = handle.read()
SOURCE = SOURCE[: SOURCE.index('\nif os.environ.get("R3_LIST"):')]
G = {"__name__": "r3", "__file__": R3_PATH}
saved = sys.argv
# r3.py's own argument list, as job 54 gave it: LIB NODE PACKAGE OUT SHARD SHARDS.
sys.argv = [R3_PATH, LIB, NODE, PACKAGE, OUT, os.environ["SHARD"], os.environ["SHARDS"]]
try:
    exec(compile(SOURCE, R3_PATH, "exec"), G)
finally:
    sys.argv = saved

D, GATE, HOST = G["D"], G["GATE"], G["HOST"]
REF, probe_flags = G["REF"], G["probe_flags"]
entry_bytes = D["entry_bytes"]


SPAWNED = []
_spawn = D["spawn"]


def counted_spawn(command, env, *args, **kwargs):
    """`discover.spawn`, noting what the command line and environment of each oracle run took (spawn itself refuses a run whose
    bytes differ from the budget its PAD was sized for)."""
    SPAWNED.append({"argv_entries": len(command), "argv_bytes": sum(entry_bytes(arg) for arg in command),
                    "startup_bytes": D["startup_bytes"](command, env), "pad_length": len(env.get("PAD", "")),
                    "env_names": sorted(env)})
    return _spawn(command, env, *args, **kwargs)


D["spawn"] = counted_spawn


def log(text):
    sys.stderr.write(text + "\n")
    sys.stderr.flush()


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()[:16]


def log_files(path):
    return sorted(set(glob.glob(path) + glob.glob(os.path.join(os.path.dirname(path), "*" + os.path.basename(path)))))


def read_events(paths):
    """The function events of sdk.mjs and of the oracle in a V8 log, in log order, split at the oracle's own functions.

    V8 logs `first-execution` only once a function has a feedback vector, which lazy feedback allocation delays past the
    call, so those events come late and cannot mark where a resolve starts. The split uses the compile events instead, which
    V8 logs when it compiles the function, at its first call: `resolveFrom` compiles at the start of the first resolve (the
    prime of a second call, else the case) and `snapshot` at its end. Items carry their event kind, so a reader can keep the
    compile events only. Preparse events are left out. `sequence` is every kept event in log order, for the levels a reader
    needs whole."""
    best = None
    for path in paths:
        scripts, events = {}, []
        with open(path, encoding="utf-8", errors="replace") as handle:
            for line in handle:
                parts = line.rstrip("\n").split(",")
                if parts[0] == "script-details" and len(parts) > 2:
                    scripts[parts[1]] = parts[2]
                elif parts[0] == "function" and len(parts) > 7 and not parts[1].startswith("preparse"):
                    events.append((parts[1], parts[2], parts[3], parts[4], ",".join(parts[7:])))
        sdk = {key for key, name in scripts.items() if name.endswith("sdk.mjs")}
        oracle = {key for key, name in scripts.items() if name.endswith("resolve_settings_oracle.mjs")}

        def marker(name, after, compiled):
            return next((i for i, one in enumerate(events) if i > after and one[1] in oracle and one[4] == name
                         and ((one[0] != "first-execution") if compiled else (one[0] == "first-execution"))), None)

        start = marker("resolveFrom", -1, True)
        end = None if start is None else marker("snapshot", start, True)
        late_start = marker("resolveFrom", -1, False)
        late_end = None if late_start is None else marker("snapshot", late_start, False)
        picked = [(i, one) for i, one in enumerate(events) if one[1] in sdk]
        found = {"bounds": {"resolveFrom": start, "snapshot": end},
                 "late_bounds": {"resolveFrom": late_start, "snapshot": late_end},
                 "events": len(events), "sdk_events": len(picked),
                 "pre": sorted({f"{k}:{s}:{e}" for i, (k, _, s, e, n) in picked if start is not None and i < start}),
                 "first": sorted({f"{k}:{s}:{e}" for i, (k, _, s, e, n) in picked
                                  if start is not None and i >= start and (end is None or i < end)}),
                 "after": sorted({f"{k}:{s}:{e}" for i, (k, _, s, e, n) in picked if end is not None and i >= end}),
                 "names": {f"{s}:{e}": n for i, (k, _, s, e, n) in picked},
                 "sequence": [[k, "s" if script in sdk else "o", s, e] + ([] if script in sdk else [n])
                              for k, script, s, e, n in events if script in sdk or script in oracle]}
        if best is None or found["sdk_events"] > best["sdk_events"]:
            best = found
    return best


def heads(spans):
    """The first 60 characters of the definition at each `start:end` of sdk.mjs (offsets in UTF-16 code units)."""
    with open(os.path.join(PACKAGE, "sdk.mjs"), encoding="utf-8") as handle:
        units = handle.read().encode("utf-16-le")
    out = {}
    for span in spans:
        start, end = (int(part) for part in span.split(":"))
        out[span] = units[2 * start:2 * min(end, start + 60)].decode("utf-16-le", "replace")
    return out


port = G["mg8_port"]()
port["GROUPS"] = [group for group in port["GROUPS"] if group[0] == "drop_merge"]
profile = dict(port["PROFILES"][HOST])
pairs = G["band8_pairs"](port, profile)
if [pair["id"] for pair in pairs] != ["drop_merge"]:
    sys.exit(f"the drop_merge pair is not the one pair of the port: {[pair['id'] for pair in pairs]}")
runs = G["Band39Runs"](port)
CALL = GATE["CALL"]
result = {"host": HOST, "call": CALL, "context": GATE["CONTEXT"], "node": subprocess.run([NODE, "--version"], capture_output=True, check=True).stdout.decode().strip(),
          "tree": TREE, "repeats": REPEATS, "ref": list(REF), "log_flags": LOG_FLAGS, "profile_level": profile["drop_merge"],
          "startup_budget": port["STARTUP_BYTES"], "levels": {}, "stops": []}


def save():
    result["seconds"] = round(time.time() - STARTED)
    with open(OUT, "w", encoding="utf-8") as handle:
        json.dump(result, handle)


def logged(case, extra, path):
    """Band39Runs.once_full with the flags `REF + LOG_FLAGS + --logfile=path`: the collapsed answer, the command's byte
    accounting and the parsed log. `port['run']` sizes PAD for the whole command, so the startup bytes stay at the budget."""
    folder = tempfile.mkdtemp()
    with runs.lock:
        runs.runs += 1
        runs.folders.add(folder)
    GATE["in_context"](G["BAND37_DEFAULT"])
    flags = [*REF, *extra, f"--logfile={path}"]
    try:
        spec_path = port["prepare"](case[0], case[1], folder)
        startup = port["case_startup"](case)
        mark = len(SPAWNED)
        answered = port["run"](spec_path, flags, startup)
        account = SPAWNED[mark] if len(SPAWNED) > mark else None
        if not isinstance(answered, dict):
            return None, account, None
        return G["mg8_collapse"](answered), account, read_events(log_files(path))
    except (GATE["NativeRefusal"], GATE["NativeViolation"]) as error:
        return {"gate": f"{type(error).__name__}: {str(error)[:300]}"}, None, None
    except RuntimeError as error:
        return {"runtime": str(error)[:300]}, None, None
    except (Exception, SystemExit) as error:
        return {"failed": repr(error)[:300]}, None, None
    finally:
        GATE["in_context"](None)
        shutil.rmtree(folder, ignore_errors=True)
        for one in log_files(path):
            os.remove(one)


def unlogged(case):
    """The job 54 process: the REF reference run of `band39_locate`, through `Band39Runs.once`."""
    mark = len(SPAWNED)
    answer = runs.once(case, probe_flags(REF))
    return answer, (SPAWNED[mark] if len(SPAWNED) > mark else None)


try:
    memo = {}
    moved = G["band39_locate"](runs, pairs[0], memo)
except G["Band8Stop"] as stop:
    result["stops"].append(f"locate: {stop}")
    save()
    sys.exit(1)
answers = {"near": moved["_answers"]["near"], "far": moved["_answers"]["far"]}
result["located"] = {"near_level": moved["near_level"], "far_level": moved["far_level"], "shift": moved["shift"],
                     "near": moved["near"][0], "far": moved["far"][0], "answers_sha": {k: digest(v) for k, v in answers.items()}}
log(f"located {result['located']}")
save()


def klass(answer):
    if answer == answers["near"]:
        return "near"
    if answer == answers["far"]:
        return "far"
    return "none" if answer is None else "other"


window = range(profile["drop_merge"] - BELOW, profile["drop_merge"] + ABOVE + 1)
needed = set()
for level in window:
    case = moved["near_at"](level)
    record = {"case": case[0], "U": [], "G": []}
    result["levels"][str(level)] = record
    for repeat in range(REPEATS):
        if time.time() - STARTED > BUDGET_SECONDS:
            result["stops"].append(f"the time budget ended the window at level {level}")
            break
        answer, account = unlogged(case)
        record["U"].append({"class": klass(answer), "answer_sha": digest(answer), "account": account})
        path = tempfile.mkdtemp() + ".v8.log"
        os.rmdir(path[: -len(".v8.log")])
        answer, account, found = logged(case, LOG_FLAGS, path)
        failed = isinstance(answer, dict) and bool({"gate", "runtime", "failed"} & set(answer))
        entry = {"class": "error" if failed else klass(answer), "answer_sha": digest(answer), "account": account}
        if failed:
            entry["error"] = answer
        if found is not None:
            entry["bounds"], entry["events"], entry["sdk_events"] = found["bounds"], found["events"], found["sdk_events"]
            entry["late_bounds"] = found["late_bounds"]
            entry["pre_count"], entry["pre_sha"] = len(found["pre"]), digest(found["pre"])
            keep = level in range(profile["drop_merge"] - 1, profile["drop_merge"] + 5)
            if keep:
                entry["first"], entry["after"] = found["first"], found["after"]
                entry["pre"] = found["pre"]
                if repeat == 0:
                    entry["sequence"] = found["sequence"]
                needed.update(found["first"] + found["after"])
                result.setdefault("names", {}).update(found["names"])
            else:
                entry["first_count"], entry["first_sha"] = len(found["first"]), digest(found["first"])
                entry["after_count"], entry["after_sha"] = len(found["after"]), digest(found["after"])
        record["G"].append(entry)
    log(f"L{level} U={[one['class'] for one in record['U']]} G={[one['class'] for one in record['G']]}")
    save()

result["heads"] = heads(sorted({":".join(item.split(":")[1:]) for item in needed}))
result["folder_assertion"] = runs.tally()
save()
shutil.rmtree(D["SCRATCH"], ignore_errors=True)
log(f"done in {result['seconds']} s")
