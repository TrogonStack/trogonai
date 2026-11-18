#!/usr/bin/env python3
"""Scores the [replay54] artifacts against registration.md.

usage: python3 -I analyze_r.py ARTDIR OUT.json [JOB54ART]

ARTDIR holds the downloaded artifacts `native-replay54-<host>-<call>/replay54.json` and `r3.json`. JOB54ART holds job 54's
downloaded artifacts (`native-band40-<host>-<call>-5/r3.json`, shard 5). Their content is data: it is parsed, never run.
"""

import glob
import json
import os
import sys

ART, OUT = sys.argv[1:3]
JOB54ART = sys.argv[3] if len(sys.argv) > 3 else None
# Job 54's recorded edges (the registered values): first call, second call.
JOB54 = {"darwin arm64": (1241, 1244), "linux aarch64": (1249, 1252), "linux x86_64": (1304, 1307)}
BWE = "813217:813274"


def edge(levels, leg):
    """The last level whose answers are all near, with every level below near and every level above far; None when the window
    is not such a step or repeats disagree."""
    order = sorted(int(key) for key in levels)
    classes = {}
    for level in order:
        seen = {one["class"] for one in levels[str(level)][leg]}
        if len(seen) != 1:
            return None, f"repeats disagree at level {level}: {sorted(seen)}"
        classes[level] = seen.pop()
    last = None
    for level in order:
        if classes[level] == "near":
            if last is not None and last != level - 1:
                return None, "near answers are not contiguous"
            last = level
    if last is None:
        return None, "no near answer"
    if any(classes[level] != "far" for level in order if level > last) or any(classes[level] != "near" for level in order if level <= last):
        return None, f"not a single step: {classes}"
    return last, None


def accounts(levels):
    seen = set()
    for record in levels.values():
        for leg in ("U", "G"):
            for one in record[leg]:
                account = one.get("account")
                seen.add((leg, None if not account else account["startup_bytes"]))
    return sorted(seen, key=str)


def stable(records, field):
    """The items every repeat names (a run that did not log is left out)."""
    sets = [set(one[field]) for one in records if field in one]
    return set.intersection(*sets) if sets else None


def union(records, field):
    sets = [set(one[field]) for one in records if field in one]
    return set.union(*sets) if sets else None


report = {"hosts": {}, "problems": []}
data = {}
for path in sorted(glob.glob(os.path.join(ART, "*", "replay54.json"))):
    with open(path, encoding="utf-8") as handle:
        run = json.load(handle)
    data[(run["host"], run["call"])] = run
for host, (first_edge, second_edge) in JOB54.items():
    entry = report["hosts"].setdefault(host, {"job54": {"first": first_edge, "second": second_edge}})
    for call, expected in (("first", first_edge), ("second", second_edge)):
        run = data.get((host, call))
        if run is None:
            entry[call] = {"missing": True}
            report["problems"].append(f"{host} {call}: no replay54.json")
            continue
        levels = run["levels"]
        unlogged, why_u = edge(levels, "U")
        logged, why_g = edge(levels, "G")
        startups = accounts(levels)
        entry[call] = {"located": run.get("located"), "stops": run.get("stops"), "seconds": run.get("seconds"),
                       "edge_unlogged": unlogged, "edge_unlogged_why": why_u, "edge_logged": logged, "edge_logged_why": why_g,
                       "startup_bytes_seen": startups, "classes": {level: {"U": [one["class"] for one in rec["U"]],
                                                                              "G": [one["class"] for one in rec["G"]]}
                                                                    for level, rec in sorted(levels.items(), key=lambda kv: int(kv[0]))},
                       "P-R1_unlogged": unlogged == expected, "P-R1_logged": logged == expected,
                       "shift_located": (run.get("located") or {}).get("shift"),
                       "first_call_edge_from_profile": run.get("profile_level")}
    first = entry.get("first", {})
    entry["P-R0_neutral"] = bool(first.get("edge_unlogged") == first_edge and first.get("edge_logged") == first_edge
                                 and all(item[1] == 4384 for item in first.get("startup_bytes_seen", []) if item[1] is not None))

# The exact leg: job 54's own r3 step (same tree, command line and shard), compared edge by edge with job 54's shard 5 output.
def band_edges(path):
    with open(path, encoding="utf-8") as handle:
        unit = (json.load(handle).get("units") or {}).get("band40:goldens") or {}
    return unit.get("call"), {pid: [e.get("near_level"), e.get("far_level"), e.get("axis")]
                              for pid, e in (unit.get("edges") or {}).items() if "near_level" in e}


report["exact"] = {}
for path in sorted(glob.glob(os.path.join(ART, "native-replay54-*", "r3.json"))):
    label = os.path.basename(os.path.dirname(path))[len("native-replay54-"):]
    call_seen, edges = band_edges(path)
    row = {"call": call_seen, "edges": edges}
    if JOB54ART:
        ref = os.path.join(JOB54ART, "native-band40-%s-5" % label, "r3.json")
        if os.path.exists(ref):
            ref_call, ref_edges = band_edges(ref)
            row["job54_call"] = ref_call
            row["differs"] = {pid: {"replay": edges.get(pid), "job54": ref_edges.get(pid)}
                              for pid in sorted(set(edges) | set(ref_edges)) if edges.get(pid) != ref_edges.get(pid)}
            row["P-R1_exact"] = bool(ref_edges) and call_seen == ref_call and not row["differs"]
        else:
            report["problems"].append("no job 54 reference for " + label)
    report["exact"][label] = row


# P-R2, scored only for hosts that passed neutrality; the diffs are always listed.
for host, entry in report["hosts"].items():
    first, second = data.get((host, "first")), data.get((host, "second"))
    if not first or not second:
        continue
    edge_level = first.get("profile_level")
    names = {**first.get("names", {}), **second.get("names", {})}
    heads = {**first.get("heads", {}), **second.get("heads", {})}

    def describe(items):
        out = {}
        for item in sorted(items):
            kind, start, end = item.split(":")
            span = f"{start}:{end}"
            out.setdefault(span, {"name": names.get(span), "head": heads.get(span), "kinds": []})["kinds"].append(kind)
        return out

    def phase(run, level, which):
        records = run["levels"].get(str(level), {}).get("G", [])
        return stable(records, which)

    levels = {}
    for level in range(edge_level, edge_level + 5):
        row = {}
        row["first_call_case"] = phase(first, level, "first")
        row["second_call_prime"] = phase(second, level, "first")
        row["second_call_case"] = phase(second, level, "after")
        row["second_call_prime_unstable"] = None
        pu = union(second["levels"].get(str(level), {}).get("G", []), "first")
        ps = row["second_call_prime"]
        if pu is not None and ps is not None:
            row["second_call_prime_unstable"] = sorted(pu - ps)[:20]
        levels[level] = row
    d_sets, q_set = {}, None
    for level in (edge_level + 3, edge_level + 4):
        a, b = levels[level]["first_call_case"], levels[level]["second_call_case"]
        d_sets[level] = None if a is None or b is None else a - b
    p3, p4 = levels[edge_level + 3]["second_call_prime"], levels[edge_level + 4]["second_call_prime"]
    q_set = None if p3 is None or p4 is None else p3 - p4
    d_all = None
    if all(value is not None for value in d_sets.values()):
        d_all = set.intersection(*d_sets.values())
    both = None if d_all is None or q_set is None else d_all & q_set
    spans = None if both is None else {item.split(":", 1)[1] for item in both}
    entry["P-R2"] = {
        "scored": bool(entry.get("P-R0_neutral")),
        "edge_level": edge_level,
        "D_sizes": {str(level): None if value is None else len(value) for level, value in d_sets.items()},
        "Q_size": None if q_set is None else len(q_set),
        "D_and_Q": None if both is None else describe(both),
        "bwe_in_D_and_Q": None if spans is None else BWE in spans,
        "D_and_Q_at_most_5_functions": None if spans is None else len(spans) <= 5,
        "per_level": {str(level): {
            "first_call_case": None if row["first_call_case"] is None else describe(row["first_call_case"] - (row["second_call_case"] or set())),
            "prime_not_in_next_level_prime": None if level == edge_level + 4 or row["second_call_prime"] is None or levels[level + 1]["second_call_prime"] is None
            else describe(row["second_call_prime"] - levels[level + 1]["second_call_prime"]),
            "prime_count": None if row["second_call_prime"] is None else len(row["second_call_prime"]),
            "case_count_second": None if row["second_call_case"] is None else len(row["second_call_case"]),
            "case_count_first": None if row["first_call_case"] is None else len(row["first_call_case"]),
            "bwe_in_prime": None if row["second_call_prime"] is None else any(item.split(":", 1)[1] == BWE for item in row["second_call_prime"]),
            "bwe_in_second_case": None if row["second_call_case"] is None else any(item.split(":", 1)[1] == BWE for item in row["second_call_case"]),
            "bwe_in_first_case": None if row["first_call_case"] is None else any(item.split(":", 1)[1] == BWE for item in row["first_call_case"]),
            "unstable_prime_items": row["second_call_prime_unstable"],
        } for level, row in levels.items()},
    }

summary = {}
for host, entry in report["hosts"].items():
    summary[host] = {"P-R0": entry.get("P-R0_neutral"),
                     "P-R1": [entry.get(call, {}).get("P-R1_unlogged") for call in ("first", "second")]
                     + [entry.get(call, {}).get("P-R1_logged") for call in ("first", "second")],
                     "P-R2_bwe": entry.get("P-R2", {}).get("bwe_in_D_and_Q"),
                     "edges": {call: [entry.get(call, {}).get("edge_unlogged"), entry.get(call, {}).get("edge_logged")]
                               for call in ("first", "second")}}
summary["exact"] = {label: {"P-R1_exact": row.get("P-R1_exact"), "drop_merge": row["edges"].get("drop_merge"),
                            "differs": len(row.get("differs") or {})} for label, row in report["exact"].items()}
report["summary"] = summary
with open(OUT, "w", encoding="utf-8") as handle:
    json.dump(report, handle, indent=1, sort_keys=True)
print(json.dumps(summary, indent=1, sort_keys=True))
