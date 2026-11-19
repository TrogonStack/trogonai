#!/usr/bin/env python3
"""Diagnostic for the J55-A rows (Ruling 48 rule 1: discovery only, R43-6, never a golden, a fixture or a model row).

Ruling 48 rule 1 asks for failed-validation provenance of job 55: why the J55-A cases (machine_after_held_array at rel -1..+3
and machine_after_held_arrays at rel -3..+3, prime Pm, second call, toplevel context) answer a null managed_source with the
reference flags and a set one with the cross-check flags. Every output is evidence, kept as it is. REF and CROSS stay separate
evidence, nothing is scored, and nothing here is written into a golden. The predictions and falsifiers are registered in PLAN
Update 169 before any run.

usage: python3 -I diag55a.py LIB NODE PACKAGE OUT.json WORKTREE

WORKTREE is the checkout of the commit job 55 ran (71e5304c). This driver execs that tree's own r3.py (the text before its
R3_LIST branch) with `__file__` set to the tree's path, runs its pm8 units (which fill the profile the way job 55's own run
did), and then runs the case-building text of its `j55_unit` as written (the lines from the call and context gate up to the
unit's `record` helper), so the cases, primes, startup bytes and the patched `prepare` and `run` are job 55's. It keeps the 12
J55-A cases per host and their P0 twins (same family and rel, no prime). The job runs with R3_CALL=second and R3_CONTEXT=toplevel
as job 55 did, and the frozen text refuses any other. Each run is a Node process of its own with the case alone in it. Flags
added to a run shrink PAD by what they add (`port["run"]` sizes PAD for the whole command, and `discover.spawn` refuses a run
whose bytes differ from the budget), so every run starts Node with the job 55 startup bytes.

Legs, per case (each run a fresh process):
  R    REF flags, twice (the Pm case).
  C    CROSS flags, twice (the Pm case).
  C0   CROSS flags, once, on the P0 twin.
  Cs   CROSS + --no-sparkplug.
  Cm   CROSS + --no-maglev.
  Ct   CROSS + --no-turbofan.
  Cb   CROSS + --no-maglev --no-turbofan (Sparkplug is the only tier).
  Ctr  CROSS + --trace-opt --trace-deopt + the V8 function event log flags, with the trace output kept in the artifact folder
       and the tiered sdk.mjs functions listed before and after the case's marker.
  Ctr0 the same as Ctr on the P0 twin (an addition, so that the prediction about the twin has data).
  Edge under the C flags only, for each of the two families with prime Pm, from rel +3 upward by band8_next: the first level
       where managed_source is null, by bisection from a bracket to rel +130, once to +400 (hist43b's bracket, PLAN Update 132).
Each run records effective, exact, machine_policy (lengths and digests, with the head), managed_source, policyOrigin, error,
the flags and the spawn byte record. A time budget (DIAG_BUDGET_SECONDS, default 150 minutes) ends the work; the legs not
reached are listed and the run still ends cleanly.
"""

import concurrent.futures
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
BUDGET_SECONDS = int(os.environ.get("DIAG_BUDGET_SECONDS", str(150 * 60)))
LOG_FLAGS = ["--log-function-events", "--no-logfile-per-isolate"]
TRACE_FLAGS = ["--trace-opt", "--trace-deopt"]
MARKER_TEXT = '{"marker":"cases"}'
STARTED = time.time()
ARTIFACTS = os.path.join(os.path.dirname(os.path.abspath(OUT)), "diag55a-traces")
os.makedirs(ARTIFACTS, exist_ok=True)

FAMILY_A, FAMILY_B = "machine_after_held_array", "machine_after_held_arrays"
SELECTED = [(FAMILY_A, rel) for rel in range(-1, 4)] + [(FAMILY_B, rel) for rel in range(-3, 4)]
EDGE_FROM, EDGE_BRACKET, EDGE_FAR = 3, 130, 400

HERE = os.path.join(TREE, "oracle-recording")
R3_PATH = os.path.join(HERE, "r3.py")
with open(R3_PATH, encoding="utf-8") as handle:
    SOURCE = handle.read()
SOURCE = SOURCE[: SOURCE.index('\nif os.environ.get("R3_LIST"):')]
G = {"__name__": "r3", "__file__": R3_PATH}
saved = sys.argv
# r3.py's own argument list, as job 55 gave it: LIB NODE PACKAGE OUT SHARD SHARDS (shard 1 of 1).
sys.argv = [R3_PATH, LIB, NODE, PACKAGE, OUT, "1", "1"]
try:
    exec(compile(SOURCE, R3_PATH, "exec"), G)
finally:
    sys.argv = saved

D, GATE, HOST = G["D"], G["GATE"], G["HOST"]
REF, CROSS = list(G["REF"]), list(G["CROSS"])
entry_bytes = D["entry_bytes"]

SPAWNED = []
LAST = {}
MODE = {"trace": False, "marker": False}
_spawn = D["spawn"]
TRACE_LOG_ENTRY = "TRACE_LOG=/dev/stdout"


def counted_spawn(command, env, *args, **kwargs):
    """`discover.spawn`, noting what the command line and environment of each oracle run took (spawn itself refuses a run
    whose bytes differ from the budget its PAD was sized for) and keeping the raw output of the run.

    In a trace run the oracle's own `TRACE_LOG` is pointed at the standard output, so the marker the oracle writes between the
    primes and the recorded cases lands in the same stream as the V8 trace lines; PAD shrinks by the bytes of that variable.
    The answer is cut out of the stream and handed on, so the recorder reads it as it always does."""
    if MODE["trace"] and MODE["marker"]:
        env = dict(env)
        cut = entry_bytes(TRACE_LOG_ENTRY)
        if len(env.get("PAD", "")) < cut:
            raise RuntimeError("PAD is too short to make room for TRACE_LOG")
        env["PAD"] = env["PAD"][: len(env["PAD"]) - cut]
        env["TRACE_LOG"] = "/dev/stdout"
    SPAWNED.append({"argv_entries": len(command), "argv_bytes": sum(entry_bytes(arg) for arg in command),
                    "startup_bytes": D["startup_bytes"](command, env), "pad_length": len(env.get("PAD", "")),
                    "env_names": sorted(env)})
    LAST.clear()
    process = _spawn(command, env, *args, **kwargs)
    stdout = getattr(process, "stdout", None)
    if isinstance(stdout, bytes):
        LAST["returncode"] = process.returncode
        if MODE["trace"] and process.returncode == 0:
            text = stdout.decode("utf-8", "replace")
            at = text.rfind('[{"name":')
            if at >= 0:
                try:
                    _, end = json.JSONDecoder().raw_decode(text, at)
                    LAST["trace"] = text[:at] + text[end:]
                    LAST["stdout"] = text[at:end].encode("utf-8")
                    process.stdout = LAST["stdout"]
                    return process
                except ValueError:
                    pass
            LAST["trace"] = text
        else:
            LAST["stdout"] = stdout
    return process


D["spawn"] = counted_spawn


def log(text):
    sys.stderr.write(text + "\n")
    sys.stderr.flush()


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()[:16]


def log_files(path):
    return sorted(set(glob.glob(path) + glob.glob(os.path.join(os.path.dirname(path), "*" + os.path.basename(path)))))


TIER_KIND = re.compile(r"optim|baseline|sparkplug|maglev|turbofan|osr|deopt", re.I)


def read_events(paths):
    """The function events of sdk.mjs and of the oracle in a V8 log, in log order, split at the oracle's own functions.

    As replay54's reader does: V8 logs `first-execution` only once a function has a feedback vector, so the split uses the
    compile events, which V8 logs when it compiles the function at its first call: `resolveFrom` compiles at the start of the
    first resolve (the prime of a second call, else the case) and `snapshot` at its end. `pre` is before the first resolve,
    `first` the first resolve (the prime when there is one), `after` from the end of that resolve on. `kinds` counts every
    event kind seen for sdk.mjs and `tier_events` keeps the ones that name a tier or an optimization."""
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
        picked = [(i, one) for i, one in enumerate(events) if one[1] in sdk]
        kinds = {}
        for _, one in picked:
            kinds[one[0]] = kinds.get(one[0], 0) + 1
        found = {"bounds": {"resolveFrom": start, "snapshot": end}, "events": len(events), "sdk_events": len(picked),
                 "kinds": kinds,
                 "pre": sorted({f"{k}:{s}:{e}" for i, (k, _, s, e, n) in picked if start is not None and i < start}),
                 "first": sorted({f"{k}:{s}:{e}" for i, (k, _, s, e, n) in picked
                                  if start is not None and i >= start and (end is None or i < end)}),
                 "after": sorted({f"{k}:{s}:{e}" for i, (k, _, s, e, n) in picked if end is not None and i >= end}),
                 "tier_events": sorted({f"{k}:{s}:{e}:{n}" for _, (k, _, s, e, n) in picked if TIER_KIND.search(k)}),
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


TRACE_LINE = re.compile(r"^\[(marking|compiling method|completed (?:optimizing|compiling)|bailout|found|evicting|discarding"
                        r"|deoptimizing|aborted|optimization)\b.*?<JSFunction ([^ >]*)", re.M)
TIER_NAME = re.compile(r"\b(MAGLEV|TURBOFAN_JS|TURBOFAN|TURBOSHAFT|SPARKPLUG|BASELINE|INTERPRETED)\b")
DEOPT_KIND = re.compile(r"kind: ([\w-]+)")


def parse_trace(text, ranges_of):
    """The trace lines that name a function, as one entry per (kind, function, tier): name, the sdk.mjs ranges the log gives
    that name (empty when the log names none), count. `kind` is the first word of the line."""
    found = {}
    for line in text.split("\n"):
        hit = TRACE_LINE.match(line)
        if not hit:
            continue
        tier = TIER_NAME.search(line)
        key = (hit.group(1), hit.group(2), tier.group(1) if tier else None)
        entry = found.setdefault(key, {"kind": key[0], "name": key[1], "tier": key[2],
                                       "ranges": ranges_of.get(key[1], []), "count": 0})
        entry["count"] += 1
        if key[0] == "bailout":
            reason = DEOPT_KIND.search(line)
            if reason:
                entry.setdefault("deopt_kinds", [])
                if reason.group(1) not in entry["deopt_kinds"]:
                    entry["deopt_kinds"].append(reason.group(1))
    return sorted(found.values(), key=lambda one: (one["kind"], one["name"], str(one["tier"])))


# ---- the frozen pm8 units and the j55 case building text ----
def freeze_units():
    """job 55's pm8 units, run as r3.py's main runs them, so that its profile step has the classes it fills from."""
    state = {}
    G["R8_STATE"]["units"] = state
    chosen = [unit for unit in G["units"]() if re.search(r"^pm8:", unit[0])]
    with concurrent.futures.ThreadPoolExecutor(max_workers=G["WORKERS"]) as pool, \
            concurrent.futures.ThreadPoolExecutor(max_workers=G["QUIET"]) as quiet:
        for name, go in chosen:
            try:
                state[name] = go(pool, quiet)
            except Exception as error:
                state[name] = {"error": repr(error)}
            log(f"{name} {json.dumps({k: v for k, v in state[name].items() if k in ('class', 'error')})[:300]}")
    return {name: {k: v for k, v in found.items() if k in ("class", "error")} for name, found in state.items()}


def define_builder():
    """`j55_build(pool, quiet)`: the text of the frozen `j55_unit` from its call and context gate up to its `record` helper, as
    written there, made a function of its own that hands back what it built."""
    unit = SOURCE.index("def j55_unit():")
    start = SOURCE.index('        if GATE["CALL"] != "second":', unit)
    stop = SOURCE.index("        def record(case):", start)
    wrapped = ("def j55_build(pool, quiet):\n" + SOURCE[start:stop]
               + "        return {'built': True, 'port': port, 'cases': cases, 'rows': rows, 'registry': registry,\n"
                 "                'pairs': pairs, 'margins': margins, 'wanted': wanted, 'fixed': fixed, 'out': out}\n")
    exec(compile(wrapped, R3_PATH + "#j55_build", "exec"), G)
    return G["j55_build"]


result = {"host": HOST, "call": GATE["CALL"], "context": GATE["CONTEXT"],
          "node": subprocess.run([NODE, "--version"], capture_output=True, check=True).stdout.decode().strip(),
          "tree": "71e5304c", "budget_seconds": BUDGET_SECONDS,
          "flags": {"REF": REF, "CROSS": CROSS, "log": LOG_FLAGS, "trace": TRACE_FLAGS},
          "startup_budget": None, "pm8": None, "built": None, "self_check": None, "runs": [], "edge": {},
          "not_reached": [], "stops": [], "summary": {}}


def save():
    result["seconds"] = round(time.time() - STARTED)
    with open(OUT, "w", encoding="utf-8") as handle:
        json.dump(result, handle)


def over():
    return time.time() - STARTED > BUDGET_SECONDS


def finish(code):
    save()
    shutil.rmtree(D["SCRATCH"], ignore_errors=True)
    sys.exit(code)


try:
    result["pm8"] = freeze_units()
    save()
    # The build text profiles through mg8_profile(pool, ...), so it gets the same two pools r3.py's main hands a unit.
    with concurrent.futures.ThreadPoolExecutor(max_workers=G["WORKERS"]) as build_pool, \
            concurrent.futures.ThreadPoolExecutor(max_workers=G["QUIET"]) as build_quiet:
        built = define_builder()(build_pool, build_quiet)
except (Exception, SystemExit) as error:
    result["stops"].append(f"build: {error!r}"[:600])
    finish(1)
if not isinstance(built, dict) or "built" not in built:
    result["stops"].append(f"the j55 build text gave no cases: {json.dumps(built, default=str)[:600]}")
    finish(1)

port, cases, rows, registry = built["port"], built["cases"], built["rows"], built["registry"]
pairs, margins, wanted, fixed = built["pairs"], built["margins"], built["wanted"], built["fixed"]
result["startup_budget"] = port["STARTUP_BYTES"]
by_key = {(row["family"], row["rel"], row["prime"]): index for index, row in enumerate(rows)}

selected = []
for family, rel in SELECTED:
    pm, p0 = by_key.get((family, rel, "Pm")), by_key.get((family, rel, "P0"))
    if pm is None or p0 is None:
        result["stops"].append(f"no job 55 case for {family} rel {rel} (Pm {pm}, P0 {p0})")
        continue
    selected.append({"family": family, "rel": rel, "level": rows[pm]["level"], "pm": pm, "p0": p0})
result["built"] = {"cases_total": len(cases), "selected": [{**one, "pm_case": cases[one["pm"]][0], "p0_case": cases[one["p0"]][0],
                                                          "startup": rows[one["pm"]]["startup"]} for one in selected]}
save()


def run_once(case, flags, leg, info, trace=False, marker=True):
    """One case in a process of its own with `flags`: the recorded answer fields, the flags and the spawn byte record."""
    folder = tempfile.mkdtemp()
    started = time.time()
    record = {"leg": leg, **info, "case": case[0], "flags": list(flags)}
    mark = len(SPAWNED)
    LAST.clear()
    MODE["trace"], MODE["marker"] = trace, marker
    GATE["in_context"](G["BAND37_DEFAULT"])
    try:
        spec_path = port["prepare"](case[0], case[1], folder)
        answered = port["run"](spec_path, flags, port["case_startup"](case))
        if not isinstance(answered, dict):
            record["failed"] = f"answer is not a dict: {type(answered).__name__}"
        else:
            record["answer"] = summarize(answered)
            record["policyOrigin"] = origin_of(LAST.get("stdout"))
            record["error"] = answered.get("error")
    except (GATE["NativeRefusal"], GATE["NativeViolation"]) as error:
        record["gate"] = f"{type(error).__name__}: {str(error)[:300]}"
    except RuntimeError as error:
        record["runtime"] = str(error)[:300]
    except (Exception, SystemExit) as error:
        record["failed"] = repr(error)[:300]
    finally:
        MODE["trace"] = MODE["marker"] = False
        GATE["in_context"](None)
        shutil.rmtree(folder, ignore_errors=True)
    record["account"] = SPAWNED[mark] if len(SPAWNED) > mark else None
    record["seconds"] = round(time.time() - started, 1)
    return record


def piece(value):
    """A long answer member as its length, digest and head."""
    if value is None:
        return None
    text = value if isinstance(value, str) else json.dumps(value)
    return {"len": len(text), "sha": hashlib.sha256(text.encode("utf-8", "replace")).hexdigest()[:16], "head": text[:160]}


def summarize(answered):
    managed = answered.get("managed_source")
    text = managed if isinstance(managed, str) else json.dumps(managed)
    return {"effective": piece(answered.get("effective")), "exact": piece(answered.get("exact")),
            "machine_policy": piece(answered.get("machine_policy")),
            "managed_source": G["mg8_collapse"](managed)[:2000] if managed is not None else None,
            "managed_source_sha": hashlib.sha256(text.encode("utf-8", "replace")).hexdigest()[:16],
            "managed_null": managed in (None, "null"),
            "effective_set": answered.get("effective") not in (None, "null"),
            "answer_sha": digest(answered)}


def origin_of(stdout):
    """`policyOrigin` of the managed source the oracle printed (the recorder's answer holds no such member; it is the member
    of the managed source), else the member of the result itself, else None."""
    if not stdout:
        return None
    try:
        entries = json.loads(stdout.decode("utf-8"))
        found = entries[0]["result"]
        managed = next((one for one in found.get("sources", []) if one.get("source") == "managed"), None)
        if managed is not None:
            return managed.get("policyOrigin", "absent")
        return found.get("policyOrigin")
    except (ValueError, KeyError, IndexError, TypeError, AttributeError, RecursionError):
        return "unreadable"


def klass(record):
    if "answer" not in record:
        return "fail"
    if record.get("error"):
        return "error"
    return "null" if record["answer"]["managed_null"] else "set"


def keep(record, key):
    result["runs"].append(record)
    result["summary"].setdefault(key, {}).setdefault(record["leg"], []).append(klass(record))
    save()
    return record


def skip(key, leg, count=1):
    result["not_reached"].extend([f"{key}/{leg}"] * count)


def ctr(case, info, key, leg):
    """The trace leg: CROSS + the trace flags + the V8 function event log, with the oracle marker in the trace stream."""
    base = tempfile.mkdtemp()
    path = base + ".v8.log"
    os.rmdir(base)
    flags = [*CROSS, *TRACE_FLAGS, *LOG_FLAGS, f"--logfile={path}"]
    try:
        record = run_once(case, flags, leg, info, trace=True)
        # One run, never repeated: a trace run that gives no answer is recorded as it is (no retry, mg37 rule).
        text = LAST.get("trace")
        found = read_events(log_files(path)) if os.path.exists(path) or log_files(path) else None
        record["marker_in_stdout"] = bool(text) and MARKER_TEXT in text
        if found is None:
            record["events"] = None
        else:
            ranges_of = {}
            for span, name in found["names"].items():
                ranges_of.setdefault(name, []).append(span)
            if text:
                at = text.find(MARKER_TEXT)
                before, after = (text[:at], text[at + len(MARKER_TEXT):]) if at >= 0 else (text, "")
                record["trace"] = {"bytes": len(text), "split": "marker" if at >= 0 else "none",
                                   "before": parse_trace(before, ranges_of), "after": parse_trace(after, ranges_of)}
                name = f"{case[0]}.{leg}.trace.txt"
                with open(os.path.join(ARTIFACTS, name), "w", encoding="utf-8") as handle:
                    handle.write(text)
                record["trace"]["file"] = name
            record["events"] = {k: found[k] for k in ("bounds", "events", "sdk_events", "kinds", "pre", "first", "after",
                                                     "tier_events", "names")}
            wanted_spans = sorted({span for part in ("before", "after") for one in record.get("trace", {}).get(part, [])
                                   for span in one["ranges"]})
            record["heads"] = heads(wanted_spans[:400])
            with open(os.path.join(ARTIFACTS, f"{case[0]}.{leg}.events.json"), "w", encoding="utf-8") as handle:
                json.dump(found["sequence"], handle)
        if text is None:
            record["trace_missing"] = True
        return keep(record, key)
    finally:
        for one in log_files(path):
            os.remove(one)


LEGS = (("R", REF, 2), ("C", CROSS, 2), ("Cs", CROSS + ["--no-sparkplug"], 1), ("Cm", CROSS + ["--no-maglev"], 1),
        ("Ct", CROSS + ["--no-turbofan"], 1), ("Cb", CROSS + ["--no-maglev", "--no-turbofan"], 1))

for one in selected:
    key = f"{one['family']}/{one['rel']}"
    pm_case, p0_case = cases[one["pm"]], cases[one["p0"]]
    info = {"family": one["family"], "rel": one["rel"], "level": one["level"]}
    plan = [(name, flags, pm_case, "Pm") for name, flags, count in LEGS for _ in range(count)]
    plan.insert(4, ("C0", CROSS, p0_case, "P0"))
    plan += [("Ctr", None, pm_case, "Pm"), ("Ctr0", None, p0_case, "P0")]
    for number, (leg, flags, case, prime) in enumerate(plan):
        if over():
            skip(key, leg)
            continue
        if leg.startswith("Ctr"):
            ctr(case, {**info, "prime": prime}, key, leg)
        else:
            keep(run_once(case, flags, leg, {**info, "prime": prime}), key)
    log(f"{key} " + " ".join(f"{leg}={','.join(classes)}" for leg, classes in result["summary"].get(key, {}).items()))


# ---- the edge leg: the first level above rel +3 where managed_source is null under the C flags ----
def pm_case(family, level):
    """The Pm case of `family` at `level`, built the way the frozen unit builds it (its own `j55_level` and pair makers)."""
    pair = pairs[family]
    made = pair["near_at"](level)
    primed = []
    for prime in wanted["Pm"]["primes"]:
        of = family if prime["family"] == "own" else prime["family"]
        primed.append(pairs[of]["near_at"](G["j55_level"](pairs, margins, of, prime, level))[1])
    name = f"{made[0]}__Pm"
    if name in registry and registry[name] != primed:
        name = f"{name}_e{level}"
    registry[name] = primed
    return (name, made[1]) + tuple(made[2:])


def self_check():
    """The edge builder must give the frozen cases back for the rows the frozen text built (rel -3..+3 of both families)."""
    checked = {}
    for one in selected:
        frozen = cases[one["pm"]]
        mine = pm_case(one["family"], one["level"])
        checked[f"{one['family']}/{one['rel']}"] = bool(
            list(mine[1:]) == list(frozen[1:]) and registry[mine[0]] == registry[frozen[0]])
    result["self_check"] = checked
    return all(checked.values())


def edge(family):
    state = {"status": None, "probes": {}}
    result["edge"][family] = state
    first = next((one for one in selected if one["family"] == family and one["rel"] == EDGE_FROM), None)
    if first is None:
        state["status"] = "no rel +3 row"
        return
    levels = {EDGE_FROM: first["level"]}

    def level_at(rel):
        for step in range(max(levels) + 1, rel + 1):
            levels[step] = G["band8_next"](pairs[family], levels[step - 1], "far")
        return levels[rel]

    def null_at(rel):
        """True when managed_source is null at rel, False when set, None when the run gave no answer or an error."""
        if over():
            state["status"] = f"budget ended before rel +{rel}"
            return None
        try:
            level = level_at(rel)
            case = pm_case(family, level)
        except (Exception, SystemExit) as error:
            state["status"] = f"no level at rel +{rel}: {error!r}"[:300]
            return None
        record = run_once(case, CROSS, "E", {"family": family, "rel": rel, "level": level, "prime": "Pm"})
        keep(record, f"{family}/edge")
        state["probes"][str(rel)] = klass(record)
        return {"null": True, "set": False}.get(klass(record))

    low = null_at(EDGE_FROM)
    if low is None:
        state["status"] = state["status"] or "no answer at +3"
        return
    if low:
        state["status"] = "null at +3: the edge is at or below +3"
        return
    high = EDGE_BRACKET
    answer = null_at(high)
    if answer is False:
        high = EDGE_FAR
        answer = null_at(high)
    if answer is None:
        state["status"] = state["status"] or f"no answer at +{high}"
        return
    if answer is False:
        state["status"] = f"managed_source set through +{EDGE_FAR}"
        return
    low = EDGE_FROM
    while high - low > 1:
        middle = (low + high) // 2
        answer = null_at(middle)
        if answer is None:
            state["status"] = state["status"] or f"no answer at +{middle}"
            return
        low, high = (middle, high) if not answer else (low, middle)
    state.update({"status": "ok", "last_set_rel": low, "first_null_rel": high, "first_null_level": levels.get(high),
                  "last_set_level": levels.get(low)})


try:
    if not self_check():
        result["stops"].append("the edge builder does not rebuild the frozen cases; the edge leg is not run")
        result["not_reached"].append("edge/all")
    else:
        for family in (FAMILY_A, FAMILY_B):
            edge(family)
            log(f"edge {family} {result['edge'][family]}")
except (Exception, SystemExit) as error:
    result["stops"].append(f"edge: {error!r}"[:600])

save()
shutil.rmtree(D["SCRATCH"], ignore_errors=True)
log(f"done in {result['seconds']} s")
