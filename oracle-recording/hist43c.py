#!/usr/bin/env python3
"""Ruling 44, hist43c: the lazy-compile gain of a second call, named by function and replicated as job 54 ran it (discovery only).

usage: python3 -I hist43c.py LIB NODE PACKAGE OUT

hist43b showed that a prime changes the merge edge of the drop-in families by the lazy-compile reserve or not at all. This
run (registration_c.md, written before the run) (a) probes the V8 flags from `node --v8-options` and fails when a
registered variant is skipped, (b) names the functions of the bundle compiled during the prime and during the measured
call, split by a marker the oracle calls between them, in runs separate from every scored run, (c) scores the reference
flags and the cross-check flags each against their own first-call edge, (d) walks the caught distance of the prime level by
level, (e) replays the prime that job 54 ran before its measured second call, and (f) repeats the controls of hist43b.
The output is evidence and never a golden.
"""

import os
import sys

with open(os.path.abspath(__file__), encoding="utf-8") as handle:
    SELF = handle.read()
with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "hist43.py"), encoding="utf-8") as handle:
    BASE = handle.read()
BASE = BASE[: BASE.rindex("\nmain()")]
NS = {"__name__": "hist43c", "__file__": os.path.abspath(__file__)}
exec(compile(BASE, os.path.join(os.path.dirname(os.path.abspath(__file__)), "hist43.py"), "exec"), NS)
exec(compile(SELF[SELF.index("\n# ---- body ----\n") + 1 :], os.path.abspath(__file__), "exec"), NS)
sys.exit(0)

# ---- body ----
import glob
import time

START = time.monotonic()
BUDGET_SECONDS = 215 * 60
NO_FLUSH = "--no-flush-bytecode"
EXPOSE_GC = "--expose-gc"
LOG_FLAG = "--log-function-events"
SELF_TEST_FLAGS = ("--expose-gc", "--lazy")
MARKER = "phaseMarker43c"
DM, AR, MM, ADM = "drop_merge", "drop_merge_arrays", "main_merge", "admin_inner"
DA2 = (DM, AR)
ALL_FAMILIES = (DM, AR, MM, ADM)
PROBE = {"names": 0, "selftest": {}, "per_isolate": None, "log_selftest": None}


# ---- the flag probe ----
def norm_flag(flag):
    name = flag.lstrip("-").replace("_", "-")
    return name[3:] if name.startswith("no-") else name


def v8_flag_names():
    """Every flag `node --v8-options` lists, as its positive spelling: a flag is available under either spelling."""
    text = subprocess.run([NODE, "--v8-options"], capture_output=True, timeout=120).stdout.decode("utf-8", "replace")
    names = set()
    for line in text.splitlines():
        found = re.match(r"^\s{0,6}--([A-Za-z0-9_-]+)", line)
        if found:
            names.add(norm_flag(found.group(1)))
    return names


def probe_logging(names):
    """Choose how the V8 log is written: one file at --logfile with --no-logfile-per-isolate when that flag exists, else the
    isolate-prefixed file found by glob. A run of the chosen spelling must leave a log that names a function event."""
    PROBE["per_isolate"] = "logfile-per-isolate" in names
    if "log-function-events" not in names or "logfile" not in names:
        return False
    base = os.path.join(SCRATCH, unique("lg") + ".v8.log")
    try:
        command = [NODE, LOG_FLAG, *log_args(base), "-e", "function f43(){return 1}; f43()"]
        done = subprocess.run(command, capture_output=True, timeout=120)
        files = log_files(base)
        PROBE["log_selftest"] = {"returncode": done.returncode, "files": len(files), "bytes": sum(os.path.getsize(one) for one in files)}
        return done.returncode == 0 and bool(files)
    finally:
        for one in log_files(base):
            os.remove(one)


def log_args(path):
    return (["--no-logfile-per-isolate"] if PROBE["per_isolate"] else []) + [f"--logfile={path}"]


def log_files(path):
    return sorted(set(glob.glob(path) + glob.glob(os.path.join(os.path.dirname(path), "*" + os.path.basename(path)))))


# ---- the variants ----
def P(family, how, amount=0, gc=0):
    """A prime: `family`'s document ("own" is the case's family) at the case's level (how "n"), at that family's first-call
    edge plus `amount` (how "rel"), or at the level `amount` (how "abs"), then `gc` forced full collections."""
    return {"family": family, "how": how, "amount": amount, "options": "same", "gc": gc}


VARIANTS = {}
STAGES = []  # (stage name, [variant names]) in priority order


def add(stage, name, call, extras, fams, primes, base, kind=None, kinds=("ref",), at=None):
    """kind: what each prime must have done ("merged" or "dropped"); kinds: the flag sets scored; at: case levels of a logging
    variant, relative to the first-call edge (a logging variant is run twice at each, never scored)."""
    VARIANTS[name] = dict(call=call, extra=list(extras), fams=list(fams), primes=primes, base=base, kind=kind, kinds=list(kinds), at=at)
    for entry in STAGES:
        if entry[0] == stage:
            entry[1].append(name)
            return
    STAGES.append((stage, [name]))


BOTH = ("ref", "cross")
add("baselines", "B0", "first", (), ALL_FAMILIES, [], None, kinds=BOTH)
add("baselines", "B1", "second", (), DA2, [], "B0", kinds=BOTH)
add("baselines", "W2", "second", (), DA2, [P("own", "rel", -8)], "B0", "merged", kinds=BOTH)
add("baselines", "W2o", "second", (), (MM, ADM), [P("own", "rel", -8)], "B0", "merged")
# The main_merge prime that job 54 ran, at the levels round its own first-call edge.
for amount in (-8, -2, -1, 0, 1, 2, 3, 10):
    add("j54", f"M{amount}", "second", (), DA2, [P(MM, "rel", amount)], "B0")
# Function events (one level below every edge, merged everywhere, and the level that tells gain from no gain).
LOGS = (("LGf", "first", []), ("LG0", "second", []), ("LGw", "second", [P("own", "rel", -8)]),
        ("LGm", "second", [P(MM, "rel", -8)]), ("LGx", "second", [P(ADM, "rel", -8)]),
        ("LGd199", "second", [P("own", "abs", 199)]), ("LGd200", "second", [P("own", "abs", 200)]),
        ("LGd201", "second", [P("own", "abs", 201)]), ("LGc1", "second", [P("own", "rel", 1)]),
        ("LGc30", "second", [P("own", "rel", 30)]), ("LGa", "second", [P(AR, "rel", 10)]))
for name, call, primes in LOGS:
    add("lg", name, call, (LOG_FLAG,), DA2 if name in ("LGf", "LG0", "LGw", "LGd200", "LGc1", "LGc30") else (DM,), primes, None, at=(-6, 3))
# The caught distance, every level: C1 through C30, both drop families (the reference flags), and three on the cross-check flags.
for amount in range(1, 31):
    add("sweep", f"C{amount}", "second", (), DA2, [P("own", "rel", amount)], "B0", "dropped", kinds=BOTH if amount in (1, 3, 30) else ("ref",))
# The controls of hist43b, unchanged.
add("controls", "C300", "second", (), DA2, [P("own", "rel", 300)], "B0", "dropped")
add("controls", "D600", "second", (), (DM,), [P("own", "abs", 600)], "B0", "merged")
add("controls", "G0", "first", (EXPOSE_GC,), (DM,), [], None)
add("controls", "GN0", "first", (EXPOSE_GC, NO_FLUSH), (DM,), [], None)
add("controls", "G16", "second", (EXPOSE_GC,), (DM,), [P("own", "rel", -8, gc=16)], "G0", "merged")
add("controls", "GN16", "second", (EXPOSE_GC, NO_FLUSH), (DM,), [P("own", "rel", -8, gc=16)], "GN0", "merged")

# ---- the job 54 replica ----
# The second call of job 54 ran one oracle process per host with the whole golden list: the legacy second-call block takes the
# first case (main_merge at its first-call edge, the user settings and one managed file) as the prime with the options object of
# that case, then runs every case in list order in the same process. The rows below are the cases of that list that the four
# families here can build, in list order, as levels relative to each family's first-call edge (the same on all three hosts).
LADDER = [(MM, 0), (MM, -1), (MM, 1), (MM, 2), (ADM, 0), (ADM, -1), (ADM, 1), (ADM, 2),
          (DM, 3), (DM, 2), (DM, 4), (DM, 5), (AR, 2), (AR, 3), (AR, 4), (AR, 5)]
PRIME_ROW = (MM, 0)
ROWS_PLUS3 = [(DM, 3), (DM, 4), (AR, 3), (AR, 4)]
J54 = {
    "JS": ("single row after the prime", lambda row: [PRIME_ROW, row]),
    "JM": ("prime then the drop-in rows of the list", lambda row: [PRIME_ROW] + [one for one in LADDER if one[0] in DA2]),
    "JL": ("the buildable cases of the list in order", lambda row: list(LADDER)),
}


def level_of(family, delta):
    return PROFILE[family] + delta


def prime_level(family, prime, n):
    if prime["how"] == "n":
        return n
    if prime["how"] == "abs":
        return prime["amount"]
    return PROFILE[family] + prime["amount"]


def check_flags(flags, extras, log):
    wanted = list(extras) + (log_args("PATH") if log else [])
    base = [one for one in (REF, CROSS) if list(flags[: len(one)]) == one]
    if not base or [part for part in flags[len(base[0]):] if not part.startswith("--logfile=")] != [part for part in wanted if not part.startswith("--logfile=")]:
        raise RuntimeError(f"a probe would start Node with {flags!r}, not the reference or cross-check flags plus {extras!r}")


def assemble(root, cases, primes, marker=False):
    """The spec of one process. `cases`: file lists in run order (the first is the prime of a legacy second call when
    `primes` is None); `primes`: [(files, gc)] for the explicit prime list. Returns the text, each case's deep length and
    each prime's."""
    for folder in ("home/.claude", "work/.claude"):
        os.makedirs(os.path.join(root, folder), exist_ok=True)
    write(os.path.join(root, PLACES["user"]), content(USER))
    case_sets = [disk(files) for files in cases]
    prime_sets = [disk(files) for files, _ in (primes or [])]
    paths = sorted(set().union(*case_sets, *prime_sets))
    full = [{path: one.get(path) for path in paths} for one in case_sets + prime_sets]
    options = {"cwd": os.path.join(root, "work")}
    spec = {
        "root": root, "platform": sys.platform, "env": {"HOME": os.path.join(root, "home")}, "deep": True, "rejections": True,
        "cases": [{"name": f"case{i}", "options": dict(options), "files": full[i]} for i in range(len(cases))],
    }
    if primes is not None:
        spec["primes"] = [{"name": f"prime{i}", "options": "same", "gc": gc, "files": full[len(cases) + i]} for i, (_, gc) in enumerate(primes)]
    if marker:
        spec["phaseMarker"] = True
    return json.dumps(spec), [deep_length(files) for files in cases], [deep_length(files) for files, _ in (primes or [])]


def read_log(paths):
    """The function events of the bundle in a V8 log, split at the marker the oracle calls between prime and cases: two sorted
    lists of "kind:start:end:name" (before and after), or None when no log names the bundle."""
    best = None
    for path in paths:
        scripts, events = {}, []
        with open(path, encoding="utf-8", errors="replace") as handle:
            for line in handle:
                parts = line.rstrip("\n").split(",")
                if parts[0] == "script-details" and len(parts) > 2:
                    scripts[parts[1]] = parts[2]
                elif parts[0] == "function" and len(parts) > 7:
                    events.append((parts[1], parts[2], parts[3], parts[4], ",".join(parts[7:])))
        boundary = next((index for index, one in enumerate(events) if MARKER in one[4] and not one[0].startswith("preparse")), None)
        keep = {script for script, name in scripts.items() if name.endswith("sdk.mjs")}
        pick = [(index, one) for index, one in enumerate(events) if one[1] in keep and not one[0].startswith("preparse")]
        cut = boundary if boundary is not None else len(events)
        found = {
            "marker": boundary is not None,
            "prime": sorted({f"{k}:{s}:{e}:{n}" for index, (k, _, s, e, n) in pick if index < cut}),
            "case": sorted({f"{k}:{s}:{e}:{n}" for index, (k, _, s, e, n) in pick if index >= cut}),
        }
        if best is None or len(found["prime"]) + len(found["case"]) > len(best["prime"]) + len(best["case"]):
            best = found
    return best


def execute(root, spec_text, flags, call, extras, log):
    """Run one process. Returns (record fields, parsed entries, parsed prime answers); the status is "ok" only for a run
    that wrote its answers."""
    spec_path = root + ".json"
    log_path = root + ".v8.log" if log else None
    flags = list(flags) + (log_args(log_path) if log else [])
    out = {}
    try:
        check_flags(flags, extras, log)
        with open(spec_path, "w", encoding="utf-8") as handle:
            handle.write(spec_text)
        command = [NODE, *flags, ORACLE, PACKAGE, spec_path]
        env = padded_env(MANAGED["pad"], command, STARTUP)
        GATE["CALL"] = call
        with tempfile.TemporaryFile() as errors:
            try:
                process = spawn(command, env, stdout=subprocess.PIPE, stderr=errors, timeout=600)
            except Exception as error:  # a gate violation or refusal is a result of this run, not of the driver
                return dict(status="gate", error=f"{type(error).__name__}: {str(error)[:300]}"), None, []
            errors.seek(0)
            stderr = errors.read().decode("utf-8", "replace")
        if process.returncode != 0:
            found = re.search(r"^(\w*Error): (.*)$", stderr.replace("\r\n", "\n"), re.M)
            return dict(status="fail", error=(found.group(1) + ": " + found.group(2)[:200]) if found else stderr[-300:]), None, []
        entries = json.loads(process.stdout.decode("utf-8"))
        primes = []
        if os.path.exists(spec_path + ".primes.json"):
            with open(spec_path + ".primes.json", encoding="utf-8") as handle:
                primes = json.load(handle)
        out["status"] = "ok"
        if log:
            out["functions"] = read_log(log_files(log_path))
        return out, entries, primes
    except Exception as error:
        return dict(status="gate", error=f"{type(error).__name__}: {str(error)[:300]}"), None, []
    finally:
        shutil.rmtree(root, ignore_errors=True)
        for suffix in ("", ".native.json", ".primes.json"):
            if os.path.exists(spec_path + suffix):
                os.remove(spec_path + suffix)
        if log_path:
            for one in log_files(log_path):
                os.remove(one)


def kind_of(flags):
    return "ref" if list(flags[: len(REF)]) == REF else "cross"


def answer(entry, length):
    rejected = entry.get("error", {}).get("name") if "error" in entry else None
    effective = None if rejected else len(single(entry["result"])["effective"])
    return classify(effective, rejected, length)


def one_run(variant, family, n, flags, call, log=False):
    """One process, one case: the answer of the case and of each prime, classified."""
    config = VARIANTS[variant]
    root = os.path.join(SCRATCH, unique("h"))
    record = {"variant": variant, "family": family, "n": n, "kind": kind_of(flags), "call": call}
    primes = []
    if call == "second":
        for prime in config["primes"]:
            of = family if prime["family"] == "own" else prime["family"]
            primes.append((FAMILIES[of](prime_level(of, prime, n)), prime["gc"]))
    text, lengths, prime_lengths = assemble(root, [FAMILIES[family](n)], primes if call == "second" else None, marker=log)
    fields, entries, answered = execute(root, text, list(flags) + list(config["extra"]), call, config["extra"], log)
    record.update(fields)
    if entries is not None:
        record["cls"] = answer(entries[0], lengths[0])
        record["primes"] = [{"thrown": one["thrown"], "cls": classify(one["effective_length"], one["thrown"], length)} for one, length in zip(answered, prime_lengths)]
    return record


def multi_run(variant, rows, flags, call, log=False):
    """One process with a list of cases (the legacy second call when `call` is "second": the first is the prime)."""
    root = os.path.join(SCRATCH, unique("j"))
    files = [FAMILIES[family](level_of(family, delta)) for family, delta in rows]
    text, lengths, _ = assemble(root, files, None, marker=log)
    extras = [LOG_FLAG] if log else []
    fields, entries, _ = execute(root, text, list(flags) + extras, call, extras, log)
    record = {"variant": variant, "kind": kind_of(flags), "call": call, "rows": [list(one) for one in rows]}
    record.update(fields)
    if entries is not None:
        by_name = {one["name"]: one for one in entries}
        # An entry of the prime is not one of the cases; the oracle writes only the recorded cases to stdout.
        record["cases"] = [{"family": family, "delta": delta, "cls": answer(by_name[f"case{index}"], lengths[index])}
                           for index, (family, delta) in enumerate(rows)]
    return record


# ---- the cells ----
def expired():
    return time.monotonic() - START > BUDGET_SECONDS


def cell(variant, family, kind):
    """One (variant, family, flag set): the case level where it stops merging, by bisection on single runs from the first-call
    edge, then a window round the result (two reference runs, or one cross-check run, per level; widened once)."""
    if expired():
        return [], "deadline"
    config = VARIANTS[variant]
    call, out, base = config["call"], [], REF if kind == "ref" else CROSS
    centre = PROFILE[family]
    flags = base + config["extra"]

    def at(n, phase):
        record = one_run(variant, family, n, base, call)
        record["phase"] = phase
        out.append(record)
        return record

    def merged_at(n):
        if expired():
            return None
        record = at(n, "bisect")
        return record["cls"] == "merged" if record.get("status") == "ok" else None

    low = centre - 6
    if merged_at(low) is not True:
        return out, "unbracketed_low"
    high = centre + 70
    answer_high = merged_at(high)
    for far in (130, 400):
        if answer_high is True:
            high = centre + far
            answer_high = merged_at(high)
    if answer_high is not False:
        return out, "unbracketed_high" if answer_high is True else "void"
    while high - low > 1:
        middle = (low + high) // 2
        got = merged_at(middle)
        if got is None:
            return out, "void"
        low, high = (middle, high) if got else (low, middle)
    runs = 2 if kind == "ref" else 1
    for width in (3, 8):
        classes = {}
        for n in range(low - width, low + width + 1):
            classes[n] = [at(n, f"window{width}").get("cls") for _ in range(runs)]
        ordered = sorted(classes)
        if classes[ordered[0]][0] == "merged" and classes[ordered[-1]][0] != "merged":
            return out, "ok" if all(len(set(one)) == 1 for one in classes.values()) else "unstable"
    return out, "window_miss"


def fixed(variant, family):
    """A logging variant: two runs at each registered level, the reference flags, one log each."""
    out = []
    for offset in VARIANTS[variant]["at"]:
        for index in range(2):
            if expired():
                return out, "deadline"
            record = one_run(variant, family, PROFILE[family] + offset, REF, VARIANTS[variant]["call"], log=True)
            record["phase"] = f"log{offset:+d}"
            out.append(record)
    return out, "fixed"


def j54(name, row):
    """The replica rows: the reference flags twice and the cross-check flags once, second call, plus the first call alone."""
    out = []
    rows = J54[name][1](row)
    target = rows[-1] if name == "JS" else None
    for flags in (REF, REF, CROSS):
        if expired():
            return out, "deadline"
        record = multi_run(f"{name}", rows, flags, "second")
        record["target"] = list(target) if target else None
        out.append(record)
    return out, "ok"


def j54_first(row):
    out = []
    for flags in (REF, REF, CROSS):
        record = multi_run("JB", [row], flags, "first")
        out.append(record)
    return out, "ok"


def j54_log(name, row):
    out = []
    for _ in range(2):
        record = multi_run(f"{name}-log", J54[name][1](row), REF, "second", log=True)
        out.append(record)
    return out, "log"


def work_items(skipped):
    """(stage, key, callable) in priority order."""
    items = []
    for stage, names in STAGES:
        if stage == "j54":
            for name in ("JS", "JM", "JL"):
                if name == "JS":
                    for row in ROWS_PLUS3:
                        items.append((stage, f"JS/{row[0]}+{row[1]}", lambda row=row: j54("JS", row)))
                else:
                    items.append((stage, name, lambda name=name: j54(name, None)))
            for row in ROWS_PLUS3:
                items.append((stage, f"JB/{row[0]}+{row[1]}", lambda row=row: j54_first(row)))
        for variant in names:
            config = VARIANTS[variant]
            if variant in skipped:
                continue
            for family in config["fams"]:
                if config["at"] is not None:
                    items.append((stage, f"{variant}/{family}", lambda variant=variant, family=family: fixed(variant, family)))
                else:
                    for kind in config["kinds"]:
                        items.append((stage, f"{variant}/{family}/{kind}", lambda variant=variant, family=family, kind=kind: cell(variant, family, kind)))
        if stage == "lg":
            items.append((stage, "JS-log", lambda: j54_log("JS", (DM, 3))))
            items.append((stage, "JL-log", lambda: j54_log("JL", None)))
    return items


def main_c():
    names = v8_flag_names()
    selftest = {flag: norm_flag(flag) in names for flag in SELF_TEST_FLAGS}
    PROBE.update(names=len(names), selftest=selftest)
    available = {flag: norm_flag(flag) in names for flag in (EXPOSE_GC, NO_FLUSH, LOG_FLAG, "--logfile")}
    result = {"host": HOST, "probe": PROBE, "flags_available": available, "skipped": {}, "statuses": {}, "records": []}
    result["node"] = subprocess.run([NODE, "-p", "JSON.stringify([process.version,process.versions.v8])"], capture_output=True).stdout.decode().strip()
    exit_code = 0
    if len(names) < 100 or not all(selftest.values()):
        result["selftest_failed"] = True
        exit_code = 3
    elif not probe_logging(names):
        available[LOG_FLAG] = False
    log(f"flags {available} probe {PROBE}")
    skipped = {}
    if exit_code == 0:
        for variant, config in VARIANTS.items():
            missing = [flag for flag in config["extra"] if not available.get(flag)]
            if missing:
                skipped[variant] = "flag unavailable: " + ", ".join(missing)
    result["skipped"] = skipped
    if skipped:
        exit_code = exit_code or 2
    if exit_code != 3:
        items = work_items(skipped)
        log(f"{len(items)} work items")
        with concurrent.futures.ThreadPoolExecutor(max_workers=WORKERS) as pool:
            futures = [(stage, key, pool.submit(one)) for stage, key, one in items]
            for stage, key, future in futures:
                try:
                    out, status = future.result()
                except Exception as error:
                    out, status = [], f"error: {type(error).__name__}: {str(error)[:200]}"
                for record in out:
                    record["stage"] = stage
                result["records"].extend(out)
                result["statuses"][key] = status
                if status.startswith("error"):
                    exit_code = exit_code or 4
                if stage == "lg" and any(one.get("status") != "ok" or not (one.get("functions") or {}).get("marker") for one in out):
                    exit_code = exit_code or 5  # a log that failed, or one without the marker, names nothing
        result["variants"] = {name: {key: config[key] for key in ("call", "extra", "base", "kind", "kinds", "at", "primes", "fams")} for name, config in VARIANTS.items()}
    result["profile"] = {family: PROFILE[family] for family in ALL_FAMILIES}
    result["exit_code"] = exit_code
    try:
        result["gate"] = json.loads(json.dumps(GATE["TALLY"]))
    except (KeyError, TypeError, ValueError):
        pass
    with open(OUT, "w", encoding="utf-8") as handle:
        json.dump(result, handle, sort_keys=True)
    log(f"wrote {len(result['records'])} runs to {OUT}, exit {exit_code}")
    sys.exit(exit_code)


main_c()
