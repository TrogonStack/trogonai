import json, sys, glob, hashlib, collections
P = sys.argv[1]
ic = json.load(open(P + "/ic40.json"))
hostmap = {"darwin arm64": "darwin-arm64", "linux aarch64": "linux-arm64", "linux x86_64": "linux-x64"}
rows = []
for f in sorted(glob.glob(P + "/j53art/*/band39/third.jsonl")):
    for line in open(f):
        if line.strip():
            rows.append(json.loads(line))
print("rows", len(rows), "keys", sorted(rows[0].keys()))
groups = collections.OrderedDict()
for r in rows:
    k = (r["host"], r["call"], json.dumps(r["files"]), r["startup"])
    groups.setdefault(k, []).append(r)
out = open(P + "/model40/scan53.jsonl", "w")
for (host, call, files, startup), rs in groups.items():
    # The union of every context's I_c for this host and call, so the scan covers each context's offsets.
    ivs = [v["interval"] for v in ic["intervals"][hostmap[host]][call].values()]
    iv = [min(v[0] for v in ivs), max(v[1] for v in ivs)]
    key = hashlib.sha256(json.dumps([host, call, files, startup]).encode()).hexdigest()[:16]
    out.write(json.dumps({"key": key, "host": host, "call": call, "files": json.loads(files), "startup": startup, "low": iv[0], "high": iv[1]}) + "\n")
print("groups", len(groups))
for h in hostmap.values():
    for c in ("first", "second"):
        print(h, c, {ctx: ic["intervals"][h][c][ctx]["interval"] for ctx in ic["intervals"][h][c]})
