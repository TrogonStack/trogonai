#!/usr/bin/env python3
"""Direct measurement of every pass budget, every caller-context offset and `shallow_array` (S) (Ruling 50 rules 1, 2, 4).

Discovery only (R43-6): never a golden, a fixture or a model row. This driver is diagsa.py with its S legs kept as they are,
the harness correction of Ruling 50 rule 2 (every window bound is an int on the knob step, so the darwin timer, immediate and
regexp trace legs run), and the parameter legs added around them.

usage: python3 -I diagpm.py LIB NODE PACKAGE OUT.json WORKTREE
       python3 -I diagpm.py --plan [HOST]        (arithmetic only: prints the per-host design, runs nothing)

WORKTREE is the checkout of the commit job 55 ran (71e5304c). Like diagsa this driver execs that tree's own r3.py (the text
before its R3_LIST branch) with `__file__` set to the tree's path, and takes the case text and the patched `prepare` and `run`
from its round 8 port (`mg8_port`). The job runs with R3_CALL=first: every case is a first call in a fresh process (P0), with
the reference flags.

The parameter legs (the `pm` part). Every run is one point q, the cost the decisive path of the case spends in the pass under
test plus what the room knob takes from the stack:
  darwin      q = cost + startup bytes (StartupBytes::Subtract), startup on the 16-byte step in [2048, 8192].
  linux       q = cost - 1024 * k with `--stack-size` = default + k KB (StartupBytes::BesideStack); the startup bytes stay at
              4384. Each point is an (n, secondary, leaf, k) choice, the cheapest one for its q.
The pass fits exactly when q <= B, B the budget the model profile names for it (node_stack.rs). A sweep runs its points in
descending q (ascending room), one run per point, never repeated, so the classes read over...over, fit...fit; an over after a
fit is a monotonicity violation, recorded and never retried. The bracket is B in [q_fit, q_fail - 1] and the reported value is
its hi edge, q_fail - 1, the largest q that still fits.

Shapes (r3 `deep` text, `LEAVES` of the round 8 port: null_array [null], empty_array [], null_object {"x":null}, empty_object {}):
  nest(o, a, leaf)    o nested objects, then a nested arrays around the leaf. Path cost o*object + a*array + leaf.
  proto(o, a, leaf)   {"x":{"__proto__": nest(o, a, leaf)}}. Only the copy enters __proto__: (2+o)*object + a*array + leaf.
  model(o, a, leaf)   {"model": nest(o, a, leaf)}: (1+o)*object + a*array + leaf.
  levels(n)           n nested objects around null: a merge spends n*level (+ cold_assign 416 when it is the first assign).
  admin(n)            {"x":[ n nested objects around null ]} merged over HELD {"x":[null]}: (n+2)*level.
Each budget has its own family, files and observable (PM_PARAMS); guards keep every point where only the pass under test can
flip. Each budget has a control twin run in the same job: the same q values reached by another primary size n (darwin n-1 or
n+1 with the startup moved by the per-level cost; linux the cheapest other (n, k) with the same q), which must give a bracket that
intersects the primary bracket and nests with it.

Offsets: the effective_copy family (proto, effective holds x) in each caller context (nested, timer, immediate, io), over a window
around B + the model interval, with its own control twin. The offset is the A-free twin difference of the context flip and the
toplevel flip of the same shape (the effective_copy toplevel sweeps). child_exit is a per-process sample (its room moves from
one process to the next), so it gets a coarse sweep over its whole interval I_child and a fine sweep with a control around the
first coarse fit, and reports the bounds every run sets.

Adoption rule (Ruling 50 rule 1): adopt only a bracket whose control passes, whose sweeps are monotone and complete, and whose
width is strictly finer than the tolerance of the budget (the smallest margin a discovery or golden row has against the budget in the
current model, else 16 bytes). A bracket that partly overlaps the interval the rows allow is unresolved (never a value picked
from inside the overlap); one disjoint from it is adopted and flagged. Verdicts: adopt, unresolved, control-failed,
non-monotone, not-reached, unmeasurable.

The S legs (from diagsa, unchanged): the first-call M_arr / M_ctl micro-cases in the sources and machine policy folder merges,
their control twins, the timer and immediate discovery sweeps, the Linux flat and invariance checks and the regexp trace leg.

Order: the budgets the discovery rows decide on, then the offsets, then the S legs, then the remaining budgets, then
child_exit. Every leg records its failure as data and the job goes on. A time budget (DIAG_BUDGET_SECONDS, 12000 by default)
ends the work; the legs not reached are listed. diagpm.json holds every run, every sweep, `results.params`, `results.offsets`,
`results.child_exit`, `results.S` and `stops`.
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
    order of the linear sweep: dropped first, fits after). ROOM = knob - depth * level, depth = n + shift. The window bounds
    are made ints first (Ruling 50 rule 2: a float bound reached range() and raised TypeError on darwin)."""
    vlo, vhi = int(math.floor(vlo)), int(math.ceil(vhi))
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
    """v window of an M_arr sweep in a context whose offset is nominal: v = S - offset at the flip. Both bounds are ints on the
    knob step of the host (floor for the low bound, ceil for the high one), whatever `s_mid` is (Ruling 50 rule 2)."""
    g = grid(host)
    pad = max(192, 2 * g)
    center = s_mid - CONTEXT_OFFSET[context][host]
    return floor_to(int(math.floor(center - pad)), g), ceil_to(int(math.ceil(center + pad)), g)


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
        total += 2        # trace leg
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
    controls' windows must hold their predicted flips. Every context window bound is an int on the knob step."""
    for host in HOSTS:
        for kind in ("arr", "arrm"):
            vs = [one["v"] for one in leg_points(host, kind)]
            assert min(vs) <= S_LO and max(vs) >= S_HI and any(abs(v - S_MODEL) <= grid(host) for v in vs), (host, kind)
            step = STEP if HOSTS[host]["share"] == "stack" else grid(host)
            assert all(b - a <= step for a, b in zip(vs, vs[1:])), (host, kind)
        for kind in ("ctl", "ctlm"):
            vs = [one["v"] - KINDS[kind]["extra"] for one in leg_points(host, kind)]
            assert min(vs) < 0 <= max(vs), (host, kind, min(vs), max(vs))
        for context in ("timer", "immediate"):
            for s_mid in (S_MODEL, (1251 + 1376) / 2, 1313.5):
                window = context_window(host, context, s_mid)
                assert all(isinstance(bound, int) and bound % grid(host) == 0 for bound in window), (host, context, window)
                assert points(host, "arr", *window), (host, context, window)


# ---- the parameter design (Ruling 50 rule 1): pure arithmetic, no run needed ----
PM_HALF = 768                   # budget window: q in [B - 768, B + 768]
PM_MARGIN = 128                 # every guard holds by at least this many bytes
PM_TOLERANCE = 16               # the tolerance of a budget no row decides on (the startup step)
ST_DEFAULT, ST_MIN, ST_MAX = 4384, 2048, 8192   # darwin startup bytes: the profile's own, and the range the sweeps may use
PM_J, PM_K = 48, 48             # linux: primary size n_ref + j, j in +-48; --stack-size default + k KB, k in +-48
RUN_SECONDS = 0.25              # planning figure per run (the last job: about 0.2 s)
DIAG_BUDGET = 12000             # the job's DIAG_BUDGET_SECONDS
OFFSET_PAD = 256                # offset window: q in [B + I_lo - 256, B + I_hi + 256]
CHILD_COARSE, CHILD_PAD, CHILD_FINE = 256, 512, 512   # child_exit: coarse bucket, window pad, fine half width
SAVE_EVERY = 50                 # runs between two writes of the output file
LEAF_NAMES = ("null_array", "empty_array", "null_object", "empty_object")
LEAF_TEXT = {"null_array": "[null]", "empty_array": "[]", "null_object": '{"x":null}', "empty_object": "{}"}
FORM_ABOVE = {"nest": 0, "model": 1, "proto": 2}


def path_cost(obj, arr, empty_object, scalar_object, empty_array, scalar_array):
    """A PathCost of node_stack.rs: per object, per array, and the leaf container that ends the path."""
    return {"object": obj, "array": arr, "leaves": {"empty_object": empty_object, "null_object": scalar_object,
                                                     "empty_array": empty_array, "null_array": scalar_array}}


# node_stack.rs NODE_24_DARWIN_ARM64, NODE_24_LINUX_ARM64, NODE_24_LINUX_X64. warm_assign (x64 448) decides only where lodash
# assigns past the stack (node_merge_cost.rs), so no shape here spends it.
PM_PROFILE = {
    "darwin arm64": {"walk": path_cost(448, 240, 144, 448, 176, 240), "clone": path_cost(288, 304, 328, 328, 304, 304),
                     "copy": path_cost(288, 512, 512, 512, 512, 512), "L": 768, "cold": 416, "stack": True},
    "linux aarch64": {"walk": path_cost(512, 272, 144, 512, 192, 272), "clone": path_cost(320, 320, 320, 320, 320, 320),
                      "copy": path_cost(512, 560, 144, 512, 192, 560), "L": 768, "cold": 416, "stack": False},
    "linux x86_64": {"walk": path_cost(608, 272, 144, 608, 192, 272), "clone": path_cost(304, 304, 304, 304, 304, 304),
                     "copy": path_cost(608, 640, 144, 608, 176, 640), "L": 736, "cold": 416, "stack": False},
}


def budgets(top, merge, sources, effective, origin, machine, admin, held_fs, held_tool, registry_fs, registry_tool, handed):
    out = {"toplevel.walk": top[0], "toplevel.clone": top[1], "toplevel.copy": top[2], "merge_budget": merge,
           "effective_copy": effective, "admin_merge": admin}
    for name, values in (("sources", sources), ("origin", origin), ("machine_policy", machine)):
        out.update({f"{name}.walk": values[0], f"{name}.clone": values[1], f"{name}.copy": values[2],
                    f"{name}.folder_merge": values[3]})
    for name, values in (("held_check.fs", held_fs), ("held_check.tool", held_tool)):
        out.update({f"{name}.walk": values[0], f"{name}.clone": values[1]})
    for name, values in (("registry.fs", registry_fs), ("registry.tool", registry_tool), ("handed_over", handed)):
        out.update({f"{name}.walk": values[0], f"{name}.clone": values[1], f"{name}.copy": values[2]})
    return out


# Every budget of the profile, flattened (top-level reader walk/clone/copy, merge_budget and every field under `policy`).
PM_BUDGETS = {
    "darwin arm64": budgets((1000451, 1000724, 1000884), 960611, (997859, 998160, 998826, 958147), 998394,
                            (998944, 999248, 999914, 959248), (999840, 1000176, 1000794, 960016), 960371,
                            (967034, 967306), (966927, 967135), (966016, 966368, 966368), (966016, 966368, 966368),
                            (999002, 999274, 999130)),
    "linux aarch64": budgets((1002224, 1002392, 1002208), 962512, (999680, 999936, 1000080, 959904), 999648,
                             (1000704, 1001120, 1001168, 961152), (1001712, 1002080, 1002048, 961872), 962064,
                             (980928, 981120), (980224, 980464), (980224, 980320, 980320), (980224, 980320, 980320),
                             (1000736, 1000976, 1000384)),
    "linux x86_64": budgets((1002688, 1002760, 1002496), 962848, (1000032, 1000312, 1000464, 960320), 1000032,
                            (1001072, 1001528, 1001520, 961392), (1002160, 1002176, 1002400, 962304), 962432,
                            (986416, 986480), (985824, 986160), (985728, 985872, 985712), (985872, 985712, 985712),
                            (1001104, 1001280, 1000736)),
}
PM_EXTRA = {"sources.folder_merge": COLD_ASSIGN, "merge_budget": COLD_ASSIGN}   # merges whose assign is the first (cold)

# Families: the shape, the pass whose table prices it, the primary axis (the size n the sweep and the control move), the
# secondary axis and leaf darwin keeps fixed and linux searches (m from `fixed` to `fixed + span - 1`, every listed leaf).
PM_FAMILIES = {
    "COPY": {"form": "proto", "pass": "copy", "primary": "a", "fixed": 0, "span": 16, "leaf": "null_array",
             "leaves": ("null_array", "empty_array"),
             "text": 'proto(o, a, leaf) = {"x":{"__proto__": o objects, a arrays around the leaf}}; primary a; copy cost '
                     "(2+o)*object + a*array + leaf; walk and clone do not enter __proto__"},
    "WALK": {"form": "nest", "pass": "walk", "primary": "o", "fixed": 0, "span": 64, "leaf": "null_object", "leaves": LEAF_NAMES,
             "text": "nest(o, a, leaf) = o nested objects, then a arrays around the leaf; primary o; walk cost o*object + "
                     "a*array + leaf"},
    "CLONE": {"form": "nest", "pass": "clone", "primary": "a", "fixed": 1, "span": 16, "leaf": "null_array", "leaves": LEAF_NAMES,
              "text": "nest(o, a, leaf) with o >= 1 (a root object holding nested arrays); primary a; clone cost o*object + "
                      "a*array + leaf"},
    "HWALK": {"form": "model", "pass": "walk", "primary": "o", "fixed": 0, "span": 64, "leaf": "null_object", "leaves": LEAF_NAMES,
              "text": 'model(o, a, leaf) = {"model": nest(o, a, leaf)}; primary o; walk cost (1+o)*object + a*array + leaf'},
    "HCLONE": {"form": "model", "pass": "clone", "primary": "a", "fixed": 0, "span": 16, "leaf": "null_array", "leaves": LEAF_NAMES,
               "text": 'model(o, a, leaf) = {"model": nest(o, a, leaf)}; primary a; clone cost (1+o)*object + a*array + leaf'},
    "LEVELS": {"form": "levels", "pass": "merge", "primary": "n",
               "text": "levels(n) = n nested objects around null; merge cost n*level (+ cold_assign 416 when the first assign)"},
    "ADMIN": {"form": "admin", "pass": "merge", "primary": "n",
              "text": 'admin(n) = {"x":[ n nested objects around null ]} over HELD {"x":[null]}; merge cost (n+2)*level (warm)'},
}
PM_FILES = {
    "user_parent_drop": "[USER, PARENT, drop-in a = the shape]",
    "user_drop_later": "[USER, drop-in a = the shape, LATER (drop-in b {\"y\":1})]",
    "user_held_drop": "[USER, HELD (managed {\"x\":[null]}), drop-in a = the shape]",
    "user_only": "[user settings = the shape] (no other file)",
    "user_remote": "[USER, remote (serverManagedSettings option) = the shape]",
}
PM_OBSERVABLES = {
    "E": "effective holds x (r3 proto_kinds E): fit; no x: over",
    "S": "the managed source's settings hold x (proto_kinds S): fit; else over",
    "O": "the managed source's policyOrigin is file (proto_kinds O): fit; else over",
    "M": "machinePolicy didNotLoad is false (proto_kinds M): fit; else over",
    "sources_merge": "managed_source set: fit; null: over",
    "machine_merge": "needs managed_source null (the sources folder merge overflowed), else invalid; machine policy didNotLoad "
                     "false: fit; else over",
    "sources_read": "managed_source null (the read fit, the folder merge then overflows): fit; managed settings exactly "
                    "{\"y\":1} (the reader dropped drop-in a): over; else invalid",
    "origin_read": "needs managed settings exactly {\"y\":1} (the sources reader dropped drop-in a), else invalid; policyOrigin "
                   "not file (the origin read fit, its merge overflows): fit; file: over",
    "machine_read": "needs policyOrigin file (the origin reader dropped drop-in a), else invalid; machinePolicy didNotLoad not "
                    "false (the machine read fit, its merge overflows): fit; false: over",
    "no_error": "no error: fit; RangeError: over; another error: invalid",
    "range_error": "RangeError (the read fit, the merge then overflows): fit; no error (the reader dropped the file): over; "
                   "another error: invalid",
}
# The measured budgets, in run order within each phase. Phase 1: the passes the discovery rows decide on; phase 2 the rest.
# A guard ("fits", B') keeps a point only where the same shape fits budget B' by PM_MARGIN; ("fails", B') where it misses B'
# by PM_MARGIN. They keep every point where only the pass under test can flip the observable.
PM_PARAMS = {
    "effective_copy": {"family": "COPY", "files": "user_parent_drop", "observable": "E", "guards": (), "phase": 1},
    "sources.copy": {"family": "COPY", "files": "user_parent_drop", "observable": "S", "guards": (), "phase": 1},
    "origin.copy": {"family": "COPY", "files": "user_parent_drop", "observable": "O", "guards": (), "phase": 1},
    "machine_policy.copy": {"family": "COPY", "files": "user_parent_drop", "observable": "M", "guards": (), "phase": 1},
    "sources.folder_merge": {"family": "LEVELS", "files": "user_drop_later", "observable": "sources_merge", "guards": (),
                             "phase": 1},
    "machine_policy.folder_merge": {"family": "LEVELS", "files": "user_drop_later", "observable": "machine_merge",
                                    "guards": (("fails", "sources.folder_merge"), ("fails", "origin.folder_merge")), "phase": 1},
    "origin.folder_merge": {"family": "LEVELS", "files": "user_parent_drop", "observable": "O",
                            "guards": (("fails", "sources.folder_merge"),), "phase": 1},
    "sources.walk": {"family": "WALK", "files": "user_drop_later", "observable": "sources_read",
                     "guards": (("fits", "sources.clone"),), "phase": 1},
    "sources.clone": {"family": "CLONE", "files": "user_drop_later", "observable": "sources_read",
                      "guards": (("fits", "sources.walk"),), "phase": 1},
    "origin.walk": {"family": "WALK", "files": "user_drop_later", "observable": "origin_read",
                    "guards": (("fits", "origin.clone"), ("fails", "sources.walk")), "phase": 1},
    "origin.clone": {"family": "CLONE", "files": "user_drop_later", "observable": "origin_read",
                     "guards": (("fits", "origin.walk"), ("fails", "sources.clone")), "phase": 1},
    "machine_policy.walk": {"family": "WALK", "files": "user_drop_later", "observable": "machine_read",
                            "guards": (("fits", "machine_policy.clone"), ("fails", "origin.walk")), "phase": 1},
    "machine_policy.clone": {"family": "CLONE", "files": "user_drop_later", "observable": "machine_read",
                             "guards": (("fits", "machine_policy.walk"), ("fails", "origin.clone")), "phase": 1},
    "admin_merge": {"family": "ADMIN", "files": "user_held_drop", "observable": "no_error", "guards": (), "phase": 1},
    "toplevel.walk": {"family": "WALK", "files": "user_only", "observable": "range_error",
                      "guards": (("fits", "toplevel.clone"),), "phase": 2},
    "toplevel.clone": {"family": "CLONE", "files": "user_only", "observable": "range_error",
                       "guards": (("fits", "toplevel.walk"),), "phase": 2},
    "toplevel.copy": {"family": "COPY", "files": "user_only", "observable": "no_error", "guards": (), "phase": 2},
    "merge_budget": {"family": "LEVELS", "files": "user_only", "observable": "no_error", "guards": (), "phase": 2},
    "handed_over.walk": {"family": "HWALK", "files": "user_remote", "observable": "no_error",
                         "guards": (("fits", "handed_over.clone"),), "phase": 2},
    "handed_over.clone": {"family": "HCLONE", "files": "user_remote", "observable": "no_error",
                          "guards": (("fits", "handed_over.walk"),), "phase": 2},
    "handed_over.copy": {"family": "COPY", "files": "user_remote", "observable": "no_error", "guards": (), "phase": 2},
}
# The reader passes whose fit side must overflow a merge (so the observable separates a fit read from a dropped file): the
# folder or top-level merge budget each must exceed by PM_MARGIN at every point.
READER_MERGE = {"sources": "sources.folder_merge", "origin": "origin.folder_merge", "machine_policy": "machine_policy.folder_merge",
                "toplevel": "merge_budget"}

_HELD_FS = ("the held check reads the managed folder only on the WSL and win32 routes; the native gate refuses a fake /proc, "
            "so no native run on this host reaches the pass")
_HELD_TOOL = ("the held check tool state is entered only on the WSL and win32 routes, and it resumes on one of two stacks "
              "that change from run to run, so one run per point cannot isolate it")
_REG_FS = ("the registry file cache is reached only through the WSL /proc route or the organization path with a simulated "
           "clock; the native gate refuses both on this host")
_REG_TOOL_LINUX = "the registry tool runs only on WSL; no native Linux run reaches the pass"
_REG_TOOL_DARWIN = ("the plist route of the registry tool resumes on one of two stacks 34656 bytes apart from run to run, so a "
                    "point needs repeated runs to read, which the one-run-per-point rule forbids")
PM_UNMEASURABLE = {}
for _host in HOSTS:
    PM_UNMEASURABLE[_host] = {"held_check.fs.walk": _HELD_FS, "held_check.fs.clone": _HELD_FS,
                              "held_check.tool.walk": _HELD_TOOL, "held_check.tool.clone": _HELD_TOOL}
    for _which in ("walk", "clone", "copy"):
        PM_UNMEASURABLE[_host][f"registry.fs.{_which}"] = _REG_FS
        PM_UNMEASURABLE[_host][f"registry.tool.{_which}"] = _REG_TOOL_DARWIN if _host == "darwin arm64" else _REG_TOOL_LINUX

# Caller-context offsets of the model (node_stack_contexts_tests.rs and its fixture, first call): (value, I_lo, I_hi). toplevel
# and nested sit at MODULE_OFFSET 0; child_exit has no single value, only its interval I_child.
PM_CONTEXTS = {
    "darwin arm64": {"nested": (0, -16, 16), "timer": (-192, -208, -176), "immediate": (-432, -464, -400),
                     "io": (-34720, -34736, -34704), "child_exit": (None, -38832, 0)},
    "linux aarch64": {"nested": (0, -16, 16), "timer": (-256, -336, -208), "immediate": (-528, -656, -496),
                      "io": (-22640, -22736, -22560), "child_exit": (None, -26832, 0)},
    "linux x86_64": {"nested": (0, -16, 16), "timer": (-192, -208, 16), "immediate": (-480, -496, -288),
                     "io": (-17360, -17424, -17312), "child_exit": (None, -21520, 0)},
}
OFFSET_CONTEXTS = ("nested", "timer", "immediate", "io")

# The discovery and golden rows (round 8 PROFILES, `pair`: rows at n and n + 1), the budget each decides on and its cost form.
PM_GROUPS = {
    "drop_merge": ("sources.folder_merge", "levels"), "drop_read": ("sources.walk", "read"),
    "drop_read_arrays": ("sources.clone", "read_arrays"), "admin_arrays": ("admin_merge", "admin_arrays"),
    "admin_inner": ("admin_merge", "admin_inner"), "sources_copy": ("sources.copy", "copy_arrays"),
    "machine_merge": ("machine_policy.folder_merge", "levels"), "machine_copy": ("machine_policy.copy", "copy_arrays"),
    "origin_read": ("origin.walk", "read"), "machine_read": ("machine_policy.walk", "read"),
    "origin_read_arrays": ("origin.clone", "read_arrays"), "machine_read_arrays": ("machine_policy.clone", "read_arrays"),
    "origin_copy": ("origin.copy", "proto"), "origin_merge": ("origin.folder_merge", "levels"),
}
ROW_FORMS = {
    "levels": lambda n: ("levels", n),
    "read": lambda n: ("nest", n - 1, 0, "null_object"),
    "read_arrays": lambda n: ("nest", 1, n - 1, "null_array"),
    "admin_arrays": lambda n: ("admin", n - 1),          # a root object holding n arrays: (n+1)*level
    "admin_inner": lambda n: ("admin", n),
    "copy_arrays": lambda n: ("nest", 1, n - 1, "null_array"),
    "proto": lambda n: ("proto", 0, n, "null_array"),
}
_GROUP_ORDER = ("drop_merge", "drop_read", "drop_read_arrays", "admin_arrays", "admin_inner", "sources_copy", "machine_merge",
                "machine_copy", "origin_read", "machine_read", "origin_read_arrays", "machine_read_arrays", "origin_copy",
                "origin_merge")
PM_GOLDEN = {
    "darwin arm64": dict(zip(_GROUP_ORDER, (1241, 2217, 3268, 1243, 1242, 1941, 1244, 1945, 2220, 2222, 3271, 3274, 1942, 1243))),
    "linux aarch64": dict(zip(_GROUP_ORDER, (1249, 1952, 3123, 1251, 1250, 1784, 1252, 1788, 1954, 1956, 3127, 3130, 1784, 1251))),
    "linux x86_64": dict(zip(_GROUP_ORDER, (1304, 1644, 3289, 1306, 1305, 1562, 1307, 1565, 1646, 1648, 3293, 3295, 1561, 1306))),
}
# The proto pair rows (startup 4384): (budget, (o, a, leaf)) of proto(o, a, leaf).
PM_PROTO_ROWS = {
    "darwin arm64": [("effective_copy", (0, 1939, "null_array")), ("effective_copy", (0, 1940, "null_array")),
                     ("sources.copy", (0, 1940, "null_array")), ("sources.copy", (0, 1941, "null_array")),
                     ("origin.copy", (0, 1942, "null_array")), ("origin.copy", (0, 1943, "null_array")),
                     ("machine_policy.copy", (0, 1943, "null_array")), ("machine_policy.copy", (0, 1944, "null_array"))],
    "linux aarch64": [("effective_copy", (1, 1782, "empty_array")), ("effective_copy", (20, 1764, "null_array")),
                      ("sources.copy", (23, 1762, "null_array")), ("sources.copy", (11, 1773, "null_array")),
                      ("origin.copy", (12, 1774, "null_array")), ("origin.copy", (0, 1785, "null_array")),
                      ("machine_policy.copy", (17, 1771, "null_array")), ("machine_policy.copy", (5, 1782, "null_array"))],
    "linux x86_64": [("effective_copy", (7, 1553, "null_array")), ("effective_copy", (12, 1549, "empty_array")),
                     ("sources.copy", (19, 1543, "empty_array")), ("sources.copy", (13, 1548, "null_array")),
                     ("origin.copy", (6, 1557, "empty_array")), ("origin.copy", (0, 1562, "null_array")),
                     ("machine_policy.copy", (13, 1551, "null_array")), ("machine_policy.copy", (18, 1547, "empty_array"))],
}
# Tolerances the design must reproduce (smallest row margins worked out by hand from the model).
PM_TOLERANCE_CHECKS = {("darwin arm64", "sources.folder_merge"): 260, ("darwin arm64", "sources.walk"): 189,
                       ("darwin arm64", "origin.walk"): 1, ("darwin arm64", "machine_policy.copy"): 6,
                       ("darwin arm64", "sources.copy"): 75, ("linux aarch64", "sources.copy"): 1}


def pm_pass(name):
    """The pass a budget prices: walk, clone, copy or merge."""
    if name == "effective_copy":
        return "copy"
    last = name.rsplit(".", 1)[-1]
    return last if last in ("walk", "clone", "copy") else "merge"


def pm_shape(family, n, m=None, leaf=None):
    """The shape of `family` at primary size n (and, for path families, secondary m and leaf; darwin keeps the defaults)."""
    f = PM_FAMILIES[family]
    if f["pass"] == "merge":
        return (f["form"], n)
    m = f["fixed"] if m is None else m
    leaf = leaf or f["leaf"]
    return (f["form"], n, m, leaf) if f["primary"] == "o" else (f["form"], m, n, leaf)


def pm_levels(shape):
    """The containers a merge enters on the deepest path of `shape` (the leaf container is one)."""
    form = shape[0]
    if form == "levels":
        return shape[1]
    if form == "admin":
        return shape[1] + 2
    if form == "proto":
        return 2
    return FORM_ABOVE[form] + shape[1] + shape[2] + 1


def pm_cost(host, budget, shape):
    """What the decisive path of `shape` spends in the pass of `budget` (None when that pass does not enter the shape)."""
    profile = PM_PROFILE[host]
    which = pm_pass(budget)
    form = shape[0]
    if which == "merge":
        return pm_levels(shape) * profile["L"] + PM_EXTRA.get(budget, 0)
    if form in ("levels", "admin") or (form == "proto" and which != "copy"):
        return None
    table = profile[which]
    _, o, a, leaf = shape
    return (FORM_ABOVE[form] + o) * table["object"] + a * table["array"] + table["leaves"][leaf]


def pm_text(shape):
    """The document text of `shape`."""
    form = shape[0]
    if form == "levels":
        return '{"x":' * shape[1] + "null" + "}" * shape[1]
    if form == "admin":
        return '{"x":[' + '{"x":' * shape[1] + "null" + "}" * shape[1] + "]}"
    _, o, a, leaf = shape
    inner = '{"x":' * o + "[" * a + LEAF_TEXT[leaf] + "]" * a + "}" * o
    if form == "proto":
        return '{"x":{"__proto__":' + inner + "}}"
    if form == "model":
        return '{"model":' + inner + "}"
    return inner


def pm_guards_ok(host, pname, shape, room_term):
    """Whether every guard of `pname` holds for `shape` at the knob term `room_term` (darwin +startup, linux -1024 k)."""
    for kind, other in PM_PARAMS[pname]["guards"]:
        cost = pm_cost(host, other, shape)
        bound = PM_BUDGETS[host][other]
        if kind == "fits":
            if cost is not None and cost + room_term > bound - PM_MARGIN:
                return False
        elif cost is None or cost + room_term < bound + PM_MARGIN:
            return False
    return True


def _point(q, shape, family, n, m, leaf, st, k, ss0):
    return {"q": q, "n": n, "m": m, "leaf": leaf, "st": st, "k": k, "stack_kb": None if k is None else ss0 + k,
            "r": st if k is None else -1024 * k, "shape": list(shape), "family": family}


PM_MEMO = {}


def pm_points(host, pname, qlo, qhi, ss0=DEFAULT_STACK_KB):
    """The primary and control points of `pname`'s family with q in [qlo, qhi], in descending q (ascending room), and the
    count of grid values no point could reach. darwin: the 16-byte startup grid, primary n_ref (or the n nearest the default
    startup when n_ref leaves [2048, 8192]); control n - 1 (or n + 1) at the same q. linux: per q the cheapest (n, m, leaf, k),
    rank 2|k| + |j| + (m - first m) + 4 * leaf index; control the cheapest with another n at the same q."""
    key = (host, pname, qlo, qhi, ss0)
    if key in PM_MEMO:
        return PM_MEMO[key]
    family = PM_PARAMS[pname]["family"]
    spec = PM_FAMILIES[family]
    c0 = pm_cost(host, pname, pm_shape(family, 0))
    unit = pm_cost(host, pname, pm_shape(family, 1)) - c0
    primary, control, clipped = [], [], 0
    if PM_PROFILE[host]["stack"]:
        qlo, qhi = ceil_to(int(qlo), STEP), floor_to(int(qhi), STEP)
        n_ref = max(1, ((qlo + qhi) // 2 - ST_DEFAULT - c0 + unit // 2) // unit)
        m = spec.get("fixed")
        leaf = spec.get("leaf")

        def at(q, n):
            if n < 1:
                return None
            shape = pm_shape(family, n)
            st = q - pm_cost(host, pname, shape)
            if not ST_MIN <= st <= ST_MAX or not pm_guards_ok(host, pname, shape, st):
                return None
            return _point(q, shape, family, n, m, leaf, st, None, ss0)

        for q in range(qhi, qlo - 1, -STEP):
            nearest = max(1, (q - ST_DEFAULT - c0 + unit // 2) // unit)
            one = at(q, n_ref) or at(q, nearest)
            if one is None:
                clipped += 1
                continue
            primary.append(one)
            twin = at(q, one["n"] - 1) or at(q, one["n"] + 1)
            if twin is not None:
                control.append(twin)
    else:
        qlo, qhi = int(qlo), int(qhi)
        merge = spec["pass"] == "merge"
        seconds = [None] if merge else list(range(spec["fixed"], spec["fixed"] + spec["span"]))
        leaves = [None] if merge else list(spec["leaves"])
        n_ref = max(1, ((qlo + qhi) // 2 - c0 + unit // 2) // unit)
        found = {}
        for j in range(-PM_J, PM_J + 1):
            n = n_ref + j
            if n < 1:
                continue
            for mi, m in enumerate(seconds):
                for li, leaf in enumerate(leaves):
                    shape = pm_shape(family, n, m, leaf)
                    cost = pm_cost(host, pname, shape)
                    for k in range(max(-PM_K, -((qhi - cost) // 1024)), min(PM_K, (cost - qlo) // 1024) + 1):
                        if not pm_guards_ok(host, pname, shape, -1024 * k):
                            continue
                        rank = (2 * abs(k) + abs(j) + mi + 4 * li, abs(k), abs(j), mi, li)
                        found.setdefault(cost - 1024 * k, []).append((rank, n, m, leaf, k))
        for q in sorted(found, reverse=True):
            options = sorted(found[q])
            _, n, m, leaf, k = options[0]
            primary.append(_point(q, pm_shape(family, n, m, leaf), family, n, m, leaf, None, k, ss0))
            twin = next((one for one in options if one[1] != n), None)
            if twin is not None:
                _, tn, tm, tleaf, tk = twin
                control.append(_point(q, pm_shape(family, tn, tm, tleaf), family, tn, tm, tleaf, None, tk, ss0))
    PM_MEMO[key] = (primary, control, clipped)
    return PM_MEMO[key]


def pm_resolution(qs, edge):
    """The largest gap between consecutive q of a sweep, the gap around `edge` (the predicted flip) and the bracket the model
    predicts there."""
    qs = sorted(set(qs), reverse=True)
    gaps = [x - y for x, y in zip(qs, qs[1:])]
    above = [q for q in qs if q > edge]
    below = [q for q in qs if q <= edge]
    out = {"points": len(qs), "max_gap": max(gaps) if gaps else None, "gap_at_model": None, "expected_bracket": None}
    if above and below:
        out["gap_at_model"] = min(above) - max(below)
        out["expected_bracket"] = [max(below), min(above) - 1]
    return out


def pm_rows(host, pname):
    """The rows the model decides with budget `pname`: q, fit or not, margin (B - q + 1 when it fits, q - B when not); the
    interval of budgets every row allows; the tolerance (the smallest margin, else 16) and why."""
    b = PM_BUDGETS[host][pname]
    st = ST_DEFAULT if PM_PROFILE[host]["stack"] else 0
    found = []
    for group in _GROUP_ORDER:
        param, form = PM_GROUPS[group]
        if param == pname:
            for n in (PM_GOLDEN[host][group], PM_GOLDEN[host][group] + 1):
                found.append((group, n, ROW_FORMS[form](n)))
    for param, (o, a, leaf) in PM_PROTO_ROWS[host]:
        if param == pname:
            found.append(("proto_pair", a, ("proto", o, a, leaf)))
    rows = []
    for group, n, shape in found:
        q = pm_cost(host, pname, shape) + st
        rows.append({"group": group, "n": n, "shape": list(shape), "q": q, "fits": q <= b,
                     "margin": b - q + 1 if q <= b else q - b})
    if not rows:
        return {"rows": [], "interval": None, "tolerance": PM_TOLERANCE,
                "reason": "no discovery or golden row decides on this budget in the model: 16 bytes (the startup step)"}
    fits = [row["q"] for row in rows if row["fits"]]
    fails = [row["q"] for row in rows if not row["fits"]]
    low = min(rows, key=lambda row: row["margin"])
    return {"rows": rows, "interval": [max(fits) if fits else None, min(fails) - 1 if fails else None],
            "tolerance": low["margin"],
            "reason": f"the smallest margin a row has against this budget in the model: {low['group']} n={low['n']} "
                      f"q={low['q']} ({'fits' if low['fits'] else 'over'})"}


def _where(one):
    return {"q": one["q"], "n": one["n"], "m": one["m"], "leaf": one["leaf"], "startup": one["st"], "stack_kb": one["stack_kb"]}


def pm_param_plan(host, pname, ss0=DEFAULT_STACK_KB, keep_points=False):
    """The design of one budget: model value, shape, observable, window, expected flip, control, tolerance, resolution,
    whether adoption is achievable and the runs."""
    spec = PM_PARAMS[pname]
    family = PM_FAMILIES[spec["family"]]
    b = PM_BUDGETS[host][pname]
    qlo, qhi = b - PM_HALF, b + PM_HALF
    primary, control, clipped = pm_points(host, pname, qlo, qhi, ss0)
    rows = pm_rows(host, pname)
    resolution = pm_resolution([one["q"] for one in primary], b)
    tolerance = rows["tolerance"]
    expected = resolution["expected_bracket"]
    by_q = {one["q"]: one for one in primary}
    out = {"model": b, "pass": pm_pass(pname), "phase": spec["phase"], "family": spec["family"], "shape": family["text"],
           "files": PM_FILES[spec["files"]], "observable": PM_OBSERVABLES[spec["observable"]],
           "guards": [f"{kind} {other} by {PM_MARGIN}" for kind, other in spec["guards"]],
           "window_q": [qlo, qhi], "knob": "startup bytes, 16-byte step" if PM_PROFILE[host]["stack"] else "(n, m, leaf, --stack-size)",
           "expected_flip": {"bracket": expected, "reported_hi": expected[1] if expected else None,
                             "last_fit": _where(by_q[expected[0]]) if expected else None,
                             "first_over": _where(by_q[expected[1] + 1]) if expected else None},
           "control": {"what": "the same q values reached by another primary size n" + (
               " (n - 1, or n + 1, with the startup moved by the per-level cost)" if PM_PROFILE[host]["stack"] else
               " (the cheapest other (n, k) for the q)"),
               "criterion": "the control sweep is monotone with a bracket, the two brackets intersect and one contains the other",
               "points": len(control)},
           "rows": [[row["group"], row["n"], row["q"], row["margin"], row["fits"]] for row in rows["rows"]],
           "rows_interval": rows["interval"], "tolerance": tolerance, "tolerance_reason": rows["reason"],
           "resolution": resolution,
           "adoption_achievable": resolution["gap_at_model"] is not None and resolution["gap_at_model"] < tolerance,
           "adoption_achievable_everywhere": resolution["max_gap"] is not None and resolution["max_gap"] < tolerance,
           "points": {"primary": len(primary), "control": len(control), "clipped": clipped},
           "runs": len(primary) + len(control)}
    if keep_points:
        out["point_lists"] = {"primary": primary, "control": control}
    return out


def pm_offset_points(host, context, ss0=DEFAULT_STACK_KB):
    b = PM_BUDGETS[host]["effective_copy"]
    _, lo, hi = PM_CONTEXTS[host][context]
    return pm_points(host, "effective_copy", b + lo - OFFSET_PAD, b + hi + OFFSET_PAD, ss0)


def pm_offset_plan(host, ss0=DEFAULT_STACK_KB):
    b = PM_BUDGETS[host]["effective_copy"]
    out = {"toplevel": {"model": 0, "interval": [0, 0], "reference": "the effective_copy toplevel sweeps (primary and control) "
                        "of phase 1; adopted as 0 when that leg is monotone with a bracket and its control passes",
                        "runs": 0}}
    for context in OFFSET_CONTEXTS:
        value, lo, hi = PM_CONTEXTS[host][context]
        primary, control, clipped = pm_offset_points(host, context, ss0)
        resolution = pm_resolution([one["q"] for one in primary], b + value)
        out[context] = {"model": value, "interval": [lo, hi], "family": "COPY (effective_copy: proto, observable E)",
                        "window_q": [b + lo - OFFSET_PAD, b + hi + OFFSET_PAD], "expected_flip_q": b + value,
                        "resolution": resolution, "tolerance": 16,
                        "tolerance_reason": "the 16-byte frame alignment of the model (STEP 16 of the context tests)",
                        "control": "the same q values by another n in the same context, criterion as for the budgets; the "
                                   "toplevel reference must pass its own control",
                        "value": "A-free twin difference d = q_fit(context) - q_fit(toplevel); strict bracket "
                                 "[q_fit_c - (q_fail_t - 1), (q_fail_c - 1) - q_fit_t]; d adopted only when both widths are 16 "
                                 "and q_fail_c - q_fail_t = d (the only 16-aligned value in the strict bracket)",
                        "adoption_achievable": resolution["gap_at_model"] is not None and resolution["gap_at_model"] <= 16,
                        "points": {"primary": len(primary), "control": len(control), "clipped": clipped},
                        "runs": len(primary) + len(control)}
    return out


def pm_coarse(points_, qlo):
    """One point per CHILD_COARSE bucket of q (the smallest q in each), in descending q."""
    chosen = {}
    for one in points_:
        bucket = (one["q"] - qlo) // CHILD_COARSE
        if bucket not in chosen or one["q"] < chosen[bucket]["q"]:
            chosen[bucket] = one
    return sorted(chosen.values(), key=lambda one: -one["q"])


def pm_child_plan(host, ss0=DEFAULT_STACK_KB):
    b = PM_BUDGETS[host]["effective_copy"]
    _, lo, hi = PM_CONTEXTS[host]["child_exit"]
    qlo, qhi = b + lo - CHILD_PAD, b + hi + CHILD_PAD
    primary, _, _ = pm_points(host, "effective_copy", qlo, qhi, ss0)
    coarse = pm_coarse(primary, qlo)
    fine_primary, fine_control, _ = pm_points(host, "effective_copy", b - CHILD_FINE, b + CHILD_FINE, ss0)
    return {"interval_I_child": [lo, hi], "window_q": [qlo, qhi], "coarse_bucket": CHILD_COARSE, "coarse_points": len(coarse),
            "fine": f"q in [first coarse fit - {CHILD_FINE}, + {CHILD_FINE}] without the q already run, primary and control",
            "fine_points_estimate": len(fine_primary) + len(fine_control),
            "value": "per-process sample: each fit at q says that run had offset >= q - B, each over that it had offset < q - B; "
                     "the bounds are checked against I_child; no single value is adopted",
            "runs": len(coarse) + len(fine_primary) + len(fine_control)}


def pm_table_line(name, entry):
    exp = entry["expected_flip"]["bracket"]
    res = entry["resolution"]
    return (f"{name}: B={entry['model']} {entry['family']} window={entry['window_q']} expected={exp} tol={entry['tolerance']} "
            f"gap@B={res['gap_at_model']} maxgap={res['max_gap']} adopt_possible={entry['adoption_achievable']} "
            f"runs={entry['runs']} (clipped {entry['points']['clipped']})")


def pm_plan(host, ss0=DEFAULT_STACK_KB):
    params = {pname: pm_param_plan(host, pname, ss0) for pname in PM_PARAMS}
    offsets = pm_offset_plan(host, ss0)
    child = pm_child_plan(host, ss0)
    s_runs = plan(host)["total_runs_max"]
    runs = {"phase1_budgets": sum(params[p]["runs"] for p in PM_PARAMS if PM_PARAMS[p]["phase"] == 1),
            "offsets": sum(offsets[c]["runs"] for c in OFFSET_CONTEXTS), "S_legs": s_runs,
            "phase2_budgets": sum(params[p]["runs"] for p in PM_PARAMS if PM_PARAMS[p]["phase"] == 2),
            "child_exit": child["runs"]}
    runs["total"] = sum(runs.values())
    table = [pm_table_line(pname, params[pname]) for pname in PM_PARAMS]
    table += [f"offset {c}: model={offsets[c]['model']} I={offsets[c]['interval']} gap@flip={offsets[c]['resolution']['gap_at_model']} "
              f"runs={offsets[c]['runs']}" for c in OFFSET_CONTEXTS]
    table.append(f"child_exit: I_child={child['interval_I_child']} runs<={child['runs']}")
    table += [f"{name}: unmeasurable in this harness" for name in PM_UNMEASURABLE[host]]
    return {"host": host, "label": HOSTS[host]["label"],
            "order": ["phase 1 budgets: " + ", ".join(p for p in PM_PARAMS if PM_PARAMS[p]["phase"] == 1),
                      "offsets: " + ", ".join(OFFSET_CONTEXTS), "S legs (diagsa)",
                      "phase 2 budgets: " + ", ".join(p for p in PM_PARAMS if PM_PARAMS[p]["phase"] == 2), "child_exit"],
            "table": table, "params": params, "offsets": offsets, "child_exit": child,
            "unmeasurable": {name: {"model": PM_BUDGETS[host][name], "reason": reason}
                             for name, reason in PM_UNMEASURABLE[host].items()},
            "runs": runs, "estimated_seconds": round(runs["total"] * RUN_SECONDS), "budget_seconds": DIAG_BUDGET}


def pm_check_design():
    """Every budget accounted for (measured or unmeasurable); the profile matches the S design; the model reproduces every
    golden pair (n fits, n + 1 over); each window brackets its budget for the primary and the control; the reader shapes
    overflow their merge on the fit side; the offset windows cover the model intervals; the known tolerances come out; the
    job fits the time budget."""
    for host in HOSTS:
        bud, profile = PM_BUDGETS[host], PM_PROFILE[host]
        assert bud["sources.folder_merge"] == HOSTS[host]["A"] and bud["machine_policy.folder_merge"] == HOSTS[host]["AM"], host
        assert profile["L"] == HOSTS[host]["L"] and profile["stack"] == (HOSTS[host]["share"] == "stack"), host
        assert set(PM_PARAMS).isdisjoint(PM_UNMEASURABLE[host]), host
        assert set(PM_PARAMS) | set(PM_UNMEASURABLE[host]) == set(bud), (host, sorted(set(bud) ^ (set(PM_PARAMS) | set(PM_UNMEASURABLE[host]))))
        st = ST_DEFAULT if profile["stack"] else 0
        for group in _GROUP_ORDER:
            param, form = PM_GROUPS[group]
            value = PM_GOLDEN[host][group]
            q_n = pm_cost(host, param, ROW_FORMS[form](value)) + st
            q_next = pm_cost(host, param, ROW_FORMS[form](value + 1)) + st
            assert q_n <= bud[param] < q_next, (host, group, q_n, bud[param], q_next)
        for pname in PM_PARAMS:
            entry = pm_param_plan(host, pname, keep_points=True)
            b = bud[pname]
            for label in ("primary", "control"):
                qs = [one["q"] for one in entry["point_lists"][label]]
                assert any(q <= b for q in qs) and any(q > b for q in qs), (host, pname, label)
                assert qs == sorted(qs, reverse=True), (host, pname, label)
            if entry["rows_interval"] is not None and None not in entry["rows_interval"]:
                assert entry["rows_interval"][0] <= entry["rows_interval"][1], (host, pname, entry["rows_interval"])
            prefix = pname.split(".")[0]
            if pm_pass(pname) in ("walk", "clone") and prefix in READER_MERGE:
                folder = bud[READER_MERGE[prefix]]
                for one in entry["point_lists"]["primary"] + entry["point_lists"]["control"]:
                    merged = pm_levels(tuple(one["shape"])) * profile["L"] + one["r"]
                    assert merged >= folder + PM_MARGIN, (host, pname, one["q"], merged, folder)
        b = bud["effective_copy"]
        for context in OFFSET_CONTEXTS:
            _, lo, hi = PM_CONTEXTS[host][context]
            for points_ in pm_offset_points(host, context)[:2]:
                qs = [one["q"] for one in points_]
                assert any(q <= b + lo for q in qs) and any(q > b + hi for q in qs), (host, context)
        child = pm_child_plan(host)
        assert child["coarse_points"] > 0, host
        total = pm_plan(host)["runs"]["total"]
        assert total * RUN_SECONDS <= DIAG_BUDGET, (host, total)
    for (host, pname), want in PM_TOLERANCE_CHECKS.items():
        got = pm_rows(host, pname)["tolerance"]
        assert got == want, (host, pname, got, want)


if len(sys.argv) > 1 and sys.argv[1] == "--plan":
    # Arithmetic only: nothing below this branch (the exec of r3.py, the spawns) runs.
    check_design()
    pm_check_design()
    wanted = sys.argv[2:] or list(HOSTS)
    print(json.dumps([{"host": host, "S": plan(host), "pm": pm_plan(host)} for host in wanted], indent=1))
    sys.exit(0)

LIB, NODE, PACKAGE, OUT, TREE = sys.argv[1:6]
BUDGET_SECONDS = int(os.environ.get("DIAG_BUDGET_SECONDS", str(DIAG_BUDGET)))
STARTED = time.time()
ARTIFACTS = os.path.join(os.path.dirname(os.path.abspath(OUT)), "diagpm-traces")
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
          "default_stack_kb": None, "design": None, "pm_design": None, "runs": [], "sweeps": {}, "pm_sweeps": {},
          "flat": None, "invariance": None, "trace_leg": None,
          "results": {"params": {}, "offsets": {}, "child_exit": None}, "not_reached": [], "stops": [], "summary": {}}
UNSAVED = {"runs": 0}


def save():
    UNSAVED["runs"] = 0
    result["seconds"] = round(time.time() - STARTED)
    with open(OUT, "w", encoding="utf-8") as handle:
        json.dump(result, handle)


def save_soon():
    """Writes the output every SAVE_EVERY runs (each sweep and each leg end writes it too), so the file stays current
    without rewriting thousands of runs after every one."""
    UNSAVED["runs"] += 1
    if UNSAVED["runs"] >= SAVE_EVERY:
        save()


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
PARENT, HELD = port["PARENT"], port["HELD"]
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
try:
    result["pm_design"] = pm_plan(HOST, SS0)
except (Exception, SystemExit) as error:
    stop(f"pm_design: {error!r}")
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
    save_soon()
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


# ---- the parameter legs: runs, analysis, verdicts ----
PM_SEQ = {"n": 0}
PM_DONE = {}


def pm_files(key, doc):
    drop = ["drop_a", doc]
    return {"user_parent_drop": [USER, PARENT, drop], "user_drop_later": [USER, drop, LATER],
            "user_held_drop": [USER, HELD, drop], "user_only": [["user", doc]],
            "user_remote": [USER, ["remote", doc]]}[key]


def pm_kinds(stdout):
    """What the oracle's own output says of the managed source and the machine policy (r3 proto_kinds, plus the managed
    settings being exactly the LATER document)."""
    if not stdout:
        return None
    try:
        found = json.loads(stdout.decode("utf-8"))[0]["result"]
        managed = next((one for one in found.get("sources", []) if one.get("source") == "managed"), None)
        settings = managed.get("settings") if managed else None
        machine = found.get("machinePolicy")
        effective = found.get("effective")
        return {"E": isinstance(effective, dict) and "x" in effective,
                "S": bool(managed) and isinstance(settings, dict) and "x" in settings,
                "O": bool(managed) and managed.get("policyOrigin") == "file",
                "M": isinstance(machine, dict) and machine.get("didNotLoad") is False,
                "managed": managed is not None, "only_y": settings == {"y": 1},
                "policyOrigin": managed.get("policyOrigin") if managed else None}
    except (ValueError, KeyError, IndexError, TypeError, AttributeError, RecursionError):
        return {"unreadable": True}


def pm_class(observable, record):
    """fit / over / invalid / norun of one run for the observable of its budget (PM_OBSERVABLES)."""
    if any(key in record for key in ("failed", "gate", "runtime")):
        return "norun"
    error = record.get("error")
    if observable == "no_error":
        return "fit" if error is None else ("over" if error == "RangeError" else "invalid")
    if observable == "range_error":
        return "fit" if error == "RangeError" else ("over" if error is None else "invalid")
    if error is not None:
        return "invalid"
    if observable == "sources_merge":
        return "over" if record["managed_null"] else "fit"
    if observable == "machine_merge":
        return "invalid" if not record["managed_null"] else ("fit" if record["mp_ok"] else "over")
    kinds = record.get("kinds")
    if observable == "sources_read":
        if record["managed_null"]:
            return "fit"
        return "over" if kinds and kinds.get("only_y") else "invalid"
    if not kinds or kinds.get("unreadable"):
        return "invalid"
    if observable in ("E", "S", "O", "M"):
        return "fit" if kinds[observable] else "over"
    if observable == "origin_read":
        return "invalid" if not kinds["only_y"] else ("over" if kinds["O"] else "fit")
    if observable == "machine_read":
        return "invalid" if not kinds["O"] else ("over" if kinds["M"] else "fit")
    return "invalid"


def pm_run(pname, point, context, leg):
    """One case of budget `pname` at `point` in a process of its own. One run per point, never repeated: a run that gives no
    answer is recorded as it is."""
    PM_SEQ["n"] += 1
    name = "dp%06d" % PM_SEQ["n"]          # fixed length, and never an effective_ or org_ name
    spec = PM_PARAMS[pname]
    flags = list(REF)
    if point["stack_kb"] is not None:
        flags.append(f"--stack-size={point['stack_kb']}")
    startup = point["st"] if point["st"] is not None else STARTUP
    record = {"leg": leg, "param": pname, "context": context, "case": name, "q": point["q"], "n": point["n"], "m": point["m"],
              "leaf": point["leaf"], "startup": startup, "stack_kb": point["stack_kb"]}
    folder = tempfile.mkdtemp()
    started = time.time()
    mark = len(SPAWNED)
    LAST.clear()
    MODE["trace"] = False
    GATE["in_context"](context)
    try:
        spec_path = port["prepare"](name, pm_files(spec["files"], pm_text(tuple(point["shape"]))), folder)
        answered = port["run"](spec_path, flags, startup)
        if not isinstance(answered, dict):
            record["failed"] = f"answer is not a dict: {type(answered).__name__}"
        else:
            error = answered.get("error")
            record["error"] = (error.get("name") if isinstance(error, dict) else str(error)) if error else None
            if isinstance(error, dict):
                record["error_message"] = str(error.get("message"))[:160]
            managed = answered.get("managed_source")
            policy = answered.get("machine_policy")
            policy_text = re.sub(r"\s", "", policy if isinstance(policy, str) else json.dumps(policy))
            record["managed_null"] = managed in (None, "null")
            record["mp_ok"] = '"didNotLoad":false' in policy_text
            record["answer_sha"] = digest(answered)
            record["kinds"] = pm_kinds(LAST.get("stdout")) if not error else None
    except (GATE["NativeRefusal"], GATE["NativeViolation"]) as error:
        record["gate"] = f"{type(error).__name__}: {str(error)[:300]}"
    except RuntimeError as error:
        record["runtime"] = str(error)[:300]
    except (Exception, SystemExit) as error:
        record["failed"] = repr(error)[:300]
    finally:
        GATE["in_context"](None)
        shutil.rmtree(folder, ignore_errors=True)
    account = SPAWNED[mark] if len(SPAWNED) > mark else None
    record["startup_bytes"] = account["startup_bytes"] if account else None
    record["seconds"] = round(time.time() - started, 2)
    record["class"] = pm_class(spec["observable"], record)
    if record["class"] == "norun":
        stop(f"{leg} {name}: {record.get('failed') or record.get('gate') or record.get('runtime')}")
    result["runs"].append(record)
    save_soon()
    return record


def pm_analyze(runs):
    """The bracket of a sweep run in descending q: over...over then fit...fit. An over after a fit is a monotonicity
    violation, recorded and never retried. B is in [q_fit, q_fail - 1]; the reported value is its hi edge."""
    usable = [one for one in runs if one["class"] in ("fit", "over")]
    violations, seen = [], False
    for one in usable:
        if one["class"] == "fit":
            seen = True
        elif seen:
            violations.append({"q": one["q"], "case": one["case"], "n": one["n"], "startup": one["startup"],
                               "stack_kb": one["stack_kb"]})
    counts = {}
    for one in runs:
        counts[one["class"]] = counts.get(one["class"], 0) + 1
    out = {"points": len(runs), "usable": len(usable), "classes_count": counts,
           "classes": "".join("F" if one["class"] == "fit" else "o" for one in usable),
           "monotone": not violations, "violations": violations[:40]}
    first = next((i for i, one in enumerate(usable) if one["class"] == "fit"), None)
    if not usable:
        out["flip"] = "no usable run"
    elif first is None:
        out["flip"] = "none: every point over (the budget lies below the window)"
    elif first == 0:
        out["flip"] = "none: every point fits (the budget lies above the window)"
    else:
        fit, fail = usable[first], usable[first - 1]
        out.update({"flip": "bracket", "q_fit": fit["q"], "q_fail": fail["q"], "bracket": [fit["q"], fail["q"] - 1],
                    "width": fail["q"] - fit["q"],
                    "fit_point": {"n": fit["n"], "m": fit["m"], "leaf": fit["leaf"], "startup": fit["startup"],
                                  "stack_kb": fit["stack_kb"]},
                    "fail_point": {"n": fail["n"], "m": fail["m"], "leaf": fail["leaf"], "startup": fail["startup"],
                                   "stack_kb": fail["stack_kb"]}})
    return out


def pm_sweep(key, pname, plist, context):
    """A linear sweep of `plist` (descending q), one run per point, never retried."""
    entry = {"param": pname, "context": context, "planned": len(plist)}
    result["pm_sweeps"][key] = entry
    runs = []
    for point in plist:
        if over():
            result["not_reached"].append(f"{key}: {len(plist) - len(runs)} points")
            break
        runs.append(pm_run(pname, point, context, key))
    entry["complete"] = len(runs) == len(plist)
    entry["analysis"] = pm_analyze(runs)
    entry["runs_q"] = [[one["q"], one["class"]] for one in runs]
    save()
    a = entry["analysis"]
    log(f"{key}: {a['classes']} flip={a['flip']} monotone={a['monotone']} bracket={a.get('bracket')}")
    return entry


def pm_control(a, c):
    """The control twin against the primary: it passes when it is monotone with a bracket and the two brackets intersect
    and nest (one contains the other)."""
    out = {"criterion": "control monotone with a bracket; brackets intersect and one contains the other",
           "control_flip": c.get("flip"), "control_monotone": c["monotone"], "control_bracket": c.get("bracket")}
    if c.get("flip") != "bracket" or a.get("flip") != "bracket" or not c["monotone"]:
        out["passes"] = False
        return out
    (alo, ahi), (clo, chi) = a["bracket"], c["bracket"]
    out["intersect"] = max(alo, clo) <= min(ahi, chi)
    out["nested"] = (alo <= clo and chi <= ahi) or (clo <= alo and ahi <= chi)
    out["passes"] = out["intersect"] and out["nested"]
    return out


def pm_relation(bracket, interval):
    """inside / partial / disjoint of the bracket [lo, hi] against the interval the rows allow (None ends are open)."""
    if interval is None:
        return "no rows"
    lo, hi = bracket
    low = -math.inf if interval[0] is None else interval[0]
    high = math.inf if interval[1] is None else interval[1]
    if hi < low or lo > high:
        return "disjoint"
    if lo >= low and hi <= high:
        return "inside"
    return "partial"


def pm_verdict(pname, design, prim, ctl):
    a, c = prim["analysis"], ctl["analysis"]
    control = pm_control(a, c)
    tolerance = design["tolerance"]
    out = {"model": design["model"], "bracket": a.get("bracket"), "resolution": a.get("width"),
           "resolution_design": design["resolution"], "tolerance": tolerance, "tolerance_reason": design["tolerance_reason"],
           "rows_interval": design["rows_interval"], "control": control, "monotone": a["monotone"],
           "complete": prim["complete"] and ctl["complete"], "verdict": None, "adopted": None}
    if not out["complete"]:
        out["verdict"], out["reason"] = "not-reached", "a sweep ran out of the time budget"
    elif not a["monotone"] or not c["monotone"]:
        out["verdict"], out["reason"] = "non-monotone", "primary" if not a["monotone"] else "control"
    elif a.get("flip") != "bracket":
        out["verdict"], out["reason"] = "unresolved", a.get("flip")
    elif not control["passes"]:
        out["verdict"], out["reason"] = "control-failed", "the control bracket does not intersect and nest with the primary"
    elif a["width"] >= tolerance:
        out["verdict"], out["reason"] = "unresolved", f"resolution {a['width']} is not finer than the tolerance {tolerance}"
    else:
        relation = pm_relation(a["bracket"], design["rows_interval"])
        out["vs_rows"] = relation
        if relation == "partial":
            out["verdict"], out["reason"] = "unresolved", "the bracket partly overlaps the interval the rows allow"
        else:
            out["verdict"], out["adopted"] = "adopt", a["bracket"][1]
            out["contradicts_rows"] = relation == "disjoint"
    if a.get("bracket"):
        out["hi_minus_model"] = a["bracket"][1] - design["model"]
    return out


def pm_measure(pname):
    design = pm_param_plan(HOST, pname, SS0, keep_points=True)
    lists = design.pop("point_lists")
    prim = pm_sweep(f"{pname}/toplevel/primary", pname, lists["primary"], "toplevel")
    ctl = pm_sweep(f"{pname}/toplevel/control", pname, lists["control"], "toplevel")
    PM_DONE[pname] = (prim, ctl)
    result["results"]["params"][pname] = pm_verdict(pname, design, prim, ctl)
    save()
    one = result["results"]["params"][pname]
    log(f"{pname}: {one['verdict']} bracket={one['bracket']} adopted={one['adopted']} model={one['model']}")


def pm_phase(phase):
    for pname, spec in PM_PARAMS.items():
        if spec["phase"] != phase:
            continue
        try:
            pm_measure(pname)
        except (Exception, SystemExit) as error:
            stop(f"{pname}: {error!r}")
            result["results"]["params"][pname] = {"model": PM_BUDGETS[HOST][pname], "verdict": "not-reached",
                                                  "reason": f"leg failed: {error!r}"[:300], "adopted": None}
        save()


def pm_offset_verdict(context, prim, ctl, top):
    value, lo, hi = PM_CONTEXTS[HOST][context]
    out = {"model": value, "interval": [lo, hi], "tolerance": 16, "verdict": None, "adopted": None,
           "bracket_context_q": prim["analysis"].get("bracket")}
    a_c, c_c = prim["analysis"], ctl["analysis"]
    out["control"] = pm_control(a_c, c_c)
    if top is None:
        out["verdict"], out["reason"] = "not-reached", "no toplevel reference (the effective_copy leg did not run)"
        return out
    a_t, c_t = top[0]["analysis"], top[1]["analysis"]
    out["control_toplevel"] = pm_control(a_t, c_t)
    if not (prim["complete"] and ctl["complete"] and top[0]["complete"] and top[1]["complete"]):
        out["verdict"], out["reason"] = "not-reached", "a sweep ran out of the time budget"
    elif not all(one["monotone"] for one in (a_c, c_c, a_t, c_t)):
        out["verdict"], out["reason"] = "non-monotone", "a context or toplevel sweep is not monotone"
    elif a_c.get("flip") != "bracket" or a_t.get("flip") != "bracket":
        out["verdict"], out["reason"] = "unresolved", f"context {a_c.get('flip')}; toplevel {a_t.get('flip')}"
    elif not (out["control"]["passes"] and out["control_toplevel"]["passes"]):
        out["verdict"], out["reason"] = "control-failed", "the context or the toplevel control does not agree"
    if a_c.get("flip") == "bracket" and a_t.get("flip") == "bracket":
        strict = [a_c["q_fit"] - (a_t["q_fail"] - 1), (a_c["q_fail"] - 1) - a_t["q_fit"]]
        d = a_c["q_fit"] - a_t["q_fit"]
        aligned = a_c["q_fail"] - a_t["q_fail"] == d and a_c["width"] <= 16 and a_t["width"] <= 16
        out.update({"bracket": strict, "resolution": strict[1] - strict[0], "aligned_value": d, "aligned": aligned})
        if out["verdict"] is None:
            if not aligned:
                out["verdict"], out["reason"] = "unresolved", "the brackets are wider than 16 or not aligned"
            else:
                out["verdict"], out["adopted"] = "adopt", d
                out["outside_model_interval"] = not lo <= d <= hi
    return out


def pm_offsets():
    top = PM_DONE.get("effective_copy")
    offsets = result["results"]["offsets"]
    if top is not None:
        a_t, control = top[0]["analysis"], pm_control(top[0]["analysis"], top[1]["analysis"])
        good = top[0]["complete"] and top[1]["complete"] and a_t["monotone"] and a_t.get("flip") == "bracket" and control["passes"]
        offsets["toplevel"] = {"model": 0, "reference": "effective_copy toplevel sweeps", "control": control,
                               "bracket_q": a_t.get("bracket"), "verdict": "adopt" if good else "unresolved",
                               "adopted": 0 if good else None}
    else:
        offsets["toplevel"] = {"model": 0, "verdict": "not-reached", "adopted": None, "reason": "effective_copy did not run"}
    for context in OFFSET_CONTEXTS:
        try:
            primary, control, _ = pm_offset_points(HOST, context, SS0)
            prim = pm_sweep(f"offset/{context}/primary", "effective_copy", primary, context)
            ctl = pm_sweep(f"offset/{context}/control", "effective_copy", control, context)
            offsets[context] = pm_offset_verdict(context, prim, ctl, top)
        except (Exception, SystemExit) as error:
            stop(f"offset {context}: {error!r}")
            offsets[context] = {"model": PM_CONTEXTS[HOST][context][0], "verdict": "not-reached", "adopted": None,
                                "reason": f"leg failed: {error!r}"[:300]}
        save()


def pm_child_exit():
    """child_exit: a coarse sweep over I_child, then a fine sweep and its control around the first coarse fit. Each run is its
    own process with its own sampled room, so every run is a one-sided bound on its own offset; the bounds are reported and
    checked against I_child, and no single value is adopted."""
    _, lo, hi = PM_CONTEXTS[HOST]["child_exit"]
    b = PM_BUDGETS[HOST]["effective_copy"]
    qlo, qhi = b + lo - CHILD_PAD, b + hi + CHILD_PAD
    out = {"interval_I_child": [lo, hi], "verdict": "unresolved", "adopted": None,
           "reason": "a child process exit gives each process its own room, so one run per point measures one sample per "
                     "run, not a fixed offset"}
    result["results"]["child_exit"] = out
    primary, _, _ = pm_points(HOST, "effective_copy", qlo, qhi, SS0)
    coarse = pm_sweep("offset/child_exit/coarse", "effective_copy", pm_coarse(primary, qlo), "child_exit")
    out["coarse"] = coarse["analysis"]
    samples = list(coarse["runs_q"])
    fits = [q for q, c in samples if c == "fit"]
    if fits:
        center = max(fits)
        done = {q for q, _ in samples}
        fine_p, fine_c, _ = pm_points(HOST, "effective_copy", center - CHILD_FINE, center + CHILD_FINE, SS0)
        fine = pm_sweep("offset/child_exit/fine", "effective_copy", [one for one in fine_p if one["q"] not in done], "child_exit")
        fine_ctl = pm_sweep("offset/child_exit/fine_control", "effective_copy", fine_c, "child_exit")
        out["fine"], out["fine_control"] = fine["analysis"], pm_control(fine["analysis"], fine_ctl["analysis"])
        samples += fine["runs_q"] + fine_ctl["runs_q"]
    fits = [q for q, c in samples if c == "fit"]
    overs = [q for q, c in samples if c == "over"]
    out["q_fit_max"] = max(fits) if fits else None
    out["q_over_min"] = min(overs) if overs else None
    top = PM_DONE.get("effective_copy")
    a_t = top[0]["analysis"] if top else {}
    if a_t.get("flip") == "bracket":
        if fits:
            out["some_run_offset_at_least"] = max(fits) - (a_t["q_fail"] - 1)
            out["falsifies_I_hi"] = out["some_run_offset_at_least"] > hi
        if overs:
            out["some_run_offset_below"] = min(overs) - a_t["q_fit"]
            out["falsifies_I_lo"] = out["some_run_offset_below"] <= lo
    else:
        out["bounds"] = "no toplevel bracket: the bounds are given in q only"
    save()


for _name, _reason in PM_UNMEASURABLE[HOST].items():
    result["results"]["params"][_name] = {"model": PM_BUDGETS[HOST][_name], "verdict": "unmeasurable", "reason": _reason,
                                          "bracket": None, "control": None, "resolution": None, "tolerance": None,
                                          "adopted": None}
save()

# ---- phase 1: the budgets the discovery rows decide on, then the caller-context offsets ----
pm_phase(1)
try:
    pm_offsets()
except (Exception, SystemExit) as error:
    stop(f"offsets: {error!r}")
save()

# ---- the S legs (diagsa) ----
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
    # Ruling 50 rule 2: an int midpoint (it was the float (lo+hi)/2, which reached range() and raised TypeError on darwin).
    s_mid = (s_model["S_greater_than"] + s_model["S_at_most"]) // 2 if s_model else S_MODEL
    save()
    log(f"adopt: {decision} S={adopted} bracket=({lo}, {hi}] legs={used}")

    # 3. Secondary (discovery only, not used for adoption): the same M_arr sweep in the timer and immediate contexts. Each
    #    context records its own failure and the next leg runs.
    contexts = {}
    result["results"]["contexts"] = contexts
    for context in ("timer", "immediate"):
        try:
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
        except (Exception, SystemExit) as error:
            stop(f"context {context}: {error!r}")
            contexts[context] = {"failed": repr(error)[:300]}
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

# ---- phase 2: the remaining budgets, then child_exit ----
pm_phase(2)
try:
    pm_child_exit()
except (Exception, SystemExit) as error:
    stop(f"child_exit: {error!r}")
    if result["results"]["child_exit"] is None:
        result["results"]["child_exit"] = {"verdict": "not-reached", "adopted": None, "reason": f"leg failed: {error!r}"[:300]}

result["results"]["verdicts"] = {name: one.get("verdict") for name, one in result["results"]["params"].items()}
save()
shutil.rmtree(D["SCRATCH"], ignore_errors=True)
log(f"done in {result['seconds']} s")
