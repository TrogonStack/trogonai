#!/usr/bin/env python3
"""Direct measurement of `shallow_array` (S), the extra native stack cost of ONE lodash frame (Ruling 49 rule 1).

Discovery only (R43-6): never a golden, a fixture or a model row. The frame is baseMergeDeep (y4e, sdk.mjs bytes 869866..870290)
entering an array that holds members, where keysIn runs arrayLikeKeys (y2e 819119..819417) and isIndex (FVe 817589..817711)
has compiled its regexp reIsUint (UVe 817570..817588) fewer than twice. The USER layer (`{"u":[null]}`) merges first and gives the
first compile, so the decisive array here is the SECOND isIndex key test (the machine-code compile), idx = 1 before it; the model
charges S for idx < 2. The measurement comes from FIRST-CALL edges of purpose-built micro-cases, not from any job 55, val45,
cells45 or report43 row.

usage: python3 -I diagsa.py LIB NODE PACKAGE OUT.json WORKTREE
       python3 -I diagsa.py --plan [HOST]        (arithmetic only: prints the per-host design, runs nothing)

WORKTREE is the checkout of the commit job 55 ran (71e5304c). Like diag55a this driver execs that tree's own r3.py (the text
before its R3_LIST branch) with `__file__` set to the tree's path, and takes the case text and the patched `prepare` and `run`
from its round 8 port (`mg8_port`). No frozen unit or build text is run, so no frozen text needs a thread pool here. The job runs
with R3_CALL=first, as the micro-cases are first-call cases (no prime, P0), with the reference flags and the toplevel caller
context (frozen offset 0).

Cases (the family shapes of the round 8 recorder: `["drop_a", objects, arrays, inner]` is `objects` nested objects, then
`arrays` nested arrays holding `inner` more objects around `null`):
  M_arr  [USER, ["drop_a", n, 1, 0], LATER]   machine_after_held_array: n nested objects, then an array holding [null]. The
         sources pass folder merge decides: the array is at depth d = n + 1 and costs level * d + S. The observable is
         managed_source (set = DS, null = dropped).
  M_ctl  [USER, ["drop_a", n, 0, 0], LATER]   drop_merge: the same folder merge with no array, n nested objects around null.
         It costs level * n + cold_assign (416), and no S.

Model arithmetic (node_merge_room.rs `fits`, node_stack.rs `available`): a frame passes when depth * level + extra <= room,
room = budget(sources folder merge) + caller offset, minus the command line and environment bytes on macOS only (the Linux
profiles are StartupBytes::BesideStack). Every run is one point ROOM = knob - depth * level where `knob` is what the sweep
moves, and a point is DS exactly when A + ROOM + offset >= extra (A the sources folder merge budget):
  darwin      knob = -startup bytes (the `startup` argument of `run(spec, flags, startup)`, 16-byte steps), n fixed per case.
  linux       the model says startup takes nothing, so the startup sweep is flat there (this driver checks that, leg `flat`).
              The byte knob there is V8's own stack limit: `--stack-size=KB` moves the room by exactly 1024 bytes per KB, and
              with n it reaches every multiple of gcd(1024, level) (256 on arm64, 32 on x64). Each point is an (n, KB) pair.
Two adoption legs, each with its own control twin, at two different budget phases (needed because on linux-arm64 the sources
pass lattice has phase 160 mod 256, so no sources bracket can lie inside the interval [1105,1184]):
  sources  arr / ctl     the sources folder merge (A); control extra 416.
  machine  arrm / ctlm   the same array at a size whose sources pass drops the document, so the machine policy folder merge (AM)
                         meets the array with idx = 1 and S is decisive again; the observable is machine_policy didNotLoad false.
                         The control (drop_merge text, extra 0, warm assign) decides on levels only: n * level <= AM.
Each leg gives a bracket (lo, hi] for S. The result is the INTERSECTION of the valid legs. Adoption rule (registered): S = hi of
the intersection only when it is contained in the interval; disjoint = outside (STOP); partial overlap = unresolved (STOP).
`--plan` prints the per-host lattice, the run counts and the possible final cells without running anything.
The control twin validates the budget arithmetic (its predicted flip has no S in it). S is then
    S = A + ROOM*_arr                        (A from the frozen profile; ROOM* the DS/dropped bracket of the sweep)
    S - 416 = ROOM*_arr - ROOM*_ctl          (twin difference form: A cancels, valid because both frames are the same pass)
On darwin ROOM*_arr = -B*_arr - d*level with B*_arr the largest startup with DS, which is
    S = available(B*_arr) - d * level,  available(B) = 958147 - B.

Every run is one process of its own with the case alone in it. One run per point, never repeated (mg37 rule): a run that gives no
answer is recorded as it is. A time budget (DIAG_BUDGET_SECONDS) ends the work; the legs not reached are listed and the run
ends cleanly. diagsa.json holds every run, the flips, S, the control check, the monotonicity flags and `stops`.
"""

import concurrent.futures
import hashlib
import json
import math
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

# ---- the design: pure arithmetic, no run needed ----
S_LO, S_HI = 500, 2100          # the S range every sweep must cover
S_MODEL = 1339                  # shallow_array of the model (all three profiles)
COLD_ASSIGN = 416               # the sources control's decisive extra: the first assign of the process (cold_assign, all profiles)
DEFAULT_STACK_KB = 984          # V8's default --stack-size on 64-bit; read from `node --v8-options` at run time when listed
STEP = 16                       # startup byte step (darwin)

# A = policy.sources.folder_merge, AM = policy.machine_policy.folder_merge, L = level (node_stack.rs). n_arr / n_ctl are the case
# sizes of the sources legs and n_marr / n_mctl those of the machine legs; the design fixes them (the sweep moves the room by
# the knob, and on Linux by n as well, so they only place the reference point of the lattice).
HOSTS = {
    "darwin arm64": {"label": "darwin-arm64", "L": 768, "A": 958147, "AM": 960016, "share": "stack",
                     "n_arr": 1238, "n_ctl": 1241, "n_marr": 1240, "n_mctl": 1244,
                     # STOP-REPORT 56 interval for S (frozen offsets): S in (1251, 1376].
                     "pred": [1252, 1376], "pred_toplevel": [1009, 1443]},
    "linux aarch64": {"label": "linux-arm64", "L": 768, "A": 959904, "AM": 961872, "share": "beside",
                      "n_arr": 1247, "n_ctl": 1249, "n_marr": 1250, "n_mctl": 1252,
                      "pred": [1105, 1184], "pred_toplevel": [1105, 1440]},
    "linux x86_64": {"label": "linux-x64", "L": 736, "A": 960320, "AM": 962304, "share": "beside",
                     "n_arr": 1302, "n_ctl": 1304, "n_marr": 1306, "n_mctl": 1307,
                     "pred": [1105, 1312], "pred_toplevel": [1089, 1312]},
}
# The legs. `pass` names the budget a leg reads (src = sources folder merge, mach = machine policy folder merge), `shift` how
# many levels the decisive frame sits below the last object of the case (the array of M_arr is one level below it), `extra`
# what the control's decisive frame spends on top of its levels (the cold assign of the sources pass; none in the machine
# pass, where the assign is warm and the levels decide, as the `machine_merge` profile levels 1244, 1252 and 1307 record),
# `arrays` the arrays of the case text, `observable` what tells the pass fits: managed_source set (src), or the machine
# policy loaded (mach: `didNotLoad` false) in a run whose sources pass dropped the document.
KINDS = {
    "arr": {"pass": "src", "shift": 1, "arrays": 1, "n": "n_arr", "extra": None},
    "ctl": {"pass": "src", "shift": 0, "arrays": 0, "n": "n_ctl", "extra": COLD_ASSIGN},
    "arrm": {"pass": "mach", "shift": 1, "arrays": 1, "n": "n_marr", "extra": None},
    "ctlm": {"pass": "mach", "shift": 0, "arrays": 0, "n": "n_mctl", "extra": 0},
}
MACHINE_KINDS = ("arrm", "ctlm")
# Caller context offsets (shallow_array.md): frozen aarch64 timer -256 and darwin timer -192; every other value is nominal and
# only places a window, since the secondary sweeps are discovery and measure the offset themselves.
CONTEXT_OFFSET = {
    "timer": {"darwin arm64": -192, "linux aarch64": -256, "linux x86_64": -192},
    "immediate": {"darwin arm64": -432, "linux aarch64": -432, "linux x86_64": -432},
}
CONTROL_HALF = {"darwin arm64": 384, "linux aarch64": 768, "linux x86_64": 256}   # control window: v - extra in [-h, +h]
LATTICE_J, LATTICE_K = 48, 48   # the (n, KB) search around the reference case size and the default stack size


def grid(host):
    h = HOSTS[host]
    return STEP if h["share"] == "stack" else math.gcd(1024, h["L"])


def ceil_to(value, step):
    return -((-value) // step) * step


def floor_to(value, step):
    return (value // step) * step


def budget_of(host, kind):
    return HOSTS[host]["A" if KINDS[kind]["pass"] == "src" else "AM"]


def points(host, kind, vlo, vhi, ss0=DEFAULT_STACK_KB):
    """The sweep points of `kind` (arr, ctl, arrm or ctlm) whose v = A_pass + ROOM lies in [vlo, vhi], in ascending ROOM (the
    order of the linear sweep: dropped first, fits after). ROOM = knob - depth * level, depth = n + shift."""
    h = HOSTS[host]
    L, A = h["L"], budget_of(host, kind)
    shift = KINDS[kind]["shift"]
    n_ref = h[KINDS[kind]["n"]]
    found = []
    if h["share"] == "stack":
        depth = n_ref + shift
        low, high = ceil_to(A - depth * L - vhi, STEP), floor_to(A - depth * L - vlo, STEP)
        for startup in range(max(low, 2048), high + 1, STEP):
            room = -startup - depth * L
            found.append({"n": n_ref, "depth": depth, "startup": startup, "stack_kb": None, "room": room, "v": A + room})
    else:
        best = {}
        for j in range(-LATTICE_J, LATTICE_J + 1):
            for k in range(-LATTICE_K, LATTICE_K + 1):
                n = n_ref + j
                room = k * 1024 - (n + shift) * L
                v = A + room
                if vlo <= v <= vhi:
                    cost = (2 * abs(k) + abs(j), abs(k), abs(j))
                    if v not in best or cost < best[v][0]:
                        best[v] = (cost, {"n": n, "depth": n + shift, "startup": None, "stack_kb": ss0 + k, "room": room, "v": v})
        found = [entry for _, entry in best.values()]
    return sorted(found, key=lambda one: one["room"])


def same_room_pairs(host, kind, room, ss0=DEFAULT_STACK_KB):
    """Every (n, KB) pair of a Linux host that gives `room`, cheapest first (the invariance check of the lattice)."""
    h = HOSTS[host]
    shift = KINDS[kind]["shift"]
    n_ref = h[KINDS[kind]["n"]]
    A = budget_of(host, kind)
    out = []
    for j in range(-LATTICE_J, LATTICE_J + 1):
        for k in range(-LATTICE_K, LATTICE_K + 1):
            n = n_ref + j
            if k * 1024 - (n + shift) * h["L"] == room:
                out.append((2 * abs(k) + abs(j), {"n": n, "depth": n + shift, "startup": None, "stack_kb": ss0 + k,
                                                  "room": room, "v": A + room}))
    return [entry for _, entry in sorted(out, key=lambda one: one[0])]


def top_window():
    return S_LO, S_HI


def context_window(host, context, s_mid):
    """v window of an M_arr sweep in a context whose offset is nominal: v = S - offset at the flip."""
    pad = max(192, 2 * grid(host))
    center = s_mid - CONTEXT_OFFSET[context][host]
    return center - pad, center + pad


def control_window(host, kind="ctl"):
    half = CONTROL_HALF[host]
    extra = KINDS[kind]["extra"]
    return extra - half, extra + half


def leg_points(host, kind):
    """The points of the adoption sweep of `kind`: arr and arrm cover S in [S_LO, S_HI], the controls the window around the
    predicted flip."""
    g = grid(host)
    if kind in ("arr", "arrm"):
        return points(host, kind, S_LO - g, S_HI + g)
    return points(host, kind, *control_window(host, kind))


def classify_bracket(bracket, interval):
    """The adoption rule applied to a bracket (lo, hi] of S against the interval [a, b]: `adopt` (S = hi) only when the
    bracket lies inside the interval; `outside` when they are disjoint (STOP); `unresolved` on a partial overlap (STOP: picking
    inside the overlap would mix the measurement with the row bounds)."""
    lo, hi = bracket
    a, b = interval
    if hi < a or lo + 1 > b:
        return "outside"
    if lo + 1 >= a and hi <= b:
        return "adopt"
    return "unresolved"


def atoms(host):
    """The brackets the intersection of the sources leg and the machine leg can end in: the cells between consecutive edges
    of the union of the two legs' lattices, each classified against the host's interval, and how many S of the interval fall
    in a cell that does not adopt."""
    edges = sorted({one["v"] for kind in ("arr", "arrm") for one in leg_points(host, kind)})
    a, b = HOSTS[host]["pred"]
    cells = []
    for lo, hi in zip(edges, edges[1:]):
        if hi < a - 1 or lo > b:
            continue
        cells.append({"bracket": [lo, hi], "width": hi - lo, "rule": classify_bracket((lo, hi), (a, b))})
    covered = {"adopt": 0, "unresolved": 0}
    for cell in cells:
        lo, hi = cell["bracket"]
        count = max(0, min(hi, b) - max(lo + 1, a) + 1)
        if cell["rule"] in covered:
            covered[cell["rule"]] += count
    total = b - a + 1
    return {"cells": cells, "S_values_in_interval": total, "adopt_values": covered["adopt"], "unresolved_values": covered["unresolved"],
            "containment_achievable": covered["adopt"] > 0, "single_leg_cell_width": grid(host) if HOSTS[host]["share"] == "beside" else STEP,
            "src_phase": HOSTS[host]["A"] % grid(host), "mach_phase": HOSTS[host]["AM"] % grid(host)}


def plan(host, s_mid=S_MODEL):
    h = HOSTS[host]
    g = grid(host)
    legs = {kind: leg_points(host, kind) for kind in KINDS}
    out = {"host": host, "label": h["label"], "level": h["L"], "A_sources_folder_merge": h["A"], "A_machine_folder_merge": h["AM"],
           "knob": "startup" if h["share"] == "stack" else "stack-size and n", "grid_bytes": g,
           "cases": {kind: {"n_ref": h[KINDS[kind]["n"]], "depth_ref": h[KINDS[kind]["n"]] + KINDS[kind]["shift"],
                            "points": len(legs[kind])} for kind in KINDS},
           "arr_window_v": [S_LO - g, S_HI + g], "ctl_window_extra_pm": CONTROL_HALF[host], "sweeps": {}}
    total = sum(len(points_) for points_ in legs.values())
    for context in ("timer", "immediate"):
        window = context_window(host, context, s_mid)
        count = len(points(host, "arr", *window))
        out["sweeps"][context] = {"nominal_offset": CONTEXT_OFFSET[context][host], "window_v": list(window), "points": count}
        total += count
    if h["share"] == "stack":
        for kind in KINDS:
            out["cases"][kind]["startup_window"] = [legs[kind][-1]["startup"], legs[kind][0]["startup"], STEP]
        out["predicted_flips"] = {
            "ctl": h["A"] - h["n_ctl"] * h["L"] - COLD_ASSIGN,
            "ctlm": h["AM"] - h["n_mctl"] * h["L"],
            "arr_at_model_S": h["A"] - (h["n_arr"] + 1) * h["L"] - S_MODEL,
            "arrm_at_model_S": h["AM"] - (h["n_marr"] + 1) * h["L"] - S_MODEL,
            "arr_range": [h["A"] - (h["n_arr"] + 1) * h["L"] - h["pred"][1], h["A"] - (h["n_arr"] + 1) * h["L"] - h["pred"][0]],
            "arrm_range": [h["AM"] - (h["n_marr"] + 1) * h["L"] - h["pred"][1], h["AM"] - (h["n_marr"] + 1) * h["L"] - h["pred"][0]]}
    else:
        for kind in KINDS:
            out["cases"][kind]["pairs"] = [[one["n"], one["stack_kb"], one["v"] - (KINDS[kind]["extra"] or 0)] for one in legs[kind]]
        out["predicted_flips"] = "controls: fits iff v >= extra (ctl 416, ctlm 0); arr legs: fits iff S <= v"
        out["max_stack_shift_kb"] = max(abs(one["stack_kb"] - DEFAULT_STACK_KB) for points_ in legs.values() for one in points_)
        total += 8 + 4 + 2        # flat check, invariance, trace leg
    out["resolution"] = atoms(host)
    out["total_runs_max"] = total
    return out


def check_design():
    """The window of each host must cover S in [S_LO, S_HI] and put the model's S inside it, for both adoption legs, and the
    controls' windows must hold their predicted flips."""
    for host in HOSTS:
        for kind in ("arr", "arrm"):
            vs = [one["v"] for one in leg_points(host, kind)]
            assert min(vs) <= S_LO and max(vs) >= S_HI and any(abs(v - S_MODEL) <= grid(host) for v in vs), (host, kind)
            step = STEP if HOSTS[host]["share"] == "stack" else grid(host)
            assert all(b - a <= step for a, b in zip(vs, vs[1:])), (host, kind)
        for kind in ("ctl", "ctlm"):
            vs = [one["v"] - KINDS[kind]["extra"] for one in leg_points(host, kind)]
            assert min(vs) < 0 <= max(vs), (host, kind, min(vs), max(vs))


if len(sys.argv) > 1 and sys.argv[1] == "--plan":
    # Arithmetic only: nothing below this branch (the exec of r3.py, the spawns) runs.
    check_design()
    wanted = sys.argv[2:] or list(HOSTS)
    print(json.dumps([plan(host) for host in wanted], indent=1))
    sys.exit(0)

LIB, NODE, PACKAGE, OUT, TREE = sys.argv[1:6]
BUDGET_SECONDS = int(os.environ.get("DIAG_BUDGET_SECONDS", str(120 * 60)))
STARTED = time.time()
ARTIFACTS = os.path.join(os.path.dirname(os.path.abspath(OUT)), "diagsa-traces")
os.makedirs(ARTIFACTS, exist_ok=True)
TRACE_FLAG = "--trace-regexp-tier-up"

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
REF = list(G["REF"])

SPAWNED = []
LAST = {}
MODE = {"trace": False}
_spawn = D["spawn"]


def counted_spawn(command, env, *args, **kwargs):
    """`discover.spawn`, noting what the command line and environment of each oracle run took (spawn itself refuses a run
    whose bytes differ from the budget its PAD was sized for) and keeping the raw output of the run. In a trace run the V8
    trace lines share the standard output with the oracle's answer: the answer is cut out of the stream and handed on, so the
    recorder reads it as it always does."""
    SPAWNED.append({"argv_entries": len(command), "argv_bytes": sum(D["entry_bytes"](arg) for arg in command),
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


def piece(value):
    """A long answer member as its length, digest and head."""
    if value is None:
        return None
    text = value if isinstance(value, str) else json.dumps(value)
    return {"len": len(text), "sha": hashlib.sha256(text.encode("utf-8", "replace")).hexdigest()[:16], "head": text[:160]}


def summarize(answered):
    managed = answered.get("managed_source")
    text = managed if isinstance(managed, str) else json.dumps(managed)
    policy = answered.get("machine_policy")
    policy_text = re.sub(r"\s", "", policy if isinstance(policy, str) else json.dumps(policy))
    return {"effective": piece(answered.get("effective")), "exact": piece(answered.get("exact")),
            "machine_policy": piece(answered.get("machine_policy")),
            "managed_source": G["mg8_collapse"](managed)[:2000] if managed is not None else None,
            "managed_source_sha": hashlib.sha256(text.encode("utf-8", "replace")).hexdigest()[:16],
            "managed_null": managed in (None, "null"),
            "mp_ok": '"didNotLoad":false' in policy_text,
            "effective_set": answered.get("effective") not in (None, "null"),
            "answer_sha": digest(answered)}


def origin_of(stdout):
    """`policyOrigin` of the managed source the oracle printed, else the member of the result itself, else None."""
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
    """`set` = the pass of the leg fits, `null` = it does not. The sources legs read managed_source. The machine legs read the
    machine policy (`didNotLoad` false) in a run whose sources pass dropped the document: where the sources pass loaded it,
    the run says nothing of the machine pass (class `invalid`)."""
    if "answer" not in record:
        return "fail"
    if record.get("error"):
        return "error"
    answer = record["answer"]
    if record["kind"] in MACHINE_KINDS:
        if not answer["managed_null"]:
            return "invalid"
        return "set" if answer["mp_ok"] else "null"
    return "null" if answer["managed_null"] else "set"


result = {"host": HOST, "call": GATE["CALL"], "context": GATE["CONTEXT"],
          "node": subprocess.run([NODE, "--version"], capture_output=True, check=True).stdout.decode().strip(),
          "tree": "71e5304c", "budget_seconds": BUDGET_SECONDS, "flags": {"REF": REF, "trace": [TRACE_FLAG]},
          "default_stack_kb": None, "design": None, "runs": [], "sweeps": {}, "flat": None, "invariance": None,
          "trace_leg": None, "results": {}, "not_reached": [], "stops": [], "summary": {}}


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


def stop(text):
    if len(result["stops"]) < 40:
        result["stops"].append(text[:600])


if HOST not in HOSTS:
    stop(f"no design for host {HOST}")
    finish(1)
H = HOSTS[HOST]
LINUX = H["share"] == "beside"

try:
    port = G["mg8_port"]()
except (Exception, SystemExit) as error:
    stop(f"mg8_port: {error!r}")
    finish(1)
USER, LATER = port["USER"], port["LATER"]
STARTUP = port["STARTUP_BYTES"]

# V8's default stack size for this Node, as it lists it (the design assumes 984 KB; the control sweep checks the arithmetic).
try:
    listing = subprocess.run([NODE, "--v8-options"], capture_output=True, check=True, env={}).stdout.decode("utf-8", "replace")
    found = re.search(r"^\s+--stack-size[^\n]*\n[^\n]*default:\s*(\d+)", listing, re.M)
    result["default_stack_kb"] = int(found.group(1)) if found else None
    result["trace_flag_listed"] = bool(re.search(r"^\s+" + re.escape(TRACE_FLAG) + r"(\s|$)", listing, re.M))
except (Exception, SystemExit) as error:
    result["default_stack_kb"] = None
    result["trace_flag_listed"] = False
    stop(f"v8-options: {error!r}")
SS0 = result["default_stack_kb"] or DEFAULT_STACK_KB
if result["default_stack_kb"] not in (None, DEFAULT_STACK_KB):
    stop(f"default stack size {result['default_stack_kb']} KB, the design assumed {DEFAULT_STACK_KB}; the lattice is built on the listed one")
result["design"] = plan(HOST)
save()


def case_of(kind, n, startup, context):
    """The first-call case of `n` levels: M_arr (arr, arrm: one array below the last object) or M_ctl (ctl, ctlm: none)."""
    objects, arrays = n, KINDS[kind]["arrays"]
    name = f"sa_{kind}_{context}_{n}_{startup}"
    return (name, [USER, ["drop_a", objects, arrays, 0], LATER])


def run_once(kind, point, context, leg, flags=None, trace=False, startup=None, note=None):
    """One case in a process of its own: the recorded answer fields, the flags and the spawn byte record. One run per
    point, never repeated (mg37 rule): a run that gives no answer is recorded as it is."""
    folder = tempfile.mkdtemp()
    started = time.time()
    use_startup = startup if startup is not None else (point["startup"] if point["startup"] is not None else STARTUP)
    node_flags = list(flags if flags is not None else REF)
    if point["stack_kb"] is not None:
        node_flags.append(f"--stack-size={point['stack_kb']}")
    case = case_of(kind, point["n"], use_startup if LINUX is False else point["stack_kb"], context)
    record = {"leg": leg, "kind": kind, "context": context, "case": case[0], "n": point["n"], "depth": point["depth"],
              "startup": use_startup, "stack_kb": point["stack_kb"], "room": point["room"], "v": point["v"],
              "flags": node_flags, "note": note}
    mark = len(SPAWNED)
    LAST.clear()
    MODE["trace"] = trace
    GATE["in_context"](context)
    try:
        spec_path = port["prepare"](case[0], case[1], folder)
        answered = port["run"](spec_path, node_flags, use_startup)
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
        MODE["trace"] = False
        GATE["in_context"](None)
        shutil.rmtree(folder, ignore_errors=True)
    record["account"] = SPAWNED[mark] if len(SPAWNED) > mark else None
    record["seconds"] = round(time.time() - started, 1)
    record["class"] = klass(record)
    if record["class"] in ("fail", "error"):
        stop(f"{leg} {case[0]}: {record['class']} {record.get('failed') or record.get('gate') or record.get('runtime') or record.get('error')}")
    result["runs"].append(record)
    result["summary"].setdefault(leg, []).append(record["class"])
    save()
    return record


def analyze(runs):
    """The bracket of a sweep: runs sorted by ascending ROOM, DS = managed_source set. A point is DS when A + ROOM + offset >=
    extra, so the classes must read dropped...dropped, DS...DS; a dropped point above a DS one is a monotonicity violation,
    recorded as a finding and never retried."""
    usable = sorted((one for one in runs if one["class"] in ("set", "null")), key=lambda one: one["room"])
    classes = ["ds" if one["class"] == "set" else "drop" for one in usable]
    violations, seen_ds = [], False
    for one, c in zip(usable, classes):
        if c == "ds":
            seen_ds = True
        elif seen_ds:
            violations.append({"room": one["room"], "case": one["case"], "n": one["n"], "startup": one["startup"],
                               "stack_kb": one["stack_kb"]})
    out = {"points": len(runs), "usable": len(usable), "unusable": len(runs) - len(usable), "classes": "".join(
        "D" if c == "ds" else "." for c in classes), "monotone": not violations, "violations": violations,
        "rooms": [one["room"] for one in usable],
        "answer_changes": [[usable[i - 1]["room"], usable[i]["room"]] for i in range(1, len(usable))
                           if usable[i]["answer"]["answer_sha"] != usable[i - 1]["answer"]["answer_sha"]]}
    if not usable:
        out["flip"] = "no usable run"
        return out
    first = next((i for i, c in enumerate(classes) if c == "ds"), None)
    if first is None:
        out["flip"] = "none: every point dropped (flip is above the window)"
    elif first == 0:
        out["flip"] = "none: every point DS (flip is below the window)"
    else:
        low, high = usable[first - 1], usable[first]
        out.update({"flip": "bracket", "room_drop": low["room"], "room_ds": high["room"], "width": high["room"] - low["room"],
                    "drop_point": {"n": low["n"], "startup": low["startup"], "stack_kb": low["stack_kb"], "v": low["v"]},
                    "ds_point": {"n": high["n"], "startup": high["startup"], "stack_kb": high["stack_kb"], "v": high["v"]}})
        if not LINUX:
            out["B_star_last_ds_startup"], out["first_drop_startup"] = high["startup"], low["startup"]
    return out


def sweep(name, kind, context, plist):
    """A linear sweep (not bisection) of `plist` in ascending ROOM, so monotonicity is checked, never retried."""
    runs = []
    result["sweeps"][name] = {"kind": kind, "context": context, "planned": len(plist), "analysis": None}
    for point in plist:
        if over():
            result["not_reached"].append(f"{name}: {len(plist) - len(runs)} points")
            break
        runs.append(run_once(kind, point, context, name))
    result["sweeps"][name]["analysis"] = analyze(runs)
    result["sweeps"][name]["runs"] = [one["case"] + "|" + str(one["startup"]) + "|" + str(one["stack_kb"]) for one in runs]
    save()
    a = result["sweeps"][name]["analysis"]
    log(f"{name}: {a['classes']} flip={a['flip']} monotone={a['monotone']}")
    return a


def s_from(analysis, kind):
    """S bracket from the pass budget the leg reads, for an arr or arrm sweep: fits iff S <= A_pass + ROOM (offset 0)."""
    if analysis.get("flip") != "bracket":
        return None
    a_pass = budget_of(HOST, kind)
    return {"S_greater_than": a_pass + analysis["room_drop"], "S_at_most": a_pass + analysis["room_ds"],
            "width": analysis["width"]}


def inside(bracket, interval):
    """How the S bracket (lo, hi] sits against the interval [a, b] of the STOP-REPORT 56 prediction."""
    if bracket is None:
        return None
    lo, hi = bracket["S_greater_than"], bracket["S_at_most"]
    a, b = interval
    return {"intersects": hi >= a and lo < b, "contained": lo + 1 >= a and hi <= b}


# ---- the legs ----
try:
    s_lo, s_hi = top_window()
    g = grid(HOST)
    # 1. The sources leg: M_arr (arr) and its control twin (ctl), toplevel, REF flags, P0.
    top = sweep("arr_toplevel", "arr", "toplevel", points(HOST, "arr", s_lo - g, s_hi + g, SS0))
    ctl = sweep("ctl_toplevel", "ctl", "toplevel", points(HOST, "ctl", *control_window(HOST, "ctl"), SS0))
    # 2. The machine leg: the same array case at a size whose sources pass drops the document, so the machine pass (budget
    #    phase 80 mod 256 on aarch64) meets the array with the same isIndex count as the sources pass did, and its control twin.
    topm = sweep("arrm_toplevel", "arrm", "toplevel", points(HOST, "arrm", s_lo - g, s_hi + g, SS0))
    ctlm = sweep("ctlm_toplevel", "ctlm", "toplevel", points(HOST, "ctlm", *control_window(HOST, "ctlm"), SS0))

    def control_of(kind, analysis):
        """The control twin against its predicted flip: fits iff A_pass + ROOM >= extra."""
        extra = KINDS[kind]["extra"]
        out = {"kind": kind, "extra": extra, "analysis_flip": analysis.get("flip"), "monotone": analysis["monotone"],
               "predicted_room": extra - budget_of(HOST, kind), "tolerance_bytes": 16}
        if analysis.get("flip") == "bracket":
            pred = out["predicted_room"]
            out["bracket_room"] = [analysis["room_drop"], analysis["room_ds"]]
            out["resolution"] = analysis["width"]
            out["passes"] = bool(analysis["monotone"] and analysis["room_drop"] - 16 < pred <= analysis["room_ds"] + 16)
            out["off_by"] = 0 if analysis["room_drop"] < pred <= analysis["room_ds"] else min(
                abs(pred - analysis["room_drop"]), abs(pred - analysis["room_ds"]))
            if not LINUX:
                out["B_star_observed"] = analysis["B_star_last_ds_startup"]
                out["B_star_predicted"] = budget_of(HOST, kind) - H[KINDS[kind]["n"]] * H["L"] - extra
        else:
            out["passes"] = False
            out["off_by"] = None
        return out

    def leg_of(name, kind, arr_analysis, ctl_kind, ctl_analysis):
        """One adoption leg: the S bracket of its arr sweep, its control check and its twin difference. The leg is valid
        only when its sweep is monotone with a bracket and its control passes."""
        bracket = s_from(arr_analysis, kind)
        control = control_of(ctl_kind, ctl_analysis)
        leg = {"pass": KINDS[kind]["pass"], "budget": budget_of(HOST, kind), "kind": kind, "depth_ref": H[KINDS[kind]["n"]] + 1,
               "flip": arr_analysis["flip"], "monotone": arr_analysis["monotone"], "bracket": bracket, "control": control,
               "valid": bool(bracket and arr_analysis["monotone"] and control["passes"]),
               "vs_interval": classify_bracket((bracket["S_greater_than"], bracket["S_at_most"]), H["pred"]) if bracket else None}
        if not leg["valid"]:
            leg["invalid_because"] = [text for text, bad in (
                ("no bracket", bracket is None), ("sweep not monotone", not arr_analysis["monotone"]),
                ("control failed", not control["passes"])) if bad]
        if not LINUX and bracket:
            leg["B_star_startup"] = arr_analysis["B_star_last_ds_startup"]
            leg["available_at_B_star"] = budget_of(HOST, kind) - arr_analysis["B_star_last_ds_startup"]
        if bracket and ctl_analysis.get("flip") == "bracket":
            extra = KINDS[ctl_kind]["extra"]
            leg["twin_difference"] = {
                "S_greater_than": extra + arr_analysis["room_drop"] - ctl_analysis["room_ds"],
                "S_at_most": extra + arr_analysis["room_ds"] - ctl_analysis["room_drop"],
                "note": "A cancels; valid when the control passes (same pass, the array frame against the control's frame)"}
        return leg

    legs = {"sources": leg_of("sources", "arr", top, "ctl", ctl), "machine": leg_of("machine", "arrm", topm, "ctlm", ctlm)}
    used = [name for name, leg in legs.items() if leg["valid"]]
    if used:
        lo = max(legs[name]["bracket"]["S_greater_than"] for name in used)
        hi = min(legs[name]["bracket"]["S_at_most"] for name in used)
    else:
        lo = hi = None
    if not used:
        decision, adopted = "no_valid_leg", None
    elif lo >= hi:
        decision, adopted = "inconsistent", None
    else:
        decision = classify_bracket((lo, hi), H["pred"])
        adopted = hi if decision == "adopt" else None
    result["results"]["control"] = {"sources": legs["sources"]["control"], "machine": legs["machine"]["control"]}
    result["results"]["S"] = {
        "formula": "S in (A_pass + ROOM_drop, A_pass + ROOM_ds] per leg (ROOM = knob - depth * level; sources pass A = sources folder"
                   " merge, machine pass A = machine policy folder merge); darwin: available(B*) - depth * level",
        "A": H["A"], "A_machine": H["AM"], "level": H["L"], "reads_context": "toplevel", "model_S": S_MODEL,
        "legs": legs, "legs_used": used,
        "intersection": {"S_greater_than": lo, "S_at_most": hi, "width": None if lo is None else hi - lo},
        "adopt": {"rule": "adopt S = hi of the intersection of the valid legs' brackets only when contained in the interval; "
                          "disjoint = outside (STOP); partial overlap = unresolved (STOP); empty intersection = inconsistent (STOP)",
                  "interval": H["pred"], "decision": decision, "S": adopted,
                  "per_leg": {name: leg["vs_interval"] for name, leg in legs.items()}},
        "toplevel_interval": H["pred_toplevel"], "resolution_design": atoms(HOST)}
    s_model = s_from(top, "arr")
    s_mid = (s_model["S_greater_than"] + s_model["S_at_most"]) / 2 if s_model else S_MODEL
    save()
    log(f"adopt: {decision} S={adopted} bracket=({lo}, {hi}] legs={used}")

    # 3. Secondary (discovery only, not used for adoption): the same M_arr sweep in the timer and immediate contexts.
    contexts = {}
    for context in ("timer", "immediate"):
        window = context_window(HOST, context, s_mid)
        found = sweep(f"arr_{context}", "arr", context, points(HOST, "arr", *window, SS0))
        entry = {"nominal_offset": CONTEXT_OFFSET[context][HOST], "window_v": list(window), "analysis_flip": found.get("flip"),
                 "monotone": found["monotone"]}
        if found.get("flip") == "bracket":
            # v = A + ROOM at the flip is S - offset, so offset = S_toplevel - v_context.
            entry["S_minus_offset_bracket"] = [H["A"] + found["room_drop"], H["A"] + found["room_ds"]]
            if s_model:
                entry["offset_bracket"] = [s_model["S_greater_than"] - (H["A"] + found["room_ds"]),
                                           s_model["S_at_most"] - (H["A"] + found["room_drop"])]
        contexts[context] = entry
    result["results"]["contexts"] = contexts
    save()

    # 4. On Linux: the startup sweep is expected to be flat (the model: BesideStack). One run each at the points that bracket
    #    the toplevel flip, with the command line and environment far from the budget's 4384 bytes.
    if LINUX:
        flat = {"expected": "no answer changes with startup bytes (StartupBytes::BesideStack)", "runs": []}
        result["flat"] = flat
        if top.get("flip") == "bracket":
            ds_point = {"n": top["ds_point"]["n"], "depth": top["ds_point"]["n"] + 1, "startup": None, "stack_kb": top["ds_point"]["stack_kb"],
                        "room": top["room_ds"], "v": top["ds_point"]["v"]}
            drop_point = {"n": top["drop_point"]["n"], "depth": top["drop_point"]["n"] + 1, "startup": None,
                          "stack_kb": top["drop_point"]["stack_kb"], "room": top["room_drop"], "v": top["drop_point"]["v"]}
            for label, point, values in (("ds", ds_point, (STARTUP + 16, STARTUP + 256, STARTUP + 1024, STARTUP + 4096)),
                                         ("drop", drop_point, (STARTUP - 16, STARTUP - 256, STARTUP - 1024, STARTUP - 2048))):
                for value in values:
                    if over():
                        result["not_reached"].append(f"flat {label} {value}")
                        continue
                    one = run_once("arr", point, "toplevel", f"flat_{label}", startup=value, note=f"startup {value} at the {label} bracket point")
                    flat["runs"].append({"side": label, "startup": value, "class": one["class"]})
            flat["changed"] = [one for one in flat["runs"] if one["class"] != ("set" if one["side"] == "ds" else "null")]
            flat["verdict"] = "startup moves the answer: the profile's BesideStack is wrong here" if flat["changed"] else "flat"
        else:
            flat["verdict"] = "not run: no toplevel flip bracket"
        save()

        # 5. Lattice invariance: other (n, KB) pairs with the same ROOM as the bracket points must answer alike.
        invariance = {"runs": []}
        result["invariance"] = invariance
        if top.get("flip") == "bracket":
            for label, room, want in (("ds", top["room_ds"], "set"), ("drop", top["room_drop"], "null")):
                taken = {(top[f"{label}_point"]["n"], top[f"{label}_point"]["stack_kb"])}
                for pair in [p for p in same_room_pairs(HOST, "arr", room, SS0) if (p["n"], p["stack_kb"]) not in taken][:2]:
                    if over():
                        result["not_reached"].append(f"invariance {label}")
                        continue
                    one = run_once("arr", pair, "toplevel", f"invariance_{label}", note="same ROOM as the bracket point, another (n, KB)")
                    invariance["runs"].append({"side": label, "n": pair["n"], "stack_kb": pair["stack_kb"], "class": one["class"],
                                               "agrees": one["class"] == want})
            invariance["all_agree"] = all(one["agrees"] for one in invariance["runs"]) if invariance["runs"] else None
        save()

    # 6. Optional evidence leg: M_arr at the flip with a V8 regexp trace flag. Its failure never stops the job.
    leg = {"flag": TRACE_FLAG, "listed": result.get("trace_flag_listed"), "runs": []}
    result["trace_leg"] = leg
    try:
        if not leg["listed"]:
            leg["status"] = "failed: this Node does not list the flag in --v8-options; the leg was not run"
        elif top.get("flip") != "bracket":
            leg["status"] = "not run: no toplevel flip bracket"
        else:
            for label in ("ds", "drop"):
                if over():
                    result["not_reached"].append(f"trace {label}")
                    continue
                at = top[f"{label}_point"]
                point = {"n": at["n"], "depth": at["n"] + 1, "startup": at["startup"], "stack_kb": at["stack_kb"],
                         "room": top[f"room_{label}"], "v": at["v"]}
                one = run_once("arr", point, "toplevel", f"trace_{label}", flags=[*REF, TRACE_FLAG], trace=True,
                               note="REF + the regexp tier-up trace flag at the flip")
                text = LAST.get("trace")
                entry = {"side": label, "class": one["class"], "trace_bytes": len(text) if text else None}
                if text:
                    name = f"{one['case']}.{label}.trace.txt"
                    with open(os.path.join(ARTIFACTS, name), "w", encoding="utf-8") as handle:
                        handle.write(text[:2_000_000])
                    lines = [line[:240] for line in text.split("\n") if re.search(r"regexp|tier", line, re.I)]
                    entry.update({"file": name, "regexp_lines": len(lines), "head": lines[:60]})
                leg["runs"].append(entry)
            leg["status"] = "ran"
    except (Exception, SystemExit) as error:
        leg["status"] = f"failed: {error!r}"[:300]
    save()
except (Exception, SystemExit) as error:
    stop(f"legs: {error!r}")

save()
shutil.rmtree(D["SCRATCH"], ignore_errors=True)
log(f"done in {result['seconds']} s")
