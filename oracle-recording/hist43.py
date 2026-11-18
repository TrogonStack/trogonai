#!/usr/bin/env python3
"""Ruling 43, rule 6: which process history moves the edge of a merge family on a second call.

usage: python3 -I hist43.py LIB NODE PACKAGE OUT

A second call's merge edge (the deepest level at which a drop-in or managed file still merges) sits a few levels past
the first call's for the drop-in families and not for the others. This run varies what the process did before the
measured resolve and records, per variant, family and level, whether the document merged or was dropped, so the
mechanism behind the gain can be told apart (oracle-recording/hist43 registration, written before the run).

Every measured resolve is a process of its own with one case: a second case in the same process would itself be a
later call. Each (variant, family, level) is run twice with the reference flags and once with the cross-check flags,
each plus the V8 flag the variant adds, if any. The output is evidence for the process-history model; it is never a
golden.
"""

import concurrent.futures
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
LIB, NODE, PACKAGE, OUT = sys.argv[1:5]
with open(os.path.join(HERE, "r2.py"), encoding="utf-8") as handle:
    SOURCE = handle.read()
SOURCE = SOURCE[: SOURCE.index('\nif os.environ.get("R2_LIST"):')]
R2 = {"__name__": "r2", "__file__": os.path.join(HERE, "r2.py")}
saved = sys.argv
sys.argv = [R2["__file__"], LIB, NODE, PACKAGE, OUT + ".r2", "1", "1", "1"]
try:
    exec(compile(SOURCE, R2["__file__"], "exec"), R2)
finally:
    sys.argv = saved
D, MANAGED = R2["D"], R2["MANAGED"]
GATE = D["GATE"]
HOST = D["HOST"]
REF, CROSS = list(R2["REF"]), list(R2["CROSS"])
STARTUP = MANAGED["STARTUP_BYTES"]
PLACES = MANAGED["PLACES"]
USER, LATER, HELD = MANAGED["USER"], MANAGED["LATER"], MANAGED["HELD"]
content, single = MANAGED["content"], MANAGED["single"]
PROFILE = MANAGED["PROFILES"][HOST]
unique, write, spawn, padded_env = D["unique"], D["write"], D["spawn"], D["padded_env"]
ORACLE = MANAGED["ORACLE"]
SCRATCH = D["SCRATCH"]
WORKERS = 2

# Each family: the files of a case at level n. The deep file is the one with more than two members.
FAMILIES = {
    "drop_merge": lambda n: [USER, ["drop_a", n, 0, 0], LATER],
    "drop_merge_arrays": lambda n: [USER, ["drop_a", 1, n, 0], LATER],
    "main_merge": lambda n: [USER, ["managed", n, 0, 0]],
    "admin_inner": lambda n: [USER, HELD, ["drop_a", 1, 1, n]],
}
ALL4 = tuple(FAMILIES)
DA = ("drop_merge", "drop_merge_arrays")

# The levels measured around a family's edge: from LOW below it to HIGH above it.
LOW, HIGH = -2, 10

NO_LAZY_FEEDBACK = "--no-lazy-feedback-allocation"
NO_LAZY = "--no-lazy"
NO_LAZY_POSITIONS = "--no-enable-lazy-source-positions"
EXTRA_FLAGS = (NO_LAZY_FEEDBACK, NO_LAZY, NO_LAZY_POSITIONS)


def P(family, how, amount=0, options="same"):
    """A prime: `family`'s document ("own" is the case's family) at the case's level (how "n"), at the edge plus `amount`
    (how "rel"), or at the level `amount` (how "abs"), with the case's own options object ("same") or an equal one."""
    return {"family": family, "how": how, "amount": amount, "options": options}


# id: call, extra V8 flag, families, primes, and the id of the first-call run the gain is measured against.
VARIANTS = {
    "V0": dict(call="first", extra=None, fams=ALL4, primes=[], base=None),
    "V1": dict(call="second", extra=None, fams=ALL4, primes=[], base="V0"),
    "V2": dict(call="second", extra=None, fams=DA, primes=[P("own", "rel", -8)], base="V0"),
    "V3": dict(call="second", extra=None, fams=DA, primes=[P("own", "abs", 3)], base="V0"),
    "V3m": dict(call="second", extra=None, fams=DA, primes=[P("own", "abs", 40)], base="V0"),
    "V4": dict(call="second", extra=None, fams=ALL4, primes=[P("own", "n")], base="V0"),
    "V4f": dict(call="second", extra=None, fams=DA, primes=[P("own", "n", options="fresh")], base="V0"),
    "V5b": dict(call="first", extra=NO_LAZY_FEEDBACK, fams=DA, primes=[], base=None),
    "V5": dict(call="second", extra=NO_LAZY_FEEDBACK, fams=DA, primes=[P("own", "n")], base="V5b"),
    "V6b": dict(call="first", extra=NO_LAZY, fams=DA, primes=[], base=None),
    "V6": dict(call="second", extra=NO_LAZY, fams=DA, primes=[P("own", "n")], base="V6b"),
    "V6sb": dict(call="first", extra=NO_LAZY_POSITIONS, fams=DA, primes=[], base=None),
    "V6s": dict(call="second", extra=NO_LAZY_POSITIONS, fams=DA, primes=[P("own", "n")], base="V6sb"),
    "V7": dict(call="second", extra=None, fams=DA, primes=[P("own", "n"), P("own", "n")], base="V0"),
    "V8a": dict(call="second", extra=None, fams=("drop_merge",), primes=[P("drop_merge_arrays", "rel", 10)], base="V0"),
    "V8x": dict(call="second", extra=None, fams=("drop_merge",), primes=[P("main_merge", "rel", 10)], base="V0"),
}


def log(message):
    sys.stderr.write(message + "\n")
    sys.stderr.flush()


def disk(files):
    """The files of a case below the root, by path: the settings of the user are written once, at the start."""
    return {PLACES[file[0]]: content(file) for file in files if file[0] != "user"}


def deep_length(files):
    return len(content(next(file for file in files if len(file) > 2)))


def check_flags(flags, extra):
    allowed = [REF, CROSS]
    if extra:
        allowed = [one + [extra] for one in allowed]
    if list(flags) not in allowed:
        raise RuntimeError(f"a probe would start Node with {flags!r}, not the reference or cross-check flags (plus {extra!r})")


def prime_level(edges, family, prime, n):
    if prime["how"] == "n":
        return n
    if prime["how"] == "abs":
        return prime["amount"]
    return edges[family] + prime["amount"]


def build(root, family, n, primes, edges):
    """The spec of one process: the primes (each with its own files), then the one case. Returns the spec text and the
    length of the case's deep document and each prime's."""
    for folder in ("home/.claude", "work/.claude"):
        os.makedirs(os.path.join(root, folder), exist_ok=True)
    case_files = FAMILIES[family](n)
    write(os.path.join(root, PLACES["user"]), content(USER))
    sets = [(None, disk(case_files))]
    prime_specs, prime_lengths = [], []
    for index, prime in enumerate(primes):
        family_of = family if prime["family"] == "own" else prime["family"]
        files = FAMILIES[family_of](prime_level(edges, family_of, prime, n))
        sets.append((prime, disk(files)))
        prime_lengths.append(deep_length(files))
    paths = sorted(set().union(*[disk_set for _, disk_set in sets]))
    # A path a step does not write is removed, so each resolve sees exactly its own files.
    full = [{path: disk_set.get(path) for path in paths} for _, disk_set in sets]
    spec = {
        "root": root,
        "platform": sys.platform,
        "env": {"HOME": os.path.join(root, "home")},
        "deep": True,
        "rejections": True,
        "primes": [{"name": f"prime{i}", "options": prime["options"], "files": full[i + 1]} for i, prime in enumerate(primes)],
        "cases": [{"name": "case", "options": {"cwd": os.path.join(root, "work")}, "files": full[0]}],
    }
    return json.dumps(spec), deep_length(case_files), prime_lengths


def classify(effective_length, rejected, doc_length):
    if rejected:
        return "rejected"
    if effective_length is None:
        return "none"
    return "merged" if effective_length >= doc_length else "dropped"


def one_run(variant, family, n, flags, call, edges):
    """One process: the answer of the case and of each prime, classified."""
    config = VARIANTS[variant]
    check_flags(flags, config["extra"])
    root = os.path.join(SCRATCH, unique("h"))
    spec_path = root + ".json"
    record = {"variant": variant, "family": family, "n": n, "flags": "ref" if flags[: len(REF)] == REF else "cross", "call": call}
    try:
        text, doc_length, prime_lengths = build(root, family, n, config["primes"] if call == "second" else [], edges)
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
        return record
    finally:
        shutil.rmtree(root, ignore_errors=True)
        for suffix in ("", ".native.json", ".primes.json"):
            if os.path.exists(spec_path + suffix):
                os.remove(spec_path + suffix)


def passes(extra):
    """The three runs of every level: the reference flags twice and the cross-check flags once."""
    add = [extra] if extra else []
    return [(REF + add), (REF + add), (CROSS + add)]


def merged(variant, family, n, edges, extra, call):
    """Whether a single reference run at `n` merges (the bisection of a flagged variant's baseline)."""
    answer = one_run(variant, family, n, REF + ([extra] if extra else []), call, edges)
    return answer.get("cls") == "merged" if answer.get("status") == "ok" else None


def find_edge(variant, family, edges):
    """The deepest merged level of a first-call family under a variant's flags, by bisection from the unflagged edge."""
    config = VARIANTS[variant]
    low, high = edges[family] - 400, edges[family] + 400
    if merged(variant, family, low, edges, config["extra"], "first") is not True:
        return None
    if merged(variant, family, high, edges, config["extra"], "first") is not False:
        return None
    while high - low > 1:
        middle = (low + high) // 2
        if merged(variant, family, middle, edges, config["extra"], "first"):
            low = middle
        else:
            high = middle
    return low


def flag_available(flag):
    env = {**D["EXTRA_ENV"]} if "EXTRA_ENV" in D else {}
    return subprocess.run([NODE, flag, "-e", "0"], env=env, capture_output=True).returncode == 0


def task(variant, family, n, base_edges):
    config = VARIANTS[variant]
    return [one_run(variant, family, n, flags, config["call"], base_edges) for flags in passes(config["extra"])]


def main():
    available = {flag: flag_available(flag) for flag in EXTRA_FLAGS}
    log(f"flags {available}")
    # The edges the windows are centred on: the profile's first-call boundary, or for a flagged variant the measured one.
    edges = {family: PROFILE[family] for family in ALL4}
    centres = {"V0": dict(edges)}
    skipped = {}
    for variant, config in VARIANTS.items():
        if config["extra"] and not available[config["extra"]]:
            skipped[variant] = "flag unavailable: " + config["extra"]
    for variant, config in VARIANTS.items():
        if config["call"] == "first" and config["extra"] and variant not in skipped:
            centres[variant] = {}
            for family in config["fams"]:
                centres[variant][family] = find_edge(variant, family, edges)
                log(f"{variant} {family} edge {centres[variant][family]}")
    for variant, config in VARIANTS.items():
        if variant in skipped or variant in centres:
            continue
        base = config["base"]
        centres[variant] = centres.get(base) or dict(edges)
        if base in skipped or any(value is None for value in centres[variant].values()):
            skipped[variant] = f"no edge for its baseline {base}"
    work = []
    for call in ("first", "second"):
        for variant, config in VARIANTS.items():
            if variant in skipped or config["call"] != call:
                continue
            for family in config["fams"]:
                centre = centres[variant].get(family)
                if centre is None:
                    continue
                for n in range(centre + LOW, centre + HIGH + 1):
                    work.append((call, variant, family, n))
    records = []
    for call in ("first", "second"):
        phase = [item for item in work if item[0] == call]
        log(f"{call}: {len(phase)} levels")
        with concurrent.futures.ThreadPoolExecutor(max_workers=WORKERS) as pool:
            futures = [pool.submit(task, variant, family, n, centres[variant]) for _, variant, family, n in phase]
            for future in futures:
                records.extend(future.result())
    result = {
        "host": HOST,
        "node": NODE and subprocess.run([NODE, "-p", "JSON.stringify([process.version,process.versions.v8])"], capture_output=True).stdout.decode().strip(),
        "flags_available": available,
        "skipped": skipped,
        "profile": {family: PROFILE[family] for family in ALL4},
        "centres": centres,
        "variants": {name: {key: config[key] for key in ("call", "extra", "base")} | {"fams": list(config["fams"]), "primes": config["primes"]} for name, config in VARIANTS.items()},
        "window": [LOW, HIGH],
        "records": records,
    }
    try:
        result["gate"] = json.loads(json.dumps(GATE["TALLY"]))
    except (KeyError, TypeError, ValueError):
        pass
    with open(OUT, "w", encoding="utf-8") as handle:
        json.dump(result, handle, sort_keys=True)
    log(f"wrote {len(records)} runs to {OUT}")


main()
