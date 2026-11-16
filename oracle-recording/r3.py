#!/usr/bin/env python3
"""Round 3: the exact stack the pinned TypeScript resolveSettings has for the file cache copy of each managed tier
evaluation and for the managed folder check, on the host it runs on, plus the levels the managed recorder needs.

usage: python3 -I r3.py LIB NODE PACKAGE OUT SHARD SHARDS

Units (dealt round robin to SHARDS shards):
  proto:<E|S|O|M>    a drop-in holding a deep value under `__proto__`, which the file cache copy enters and the merge
                     skips; E, S, O and M are the effective settings, the managed source, the policy origin and the
                     machine policy, each answered by a copy of its own evaluation.
  held:<fs|tool>:<walk|clone>   the managed folder check's parse, resumed from a file request (fs) or a spawned
                     program's real end (tool), with the walk or the clone running out first; tool on Windows only.
  held_band:tool:<walk|clone>:<job>   on Unix, the tool base tallied R3_TOOL_RUNS (12) times per level in each of
                     R3_TOOL_JOBS (3) jobs: it resumes on one of two stacks run by run, and the profile is the tight
                     one's edge.
  member:<proto|plain>:<arrays|objects>   on Windows, a drop-in holding nested arrays or objects under `__proto__`
                     inside `x`, which only the file cache copy enters, or under a plain member, which the merge
                     enters too: where the answer first changes, over 30 passes.
  key:<name>         a PROFILES key of the managed recorder: located on Unix when the profile lacks it or PASS_KEYS
                     names it, banded over 30 passes on Windows.

On Unix every cost the model can produce is a multiple of 16: a unit finds the class C whose shapes all fit while
every shape of C + 16 fails, with three shapes each and the cross-check flags. On macOS the command line and
environment share the stack, so a sweep of their size at a shape of class C then finds the byte. On Windows a unit
runs its levels 30 times each and records every answer.
"""

import collections
import concurrent.futures
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
LIB, NODE, PACKAGE, OUT, SHARD, SHARDS = sys.argv[1:7]
SHARD, SHARDS = int(SHARD), int(SHARDS)
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
HOST, WINDOWS, DARWIN = D["HOST"], D["WINDOWS"], sys.platform == "darwin"
REF, CROSS = R2["REF"], R2["CROSS"]
unique, write, wsl, win32, project, dumps = D["unique"], D["write"], R2["wsl"], R2["win32"], R2["project"], R2["dumps"]
STARTUP = MANAGED["STARTUP_BYTES"]
WORKERS = max(2, os.cpu_count() or 2)
QUIET = 2
K_REAL = 12
WINDOWS_PASSES = 30
SWEEP_RUNS = int(os.environ.get("R3_SWEEP_RUNS", "30"))
LEAVES = MANAGED["LEAVES"]

# The per-level costs the Rust model holds for each pass: (object, array, {leaf: cost}).
MODEL = {
    "darwin arm64": {
        "walk": (448, 240, {"null_array": 272, "empty_array": 176, "null_object": 448, "empty_object": 144}),
        "clone": (288, 304, {"null_array": 304, "empty_array": 304, "null_object": 328, "empty_object": 328}),
        "copy": (288, 512, {"null_array": 512, "empty_array": 512, "null_object": 528, "empty_object": 496}),
    },
    "linux aarch64": {
        "walk": (512, 272, {"null_array": 272, "empty_array": 192, "null_object": 512, "empty_object": 144}),
        "clone": (320, 320, {"null_array": 320, "empty_array": 320, "null_object": 320, "empty_object": 320}),
        "copy": (512, 560, {"null_array": 560, "empty_array": 192, "null_object": 512, "empty_object": 144}),
    },
    "linux x86_64": {
        "walk": (608, 272, {"null_array": 272, "empty_array": 192, "null_object": 608, "empty_object": 144}),
        "clone": (304, 304, {"null_array": 304, "empty_array": 304, "null_object": 304, "empty_object": 304}),
        "copy": (608, 640, {"null_array": 640, "empty_array": 176, "null_object": 608, "empty_object": 144}),
    },
}


def deep(objects, arrays, leaf):
    return '{"x":' * objects + "[" * arrays + LEAVES[leaf] + "]" * arrays + "}" * objects


def cost(table, above, shape):
    """What a path through `above` containers holding the shape spends, for one pass of the model."""
    obj, arr, leaves = table
    objects, arrays, leaf = shape
    return prefix(table, above) + objects * obj + arrays * arr + leaves[leaf]


def prefix(table, above):
    """What the containers above the shape spend: a count of objects, or (objects, arrays)."""
    above_objects, above_arrays = above if isinstance(above, tuple) else (above, 0)
    return above_objects * table[0] + above_arrays * table[1]


def shapes_at(table, above, target, major, leaf_names):
    """Up to three shapes of cost `target` ending in one of `leaf_names`, with few containers on the minor axis."""
    obj, arr, leaves = table
    found = []
    for minor in range(0, 48):
        for leaf in leaf_names:
            rest = target - prefix(table, above) - leaves[leaf] - minor * (arr if major == "o" else obj)
            step = obj if major == "o" else arr
            if rest >= 0 and rest % step == 0:
                count = rest // step
                found.append((count, minor, leaf) if major == "o" else (minor, count, leaf))
        if len(found) >= 3:
            break
    return found[:3]


def run_spec(spec_path, flags, startup):
    command = [NODE, *flags, MANAGED["ORACLE"], PACKAGE, spec_path]
    with tempfile.TemporaryFile() as errors:
        process = subprocess.run(command, stdout=subprocess.PIPE, stderr=errors,
                                 env={"PAD": "x" * MANAGED["pad"](command, startup), **D["EXTRA_ENV"]})
        errors.seek(0)
        stderr = errors.read().decode("utf-8", "replace")
    if process.returncode != 0:
        # The error line sits above its stack trace, which the tail kept for the record may not reach.
        found = re.search(r"^(\w*Error): (.*)$", stderr.replace("\r\n", "\n"), re.M)
        return {"fail": stderr[-600:], **({"rejected": found.group(1) + ": " + found.group(2)[:200]} if found else {})}
    return json.loads(process.stdout.decode("utf-8"))


# ---- proto: the copy of each evaluation ----
PARENT = MANAGED["PARENT"]
USER = MANAGED["USER"]


def proto_kinds(answered):
    if isinstance(answered, dict):
        return ("fail", answered.get("fail", "")[-200:])
    result = answered[0]["result"]
    managed = next((s for s in result["sources"] if s["source"] == "managed"), None)
    machine = result["machinePolicy"]
    return {
        "E": "x" in result["effective"],
        "S": bool(managed) and "x" in (managed.get("settings") or {}),
        "O": bool(managed) and managed.get("policyOrigin") == "file",
        "M": isinstance(machine, dict) and machine.get("didNotLoad") is False,
    }


def proto_probe(shape, flags=None, startup=STARTUP):
    name = unique("p")
    try:
        spec = MANAGED["prepare"](name, [USER, PARENT, ["drop_a", '{"x":{"__proto__":' + deep(*shape) + "}}"]])
        return proto_kinds(run_spec(spec, flags or REF, startup))
    finally:
        shutil.rmtree(os.path.join(MANAGED["FOLDER"], name), ignore_errors=True)


# ---- held: the managed folder check ----
HELD_DOC = R2["HELD_CHECK_DOC"]
HELD_HKCU = R2["HELD_CHECK_HKCU"]


def held_probe(place, shape, delivery, flags=None, startup=STARTUP):
    name = unique("h")
    text = HELD_DOC % deep(*shape)
    try:
        spec_path = MANAGED["prepare"](name, [["managed", text]] if place == "platform" else [])
        with open(spec_path, encoding="utf-8") as handle:
            spec = json.load(handle)
        root = spec["root"]
        if place == "platform":
            wsl(root, spec, R2["FLAG"], HELD_HKCU)
        elif place == "windows":
            wsl(root, spec, R2["FLAG"], HELD_HKCU, text)
        else:
            win32(root, spec, HELD_HKCU)
            write(os.path.join(root, R2["WINDOWS_MANAGED"]), text)
        if delivery != "timer":
            spec["tools"] = delivery
        with open(spec_path, "w", encoding="utf-8") as handle:
            json.dump(spec, handle)
        answered, failure = project(run_spec(spec_path, flags or REF, startup), root)
        if failure is not None:
            return None
        # The check parsed the file without running out when the Windows user policy still applies.
        return '"hkcu"' in answered
    finally:
        shutil.rmtree(os.path.join(MANAGED["FOLDER"], name), ignore_errors=True)


# ---- the class search ----
class Search:
    """Finds the class C for one predicate on Unix, and on macOS the byte."""

    def __init__(self, pool, fits, table, above, major, leaf_names):
        self.pool, self.fits_once, self.table, self.above, self.major = pool, fits, table, above, major
        self.coarse_leaf, self.leaf_names, self.log = leaf_names[0], leaf_names, []

    def at(self, target):
        return shapes_at(self.table, self.above, target, self.major, self.leaf_names)

    def fits(self, shape, flags=None, startup=STARTUP):
        result = self.fits_once(shape, flags, startup)
        if result is None:
            raise RuntimeError(f"a run failed at {shape}")
        self.log.append([list(shape), flags == CROSS, startup, bool(result)])
        return bool(result)

    def coarse_shape(self, n):
        return (n, 0, self.coarse_leaf) if self.major == "o" else (0, n, self.coarse_leaf)

    def run(self):
        low, high = 50, 6000
        if not self.fits(self.coarse_shape(low)) or self.fits(self.coarse_shape(high)):
            return {"error": "no edge between 50 and 6000", "log": self.log}
        while high - low > 1:
            middle = (low + high) // 2
            if self.fits(self.coarse_shape(middle)):
                low = middle
            else:
                high = middle
        fit_cost = cost(self.table, self.above, self.coarse_shape(low))
        fail_cost = cost(self.table, self.above, self.coarse_shape(high))
        classes = [c for c in range(fit_cost, fail_cost + 1, 16) if self.at(c)]
        lo, hi = 0, len(classes) - 1
        while hi - lo > 1:
            middle = (lo + hi) // 2
            if self.fits(self.at(classes[middle])[0]):
                lo = middle
            else:
                hi = middle
        last, next_class = classes[lo], classes[hi]
        # Every cost is a multiple of 16; when this pass reaches no shape at C + 16 (every cost of linux x86_64's copy
        # is a multiple of 32), no document tells C from C + 16 and the pair brackets C with the next class.
        near, far = self.at(last), self.at(next_class)
        checks = {"near": [], "far": []}
        for side, shapes, want in (("near", near, True), ("far", far, False)):
            for shape in shapes:
                for flags in (REF, REF, REF, REF, CROSS, CROSS):
                    got = self.fits(shape, flags)
                    checks[side].append([list(shape), flags == CROSS, got])
                    if got != want:
                        return {"error": f"{side} shape {shape} answered {got}", "class": last, "checks": checks, "log": self.log}
        out = {"class": last, "next_class": next_class, "near": [list(s) for s in near], "far": [list(s) for s in far], "checks": checks}
        if DARWIN:
            out.update(self.sweep(near[0]))
        else:
            # Beside the stack: a larger command line and environment changes nothing.
            out["beside_stack"] = self.fits(near[0], REF, STARTUP + 512) and not self.fits(far[0], REF, STARTUP + 0)
        out["log"] = self.log
        return out

    def tally(self, shape, startup, runs):
        """How many of `runs` reference runs and `runs` cross-check runs fit, or None when a run failed."""
        jobs = [REF] * runs + [CROSS] * runs
        hits = list(self.pool.map(lambda flags: self.fits_once(shape, flags, startup), jobs))
        if any(hit is None for hit in hits):
            return None
        return sum(1 for hit in hits if hit)

    def sweep(self, shape):
        """The startup window on macOS: Node fits a document through a random few bytes past its edge, so N is the
        last startup size where every run fits and F the first where every run fails, each from SWEEP_RUNS runs under
        both flag sets. A first pass of four runs per size finds where to look."""
        end = STARTUP + 65
        first_pass = {}
        for startup in range(STARTUP, end):
            hits = self.tally(shape, startup, 2)
            if hits is None:
                return {"error": f"a run failed at startup {startup}"}
            first_pass[startup] = hits
        if first_pass[STARTUP] != 4:
            return {"error": "the shape does not fit at the default startup", "first_pass": first_pass}
        rough_last = max(k for k in first_pass if all(first_pass[j] == 4 for j in range(STARTUP, k + 1)))
        rough_fail = [k for k in first_pass if all(first_pass[j] == 0 for j in range(k, end))]
        if not rough_fail:
            return {"error": "no startup size where every run fails", "first_pass": first_pass}
        low, high = max(STARTUP, rough_last - 4), min(end - 1, min(rough_fail) + 4)
        rows, full = {}, 2 * SWEEP_RUNS
        for startup in range(low, high + 1):
            hits = self.tally(shape, startup, SWEEP_RUNS)
            if hits is None:
                return {"error": f"a run failed at startup {startup}"}
            rows[startup] = hits
        sweep = {str(k): v for k, v in rows.items()}
        if rows[low] != full or rows[high] != 0:
            return {"error": f"the window reaches past {low}..{high}", "sweep": sweep, "runs": full}
        last = max(k for k in rows if all(rows[j] == full for j in range(low, k + 1)))
        first = min(k for k in rows if all(rows[j] == 0 for j in range(k, high + 1)))
        if first - last > 16:
            return {"error": f"the window from {last} to {first} is wider than 16 bytes", "sweep": sweep, "runs": full}
        return {"startup_max": last, "startup_fail": first, "budget": cost(self.table, self.above, shape) + last,
                "sweep": sweep, "runs": full}


def proto_unit(observable):
    def go(pool, quiet):
        if WINDOWS:
            return windows_band(pool, lambda n: proto_probe((0, n - 1, "null_array"))[observable], 50, 6000)
        found = Search(pool, lambda shape, flags, startup: (lambda k: k[observable] if isinstance(k, dict) else None)(proto_probe(shape, flags, startup)),
                       MODEL[HOST]["copy"], 2, "a", ["null_array", "empty_array"]).run()
        if "class" in found and "error" not in found:
            found["profile"] = profile_pair(found)
        return found
    return go


def profile_pair(found):
    near, far = found["near"][0], found["far"][0]
    if DARWIN:
        return [[*near, found["startup_max"]], [*near, found["startup_fail"]]]
    return [[*near, STARTUP], [*far, STARTUP]]


def leaf_unit(leaf, observable="S"):
    """The class of one copy with every shape ending in `leaf`, to set what the copy spends at a leaf. Two chains whose
    budgets sit at different offsets within a class tell a leaf cost the other levels cannot resolve."""
    def go(pool, quiet):
        return Search(pool, lambda shape, flags, startup: (lambda k: k[observable] if isinstance(k, dict) else None)(proto_probe(shape, flags, startup)),
                      MODEL[HOST]["copy"], 2, "a", [leaf]).run()
    return go


def walk_leaf_unit(leaf):
    """The class of the managed folder check's walk with every shape ending in `leaf`."""
    def go(pool, quiet):
        probe = lambda shape, flags, startup: held_probe("platform", shape, "timer", flags, startup)
        return Search(pool, probe, MODEL[HOST]["walk"], 1, "o", [leaf]).run()
    return go


HELD_PLACES = {"fs": ["platform"], "tool": ["windows", "native"]}


def held_unit(base, which):
    def go(pool, quiet):
        major = "o" if which == "walk" else "a"
        out = {}
        for place in HELD_PLACES[base]:
            delivery = "timer" if base == "fs" else "real"
            probe = lambda shape, flags, startup, p=place, d=delivery: held_probe(p, shape, d, flags, startup)
            if WINDOWS:
                level = lambda n, p=place, d=delivery: held_probe(p, (n - 1, 0, "null_object") if major == "o" else (0, n - 1, "null_array"), d)
                out[place] = windows_band(quiet if base == "tool" else pool, level, 50, 6000)
                continue
            out[place] = Search(pool, probe, MODEL[HOST][which], 1, major, ["null_array"]).run()
        return out
    return go


# The tool base resumes on one of two stacks, run by run: the levels every run fits through belong to the tight one,
# and from the first level no run fits, neither fits. Each host tallies the same levels in three jobs; the profile is
# the tight edge, and the levels between tell how far apart the two stacks sit and how often a run lands on the roomy one.
TOOL_LEVELS = {
    "linux x86_64": {"walk": (1600, 1680), "clone": (3200, 3320)},
    "linux aarch64": {"walk": (1880, 1960), "clone": (3020, 3140)},
    "darwin arm64": {"walk": (2080, 2240), "clone": (3080, 3290)},
}
TOOL_JOBS = int(os.environ.get("R3_TOOL_JOBS", "3"))
# Runs per level and place in each job. On Linux aarch64 a run lands on the tight stack about once in 70, so its
# levels need hundreds of runs, pooled over the jobs, before a level past the tight edge shows a failure.
TOOL_RUNS = int(os.environ.get("R3_TOOL_RUNS", str(K_REAL)))
# How many times a band whose lowest level is not all-fit grows downward by its own width before it gives up.
TOOL_EXTENSIONS = 3
# The gap between the two stacks on Linux x86_64, from run 43804161293: 29 walk levels of 608 bytes and 58 clone levels
# of 304 bytes.
X64_TOOL_SPLIT = 17632


def tool_band(which):
    def go(pool, quiet):
        first, high = TOOL_LEVELS[HOST][which]
        table = MODEL[HOST][which]
        step = table[0] if which == "walk" else table[1]
        shape = (lambda n: (n, 0, "null_array")) if which == "walk" else (lambda n: (0, n, "null_array"))
        out = {}
        for place in HELD_PLACES["tool"]:
            tally = collections.Counter()
            low, todo = first, list(range(first, high + 1))
            for extension in range(TOOL_EXTENSIONS + 1):
                jobs = [level for level in todo for _ in range(TOOL_RUNS)]
                for level, got in zip(jobs, quiet.map(lambda n, p=place: held_probe(p, shape(n), "real"), jobs)):
                    if got is None:
                        return {"error": f"a run failed at level {level}"}
                    tally[level] += 1 if got else 0
                if tally[low] == TOOL_RUNS or extension == TOOL_EXTENSIONS:
                    break
                # The lowest level is not all-fit: the tight edge sits lower, so the band widens downward.
                todo = list(range(low - (high - first + 1), low))
                low = todo[0]
            levels = list(range(low, high + 1))
            band = {str(level): tally[level] for level in levels}
            found = {"levels": band, "passes": TOOL_RUNS}
            out[place] = found
            if tally[low] != TOOL_RUNS or tally[high] != 0:
                found["error"] = f"the band reaches past {low}..{high}"
                continue
            tight = max(n for n in levels if all(tally[j] == TOOL_RUNS for j in range(low, n + 1)))
            # The roomy stack fits in few runs at some places, so a level where no run fits can sit inside the mixed
            # region: the first all-fail level is the one after the last level where any run fits.
            fail = max(n for n in levels if tally[n]) + 1
            between = [tally[j] for j in range(tight + 1, fail)]
            found.update({
                "tight": tight, "fail": fail, "tight_cost": cost(table, 1, shape(tight)),
                # The roomy stack's last level is the one before `fail`.
                "split": (fail - 1 - tight) * step,
                "roomy_share": round(sum(between) / (TOOL_RUNS * len(between)), 3) if between else None,
            })
            if HOST == "linux x86_64" and found["split"] != X64_TOOL_SPLIT:
                found["error"] = f"the stacks sit {found['split']} bytes apart, not {X64_TOOL_SPLIT}"
        return out
    return go


def machine():
    """The runner a job ran on, to tell variance between machines from variance between runs."""
    import platform
    cpu = ""
    try:
        with open("/proc/cpuinfo", encoding="utf-8") as handle:
            cpu = next((line.split(":", 1)[1].strip() for line in handle if line.startswith("model name")), "")
    except OSError:
        pass
    if not cpu and DARWIN:
        cpu = subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True).stdout.decode().strip()
    system = platform.uname()
    return {"uname": f"{system.system} {system.release} {system.version} {system.machine}", "cpu": cpu or platform.processor(), "cpus": os.cpu_count(),
            "image": os.environ.get("ImageOS", ""), "image_version": os.environ.get("ImageVersion", ""),
            "runner": os.environ.get("RUNNER_NAME", "")}


def windows_band(pool, probe, low, high):
    """The levels around the edge of `probe`, each run 30 times: how many runs fit at each level."""
    if not probe(low) or probe(high):
        return {"error": f"no edge between {low} and {high}"}
    while high - low > 1:
        middle = (low + high) // 2
        if probe(middle):
            low = middle
        else:
            high = middle
    levels = list(range(low - 6, low + 8))
    jobs = [level for level in levels for _ in range(WINDOWS_PASSES)]
    tally = collections.Counter()
    for level, got in zip(jobs, pool.map(probe, jobs)):
        tally[(level, bool(got))] += 1
    return {"edge_guess": low, "levels": {str(level): tally[(level, True)] for level in levels}, "passes": WINDOWS_PASSES}


# ---- keys ----
# The merge into the folder of the origin evaluation, located on each Unix host around the level the later reads hold a
# drop-in at.
PASS_KEYS = {
    "darwin arm64": {"origin_merge": 1243},
    "linux aarch64": {"origin_merge": 1251},
    "linux x86_64": {"origin_merge": 1306},
}
GUESSES = {
    "linux x86_64": {"org_copy": 1527, "effective_copy": 1562, "effective_ask": 1561, "effective_ask_machine": 1565},
}


def key_unit(key, guess, others=None):
    """`guess` is a level, or a [low, high] range to search when no earlier run measured the key. A group with more
    than one key holds the others at `others` and moves only this one."""
    def go(pool, quiet):
        group = next(make for keys, make in MANAGED["GROUPS"] if key in keys.split())
        keys = next(keys for keys, make in MANAGED["GROUPS"] if key in keys.split()).split()
        position = keys.index(key)

        def held(other):
            value = (others or {}).get(other)
            return value if isinstance(value, int) else None

        def at(level, flags=REF):
            args = [level if k == key or held(k) is None else held(k) for k in keys]
            cases = group(*args)
            chosen = cases[2 * position] if len(cases) == 2 * len(keys) else cases[0]
            case_name, files = chosen[0], chosen[1]
            name = case_name.rsplit("_", 1)[0] + "_" + unique("k")
            try:
                spec = MANAGED["prepare"](name, files)
                # The answers carry documents as JSON strings, so a nest of objects arrives escaped and needs its own
                # collapse before a level can match the next.
                text = re.sub(r'(?:\{\\"x\\":){2,}', '{\\"x\\":*', dumps(MANAGED["run"](spec, flags)))
                return R2["collapse"](text)
            except SystemExit:
                return "fail"
            finally:
                shutil.rmtree(os.path.join(MANAGED["FOLDER"], name), ignore_errors=True)

        found_guess = guess
        if isinstance(guess, list):
            low, high = guess
            reference = at(low)
            if at(high) == reference:
                return {"error": f"no change between {low} and {high}", "reference": reference, "at_high": at(high)}
            while high - low > 1:
                middle = (low + high) // 2
                if at(middle) == reference:
                    low = middle
                else:
                    high = middle
            found_guess = low
        if WINDOWS:
            levels = list(range(found_guess - 4, found_guess + 6))
            jobs = [level for level in levels for _ in range(WINDOWS_PASSES)]
            answers, tally = [], collections.defaultdict(collections.Counter)
            for level, got in zip(jobs, quiet.map(at, jobs)):
                if got not in answers:
                    answers.append(got)
                tally[str(level)][answers.index(got)] += 1
            return {"guess": found_guess, "levels": {k: dict(v) for k, v in tally.items()}, "answers": answers}
        levels = list(range(found_guess - 60, found_guess + 61))
        answered = dict(zip(levels, pool.map(at, levels)))
        changes = [level for level in levels[:-1] if answered[level] != answered[level + 1]]
        checks = {level: [at(level, CROSS) == answered[level], at(level + 1, CROSS) == answered[level + 1]] for level in changes}
        return {"guess": found_guess, "changes": changes, "checks": {str(k): v for k, v in checks.items()}}
    return go


def windows_guesses():
    path = os.path.join(HERE, "windows_profile_guess.json")
    if not os.path.exists(path):
        return {}
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)



# ---- the copy against the merge on Windows ----
def member_doc(member, objects, arrays):
    """A drop-in holding the shape inside `x` under `member`: `__proto__`, which the file cache copy enters and the
    merge skips, or `y`, which both enter, so the two documents differ only in whether the merge reaches the shape."""
    text = MANAGED["proto"](objects, arrays, "null_array")
    return text if member == "__proto__" else text.replace('"__proto__"', '"%s"' % member, 1)


def answer_unit(make, low=50, high=6000):
    """Where the answer to the files `make(level)` names first differs from the answer at `low`, found by bisection,
    then each level near it run 30 times with every distinct answer it gave."""
    def go(pool, quiet):
        def at(level):
            name = unique("w")
            try:
                spec = MANAGED["prepare"](name, make(level))
                text = re.sub(r'(?:\{\\"x\\":){2,}', '{\\"x\\":*', dumps(MANAGED["run"](spec, REF)))
                return R2["collapse"](text)
            except SystemExit:
                return "fail"
            finally:
                shutil.rmtree(os.path.join(MANAGED["FOLDER"], name), ignore_errors=True)

        bottom, top = low, high
        reference = at(bottom)
        if at(top) == reference:
            return {"error": f"no change between {low} and {high}", "reference": reference}
        while top - bottom > 1:
            middle = (bottom + top) // 2
            if at(middle) == reference:
                bottom = middle
            else:
                top = middle
        levels = list(range(bottom - 6, bottom + 8))
        jobs = [level for level in levels for _ in range(WINDOWS_PASSES)]
        answers, tally = [reference], collections.defaultdict(collections.Counter)
        for level, got in zip(jobs, quiet.map(at, jobs)):
            if got not in answers:
                answers.append(got)
            tally[str(level)][answers.index(got)] += 1
        return {"edge_guess": bottom, "levels": {k: dict(v) for k, v in tally.items()}, "answers": answers,
                "passes": WINDOWS_PASSES}
    return go


# A level is the number of nested containers in the shape: arrays as one object holding `level` arrays, objects as
# `level` objects, both under the member of `x`. Answer 0 of each unit is the answer at level 50.
MEMBER_SHAPES = {"arrays": lambda level: (1, level), "objects": lambda level: (level, 0)}


def member_units():
    return [
        (f"member:{label}:{kind}", answer_unit(
            (lambda member, shape: lambda level: [USER, PARENT, ["drop_a", member_doc(member, *shape(level))]])(member, shape)))
        for label, member in (("proto", "__proto__"), ("plain", "y"))
        for kind, shape in MEMBER_SHAPES.items()
    ]


# ---- the Windows discovery grid ----
# Families a fit of the Windows walk, clone and copy is made on: `a:K` holds K objects and moves the arrays inside them,
# `o:N` moves the objects around N arrays. The copy is read through `__proto__`, which the merge skips; the walk and
# the clone through the managed folder check, whose walk runs out first on objects and whose clone on arrays.
GRID = {
    "copy": {"a": (0, 150, 400, 700, 1000), "o": (0, 300, 700)},
    "held": {"a": (0, 200, 500, 800, 1100), "o": (0, 200, 600, 1000, 1400)},
}
OBSERVABLES = "ESOM"


def family_shape(axis, fixed, leaf="null_array"):
    return (lambda n: (fixed, n, leaf)) if axis == "a" else (lambda n: (n, fixed, leaf))


def copy_band(pool, shape_at, low=0, high=6000):
    """Bisects on the sources copy along one family, then tallies E, S, O and M at each level near its edge over 30
    passes."""
    def probe(level):
        got = proto_probe(shape_at(level))
        if not isinstance(got, dict):
            raise RuntimeError(f"a run failed at level {level}: {got}")
        return got

    if not probe(low)["S"] or probe(high)["S"]:
        return {"error": f"no edge between {low} and {high}"}
    while high - low > 1:
        middle = (low + high) // 2
        if probe(middle)["S"]:
            low = middle
        else:
            high = middle
    levels = list(range(low - 6, low + 9))
    jobs = [level for level in levels for _ in range(WINDOWS_PASSES)]
    tally = {o: collections.Counter() for o in OBSERVABLES}
    for level, got in zip(jobs, pool.map(probe, jobs)):
        for o in OBSERVABLES:
            tally[o][level] += 1 if got[o] else 0
    return {"edge_guess": low, "levels": {o: {str(level): tally[o][level] for level in levels} for o in OBSERVABLES},
            "passes": WINDOWS_PASSES}


def grid_unit(pass_name, axis, fixed, leaf="null_array"):
    shape_at = family_shape(axis, fixed, leaf)

    def go(pool, quiet):
        if pass_name == "copy":
            return copy_band(pool, shape_at)
        return windows_band(pool, lambda n: held_probe("platform", shape_at(n), "timer"), 0, 6000)
    return go


def twin_doc(member, shape, outer="object"):
    """A document holding the shape under `member` inside `x`, or inside the one element of an array at `x`."""
    objects, arrays = shape
    core = '{"%s":' % member + '{"x":' * objects + "[" * arrays + "[null]" + "]" * arrays + "}" * objects + "}"
    return '{"x":' + core + "}" if outer == "object" else '{"x":[' + core + "]}"


# Shapes where a merge could run out before the copy, each as a twin: under `__proto__`, which only the copy enters,
# and under `y`, which the merge enters too. The files of each case at level n.
ADVERSARIAL = {
    "xdoc_main": (lambda doc: [USER, ["managed", doc], ["drop_a", doc]], "object"),
    "xdoc_drops": (lambda doc: [USER, ["drop_a", doc], ["drop_b", doc]], "object"),
    "after_held": (lambda doc: [HELD_TWICE, ["drop_a", doc], LATER], "object"),
    "admin": (lambda doc: [USER, HELD, ["drop_a", doc]], "array"),
    "main": (lambda doc: [USER, ["managed", doc]], "object"),
}
HELD_TWICE, HELD, LATER = MANAGED["HELD_TWICE"], MANAGED["HELD"], MANAGED["LATER"]
TWIN_SHAPES = {"objects": lambda n: (n, 0), "arrays": lambda n: (1, n), "mixed": lambda n: (n, 1)}


def adversarial_units():
    out = []
    for family, (files, outer) in ADVERSARIAL.items():
        for kind, shape in TWIN_SHAPES.items():
            if kind == "mixed" and family != "after_held":
                continue
            for label, member in (("proto", "__proto__"), ("plain", "y")):
                make = (lambda f, m, sh, o: lambda level: f(twin_doc(m, sh(level), o)))(files, member, shape, outer)
                out.append((f"adv:{family}:{kind}:{label}", answer_unit(make)))
    return out


def discovery_units():
    out = []
    for pass_name, axes in GRID.items():
        for axis, values in axes.items():
            out += [(f"grid:{pass_name}:{axis}:{fixed}", grid_unit(pass_name, axis, fixed)) for fixed in values]
        out += [(f"leafw:{pass_name}:{axis}:{leaf}", grid_unit(pass_name, axis, 0, leaf))
                for axis in "ao" for leaf in LEAVES if leaf != "null_array"]
    adversarial = adversarial_units()
    # Interleaved so each shard gets a share of the slower answer units.
    mixed = []
    for index in range(max(len(out), len(adversarial))):
        mixed += out[index:index + 1] + adversarial[index:index + 1]
    return mixed


# ---- the Windows top-level and registry discovery ----
# Families a fit of the walk, clone, copy and merge of the settings files themselves is made on, and of the registry
# and plist documents. A level is the number of moving containers: `a:K` holds K objects and moves the arrays inside
# them, `o:N` moves the objects around N arrays. `first` is where the answer first differs from the answer at level 50,
# `last` where it first equals the answer at level 6000: with arrays the copy rejects before the reader skips the file.
DEPTH = D["DEPTH"]
TOP_GRID = {"a": (100, 300, 600, 900, 1100), "o": (1, 2, 4, 300, 700)}
TOP_LEAVES = {"empty_object": "{}", "empty_array": "[]", "true_array": "[true]", "true": "true"}
TOP_TWINS = {"alone": lambda doc: [("userSettings", doc)],
             "onto": lambda doc: [("userSettings", doc), ("projectSettings", doc)]}
REGISTRY_GRID = (("a", 150), ("a", 600), ("o", 300))


def grid_text(axis, fixed, level, leaf="null"):
    objects, arrays = (fixed, level) if axis == "a" else (level, fixed)
    return '{"x":' * objects + "[" * arrays + leaf + "]" * arrays + "}" * objects


def top_spec(name, files):
    """The oracle spec of settings files given as (source, text), laid out as the depth recorder lays them out."""
    root = os.path.join(DEPTH["FOLDER"], name)
    for folder in ("home/.claude", "work/.claude"):
        os.makedirs(os.path.join(root, folder))
    for source, text in files:
        write(os.path.join(root, DEPTH["PLACES"][source]), text)
    spec = {"root": root, "platform": "linux", "env": {"HOME": os.path.join(root, "home")}, "deep": True,
            "cases": [{"name": name, "options": {"cwd": os.path.join(root, "work")}}]}
    path = os.path.join(DEPTH["FOLDER"], name + ".spec.json")
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(spec, handle)
    return path


def squash(answered):
    return R2["collapse"](re.sub(r'(?:\{\\"x\\":){2,}', '{\\"x\\":*', dumps(answered)))


def top_at(make):
    def at(level):
        name = unique("t")
        try:
            return squash(DEPTH["run"](top_spec(name, make(level)), REF))
        except SystemExit:
            return "fail"
        finally:
            shutil.rmtree(os.path.join(DEPTH["FOLDER"], name), ignore_errors=True)
            spec = os.path.join(DEPTH["FOLDER"], name + ".spec.json")
            if os.path.exists(spec):
                os.remove(spec)
    return at


PLAIN_DEEP = D["deep"]


def registry_deep(shape, n):
    if isinstance(shape, tuple) and shape[0] == "raw":
        # A document made whole by the caller: `n` is ignored.
        return shape[1]
    if shape == "xp":
        # Arrays under `__proto__`: the policy read keeps them on its copy of the document, and the merge skips them.
        return '{"__proto__":' + "[" * n + "null" + "]" * n + "}"
    return PLAIN_DEEP(shape, n) if isinstance(shape, str) else grid_text(shape[0], shape[1], n)


D["deep"] = registry_deep


def registry_at(place, key, shape):
    probe = D["registry_probe"](place, key, shape)
    return lambda level: probe(level, REF)


def transition_unit(at, last=False, low=50, high=6000, passes=WINDOWS_PASSES, below=6, above=7):
    """Where the answer first differs from the answer at `low`, or with `last` first equals the answer at `high`,
    found by bisection, then each level near it run `passes` times with every distinct answer it gave."""
    def go(pool, quiet):
        bottom, top = low, high
        reference, final = at(bottom), at(top)
        if final == reference:
            return {"error": f"no change between {low} and {high}", "reference": reference}
        before = (lambda got: got != final) if last else (lambda got: got == reference)
        while top - bottom > 1:
            middle = (bottom + top) // 2
            if before(at(middle)):
                bottom = middle
            else:
                top = middle
        levels = list(range(bottom - below, bottom + above + 1))
        jobs = [level for level in levels for _ in range(passes)]
        answers, tally = [reference, final], collections.defaultdict(collections.Counter)
        for level, got in zip(jobs, quiet.map(at, jobs)):
            if got not in answers:
                answers.append(got)
            tally[str(level)][answers.index(got)] += 1
        return {"edge_guess": bottom, "levels": {k: dict(v) for k, v in tally.items()}, "answers": answers,
                "passes": passes}
    return go


def top_units():
    out = []
    for axis, values in TOP_GRID.items():
        for fixed in values:
            make = (lambda a, f: lambda n: [("userSettings", "{" + '"x":' + grid_text(a, f, n) + "}")])(axis, fixed)
            out += [(f"depth:{axis}:{fixed}:{side}", transition_unit(top_at(make), side == "last"))
                    for side in ("first", "last")]
    for label, leaf in TOP_LEAVES.items():
        make = (lambda lf: lambda n: [("userSettings", '{"x":' + grid_text("o", 1, n, lf) + "}")])(leaf)
        out.append((f"depth:leaf:{label}", transition_unit(top_at(make))))
    for place, files in TOP_TWINS.items():
        for kind, shape in TWIN_SHAPES.items():
            if kind == "mixed":
                continue
            for label, member in (("proto", "__proto__"), ("plain", "y")):
                make = (lambda fl, m, sh: lambda n: fl(twin_doc(m, sh(n))))(files, member, shape)
                out.append((f"depth:twin:{place}:{kind}:{label}", transition_unit(top_at(make))))
    return out


def registry_units():
    return [(f"regd:{place}:{key}:{axis}:{fixed}", transition_unit(registry_at(place, key, (axis, fixed))))
            for place in ("hklm", "hkcu", "plist") for key in ("x", "model") for axis, fixed in REGISTRY_GRID]



# ---- round 4: the registry and plist reads with a real program end, and the Windows edges the fit left open ----
# Every registry and plist read here runs with `tools: "real"`: the stand-in program's own end runs the callback, as
# the real `reg.exe` and `plutil` end does upstream. The timer stand-in left the resumed stack in a state upstream never
# has, so its edges are not fit data.
ROUND4_PASSES = int(os.environ.get("R3_ROUND4_PASSES", "60"))
BAND_RUNS = int(os.environ.get("R3_BAND_RUNS", "50"))
LADDER_RUNS = 12
LADDER_START = {"o": 1000, "a": 1500, "xp": 1000}
LADDER_TOP = 40000
# The bytes between the two stacks a program's end resumes on, from the managed folder check's pooled tool bands
# (darwin walk and clone, linux aarch64 and x86_64 alike for both): the dense window reaches this far below the
# fine ladder's first all-fail level, so it holds the tight edge when the read resumes on two stacks.
SPLIT_BYTES = {"darwin arm64": 34656, "linux aarch64": 23040, "linux x86_64": 17632}
REG_PASS = {"o": "walk", "a": "clone", "xp": "copy"}


def reg_real_at(place, key, shape):
    """The answer of a registry or plist read holding `shape` at `key`, `level` deep, with the program's real end."""
    def at(level):
        root = os.path.join(D["SCRATCH"], unique("q"))
        spec_path = root + ".json"
        try:
            spec = D["registry_spec"](place, key, shape, level, root)
            spec["tools"] = "real"
            with open(spec_path, "w", encoding="utf-8") as handle:
                json.dump(spec, handle)
            answered, failure = project(R2["oracle"](spec_path, REF), root)
            return answered if failure is None else None
        finally:
            shutil.rmtree(root, ignore_errors=True)
            if os.path.exists(spec_path):
                os.remove(spec_path)
    return at


def step_of(shape):
    """The bytes one more level of `shape` costs the pass that runs out on it."""
    obj, arr, _ = MODEL[HOST][REG_PASS[shape]]
    return obj if shape == "o" else arr


def tally_levels(quiet, at, reference, levels, runs):
    """How many of `runs` runs at each level gave the reference answer, every other answer each run gave, or the
    level where a run failed."""
    jobs = [level for level in levels for _ in range(runs)]
    tally, others = collections.Counter(), collections.defaultdict(collections.Counter)
    for level, got in zip(jobs, quiet.map(at, jobs)):
        if got is None:
            return None, None, level
        if got == reference:
            tally[level] += 1
        else:
            others[level][got[:300]] += 1
    return tally, others, None


def ladder(quiet, at, reference, start, step, top):
    """Levels from `start` upward by `step`, LADDER_RUNS runs each, up to the first where no run gives the reference
    answer: that level, and every level run with its count."""
    seen, level = {}, start
    while level <= top:
        tally, _, failed = tally_levels(quiet, at, reference, [level], LADDER_RUNS)
        if tally is None:
            return None, seen, f"a run failed at level {failed}"
        seen[str(level)] = tally[level]
        if tally[level] == 0:
            return level, seen, None
        level += step
    return None, seen, f"no level up to {top} where every run differs from the reference"


def bracketed_band(places, key, shape):
    """The edge of `shape` at `key` over `places`, bracketed first: a coarse ladder of 100 levels upward until no run
    keeps the reference answer, a fine ladder of 10 levels inside the last step, then every level of the dense window
    BAND_RUNS times. The window reaches from the fine ladder's first all-fail level down past the split between the
    two stacks a read may resume on, and is the same for every place so the places pool."""
    def go(pool, quiet):
        out = {"passes": BAND_RUNS, "step_bytes": step_of(shape), "places": {}}
        probes, references, fails = {}, {}, []
        for place in places:
            at = reg_real_at(place, key, shape)
            reference = at(50)
            if reference is None or at(50) != reference:
                return {"error": f"{place}: the reference answer at level 50 does not repeat"}
            probes[place], references[place] = at, reference
            coarse, coarse_seen, error = ladder(quiet, at, reference, LADDER_START[shape], 100, LADDER_TOP)
            entry = {"reference": reference[:600], "coarse": coarse_seen}
            out["places"][place] = entry
            if error:
                entry["error"] = error
                out["error"] = f"{place}: {error}"
                return out
            if coarse == LADDER_START[shape]:
                entry["error"] = f"no run keeps the reference answer at the first rung {coarse}"
                out["error"] = f"{place}: {entry['error']}"
                return out
            fine, fine_seen, error = ladder(quiet, at, reference, coarse - 90, 10, coarse)
            entry["fine"] = fine_seen
            if error:
                entry["error"] = error
                out["error"] = f"{place}: {error}"
                return out
            fails.append(fine)
        top = max(fails) + 6
        split_levels = -(-SPLIT_BYTES[HOST] // step_of(shape))
        low = min(fails) - 10 - split_levels - 16
        out["window"] = [low, top]
        levels = list(range(low, top + 1))
        for place in places:
            tally, others, failed = tally_levels(quiet, probes[place], references[place], levels, BAND_RUNS)
            entry = out["places"][place]
            if tally is None:
                entry["error"] = f"a run failed at level {failed}"
                out["error"] = f"{place}: a run failed at level {failed}"
                return out
            entry["levels"] = {str(level): tally[level] for level in levels}
            entry["other_answers"] = {str(level): dict(v) for level, v in sorted(others.items())[:3]}
            if tally[low] != BAND_RUNS or tally[top] != 0:
                entry["error"] = f"the window {low}..{top} does not reach from all kept to none kept"
                continue
            tight = max(n for n in levels if all(tally[j] == BAND_RUNS for j in range(low, n + 1)))
            fail = max(n for n in levels if tally[n]) + 1
            between = [tally[j] for j in range(tight + 1, fail)]
            entry.update({"tight": tight, "fail": fail, "split": (fail - 1 - tight) * step_of(shape),
                          "roomy_share": round(sum(between) / (BAND_RUNS * len(between)), 3) if between else None})
        return out
    return go


# 3a: the policy read of a registry and plist document holding `model` deep, which the schema read strips, so its walk
# (objects) or clone (arrays) runs out first. 3b: `x` holding arrays under `__proto__`, which the policy read keeps and
# copies while the merge skips them, so the copy runs out first. 3c: the held machine policy of an organization
# session, with the depth under an allowed server entry the policy read keeps.
UNIX_REG = [
    ("reg3a:model:o", ("hklm", "plist"), "model", "o"),
    ("reg3a:model:a", ("hklm", "plist"), "model", "a"),
    ("reg3b:x:xp:tool", ("hklm", "plist"), "x", "xp"),
    ("reg3b:x:xp:fs", ("hkcu",), "x", "xp"),
    ("reg3c:x:o", ("held_hklm", "held_hkcu"), "x", "o"),
    ("reg3c:x:a", ("held_hklm", "held_hkcu"), "x", "a"),
]


def unix_round4_units():
    return [(name, bracketed_band(places, key, shape)) for name, places, key, shape in UNIX_REG]


# The Windows copy and clone edges of the root + N arrays chain, at the levels between the two fits the holdout told
# apart, over every member of the pooled chain. A member's level N is the number of arrays in its deep file, so each
# makes that file with N arrays; `onto_plus` is the member the holdout recorded one level apart, the same files at N.
U, PS = "userSettings", "projectSettings"
CHAIN = {
    "alone": lambda n: [(U, DEPTH["text"](1, n, 0))],
    "onto": lambda n: [(U, DEPTH["text"](1, 1, 0)), (PS, DEPTH["text"](1, n, 0))],
    "onto_plus": lambda n: [(U, DEPTH["text"](1, 1, 0)), (PS, DEPTH["text"](1, n, 0))],
    "under": lambda n: [(U, DEPTH["text"](500, 0, 0)), (PS, DEPTH["text"](1, n, 0))],
}
CHAIN_LEVELS = {"copy": range(1199, 1203), "clone": range(1836, 1840)}


def fixed_unit(at, levels, passes):
    """Every level of `levels` run `passes` times, with every distinct answer it gave."""
    def go(pool, quiet):
        jobs = [level for level in levels for _ in range(passes)]
        answers, tally = [], collections.defaultdict(collections.Counter)
        for level, got in zip(jobs, quiet.map(at, jobs)):
            if got not in answers:
                answers.append(got)
            tally[str(level)][answers.index(got)] += 1
        return {"levels": {k: dict(v) for k, v in tally.items()}, "answers": answers, "passes": passes}
    return go


def chain_units():
    out = []
    for edge, levels in CHAIN_LEVELS.items():
        for label, make in CHAIN.items():
            out.append((f"chain:{edge}:{label}", fixed_unit(top_at(make), list(levels), ROUND4_PASSES)))
    return out


# Empty object leaves: the walk of a file of nested objects ending in `{}` read after a shallow file, and the copy of
# one file of nested arrays ending in `{}`, each bracketed around where its answer changes.
def leaf_units():
    walk = lambda n: [(U, DEPTH["text"](1, 1, 0)), (PS, DEPTH["text"](n, 0, 0, "{}"))]
    copy = lambda n: [(U, DEPTH["text"](1, n, 0, "{}"))]
    return [(f"eoleaf:{label}", transition_unit(top_at(make), passes=ROUND4_PASSES, below=4, above=5))
            for label, make in (("walk", walk), ("copy", copy))]


def windows_reg_units():
    return [(f"wreg:{place}:{key}:{shape}", transition_unit(reg_real_at(place, key, shape), passes=ROUND4_PASSES))
            for place in ("hklm", "hkcu", "plist") for key, shape in (("model", "o"), ("model", "a"), ("x", "xp"))]


# ---- round 5: the registry reads resumed from a file request, to the byte, and the Windows edges left open ----
# H: the held machine policy of an organization session, read from hklm or hkcu after the managed folder check's file
# requests complete; the hklm read of the nested evaluation sits one frame deeper than the hkcu read, so each place
# gets its own class. K: the hkcu policy read outside the held branch, which resumes from the same file requests.
# Each Search finds the class C to 16 bytes with a real program end, and on macOS the byte. A run fits when its
# answer equals the answer of a shallow document of the same form: the collapsed answer only tells 0, 1 and many
# containers of a run apart, so a form keeps 0 and 1 and puts 2 for many.
HELD_ABOVE = (2, 1)
REG5 = [
    # name, place, key, pass, major, above, wrap
    ("reg5:H1:held_hkcu:walk", "held_hkcu", "x", "walk", "o", HELD_ABOVE, None),
    ("reg5:H2:held_hklm:walk", "held_hklm", "x", "walk", "o", HELD_ABOVE, None),
    ("reg5:H3:held_hkcu:clone", "held_hkcu", "x", "clone", "a", HELD_ABOVE, None),
    ("reg5:H4:held_hklm:clone", "held_hklm", "x", "clone", "a", HELD_ABOVE, None),
    ("reg5:K1:hkcu:walk", "hkcu", "model", "walk", "o", 1, None),
    ("reg5:K2:hkcu:clone", "hkcu", "model", "clone", "a", 1, None),
    ("reg5:K3:hkcu:copy", "hkcu", "x", "copy", "a", 2, "__proto__"),
    # The clone of the held reads with the text padded past the 8192 unit parse cache by a 7000 character key, so the
    # nested read clones again one frame deeper; its split must agree with the walk's.
    ("reg5:H3p:held_hkcu:clone", "held_hkcu", "x", "clone", "a", (3, 1), "p" * 7000),
    ("reg5:H4p:held_hklm:clone", "held_hklm", "x", "clone", "a", (3, 1), "p" * 7000),
]
REG5_LEAVES = {"walk": ["null_array"], "clone": ["null_array"], "copy": ["null_array", "empty_array"]}
REFERENCES, REFERENCE_LOCK = {}, __import__("threading").Lock()


def form_of(shape):
    objects, arrays, leaf = shape
    return (min(objects, 2), min(arrays, 2), leaf)


def reg5_text(shape, wrap):
    text = deep(*shape)
    return text if wrap is None else '{"' + wrap + '":' + text + "}"


def reg5_answer(place, key, shape, wrap, flags, startup):
    """The collapsed answer of one read holding `shape`, the rejection it surfaces, or None when the run failed."""
    root = os.path.join(D["SCRATCH"], unique("k"))
    spec_path = root + ".json"
    try:
        spec = D["registry_spec"](place, key, ("raw", reg5_text(shape, wrap)), 0, root)
        spec["tools"] = "real"
        with open(spec_path, "w", encoding="utf-8") as handle:
            json.dump(spec, handle)
        answered = run_spec(spec_path, flags or REF, startup)
        if isinstance(answered, dict):
            return dumps({"error": answered["rejected"]}) if "rejected" in answered else None
        text, failure = project(answered, root)
        return None if failure is not None else text
    finally:
        shutil.rmtree(root, ignore_errors=True)
        if os.path.exists(spec_path):
            os.remove(spec_path)


def reg5_fits(place, key, wrap):
    def fits(shape, flags, startup):
        form = (place, key, wrap, form_of(shape))
        with REFERENCE_LOCK:
            reference = REFERENCES.get(form)
        if reference is None:
            reference = reg5_answer(place, key, form_of(shape), wrap, REF, STARTUP)
            if reference is None or reference.startswith('{"error"'):
                raise RuntimeError(f"the shallow form {form} has no answer: {reference}")
            with REFERENCE_LOCK:
                REFERENCES[form] = reference
        got = reg5_answer(place, key, shape, wrap, flags, startup)
        return None if got is None else got == reference
    return fits


def reg5_unit(place, key, which, major, above, wrap):
    def go(pool, quiet):
        found = Search(pool, reg5_fits(place, key, wrap), MODEL[HOST][which], above, major, REG5_LEAVES[which]).run()
        found["above"] = list(above) if isinstance(above, tuple) else above
        if "class" in found:
            found["room_at_class"] = found["class"] - prefix(MODEL[HOST][which], above)
        return found
    return go


# The arm64 xp:tool top-up: every pre-registered level of round 4's window, at both places, TOPUP_RUNS times each, so
# the pooled total per level reaches 700 with round 4's 600.
TOPUP_LEVELS = (1745, 1746, 1747, 1788)
TOPUP_RUNS = int(os.environ.get("R5_TOPUP_RUNS", "50"))


def topup_unit():
    def go(pool, quiet):
        out = {"runs": TOPUP_RUNS, "places": {}}
        for place in ("hklm", "plist"):
            at = reg_real_at(place, "x", "xp")
            reference = at(50)
            if reference is None or at(50) != reference:
                return {"error": f"{place}: the reference answer at level 50 does not repeat"}
            tally, others, failed = tally_levels(quiet, at, reference, list(TOPUP_LEVELS), TOPUP_RUNS)
            if tally is None:
                return {"error": f"{place}: a run failed at level {failed}"}
            out["places"][place] = {"levels": {str(level): tally[level] for level in TOPUP_LEVELS},
                                    "other_answers": {str(level): dict(v) for level, v in sorted(others.items())[:2]}}
        return out
    return go


def round5_unix_units():
    out = [(name, reg5_unit(place, key, which, major, above, wrap))
           for name, place, key, which, major, above, wrap in REG5]
    if HOST == "linux aarch64":
        out.append(("reg5:topup:xp:tool", topup_unit()))
    return out


# 16a: the clone edge of the root + N arrays chain at 1837 and 1838 only, over every member. 16b: the walk of a project
# file of nested objects ending in `{}` merged into an object path, with `x` a scalar or absent in the user file; a
# shallow control must show the deep value whole in the effective settings before any pass runs.
CHAIN5_LEVELS = (1837, 1838)
CHAIN5_PASSES = int(os.environ.get("R5_CHAIN_PASSES", "100"))
EOLEAF5_USER = {"scalar": '{"u":[null],"x":0}', "absent": '{"u":[null]}'}
EOLEAF5_CONTROL = 50


def eoleaf5_files(user):
    return lambda n: [(U, user), (PS, DEPTH["text"](n, 0, 0, "{}"))]


def eoleaf5_control(make):
    """Whether the deep value reaches the effective settings whole at the control level: `x` followed CONTROL times
    ends at `{}`."""
    name = unique("v")
    try:
        answered = DEPTH["run"](top_spec(name, make(EOLEAF5_CONTROL)), REF)
    except SystemExit:
        return False, "the control run failed"
    finally:
        shutil.rmtree(os.path.join(DEPTH["FOLDER"], name), ignore_errors=True)
        spec = os.path.join(DEPTH["FOLDER"], name + ".spec.json")
        if os.path.exists(spec):
            os.remove(spec)
    if answered["effective"] is None:
        return False, answered["error"]
    value, steps = json.loads(answered["effective"]), 0
    while isinstance(value, dict) and "x" in value:
        value, steps = value["x"], steps + 1
    end = value if not isinstance(value, (dict, list)) or len(value) < 4 else "container"
    return steps == EOLEAF5_CONTROL and value == {}, {"steps": steps, "end": end}


def eoleaf5_unit(label):
    make = eoleaf5_files(EOLEAF5_USER[label])
    passes = transition_unit(top_at(make), passes=ROUND4_PASSES, below=4, above=5)

    def go(pool, quiet):
        visible, detail = eoleaf5_control(make)
        if not visible:
            return {"error": "the control does not show the deep value whole", "control": detail}
        found = passes(pool, quiet)
        found["control"] = detail
        return found
    return go


def round5_windows_units():
    out = [(f"chain5:clone:{label}", fixed_unit(top_at(make), list(CHAIN5_LEVELS), CHAIN5_PASSES))
           for label, make in CHAIN.items()]
    out += [(f"eoleaf5:walk:{label}", eoleaf5_unit(label)) for label in EOLEAF5_USER]
    return out


# Round 6, E'': the copy of the held reads in K3's form (key `x` under a `__proto__` wrap), whose class round 5 resolved
# to 16 bytes on Linux. The text stays under the 8192 unit parse cache, so the nested read of hklm runs only the copy.
HELD_COPY_ABOVE = (3, 1)
REG6 = [
    ("reg6:H5:held_hkcu:copy", "held_hkcu", "x", "copy", "a", HELD_COPY_ABOVE, "__proto__"),
    ("reg6:H6:held_hklm:copy", "held_hklm", "x", "copy", "a", HELD_COPY_ABOVE, "__proto__"),
]


def round6_held_units():
    return [(name, reg5_unit(place, key, which, major, above, wrap))
            for name, place, key, which, major, above, wrap in REG6]


# Round 6, XP: machineLoginPins reads the managed file again on a fresh store (XP, sdk.mjs 165:61413), which runs only
# when the remote tier is present (165:66832). `src` reads the file without the remote tier, `xp` with it; the file
# pins a login organization, so a read that runs out only at the XP frame drops the pin and keeps the file loaded.
XP6_FORMS = {"walk": ("model", None, 1, "o"), "clone": ("model", None, 1, "a"), "copy": ("x", "__proto__", 2, "a")}
XP6_PASSES = {"darwin arm64": ("walk", "clone"), "linux aarch64": ("walk", "copy"), "linux x86_64": ("walk", "copy")}
XP6_WINDOWS_PASSES = ("walk", "clone")
XP6_REMOTE = {"model": "remote"}


def xp6_text(key, wrap, shape):
    inner = deep(*shape)
    if wrap is not None:
        inner = '{"' + wrap + '":' + inner + "}"
    return '{"forceLoginOrgUUID":"f","' + key + '":' + inner + "}"


def xp6_answer(text, remote, flags, startup):
    """The collapsed answer of a resolve whose managed file holds `text`, the rejection it surfaces, or None when the
    run failed."""
    name = unique("xp")
    spec_path = MANAGED["prepare"](name, [["managed", text]])
    try:
        with open(spec_path, encoding="utf-8") as handle:
            spec = json.load(handle)
        if remote:
            for case in spec["cases"]:
                case["options"]["serverManagedSettings"] = XP6_REMOTE
        with open(spec_path, "w", encoding="utf-8") as handle:
            json.dump(spec, handle)
        answered = run_spec(spec_path, flags or REF, startup)
        if isinstance(answered, dict):
            return dumps({"error": answered["rejected"]}) if "rejected" in answered else None
        got, failure = project(answered, spec["root"])
        return None if failure is not None else got
    finally:
        shutil.rmtree(os.path.join(MANAGED["FOLDER"], name), ignore_errors=True)
        if os.path.exists(spec_path):
            os.remove(spec_path)


def xp6_fits(which, remote):
    key, wrap, _, _ = XP6_FORMS[which]
    references = {}

    def fits(shape, flags, startup):
        form = form_of(shape)
        with REFERENCE_LOCK:
            reference = references.get(form)
        if reference is None:
            reference = xp6_answer(xp6_text(key, wrap, form), remote, REF, STARTUP)
            if reference is None or reference.startswith('{"error"'):
                raise RuntimeError(f"the shallow form {form} has no answer: {reference}")
            with REFERENCE_LOCK:
                references[form] = reference
        got = xp6_answer(xp6_text(key, wrap, shape), remote, flags, startup)
        return None if got is None else got == reference
    return fits


def xp6_unit(which, remote):
    key, wrap, above, major = XP6_FORMS[which]

    def go(pool, quiet):
        found = Search(pool, xp6_fits(which, remote), MODEL[HOST][which], above, major, REG5_LEAVES[which]).run()
        found["above"] = above
        if found.get("far"):
            # Which read ran out past the class: the XP frame drops the pin and keeps the file loaded.
            found["far_answer"] = xp6_answer(xp6_text(key, wrap, tuple(found["far"][0])), remote, REF, STARTUP)
        return found
    return go


def xp6_windows_at(which, remote):
    key, wrap, _, major = XP6_FORMS[which]

    def at(level):
        shape = (level, 0, "null_array") if major == "o" else (0, level, "null_array")
        got = xp6_answer(xp6_text(key, wrap, shape), remote, REF, STARTUP)
        return "fail" if got is None else got
    return at


def round6_xp_units():
    if WINDOWS:
        return [(f"xp6:{side}:{which}", transition_unit(xp6_windows_at(which, side == "xp"), passes=ROUND4_PASSES))
                for which in XP6_WINDOWS_PASSES for side in ("src", "xp")]
    return [(f"xp6:{side}:{which}", xp6_unit(which, side == "xp"))
            for which in XP6_PASSES.get(HOST, ()) for side in ("src", "xp")]


def units():
    out = [(f"proto:{o}", proto_unit(o)) for o in "ESOM"]
    if not WINDOWS:
        out += [(f"leaf:{leaf}", leaf_unit(leaf)) for leaf in LEAVES]
        # held:fs:walk measures the walk with null_array, which the other leaves are measured against.
        out += [(f"walk_leaf:{leaf}", walk_leaf_unit(leaf)) for leaf in LEAVES if leaf != "null_array"]
        if HOST == "linux x86_64":
            out += [(f"leaf_{o.lower()}:{leaf}", leaf_unit(leaf, o)) for o in "EOM" for leaf in LEAVES]
    out += [(f"held:fs:{which}", held_unit("fs", which)) for which in ("walk", "clone")]
    if WINDOWS:
        out += [(f"held:tool:{which}", held_unit("tool", which)) for which in ("walk", "clone")]
    else:
        out += [(f"held_band:tool:{which}:{job}", tool_band(which)) for which in ("walk", "clone") for job in range(1, TOOL_JOBS + 1)]
    if WINDOWS:
        guesses = windows_guesses()
        out += [(f"key:{key}", key_unit(key, guess, guesses)) for key, guess in guesses.items()]
        out += member_units()
        out += discovery_units()
        out += top_units() + registry_units()
        out += chain_units() + leaf_units() + windows_reg_units()
        out += round5_windows_units()
        out += round6_xp_units()
    else:
        out += unix_round4_units() + round5_unix_units() + round6_held_units() + round6_xp_units()
        profile = MANAGED["PROFILES"].get(HOST, {})
        out += [(f"key:{key}", key_unit(key, guess)) for key, guess in GUESSES.get(HOST, {}).items() if key not in profile]
        out += [(f"key:{key}", key_unit(key, guess, profile)) for key, guess in PASS_KEYS.get(HOST, {}).items()]
        if os.environ.get("R3_DISCOVERY"):
            out += discovery_units()
    return out


def main():
    version = subprocess.run([NODE, "--version"], capture_output=True, check=True).stdout.decode().strip()
    result = {"host": HOST, "node": version, "shard": SHARD, "shards": SHARDS, "machine": machine(), "units": {}}
    started = time.time()
    only = __import__("re").compile(os.environ["R3_ONLY"]) if os.environ.get("R3_ONLY") else None
    matching = [unit for unit in units() if not only or only.search(unit[0])]
    # R3_EVERY_SHARD runs every matching unit in each shard: each shard is one job of a pooled band.
    chosen = matching if os.environ.get("R3_EVERY_SHARD") else matching[SHARD - 1::SHARDS]
    with concurrent.futures.ThreadPoolExecutor(max_workers=WORKERS) as pool, \
            concurrent.futures.ThreadPoolExecutor(max_workers=QUIET) as quiet:
        for name, go in chosen:
            try:
                result["units"][name] = go(pool, quiet)
            except Exception as error:  # a unit that cannot finish reports why and the next one runs
                result["units"][name] = {"error": repr(error)}
            result["units"][name]["seconds"] = round(time.time() - started)
            with open(OUT, "w", encoding="utf-8") as handle:
                json.dump(result, handle)
            brief = {k: v for k, v in result["units"][name].items() if k in ("class", "startup_max", "budget", "error", "changes", "edge_guess", "profile", "window", "room_at_class")}
            sys.stderr.write(f"{name} {json.dumps(brief)[:600]}\n")
            sys.stderr.flush()
    shutil.rmtree(D["SCRATCH"], ignore_errors=True)


if os.environ.get("R3_LIST"):
    listed = re.compile(os.environ["R3_ONLY"]) if os.environ.get("R3_ONLY") else None
    for number, (name, _) in enumerate(unit for unit in units() if not listed or listed.search(unit[0])):
        print(number % SHARDS + 1, name)
else:
    main()
