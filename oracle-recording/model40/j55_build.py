"""Job 55 rows (Ruling 46 rule 2, Ruling 47) in the model40 scan group format, one group per (host, call, case, primes).

usage: python3 -I j55_build.py ARTIFACTS SCAN_JSONL OUT_JSONL

ARTIFACTS holds the downloaded native-mg37-<host>-second artifact folders; every j55/rows.jsonl under it is read (the
rows of j55:rows, one per context, family, level and prime). SCAN_JSONL is the job 54 scan file, whose rows give the
interval each context scans for a host and call (the same I_c as val45_build.py, and I_child for child_exit). A group
holds the full recorded answer set of each context (every answer any of its runs gave, with its run count), as the
targets [answer, stage, role] of that context (stage None, role by the REF-flagged run as roles43 has it); the engine's answer at the frozen offset must be one of them. The row format is val45.jsonl's
plus "primes" (the prime files in order, [] for P0) and the labels "family", "rel", "level" and "prime".
"""
import collections, glob, hashlib, json, os, sys

root, scan_path, out_path = sys.argv[1:4]
CONTEXTS = ("toplevel", "nested", "timer", "immediate", "io", "child_exit")
ANSWERS = ("effective", "exact", "machine_policy", "managed_source", "error")
IV = {}
for line in open(scan_path, encoding="utf-8"):
    r = json.loads(line)
    if r["call"] != "second":
        continue
    for s in r["scans"]:
        IV.setdefault((r["host"], r["call"], s["context"]), set()).add((s["low"], s["high"]))
if any(len(v) != 1 for v in IV.values()):
    raise SystemExit("one context scans two intervals")

paths = sorted(glob.glob(os.path.join(root, "**", "j55", "rows.jsonl"), recursive=True))
if not paths:
    raise SystemExit(f"no j55/rows.jsonl under {root}")
groups = collections.OrderedDict()
for path in paths:
    for line in open(path, encoding="utf-8"):
        if not line.strip():
            continue
        row = json.loads(line)
        if row["call"] != "second" or row.get("wsl"):
            raise SystemExit(f"{path}: a row that is not a native second-call row")
        key = hashlib.sha256(json.dumps(["j55", row["host"], row["call"], row["case"], row["files"], row["startup"],
                                         row["primes"]]).encode()).hexdigest()[:16]
        group = groups.setdefault(key, {"key": key, "source": "j55", "name": row["case"], "host": row["host"],
                                        "call": "second", "startup": row["startup"], "files": row["files"],
                                        "primes": row["primes"], "family": row["family"], "rel": row["rel"],
                                        "level": row["level"], "prime": row["prime"], "answers": {}})
        if (group["family"], group["level"], group["prime"]) != (row["family"], row["level"], row["prime"]):
            raise SystemExit(f"one key names two rows: {row['case']}")
        if row["context"] in group["answers"]:
            raise SystemExit(f"{path}: {row['case']} {row['prime']} is recorded twice in {row['context']}")
        recorded = []
        for item in row["answers"]:
            answer = {k: item["answer"][k] for k in ANSWERS}
            if any(answer == other for other, _, _ in recorded):
                raise SystemExit(f"{row['case']} {row['context']}: one answer listed twice")
            recorded.append((answer, item["runs"], bool(item.get("ref"))))
        group["answers"][row["context"]] = recorded

counts = collections.Counter()
with open(out_path, "w", encoding="utf-8") as out:
    for g in groups.values():
        missing = [c for c in CONTEXTS if c not in g["answers"]]
        if missing:
            raise SystemExit(f"{g['name']} {g['prime']} on {g['host']} lacks the contexts {missing}")
        scans = []
        for context in CONTEXTS:
            low, high = next(iter(IV[(g["host"], "second", context)]))
            recorded = g["answers"][context]
            if not any(ref for _, _, ref in recorded):
                raise SystemExit(f"{g['name']} {g['prime']} on {g['host']}: the {context} scan has no REF-flagged run")
            # The roles43 rule: a target is "ref" when a REF-flagged run of the context gave its answer and its stage
            # (j55 traces no run, so every stage is None and the answer decides); every child_exit target is "alt".
            targets = [[answer, None, "ref" if context != "child_exit" and ref else "alt"]
                       for answer, _, ref in recorded]
            scans.append({"context": context, "low": low, "high": high, "all": False, "targets": targets,
                          "runs": [runs for _, runs, _ in recorded]})
        counts[g["host"]] += 1
        out.write(json.dumps({"key": g["key"], "source": "j55", "name": g["name"], "host": g["host"],
                              "call": "second", "startup": g["startup"], "files": g["files"],
                              "primes": g["primes"], "family": g["family"], "rel": g["rel"], "level": g["level"],
                              "prime": g["prime"], "scans": scans}) + "\n")
for host, number in sorted(counts.items()):
    print(host, number)
print("groups", len(groups), "files", len(paths))
