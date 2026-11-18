"""Validation rows for R43-3a from job 47 and job 53 second-call data, in the model40 scan40 group format.

usage: python3 -I build_val.py P SCAN_JSONL OUT_JSONL

Every context scans the interval the job 54 rows of SCAN_JSONL scan for that host and call (the same I_c, and
I_child for child_exit). A job 47 answer recorded without all five fields (child_exit on darwin and linux x64,
recorded before managed_source was kept) has no target in that context and is listed.

job 47: the native managed goldens of every host and context under call 'second' (one process per case, the
gate's prime is the case's own files). Each case is one group; each context's target is the answer that context
recorded, scanned over that context's I_c (ic40.json).
job 53: the band39 'third' rows under call 'second' (answers that were neither side's). The rows carry no context,
so every context scans its I_c for the answer; the row passes when any context gives it.
"""
import collections, glob, hashlib, json, os, sys

P, scan_path, out_path = sys.argv[1:4]
IV = {}
for line in open(scan_path):
    r = json.loads(line)
    for s in r["scans"]:
        IV.setdefault((r["host"], r["call"], s["context"]), set()).add((s["low"], s["high"]))
if any(len(v) != 1 for v in IV.values()):
    raise SystemExit("one context scans two intervals")
partial = []
CONTEXTS = ("toplevel", "nested", "timer", "immediate", "io", "child_exit")
LABEL = {"darwin arm64": "darwin-arm64", "linux aarch64": "linux-arm64", "linux x86_64": "linux-x64"}
HOST = {v: k for k, v in LABEL.items()}
ANSWERS = ("effective", "exact", "machine_policy", "managed_source", "error")


def interval(host, context):
    return next(iter(IV[(host, "second", context)]))


groups = collections.OrderedDict()
for label in ("darwin-arm64", "linux-arm64", "linux-x64"):
    host = HOST[label]
    for context in CONTEXTS:
        folder = os.path.join(P, "j47", "all", f"native-goldens-{label}-second-{context}", "goldens")
        name = "ts_managed_settings.json" if label == "darwin-arm64" else f"ts_managed_settings_{label.replace('-', '_')}.json"
        doc = json.load(open(os.path.join(folder, name)))
        for case in doc["cases"]:
            startup = case.get("startup_bytes", doc["startup_bytes"])
            files = case["files"]
            key = hashlib.sha256(json.dumps(["j47", host, case["name"], files, startup]).encode()).hexdigest()[:16]
            group = groups.setdefault(key, {"key": key, "source": "j47", "name": case["name"], "host": host,
                                            "call": "second", "startup": startup, "files": files, "targets": {}})
            if group["name"] != case["name"]:
                raise SystemExit(f"one key names {group['name']} and {case['name']}")
            if all(k in case for k in ANSWERS):
                group["targets"][context] = {k: case[k] for k in ANSWERS}
            else:
                partial.append([label, context, case["name"]])
for f in sorted(glob.glob(os.path.join(P, "j53art", "native-band39-*-second-*", "band39", "third.jsonl"))):
    for line in open(f):
        if not line.strip():
            continue
        row = json.loads(line)
        answer = row["answer"] if isinstance(row["answer"], dict) else json.loads(row["answer"])
        answer = {k: answer[k] for k in ANSWERS}
        key = hashlib.sha256(json.dumps(["j53", row["host"], row["case"], row["files"], row["startup"]]).encode()).hexdigest()[:16]
        group = groups.setdefault(key, {"key": key, "source": "j53", "name": row["case"], "host": row["host"],
                                        "call": "second", "startup": row["startup"], "files": row["files"],
                                        "answers": []})
        if answer not in group["answers"]:
            group["answers"].append(answer)
n = collections.Counter()
with open(out_path, "w", encoding="utf-8") as out:
    for g in groups.values():
        scans = []
        for context in CONTEXTS:
            low, high = interval(g["host"], context)
            if g["source"] == "j47":
                targets = [[g["targets"][context], None]] if context in g["targets"] else []
            else:
                targets = [[a, None] for a in g["answers"]]
            scans.append({"context": context, "low": low, "high": high, "all": False, "targets": targets})
        n[(g["source"], g["host"])] += 1
        out.write(json.dumps({"key": g["key"], "source": g["source"], "name": g["name"], "host": g["host"],
                              "call": "second", "startup": g["startup"], "files": g["files"], "scans": scans}) + "\n")
for k, v in sorted(n.items()):
    print(k, v)
print("groups", len(groups), "partial (no target)", len(partial), sorted({(a, b) for a, b, _ in partial}))
