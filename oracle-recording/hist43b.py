#!/usr/bin/env python3
"""Ruling 43, rule 6, second run: what the process history supplies to the second call of a merge family (discovery only).

usage: python3 -I hist43b.py LIB NODE PACKAGE OUT

hist43 showed that a completing prime ahead of the measured resolve moves the merge edge of the drop-in families by the
whole lazy-compile reserve and a caught prime by three levels. This run keeps the hist43 mechanics (one process per run,
the reference flags twice and the cross-check flags once, a case alone in its process) and finds, per variant and
family, the deepest level that still merges, by bisection and then a confirming window. It varies the depth of a
completing prime, the family of the prime, the V8 flags, the number of forced collections between prime and case, and
(with --log-function-events) which functions of the bundle were compiled. The output is evidence for the process-history
model (registration_b.md, written before the run) and is never a golden.
"""

import os
import sys

with open(os.path.abspath(__file__), encoding="utf-8") as handle:
    SELF = handle.read()
with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "hist43.py"), encoding="utf-8") as handle:
    BASE = handle.read()
BASE = BASE[: BASE.rindex("\nmain()")]
NS = {"__name__": "hist43b", "__file__": os.path.abspath(__file__)}
exec(compile(BASE, os.path.join(os.path.dirname(os.path.abspath(__file__)), "hist43.py"), "exec"), NS)
exec(compile(SELF[SELF.index("\n# ---- body ----\n") + 1 :], os.path.abspath(__file__), "exec"), NS)
sys.exit(0)

# ---- body ----
import time

START = time.monotonic()
BUDGET_SECONDS = 200 * 60
NO_FLUSH = "--no-flush-bytecode"
EXPOSE_GC = "--expose-gc"
LOG_FLAG = "--log-function-events"
PROBED = (NO_FLUSH, EXPOSE_GC, LOG_FLAG, "--trace-lazy")
LOG_EXTRAS = (LOG_FLAG, "--no-logfile-per-isolate")

# A drop-in file alone, deep: the measured call that runs only the `drop_b` continuation.
FAMILIES["dropb_only"] = lambda n: [USER, ["drop_b", n, 0, 0]]
ALL_FAMILIES = ALL4
LEVEL_AT = {"dropb_only": "drop_merge"}  # the family whose first-call edge centres the bracket


def centre_of(family):
    return PROFILE[LEVEL_AT.get(family, family)]


def P(family, how, amount=0, gc=0):
    """A prime: `family`'s document ("own" is the case's family) at the case's level (how "n"), at the edge plus `amount`
    (how "rel"), or at the level `amount` (how "abs"), then `gc` forced full collections."""
    return {"family": family, "how": how, "amount": amount, "options": "same", "gc": gc}


VARIANTS = {}


def add(name, call, extras, fams, primes, base, kind=None, at=None):
    """kind: what each prime must have done for the cell to count ("merged" or "dropped"); at: a fixed case level
    relative to the edge (a logging variant is run at one level, not bisected)."""
    VARIANTS[name] = dict(call=call, extra=list(extras), fams=list(fams), primes=primes, base=base, kind=kind, at=at)


DM, AR = "drop_merge", "drop_merge_arrays"
DA2 = (DM, AR)
add("B0", "first", (), ALL_FAMILIES, [], None)
# Run first so a deadline never drops them: they name the function behind the caught gain.
# Function events: which functions of the bundle were compiled, at one case level (the edge minus 6, merged everywhere).
add("LGf", "first", LOG_EXTRAS, (DM,), [], None, at=-6)
add("LG0", "second", LOG_EXTRAS, (DM,), [], None, at=-6)
add("LGc", "second", LOG_EXTRAS, (DM,), [P("own", "rel", 30)], None, "dropped", at=-6)
add("LGa", "second", LOG_EXTRAS, (DM,), [P(AR, "rel", 10)], None, "dropped", at=-6)
add("LGm", "second", LOG_EXTRAS, (DM,), [P("main_merge", "rel", 10)], None, "dropped", at=-6)
add("LGw", "second", LOG_EXTRAS, (DM,), [P("own", "rel", -8)], None, "merged", at=-6)
add("B1", "second", (), DA2, [], "B0")
add("W2", "second", (), ALL_FAMILIES, [P("own", "rel", -8)], "B0", "merged")
# The depth sweep of a completing own-family prime (qq, the first delete of the lodash Stack, runs only from depth 199 on).
for depth in (150, 196, 198, 199, 200, 201, 203):
    add(f"D{depth}", "second", (), DA2, [P("own", "abs", depth)], "B0", "merged")
for depth in (250, 255, 257, 511, 513, 600, 1023, 1025):
    add(f"D{depth}", "second", (), (DM,), [P("own", "abs", depth)], "B0", "merged")
add("DE30", "second", (), DA2, [P("own", "rel", -30)], "B0", "merged")
# Caught own-family primes at three distances past the edge.
for amount in (1, 30, 300):
    add(f"C{amount}", "second", (), DA2, [P("own", "rel", amount)], "B0", "dropped")
# Cross-family completing primes at the edge minus 8.
add("X1", "second", (), (DM,), [P(AR, "rel", -8)], "B0", "merged")
add("X2", "second", (), (AR,), [P(DM, "rel", -8)], "B0", "merged")
add("X3", "second", (), DA2, [P("main_merge", "rel", -8)], "B0", "merged")
add("X4", "second", (), DA2, [P("admin_inner", "rel", -8)], "B0", "merged")
# The caught prime followed by a case that is a drop-in file alone (nothing after the catch).
add("DBb", "first", (), ("dropb_only",), [], None)
add("DB1", "second", (), ("dropb_only",), [], "DBb")
add("DBc", "second", (), ("dropb_only",), [P(DM, "rel", 30)], "DBb", "dropped")
add("DBw", "second", (), ("dropb_only",), [P(DM, "rel", -8)], "DBb", "merged")
# The lifetime of the gain: forced collections between prime and case, with and without bytecode flushing.
add("G0", "first", (EXPOSE_GC,), (DM,), [], None)
add("N0", "first", (NO_FLUSH,), (DM,), [], None)
add("GN0", "first", (EXPOSE_GC, NO_FLUSH), (DM,), [], None)
add("N", "second", (NO_FLUSH,), (DM,), [P("own", "rel", -8)], "N0", "merged")
for count in (1, 4, 16):
    add(f"G{count}", "second", (EXPOSE_GC,), (DM,), [P("own", "rel", -8, gc=count)], "G0", "merged")
add("GN16", "second", (EXPOSE_GC, NO_FLUSH), (DM,), [P("own", "rel", -8, gc=16)], "GN0", "merged")


def prime_level(family, prime, n):
    if prime["how"] == "n":
        return n
    if prime["how"] == "abs":
        return prime["amount"]
    return PROFILE[LEVEL_AT.get(family, family)] + prime["amount"]


def check_flags(flags, extras, log_path):
    wanted = REF + list(extras) + ([f"--logfile={log_path}"] if log_path else [])
    cross = CROSS + list(extras) + ([f"--logfile={log_path}"] if log_path else [])
    if list(flags) not in (wanted, cross):
        raise RuntimeError(f"a probe would start Node with {flags!r}, not the reference or cross-check flags plus {extras!r}")


def build(root, family, n, primes):
    for folder in ("home/.claude", "work/.claude"):
        os.makedirs(os.path.join(root, folder), exist_ok=True)
    case_files = FAMILIES[family](n)
    write(os.path.join(root, PLACES["user"]), content(USER))
    sets = [disk(case_files)]
    prime_specs, prime_lengths = [], []
    for prime in primes:
        family_of = family if prime["family"] == "own" else prime["family"]
        files = FAMILIES[family_of](prime_level(family_of, prime, n))
        sets.append(disk(files))
        prime_lengths.append(deep_length(files))
        prime_specs.append(prime)
    paths = sorted(set().union(*sets))
    full = [{path: one.get(path) for path in paths} for one in sets]
    spec = {
        "root": root,
        "platform": sys.platform,
        "env": {"HOME": os.path.join(root, "home")},
        "deep": True,
        "rejections": True,
        "primes": [{"name": f"prime{i}", "options": "same", "gc": prime["gc"], "files": full[i + 1]} for i, prime in enumerate(prime_specs)],
        "cases": [{"name": "case", "options": {"cwd": os.path.join(root, "work")}, "files": full[0]}],
    }
    return json.dumps(spec), deep_length(case_files), prime_lengths


def read_log(path):
    """The function events of the bundle in a V8 log: sorted unique "kind:start:end:name" strings, or None if no log."""
    if not os.path.exists(path):
        return None
    scripts, events = {}, set()
    with open(path, encoding="utf-8", errors="replace") as handle:
        for line in handle:
            parts = line.rstrip("\n").split(",")
            if parts[0] == "script-details" and len(parts) > 2:
                scripts[parts[1]] = parts[2]
            elif parts[0] == "function" and len(parts) > 7:
                events.add((parts[1], parts[2], parts[3], parts[4], ",".join(parts[7:])))
    keep = {script for script, name in scripts.items() if name.endswith("sdk.mjs")}
    return sorted(f"{kind}:{start}:{end}:{name}" for kind, script, start, end, name in events if script in keep)


def one_run(variant, family, n, flags, call):
    config = VARIANTS[variant]
    root = os.path.join(SCRATCH, unique("h"))
    spec_path = root + ".json"
    log_path = root + ".v8.log" if LOG_FLAG in config["extra"] else None
    flags = list(flags) + ([f"--logfile={log_path}"] if log_path else [])
    check_flags(flags, config["extra"], log_path)
    record = {"variant": variant, "family": family, "n": n, "flags": "ref" if flags[: len(REF)] == REF else "cross", "call": call}
    try:
        text, doc_length, prime_lengths = build(root, family, n, config["primes"] if call == "second" else [])
        with open(spec_path, "w", encoding="utf-8") as handle:
            handle.write(text)
        command = [NODE, *flags, ORACLE, PACKAGE, spec_path]
        env = padded_env(MANAGED["pad"], command, STARTUP)
        GATE["in_call"](call)
        with tempfile.TemporaryFile() as errors:
            try:
                process = spawn(command, env, stdout=subprocess.PIPE, stderr=errors, timeout=600)
            except Exception as error:  # a gate violation or refusal is a result of this run, not of the driver
                record["status"] = "gate"
                record["error"] = f"{type(error).__name__}: {str(error)[:300]}"
                return record
            errors.seek(0)
            stderr = errors.read().decode("utf-8", "replace")
        if process.returncode != 0:
            found = re.search(r"^(\w*Error): (.*)$", stderr.replace("\r\n", "\n"), re.M)
            record["status"] = "fail"
            record["error"] = (found.group(1) + ": " + found.group(2)[:200]) if found else stderr[-300:]
            return record
        entry = json.loads(process.stdout.decode("utf-8"))[0]
        rejected = entry.get("error", {}).get("name") if "error" in entry else None
        effective = None if rejected else len(single(entry["result"])["effective"])
        record.update(status="ok", doc_length=doc_length, effective_length=effective, rejected=rejected,
                      cls=classify(effective, rejected, doc_length))
        primes = []
        if os.path.exists(spec_path + ".primes.json"):
            with open(spec_path + ".primes.json", encoding="utf-8") as handle:
                primes = json.load(handle)
        record["primes"] = [
            {"thrown": one["thrown"], "effective_length": one["effective_length"], "doc_length": length,
             "cls": classify(one["effective_length"], one["thrown"], length)}
            for one, length in zip(primes, prime_lengths)
        ]
        if log_path:
            record["functions"] = read_log(log_path)
        return record
    finally:
        shutil.rmtree(root, ignore_errors=True)
        for suffix in ("", ".native.json", ".primes.json"):
            if os.path.exists(spec_path + suffix):
                os.remove(spec_path + suffix)
        if log_path and os.path.exists(log_path):
            os.remove(log_path)


def triple(extras):
    extras = list(extras)
    return [REF + extras, REF + extras, CROSS + extras]


def cell(variant, family):
    """One (variant, family): the case level where it stops merging, by bisection on single reference runs from the
    first-call edge, then three runs (two reference, one cross-check) at each level of a window round the result. A
    logging variant is run at one level, twice."""
    if time.monotonic() - START > BUDGET_SECONDS:
        return [], "deadline"
    config = VARIANTS[variant]
    call, out = config["call"], []
    centre = centre_of(family)
    if config["at"] is not None:
        for index in range(2):
            record = one_run(variant, family, centre + config["at"], REF + config["extra"], call)
            record["phase"] = "fixed"
            out.append(record)
        return out, "fixed"

    def at(n, flags, phase):
        record = one_run(variant, family, n, flags, call)
        record["phase"] = phase
        out.append(record)
        return record

    def merged_at(n):
        record = at(n, REF + config["extra"], "bisect")
        return record["cls"] == "merged" if record.get("status") == "ok" else None

    low = centre - 6 if family != "dropb_only" else centre - 60
    if merged_at(low) is not True:
        return out, "unbracketed_low"
    high = centre + 130
    answer = merged_at(high)
    if answer is True:
        high = centre + 400
        answer = merged_at(high)
    if answer is not False:
        return out, "unbracketed_high" if answer is True else "void"
    while high - low > 1:
        middle = (low + high) // 2
        answer = merged_at(middle)
        if answer is None:
            return out, "void"
        low, high = (middle, high) if answer else (low, middle)
    for width in (3, 8):
        classes = {}
        for n in range(low - width, low + width + 1):
            runs = [at(n, flags, f"window{width}") for flags in triple(config["extra"])]
            classes[n] = runs[0].get("cls")
        ordered = sorted(classes)
        if classes[ordered[0]] == "merged" and classes[ordered[-1]] != "merged":
            return out, "ok"
    return out, "window_miss"


def probe(flag):
    return subprocess.run([NODE, flag, "-e", "0"], capture_output=True).returncode == 0


def main_b():
    available = {flag: probe(flag) for flag in PROBED}
    log(f"flags {available}")
    skipped, work = {}, []
    for variant, config in VARIANTS.items():
        missing = [flag for flag in config["extra"] if not available.get(flag)]
        if missing:
            skipped[variant] = "flag unavailable: " + ", ".join(missing)
            continue
        for family in config["fams"]:
            work.append((variant, family))
    log(f"{len(work)} cells")
    records, statuses = [], {}
    with concurrent.futures.ThreadPoolExecutor(max_workers=WORKERS) as pool:
        futures = [(variant, family, pool.submit(cell, variant, family)) for variant, family in work]
        for variant, family, future in futures:
            out, status = future.result()
            records.extend(out)
            statuses[f"{variant}/{family}"] = status
            log(f"{variant} {family} {status}")
    result = {
        "host": HOST,
        "node": subprocess.run([NODE, "-p", "JSON.stringify([process.version,process.versions.v8])"], capture_output=True).stdout.decode().strip(),
        "flags_available": available,
        "skipped": skipped,
        "profile": {family: PROFILE[family] for family in ALL4},
        "statuses": statuses,
        "variants": {name: {key: config[key] for key in ("call", "extra", "base", "kind", "at", "primes")} | {"fams": config["fams"]} for name, config in VARIANTS.items()},
        "records": records,
    }
    try:
        result["gate"] = json.loads(json.dumps(GATE["TALLY"]))
    except (KeyError, TypeError, ValueError):
        pass
    with open(OUT, "w", encoding="utf-8") as handle:
        json.dump(result, handle, sort_keys=True)
    log(f"wrote {len(records)} runs to {OUT}")


main_b()
