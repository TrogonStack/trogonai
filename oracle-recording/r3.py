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
GATE = D["GATE"]
HOST, WINDOWS, DARWIN = D["HOST"], D["WINDOWS"], sys.platform == "darwin"
REF, CROSS = R2["REF"], R2["CROSS"]
probe_flags, DEFAULT_FLAGS = D["probe_flags"], D["DEFAULT_FLAGS"]
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
    command = [NODE, *probe_flags(flags), MANAGED["ORACLE"], PACKAGE, spec_path]
    with tempfile.TemporaryFile() as errors:
        process = D["spawn"](command, {"PAD": "x" * MANAGED["pad"](command, startup)}, stdout=subprocess.PIPE,
                             stderr=errors)
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
                text = re.sub(r'(?:\{\\"x\\":){2,}', '{\\"x\\":*', dumps(MANAGED["run"](spec, probe_flags(flags))))
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
                text = re.sub(r'(?:\{\\"x\\":){2,}', '{\\"x\\":*', dumps(MANAGED["run"](spec, probe_flags(REF))))
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
    spec = {"root": root, "platform": sys.platform, "env": {"HOME": os.path.join(root, "home")}, "deep": True,
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
            return squash(DEPTH["run"](top_spec(name, make(level)), probe_flags(REF)))
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
        answered = DEPTH["run"](top_spec(name, make(EOLEAF5_CONTROL)), probe_flags(REF))
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


# ---- round 7: the held copy with its depth under a top-level key, gated on a visibility control, and the fixtures ----
# Round 6 held its depth under a member of an allowed server entry, which the schema read drops before the copy (oX,
# sdk.mjs 165:30387), so only the clone bound. Round 7 holds it under a top-level `x` wrapped in `__proto__`, which the
# schema read keeps. Each place first runs a control: the same document with a shallow `x`, read by a copy of the
# pinned package with log-only hooks, must show `x` as an own data `__proto__` on a plain object at the parse (hm), the
# clone (j7e), the schema output and the copy (oX), and print the same output as the pinned package.
R7_TRACE_FUNCTIONS = (
    'function __tr(t,k,v){try{const f=process.env.TRACE_LOG;if(f){process.getBuiltinModule("fs").appendFileSync(f,'
    'JSON.stringify({label:String(t),stage:k,info:__ti(v)})+"\\n")}}catch(q){}return v}'
    'function __ti(v){const s=v&&typeof v==="object"&&"loadState" in v?v.settings:v;if(!s||typeof s!=="object")'
    'return{settings:null};const out={keys:Object.keys(s)};if(Object.prototype.hasOwnProperty.call(s,"x")){const x=s.x;'
    'const d=x&&typeof x==="object"?Object.getOwnPropertyDescriptor(x,"__proto__"):undefined;out.x={json:'
    'JSON.stringify(x),protoOwn:!!d,protoData:!!d&&"value" in d,protoEnumerable:!!d&&d.enumerable,plainPrototype:'
    'x!==null&&typeof x==="object"&&Object.getPrototypeOf(x)===Object.prototype}}if("y" in s)out.y=JSON.stringify(s.y);'
    'if(Array.isArray(s.allowedMcpServers))out.entries=s.allowedMcpServers.map(e=>e&&typeof e==="object"?'
    'Object.keys(e):typeof e);return out}'
)
R7_PATCHES = [
    ("function pm(", R7_TRACE_FUNCTIONS + "function pm("),
    ("if(r)return oX(r);", 'if(r)return __tr(t,"copy-hit",oX(r));'),
    ("return oX(o)}", 'return __tr(t,"copy",oX(o))}'),
    ("function j7e(e,t){let n=Tr(e),", 'function j7e(e,t){let n=__tr(t,"clone",Tr(e)),'),
    ("cP(CX(t,o),t,n).safeParse(n);", 'cP(CX(t,o),t,n).safeParse(n);__tr(t,"schema",s.success?s.data:null);'),
    ("function hm(e,t,{userWritable:n=!1}={}){let r=Ya(e,!1);",
     'function hm(e,t,{userWritable:n=!1}={}){let r=__tr(t,"parse",Ya(e,!1));'),
]
R7_STAGES = ("parse", "clone", "schema")
R7_SHALLOW_X = '{"__proto__":[[null]]}'
R7_PLACES = ("held_hkcu", "held_hklm")
R7_STATE, R7_LOCK = {}, __import__("threading").Lock()


def r7_sha256(location):
    with open(location, "rb") as handle:
        return __import__("hashlib").sha256(handle.read()).hexdigest()


def r7_traced():
    """A copy of the pinned package whose sdk.mjs carries the log-only hooks, with both hashes; every anchor must occur
    exactly once."""
    with R7_LOCK:
        if "traced" not in R7_STATE:
            target = os.path.join(os.path.dirname(OUT), "package-traced")
            shutil.rmtree(target, ignore_errors=True)
            shutil.copytree(PACKAGE, target)
            bundle = os.path.join(target, "sdk.mjs")
            with open(bundle, encoding="utf-8") as handle:
                text = handle.read()
            counts = {anchor: text.count(anchor) for anchor, _ in R7_PATCHES}
            if any(count != 1 for count in counts.values()):
                R7_STATE["traced"] = {"error": "an anchor does not occur exactly once", "counts": counts}
            else:
                for anchor, replacement in R7_PATCHES:
                    text = text.replace(anchor, replacement)
                with open(bundle, "w", encoding="utf-8", newline="") as handle:
                    handle.write(text)
                R7_STATE["traced"] = {"package": target, "pinned_sha256": r7_sha256(os.path.join(PACKAGE, "sdk.mjs")),
                                      "traced_sha256": r7_sha256(bundle)}
        return R7_STATE["traced"]


def r7_oracle(package, spec_path, env):
    command = [NODE, *probe_flags(DEFAULT_FLAGS, DEFAULT_FLAGS), MANAGED["ORACLE"], package, spec_path]
    # Fixture recordings start Node with its defaults and no PAD: their answers do not depend on the depth budget.
    process = D["spawn"](command, env, unpadded=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=600)
    return process.returncode, process.stdout.decode("utf-8", "replace"), process.stderr.decode("utf-8", "replace")[-600:]


def r7_document(x):
    return '{"allowedMcpServers":[{"serverName":"h","x":null}],"x":' + x + ',"y":[{"a":null}]}'


def r7_spec(place, x, root):
    spec = D["registry_spec"](place, "x", ("raw", "null"), 0, root)
    write(os.path.join(root, "mnt/c/Windows/System32", place[5:] + ".txt"), D["reg_text"](r7_document(x)))
    return spec


def r7_control(place):
    """Whether the shallow document shows `x` reaching the copy at `place` as an own data `__proto__`, with the trace."""
    traced = r7_traced()
    if "error" in traced:
        return False, traced
    root = os.path.join(D["SCRATCH"], unique("v"))
    spec_path, trace = root + ".json", root + ".trace"
    env = {**D["EXTRA_ENV"], "TRACE_LOG": trace}

    def fresh(package):
        # The oracle writes each step's files under the root, so every run starts from a newly built root.
        shutil.rmtree(root, ignore_errors=True)
        spec = r7_spec(place, R7_SHALLOW_X, root)
        spec["tools"] = "real"
        with open(spec_path, "w", encoding="utf-8") as handle:
            json.dump(spec, handle)
        if os.path.exists(trace):
            os.remove(trace)
        return r7_oracle(package, spec_path, env)
    try:
        pinned = fresh(PACKAGE)
        instrumented = fresh(traced["package"])
        records = []
        if os.path.exists(trace):
            with open(trace, encoding="utf-8") as handle:
                records = [json.loads(line) for line in handle if line.strip()]
    finally:
        shutil.rmtree(root, ignore_errors=True)
        for location in (spec_path, trace):
            if os.path.exists(location):
                os.remove(location)
    # The native oracle marks where its recorded cases start. A warm second call reads every record, since its prime
    # holds the first parse; a first call reads the cases' records. The marker itself carries no stage. The simulated
    # oracle writes no marker and no prime, so its records are all the cases'.
    primed, cases = r8_split(records) if GATE["NATIVE"] else ([], records)
    records = [record for record in records if record != R8_MARKER] if R8_SECOND else cases
    with_x = [record for record in records if "x" in (record.get("info") or {})]

    def shows(record):
        x = record["info"]["x"]
        return x["json"] == R7_SHALLOW_X and x["protoOwn"] and x["protoData"] and x["plainPrototype"]

    stages = {stage: [shows(r) for r in with_x if r["stage"] == stage] for stage in (*R7_STAGES, "copy", "copy-hit")}
    detail = {
        "pinned_exit": pinned[0], "identical": pinned[0] == 0 and pinned == instrumented,
        "instrumented_exit": instrumented[0], "stderr": instrumented[2] if instrumented[0] else "",
        "stages": stages, "labels": sorted({record["label"] for record in with_x}), "records": records[:40],
        "answer": pinned[1][:4000], "prime_records": len(primed), "case_records": len(cases),
        "read": "all" if R8_SECOND else "cases", **{k: v for k, v in traced.items() if k != "package"},
    }
    passed = (detail["identical"] and all(stages[stage] and all(stages[stage]) for stage in R7_STAGES)
              and (stages["copy"] or stages["copy-hit"]) and all(stages["copy"] + stages["copy-hit"]))
    return passed, detail


def r7_answer(place, shape, flags, startup):
    """The collapsed answer of the corrected held document holding `shape` under `x.__proto__` at `place`, the
    rejection it surfaces, or None when the run failed."""
    root = os.path.join(D["SCRATCH"], unique("k"))
    spec_path = root + ".json"
    try:
        spec = r7_spec(place, reg5_text(shape, "__proto__"), root)
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


def r7_fits(place):
    references = {}

    def fits(shape, flags, startup):
        form = form_of(shape)
        with REFERENCE_LOCK:
            reference = references.get(form)
        if reference is None:
            reference = r7_answer(place, form, REF, STARTUP)
            if reference is None or reference.startswith('{"error"'):
                raise RuntimeError(f"the shallow form {form} has no answer: {reference}")
            with REFERENCE_LOCK:
                references[form] = reference
        got = r7_answer(place, shape, flags, startup)
        return None if got is None else got == reference
    return fits


R7_ABOVE = 2
REG7 = [("reg7:H5:held_hkcu:copy", "held_hkcu"), ("reg7:H6:held_hklm:copy", "held_hklm")]


def reg7_unit(place):
    def go(pool, quiet):
        visible, detail = r7_control(place)
        if not visible:
            return {"error": "the visibility control does not show x reaching the copy", "control": detail}
        found = Search(pool, r7_fits(place), MODEL[HOST]["copy"], R7_ABOVE, "a", REG5_LEAVES["copy"]).run()
        found["above"] = R7_ABOVE
        found["control"] = detail
        if "class" in found:
            found["room_at_class"] = found["class"] - prefix(MODEL[HOST]["copy"], R7_ABOVE)
        if found.get("far"):
            found["far_answer"] = r7_answer(place, tuple(found["far"][0]), REF, STARTUP)
        return found
    return go


# The fixture recorder: tests/resolve_settings_differential.rs's oracle run, step for step, printing what it stores as
# expected.json (the oracle's stdout with the root as `$ROOT`, trailing whitespace trimmed).
R7_FIXTURES = os.path.join(HERE, "fixtures")
R7_SCRUBBED = ("CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CONFIG_DIR", "WSL_DISTRO_NAME", "WSL_INTEROP",
               "CLAUDE_CODE_DISABLE_ADMIN_ENV_UNION")


def r7_substitute(value, root):
    if isinstance(value, str):
        return value.replace("$ROOT", root)
    if isinstance(value, list):
        return [r7_substitute(item, root) for item in value]
    if isinstance(value, dict):
        return {key: r7_substitute(item, root) for key, item in value.items()}
    return value


def r7_copy_tree(source, target):
    os.makedirs(target, exist_ok=True)
    for name in sorted(os.listdir(source)):
        here = os.path.join(source, name)
        there = os.path.join(target, ".claude" if name == "dot-claude" else name)
        if os.path.isdir(here):
            r7_copy_tree(here, there)
        else:
            shutil.copy(here, there)


def r7_record(name):
    with open(os.path.join(R7_FIXTURES, name, "case.json"), encoding="utf-8") as handle:
        spec = json.load(handle)
    scratch = tempfile.mkdtemp(prefix="f")
    root = os.path.join(scratch, name)
    try:
        r7_copy_tree(os.path.join(R7_FIXTURES, name, "tree"), root)
        os.makedirs(os.path.join(root, "work"), exist_ok=True)
        os.makedirs(os.path.join(root, "home"), exist_ok=True)
        env = {variable: None for variable in R7_SCRUBBED}
        env["HOME"] = root + "/home"
        for variable, value in (spec.get("env") or {}).items():
            env[variable] = value.replace("$ROOT", root) if isinstance(value, str) else None
        stepped = isinstance(spec.get("steps"), list)
        steps = spec["steps"] if stepped else [{}]
        document = {
            "root": root, "platform": spec.get("platform"), "env": dict(sorted(env.items())),
            "process": spec.get("process"),
            "writable": dict(sorted((path.replace("$ROOT", root), answer)
                                    for path, answer in (spec.get("writable") or {}).items())),
            "rejections": True,
            "cases": [{"name": f"{name}/{index}" if stepped else name, "at": step.get("at"), "files": step.get("files"),
                       "durations": step.get("durations") or {}, "options": r7_substitute(spec["options"], root)}
                      for index, step in enumerate(steps)],
        }
        spec_path = os.path.join(scratch, name + ".json")
        with open(spec_path, "w", encoding="utf-8") as handle:
            json.dump(document, handle)
        code, stdout, stderr = r7_oracle(PACKAGE, spec_path, dict(os.environ))
        if code != 0:
            return {"error": "the oracle failed", "stderr": stderr}
        expected = stdout.replace(root, "$ROOT").rstrip()
        return {"expected": expected, "sha256": __import__("hashlib").sha256(expected.encode()).hexdigest()}
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


def r7_instrument_unit():
    def go(pool, quiet):
        traced = r7_traced()
        return {k: v for k, v in traced.items() if k != "package"}
    return go


def round7_units():
    out = [("rec7:instrument", r7_instrument_unit())]
    out += [(f"rec7:fixture:{name}", (lambda n: lambda pool, quiet: r7_record(n))(name))
            for name in sorted(os.listdir(R7_FIXTURES)) if os.path.isfile(os.path.join(R7_FIXTURES, name, "case.json"))]
    out += [(name, reg7_unit(place)) for name, place in REG7]
    return out


# ---- round 8: the parent and remote managed settings (pm), the item 4 recordings and the parse cache order ----
# pm (sdk.mjs 165:30296) clones (j7e Tr), walks (cm) and schema-reads the document a host passes in on a miss and
# copies it (oX) on every call. The traced copy adds log-only hooks: each pm call (hit or miss, the label and the
# parse object's first-seen id), the clone, walk, schema and copy stages, the parse (hm), and each Ya read (sdk.mjs
# 165 `let r=t.length>CYe?...`): uncached past 8192 units, or a hit or miss of the 50 entry LRU with the resident keys
# most recent first, as text ids in order of first appearance.
R8_TRACE_FUNCTIONS = (
    'function __tw(o){try{const f=process.env.TRACE_LOG;if(f){process.getBuiltinModule("fs").appendFileSync(f,'
    'JSON.stringify(o)+"\\n")}}catch(q){}}'
    'function __tr(t,k,v,b){try{__tw({label:String(t),stage:k,info:b?null:__ti(v)})}catch(q){}return v}'
    'function __ti(v){const s=v&&typeof v==="object"&&"loadState" in v?v.settings:v;if(!s||typeof s!=="object")'
    'return{settings:null};const out={keys:Object.keys(s)};if(Object.prototype.hasOwnProperty.call(s,"x")){const x=s.x;'
    'const d=x&&typeof x==="object"?Object.getOwnPropertyDescriptor(x,"__proto__"):undefined;let j=null;'
    'try{j=JSON.stringify(x)}catch(q){}out.x={json:j,protoOwn:!!d,protoData:!!d&&"value" in d,protoEnumerable:'
    '!!d&&d.enumerable,plainPrototype:x!==null&&typeof x==="object"&&Object.getPrototypeOf(x)===Object.prototype}}'
    'return out}'
    'function __pe(t,e,h){try{const w=globalThis.__pmIds||(globalThis.__pmIds=new WeakMap);let i=w.get(e);'
    'if(i===undefined){i=globalThis.__pmNext||0;globalThis.__pmNext=i+1;w.set(e,i)}'
    '__tw({pm:h?"hit":"miss",label:String(t),object:i})}catch(q){}}'
    'function __yi(t){const m=globalThis.__yaIds||(globalThis.__yaIds=new Map);let i=m.get(t);'
    'if(i===undefined){i=m.size;m.set(t,i)}return i}'
    'function __ye(k,t){try{const l=globalThis.__yaLru;const id=__yi(t);'
    '__tw({ya:k,id,length:t.length,resident:l?[...l.keys()].map(__yi):null})}catch(q){}}'
)
R8_PATCHES = [
    ("function pm(e,t){let n=sX.get(e),r=n?.get(t);if(r)return oX(r);",
     R8_TRACE_FUNCTIONS + 'function pm(e,t){let n=sX.get(e),r=n?.get(t);__pe(t,e,!!r);if(r)return __tr(t,"copy-hit",oX(r));'),
    ("return oX(o)}", 'return __tr(t,"copy",oX(o))}'),
    ("function j7e(e,t){let n=Tr(e),", 'function j7e(e,t){let n=__tr(t,"clone",Tr(e)),'),
    ("r=cm(n,t,{skipMcpServerEntryFilter:!0,policySource:!0}),",
     'r=__tr(t,"walk",cm(n,t,{skipMcpServerEntryFilter:!0,policySource:!0}),!0),'),
    ("cP(CX(t,o),t,n).safeParse(n);", 'cP(CX(t,o),t,n).safeParse(n);__tr(t,"schema",s.success?s.data:null);'),
    ("function hm(e,t,{userWritable:n=!1}={}){let r=Ya(e,!1);",
     'function hm(e,t,{userWritable:n=!1}={}){let r=__tr(t,"parse",Ya(e,!1));'),
    ("let r=new Gy({max:n}),o=", "let r=new Gy({max:n});if(e===u4)globalThis.__yaLru=r;let o="),
    ("let r=t.length>CYe?u4(t,n):c4.parse(t,n);",
     'let r;if(t.length>CYe){r=u4(t,n);__ye("uncached",t)}else{const h=c4.cache.has(t);r=c4.parse(t,n);'
     '__ye(h?"hit":"miss",t)}'),
]
R8_STATE, R8_LOCK = {}, __import__("threading").Lock()
R8_LABELS = {"parent": ("managedSettings", "parent managed settings"),
             "remote": ("serverManagedSettings", "remote managed settings")}
R8_SHALLOW = '{"x":{"__proto__":[[null]]},"model":"m"}'
# name: (key, wrap, pass, major, above)
R8_PASSES = {"walk": ("model", None, "walk", "o", 1), "clone": ("model", None, "clone", "a", 1),
             "copy": ("x", "__proto__", "copy", "a", 2), "cachedcopy": ("x", "__proto__", "copy", "a", 2)}
# `copy` is the first, missed pm call's copy, found from the trace; `cachedcopy` the second, cached call's copy, which
# runs deeper and which the pinned answer shows (Update 66). Only the passes the pinned answer tells apart are goldens.
R8_GOLDEN_PASSES = ("walk", "clone", "cachedcopy")
# The stages a label logs when the cached copy runs out (the cached copy's far shape, the first copy's near shape) and
# when the first copy runs out (the first copy's far shape).
R8_CACHED_OUT = ["pm:miss", "clone", "walk", "schema", "copy", "pm:hit"]
R8_FIRST_OUT = ["pm:miss", "clone", "walk", "schema"]
# The stage the far shape must stop at: both the object-major (walk) and the array-major (clone) forms overflow
# inside the j7e structuredClone, which runs before the walk; the copy overflows after the schema read.
R8_ATTRIBUTION = {"walk": (None, "clone"), "clone": (None, "clone"), "copy": ("schema", "copy")}
# A second call (PLAN Update 82 A): the oracle marks where the prime ends, so the records split into the prime's (P)
# and the cases' (C). A warm second call passes the prime's own options object, which pm (165 col 30204) cached in its
# WeakMap, so the case logs only the cached-copy hit; a fresh identity misses again and logs the first-call lists.
R8_SECOND = GATE["CALL"] == "second"
R8_FULL1 = R8_CACHED_OUT + ["copy-hit"]
R8_WARM_CASE = (["pm:hit"], ["pm:hit", "copy-hit", "pm:hit"])
R8_MARKER = {"marker": "cases"}


def r8_traced():
    with R8_LOCK:
        if "traced" not in R8_STATE:
            target = os.path.join(os.path.dirname(OUT), "package-traced8")
            shutil.rmtree(target, ignore_errors=True)
            shutil.copytree(PACKAGE, target)
            bundle = os.path.join(target, "sdk.mjs")
            with open(bundle, encoding="utf-8") as handle:
                text = handle.read()
            counts = {anchor: text.count(anchor) for anchor, _ in R8_PATCHES}
            if any(count != 1 for count in counts.values()):
                R8_STATE["traced"] = {"error": "an anchor does not occur exactly once", "counts": counts}
            else:
                for anchor, replacement in R8_PATCHES:
                    text = text.replace(anchor, replacement)
                with open(bundle, "w", encoding="utf-8", newline="") as handle:
                    handle.write(text)
                R8_STATE["traced"] = {"package": target, "pinned_sha256": r7_sha256(os.path.join(PACKAGE, "sdk.mjs")),
                                      "traced_sha256": r7_sha256(bundle)}
        return R8_STATE["traced"]


def r8_text(which, shape):
    key, wrap = R8_PASSES[which][:2]
    return '{"' + key + '":' + reg5_text(shape, wrap) + "}"


def r8_spec(label, document, root, prime=None):
    """A spec whose one case passes `document` (spliced as text: Python's json would recurse through it) as the
    parent or remote managed settings; `prime` "fresh" primes a second call with an equal object of a new identity."""
    for folder in ("home/.claude", "work"):
        os.makedirs(os.path.join(root, folder), exist_ok=True)
    write(os.path.join(root, "home/.claude/settings.json"), '{"u":[null]}')
    spec = {"root": root, "platform": sys.platform, "env": {"HOME": os.path.join(root, "home")}, "deep": True,
            "cases": [{"name": "case", "options": {"cwd": os.path.join(root, "work"), R8_LABELS[label][0]: "@DOC@"}}]}
    if prime is not None:
        spec["prime"] = prime
    text = json.dumps(spec)
    if text.count('"@DOC@"') != 1:
        raise RuntimeError("the spec holds the document placeholder more than once")
    return text.replace('"@DOC@"', document)


def r8_run(package, label, document, flags, startup, trace=None, prime=None):
    """The collapsed answer of one run, the rejection it surfaces, or None when the run failed; with `trace`, the
    traced records too."""
    root = os.path.join(D["SCRATCH"], unique("m"))
    spec_path = root + ".json"
    try:
        with open(spec_path, "w", encoding="utf-8") as handle:
            handle.write(r8_spec(label, document, root, prime))
        command = [NODE, *probe_flags(flags), MANAGED["ORACLE"], package, spec_path]
        # TRACE_LOG takes its share of the budget from PAD, so traced and pinned runs start with the same bytes.
        env = D["padded_env"](MANAGED["pad"], command, startup, {} if trace is None else {"TRACE_LOG": trace})
        if trace is not None and os.path.exists(trace):
            os.remove(trace)
        process = D["spawn"](command, env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=600)
        stdout, stderr = process.stdout.decode("utf-8", "replace"), process.stderr.decode("utf-8", "replace")
        if process.returncode != 0:
            found = re.search(r"^(\w*Error): (.*)$", stderr.replace("\r\n", "\n"), re.M)
            answer = dumps({"error": found.group(1) + ": " + found.group(2)[:200]}) if found else None
        else:
            answer, failure = project(json.loads(stdout), root)
            answer = None if failure is not None else answer
        raw = (process.returncode, stdout.replace(root, "R"), stderr.replace(root, "R")[-600:])
        return answer, raw
    finally:
        shutil.rmtree(root, ignore_errors=True)
        if os.path.exists(spec_path):
            os.remove(spec_path)


def r8_records(trace):
    if not os.path.exists(trace):
        return []
    with open(trace, encoding="utf-8") as handle:
        records = [json.loads(line) for line in handle if line.strip()]
    os.remove(trace)
    return records


def r8_split(records):
    """The prime's records and the cases' records, split at the oracle's marker (a first call has no prime)."""
    if not records:
        return [], []
    if R8_MARKER not in records:
        raise RuntimeError("the trace holds no case marker")
    at = records.index(R8_MARKER)
    return records[:at], records[at + 1:]


def r8_control(label, prime=None):
    """Whether the shallow document reaches the clone, walk, schema read and copy of `label` with `x` an own data
    `__proto__` on a plain object, and the pinned and traced packages print the same. A warm second call reads every
    record (the prime does the walk); a first call and a fresh second call read the cases' records."""
    traced = r8_traced()
    if "error" in traced:
        return False, traced
    trace = os.path.join(D["SCRATCH"], unique("t") + ".trace")
    _, pinned = r8_run(PACKAGE, label, R8_SHALLOW, REF, STARTUP, prime=prime)
    _, instrumented = r8_run(traced["package"], label, R8_SHALLOW, REF, STARTUP, trace, prime)
    every = r8_records(trace)
    primed, cases = r8_split(every)
    records = [r for r in every if r != R8_MARKER] if R8_SECOND and prime is None else cases
    name = R8_LABELS[label][1]
    mine = [record for record in records if record.get("label") == name]

    def shows(record):
        x = (record.get("info") or {}).get("x")
        return bool(x) and x["json"] == '{"__proto__":[[null]]}' and x["protoOwn"] and x["protoData"] \
            and x["plainPrototype"]

    stages = {stage: [shows(r) for r in mine if r.get("stage") == stage] for stage in ("clone", "schema", "copy", "copy-hit")}
    calls = [r["pm"] for r in records if r.get("label") == name and "pm" in r]
    walked = [r for r in mine if r.get("stage") == "walk"]
    detail = {"identical": pinned[0] == 0 and pinned == instrumented, "pinned_exit": pinned[0],
              "instrumented_exit": instrumented[0], "stderr": instrumented[2] if instrumented[0] else "",
              "stages": stages, "walk": len(walked), "calls": calls, "records": records[:60],
              "answer": pinned[1][:4000], "prime_records": len(primed), "case_records": len(cases),
              "read": "all" if records is not cases else "cases", **{k: v for k, v in traced.items() if k != "package"}}
    passed = (detail["identical"] and all(stages[s] and all(stages[s]) for s in ("clone", "schema", "copy"))
              and all(stages["copy-hit"]) and len(walked) == 1 and calls[:1] == ["miss"])
    return passed, detail


def r8_fits(label, which, prime=None):
    references = {}

    def fits(shape, flags, startup):
        form = form_of(shape)
        with REFERENCE_LOCK:
            reference = references.get(form)
        if reference is None:
            reference = r8_run(PACKAGE, label, r8_text(which, form), REF, STARTUP, prime=prime)[0]
            if reference is None or reference.startswith('{"error"'):
                raise RuntimeError(f"the shallow form {form} has no answer: {reference}")
            with REFERENCE_LOCK:
                references[form] = reference
        got = r8_run(PACKAGE, label, r8_text(which, shape), flags or REF, startup, prime=prime)[0]
        return None if got is None else got == reference
    return fits


def r8_stages(records, label):
    name = R8_LABELS[label][1]
    return [r.get("stage") or ("pm:" + r["pm"]) for r in records if r.get("label") == name]


def r8_phases(trace, label):
    """The stages of `label` the prime logged and the ones the cases logged."""
    primed, cases = r8_split(r8_records(trace))
    return r8_stages(primed, label), r8_stages(cases, label)


def r8_side_matches(side, primed, cases, prime):
    """Whether a copy entry's stages match the registered lists of this call position, and which alternative."""
    want = R8_CACHED_OUT if side == "near" else R8_FIRST_OUT
    if not R8_SECOND or prime == "fresh":
        return cases == want, None
    alternative = R8_WARM_CASE.index(cases) if cases in R8_WARM_CASE else None
    return primed == want and alternative is not None, alternative


def r8_first_fits(label, prime=None):
    """Whether the first copy of `label` finishes at a shape: the traced run logs its copy stage. The pinned answer
    cannot tell, since a first-copy near shape runs out in the second, cached copy with the same RangeError. A warm
    second call's first copy is the prime's; otherwise it is the case's."""
    traced = r8_traced()
    if "error" in traced:
        raise RuntimeError(f"no traced package: {traced}")

    def fits(shape, flags, startup):
        trace = os.path.join(D["SCRATCH"], unique("t") + ".trace")
        answer = r8_run(traced["package"], label, r8_text("copy", shape), flags or REF, startup, trace, prime)[0]
        primed, cases = r8_phases(trace, label)
        stages = primed if R8_SECOND and prime is None else cases
        if "copy" in stages:
            return True
        return False if (answer or "").startswith('{"error":"RangeError') else None
    return fits


def r8_attribution(label, which, shape, prime=None):
    """The stages the traced package reached for `label` at a shape past the edge, and whether they match the pass.
    A first call and a fresh second call hold the cases' stages to the first-call lists; a warm second call's cached
    copy holds the prime to one of its registered lists and the case to the cached hit."""
    traced = r8_traced()
    trace = os.path.join(D["SCRATCH"], unique("t") + ".trace")
    answer, raw = r8_run(traced["package"], label, r8_text(which, shape), REF, STARTUP, trace, prime)
    primed, stages = r8_phases(trace, label)
    alternative = None
    if which == "cachedcopy" and R8_SECOND and prime is None:
        alternative = R8_WARM_CASE.index(stages) if stages in R8_WARM_CASE else None
        prime_list = [R8_FIRST_OUT, R8_CACHED_OUT, R8_FULL1].index(primed) \
            if primed in (R8_FIRST_OUT, R8_CACHED_OUT, R8_FULL1) else None
        matches = alternative is not None and prime_list is not None and raw[0] == 1
        alternative = {"case": alternative, "prime": prime_list}
    elif which == "cachedcopy":
        matches = stages == R8_CACHED_OUT and raw[0] == 1
    else:
        present, missing = R8_ATTRIBUTION[which]
        # A warm second call's case starts from the cached copy, so the pass's stages are the prime's.
        seen = primed if R8_SECOND and prime is None else stages
        matches = (present is None or present in seen) and missing not in seen
    return {"answer": answer, "stages": stages, "prime_stages": primed, "matches": matches, "exit": raw[0],
            "alternative": alternative}


def r8_copy_events(label, found, prime=None):
    """The first copy's events file of `label` on this host: the pinned and the traced run of each profile pair
    entry, with the stages the trace logged. The near entry must run out in the cached copy, the far one in the
    first copy, and both packages must answer the same."""
    traced = r8_traced()
    records, failures = [], []
    for side, entry in zip(("near", "far"), profile_pair(found)):
        shape, startup = tuple(entry[:3]), entry[3]
        text = r8_text("copy", shape)
        pinned_answer, pinned_raw = r8_run(PACKAGE, label, text, REF, startup, prime=prime)
        trace = os.path.join(D["SCRATCH"], unique("t") + ".trace")
        traced_answer, traced_raw = r8_run(traced["package"], label, text, REF, startup, trace, prime)
        primed, stages = r8_phases(trace, label)
        matches, alternative = r8_side_matches(side, primed, stages, prime)
        agree = pinned_answer is not None and pinned_answer == traced_answer and pinned_raw[0] == traced_raw[0]
        records.append({"host": HOST, "label": label, "side": side, "shape": list(shape), "startup": startup,
                        "pinned_sha256": traced["pinned_sha256"], "traced_sha256": traced["traced_sha256"],
                        "stages": stages, "prime_stages": primed, "alternative": alternative,
                        "call": GATE["CALL"], "context": GATE["CONTEXT"], "prime": prime or "same",
                        "exit": traced_raw[0], "pinned_exit": pinned_raw[0], "answer": pinned_answer,
                        "answers_agree": agree})
        if not matches:
            failures.append(f"the {side} entry logs prime {primed} and cases {stages}, not its registered lists")
        if not agree:
            failures.append(f"the pinned and traced runs of the {side} entry answer otherwise")
    folder = os.path.join(os.path.dirname(OUT), "pm8_copy")
    os.makedirs(folder, exist_ok=True)
    path = os.path.join(folder, f"{label}{'_fresh' if prime == 'fresh' else ''}.events.jsonl")
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write("".join(json.dumps(record, sort_keys=True) + "\n" for record in records))
    return {"path": path, "records": records, "failures": failures}


def pm8_unit(label, which, prime=None):
    def go(pool, quiet):
        visible, detail = r8_control(label, prime)
        if not visible:
            return {"error": "the control does not show the document reaching every stage", "control": detail}
        _, _, model_pass, major, above = R8_PASSES[which]
        fits = r8_first_fits(label, prime) if which == "copy" else r8_fits(label, which, prime)
        found = Search(pool, fits, MODEL[HOST][model_pass], above, major, REG5_LEAVES[model_pass]).run()
        found["above"] = above
        found["control"] = detail
        if "class" in found:
            found["room_at_class"] = found["class"] - prefix(MODEL[HOST][model_pass], above)
        if found.get("far"):
            far = tuple(found["far"][0])
            found["far_answer"] = r8_run(PACKAGE, label, r8_text(which, far), REF, STARTUP, prime=prime)[0]
            found["attribution"] = r8_attribution(label, which, far, prime)
            if not (found["far_answer"] or "").startswith('{"error":"RangeError'):
                found["error"] = "the far answer is not the uncaught RangeError"
            elif not found["attribution"]["matches"]:
                found["error"] = "the far shape stops at a different stage"
        if "error" in found or "class" not in found:
            return found
        if which == "copy":
            found["events"] = r8_copy_events(label, found, prime)
            if found["events"]["failures"]:
                found["error"] = "the first copy's events break the registered stage lists"
        elif which == "cachedcopy":
            found["near_answer"] = r8_run(PACKAGE, label, r8_text(which, tuple(found["near"][0])), REF, STARTUP,
                                          prime=prime)[0]
            if found["near_answer"] is None or found["near_answer"].startswith('{"error"'):
                found["error"] = "the near shape does not answer"
        return found
    return go


def r8_record(name, package, trace=None):
    """r7_record with the `padded` step of the Rust harness: each listed file grows to exactly that many bytes with
    trailing ASCII spaces."""
    with open(os.path.join(R7_FIXTURES, name, "case.json"), encoding="utf-8") as handle:
        spec = json.load(handle)
    scratch = tempfile.mkdtemp(prefix="f")
    root = os.path.join(scratch, name)
    try:
        r7_copy_tree(os.path.join(R7_FIXTURES, name, "tree"), root)
        os.makedirs(os.path.join(root, "work"), exist_ok=True)
        os.makedirs(os.path.join(root, "home"), exist_ok=True)
        for relative, size in sorted((spec.get("padded") or {}).items()):
            location = os.path.join(root, relative)
            have = os.path.getsize(location)
            if have >= size:
                return {"error": f"{relative} already holds {have} bytes, not fewer than {size}"}
            with open(location, "ab") as handle:
                handle.write(b" " * (size - have))
        env = {variable: None for variable in R7_SCRUBBED}
        env["HOME"] = root + "/home"
        for variable, value in (spec.get("env") or {}).items():
            env[variable] = value.replace("$ROOT", root) if isinstance(value, str) else None
        stepped = isinstance(spec.get("steps"), list)
        steps = spec["steps"] if stepped else [{}]
        document = {
            "root": root, "platform": spec.get("platform"), "env": dict(sorted(env.items())),
            "process": spec.get("process"),
            "writable": dict(sorted((path.replace("$ROOT", root), answer)
                                    for path, answer in (spec.get("writable") or {}).items())),
            "rejections": True,
            "cases": [{"name": f"{name}/{index}" if stepped else name, "at": step.get("at"), "files": step.get("files"),
                       "durations": step.get("durations") or {}, "options": r7_substitute(spec["options"], root)}
                      for index, step in enumerate(steps)],
        }
        spec_path = os.path.join(scratch, name + ".json")
        with open(spec_path, "w", encoding="utf-8") as handle:
            json.dump(document, handle)
        env = dict(os.environ)
        env.pop("TRACE_LOG", None)
        if trace is not None:
            env["TRACE_LOG"] = trace
        code, stdout, stderr = r7_oracle(package, spec_path, env)
        if code != 0:
            return {"error": "the oracle failed", "stderr": stderr.replace(root, "$ROOT")}
        expected = stdout.replace(root, "$ROOT").rstrip()
        return {"expected": expected, "sha256": __import__("hashlib").sha256(expected.encode()).hexdigest()}
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


R8_FIXTURES = ("fm_user_file_at_limit", "fm_user_file_over_limit", "fm_managed_file_at_limit",
               "fm_managed_file_over_limit", "fm_wsl_windows_side_at_limit", "fm_wsl_windows_side_over_limit",
               "fm_wsl_user_registry_probe_at_limit", "fm_wsl_user_registry_probe_over_limit", "parent_own_proto",
               "remote_own_proto")
R8_EVICTION = "org_held_registry_parse_cache_order"


def rec8_fixture(name, events=False):
    """The fixture's expected.json from the pinned package, the same text from the traced copy, and with `events`
    the traced pm and Ya events, which the Rust parse cache log must equal."""
    def go(pool, quiet):
        traced = r8_traced()
        if "error" in traced:
            return {"error": "the traced copy failed", "traced": traced}
        pinned = r8_record(name, PACKAGE)
        trace = os.path.join(D["SCRATCH"], unique("t") + ".trace")
        os.makedirs(D["SCRATCH"], exist_ok=True)
        instrumented = r8_record(name, traced["package"], trace)
        records = r8_records(trace)
        out = {**pinned, "identical": pinned == instrumented}
        if "error" in pinned or "error" in instrumented:
            out["error"] = "a recording failed"
            out["instrumented"] = instrumented
        elif not out["identical"]:
            out["error"] = "the traced copy printed a different answer"
            out["instrumented"] = instrumented
        if events:
            kept = [r for r in records if "ya" in r or "pm" in r]
            text = "".join(json.dumps(r, separators=(",", ":"), sort_keys=True) + "\n" for r in kept)
            out["events"] = text
            out["events_sha256"] = __import__("hashlib").sha256(text.encode()).hexdigest()
            out["evictions"] = sum(1 for a, b in zip(kept, kept[1:])
                                   if "ya" in a and "ya" in b and a["resident"] and b["resident"]
                                   and set(a["resident"]) - set(b["resident"]))
            if out["evictions"] == 0 and "error" not in out:
                out["error"] = "the parse cache never evicted"
        return out
    return go


def r8_instrument_unit():
    def go(pool, quiet):
        traced = r8_traced()
        return {k: v for k, v in traced.items() if k != "package"}
    return go


# ---- mg8: the managed goldens ----
MG8_PORT = os.path.join(HERE, "ts_managed_settings_goldens_r8.py")
MG8_PREVIOUS = {"darwin arm64": "ts_managed_settings.json", "linux aarch64": "ts_managed_settings_linux_arm64.json",
                "linux x86_64": "ts_managed_settings_linux_x64.json"}
MG8_HOSTS = {"darwin arm64": "darwin arm64, GitHub Actions macos-latest runner, native",
             "linux aarch64": "linux aarch64, GitHub Actions ubuntu-24.04-arm runner, native",
             "linux x86_64": "linux x86_64, GitHub Actions ubuntu-latest runner, native"}
# The keys this job locates: each one's range must hold the edge on every host, and the first answer change in it
# is the edge (the answers only lose members as the level grows).
MG8_LOCATE = {"xp_login": [600, 4000], "org_cache_split": [600, 4000], "org_cache_kept": [600, 4000],
              "org_cache_hit": [600, 4000]}
MG8_WINDOW = 30
MG8_OBSERVE_RUNS = 25
MG8_OLD_FIELDS = ("files", "effective", "exact", "machine_policy", "error")
MG8_ANSWERS = ("effective", "exact", "machine_policy", "managed_source", "error")
# The default-flag margins of every boundary pair, by host (PLAN Update 72 and its note A): each entry is recorded
# at its margin, never at the pair level where a default-flag run can still flip.
MG8_MARGINS = os.path.join(HERE, "margins8.json")
# The old goldens of each host that sit at a boundary move to their margins; the rest stay as committed.
MG8_OLD_MOVED, MG8_OLD_KEPT = 34, 4


def mg8_port():
    """The round 8 recorder as a namespace: everything above its run, with no cases chosen yet."""
    with open(MG8_PORT, encoding="utf-8") as handle:
        source = handle.read()
    source = source[: source.index("version = subprocess.run(")]
    cut, end = source.index('TARGET = f"'), source.index("\n\n\ndef text(")
    source = source[:cut] + "CASES = []" + source[end:]
    saved = sys.argv
    sys.argv = [MG8_PORT, NODE, PACKAGE, os.path.join(D["SCRATCH"], "mg8.json")]
    port = {"__name__": "mg8_port", "__file__": MG8_PORT}
    try:
        exec(compile(source, MG8_PORT, "exec"), port)
    finally:
        sys.argv = saved
    port["ORACLE"] = MANAGED["ORACLE"]
    port["subprocess"] = D["Spawner"]
    port["pad"] = D["budgeted"](port["pad"], port["STARTUP_BYTES"])
    return port


def mg8_collapse(answer):
    text = json.dumps(answer)
    text = re.sub(r'(?:\{\\"x\\":){2,}', '{\\"x\\":*', text)
    text = re.sub(r"(?:\}){3,}", "}*", text)
    text = re.sub(r"\[{3,}", "[*", text)
    return re.sub(r"\]{3,}", "]*", text)


def mg8_case_at(port, key, level):
    """The near case the group of `key` makes at `level`."""
    group = next(make for keys, make in port["GROUPS"] if keys == key)
    return group(level)[0]


def mg8_locate(pool, port, key):
    low, high = MG8_LOCATE[key]

    def at(level, flags=REF):
        case_name, files = mg8_case_at(port, key, level)[:2]
        name = case_name.rsplit("_", 1)[0] + "_" + unique("g")
        try:
            return mg8_collapse(port["run"](port["prepare"](name, files), probe_flags(flags)))
        except SystemExit:
            return "fail"
        finally:
            shutil.rmtree(os.path.join(port["FOLDER"], name), ignore_errors=True)

    reference_answer, top = at(low), at(high)
    if "fail" in (reference_answer, top) or top == reference_answer:
        return {"error": f"no change between {low} and {high}", "at_low": reference_answer[:2000], "at_high": top[:2000]}
    while high - low > 1:
        middle = (low + high) // 2
        if at(middle) == reference_answer:
            low = middle
        else:
            high = middle
    levels = list(range(low - MG8_WINDOW, low + MG8_WINDOW + 1))
    answered = dict(zip(levels, pool.map(at, levels)))
    changes = [level for level in levels[:-1] if answered[level] != answered[level + 1]]
    checks = {str(level): [at(level, CROSS) == answered[level], at(level + 1, CROSS) == answered[level + 1]]
              for level in changes}
    found = {"level": low, "changes": changes, "checks": checks,
             "answers": {str(level): answered[level][:3000] for level in sorted({*changes, *(c + 1 for c in changes)})}}
    if "fail" in answered.values():
        found["error"] = "a level of the window failed"
    elif not changes or changes[0] != low:
        found["error"] = "the first change in the window is not the located level"
    elif not all(all(pair) for pair in checks.values()):
        found["error"] = "the cross-check flags answer otherwise at a change"
    return found


def mg8_case_key(case, startup):
    """The files and startup a case or committed entry runs with, comparable across the two."""
    return json.dumps([[list(file) for file in case], startup])


def mg8_old_gate(entries, moves, startup):
    """Every committed golden of this host among `entries`: one at a pair level as the entry at that side's margin,
    with the same answers once the depth collapses, and every other one with the same files and answers.
    `moves` maps the files and startup of each pair-level case to every pair side that holds it, with its margin and
    the margin entry's name; `startup` is the one an entry without its own runs with. An old entry two sides hold is
    missing: it has no one margin to move to."""
    with open(os.path.join(HERE, "mg8_previous", MG8_PREVIOUS[HOST]), encoding="utf-8") as handle:
        previous = json.load(handle)
    by_name = {entry["name"]: entry for entry in entries}
    missing, matched, moved = [], {}, {}
    for old in previous["cases"]:
        own = old.get("startup_bytes")
        held = moves.get(mg8_case_key(old["files"], own if own is not None else startup), [])
        if len(held) > 1:
            missing.append(old["name"])
            continue
        if held:
            move = held[0]
            entry = by_name.get(move["name"])
            same = entry is not None and all(
                mg8_collapse(entry.get(field)) == mg8_collapse(old.get(field)) for field in MG8_OLD_FIELDS[1:])
            if same:
                moved[old["name"]] = {**move, "from": old["name"]}
            else:
                missing.append(old["name"])
            continue
        want = {field: old.get(field) for field in MG8_OLD_FIELDS}
        names = [entry["name"] for entry in entries
                 if {field: entry.get(field) for field in MG8_OLD_FIELDS} == want and entry.get("startup_bytes") == own]
        if names:
            matched[old["name"]] = names
        else:
            missing.append(old["name"])
    return {"old_entries": len(previous["cases"]), "missing": missing, "matched": matched, "moved": moved,
            "old_observed": previous.get("observed", [])}


def mg8_margin_cases(port, profile):
    """The pairs at this host's committed margins: the cases in pair order, the pairs by name, the move of each
    pair-level case, and the reason when an edge no longer sits where the margins were measured."""
    with open(MG8_MARGINS, encoding="utf-8") as handle:
        committed = json.load(handle)[HOST]
    pairs = band8_pairs(port, profile)
    built = {pair["id"]: pair for pair in pairs}
    if set(built) != set(committed):
        return None, None, None, f"the pairs {sorted(set(built) ^ set(committed))} are not on both sides"
    drifted = {pid: [[pair["axis"], pair["near_level"], pair["far_level"]],
                     [committed[pid]["axis"], committed[pid]["near_level"], committed[pid]["far_level"]]]
               for pid, pair in built.items()
               if [pair["axis"], pair["near_level"], pair["far_level"]]
               != [committed[pid]["axis"], committed[pid]["near_level"], committed[pid]["far_level"]]}
    if drifted:
        return None, None, None, f"the edge moved: {drifted}"
    cases, names, moves = [], [], {}
    for pair in pairs:
        margins = committed[pair["id"]]
        if pair.get("limit"):
            placed = list(pair["both_at"](margins["far_margin"]))
            sides = [(pair["near"], "near", margins["far_margin"]), (pair["far"], "far", margins["far_margin"])]
        else:
            placed, sides = [], []
            for side in ("near", "far"):
                margin = margins[f"{side}_margin"]
                placed.append(pair[side] if margin == pair[f"{side}_level"] else pair[f"{side}_at"](margin))
                sides.append((pair[side], side, margin))
        for (level_case, side, margin), case in zip(sides, placed):
            # Neighbouring pairs can share a pair-level case (one pair's far case is the next one's near case).
            moves.setdefault(mg8_case_key(level_case[1], port["case_startup"](level_case)), []).append(
                {"pair": pair["id"], "side": side, "level": pair[f"{side}_level"], "margin": margin, "name": case[0]})
        cases += placed
        names.append([case[0] for case in placed])
    return cases, names, moves, None


def mg8_profile(pool, port, out):
    """This host's profile, from the recorder's PROFILES, this job's pm8 classes and the located keys, into
    `out["profile"]`; the reason when a gate fails."""
    units = R8_STATE.get("units", {})
    profile = dict(port["PROFILES"].get(HOST, {}))
    out.setdefault("located", {})
    out.setdefault("pm", {})
    if set(port["PM_FORMS"]) != set(R8_GOLDEN_PASSES):
        return f"the recorder's passes {sorted(port['PM_FORMS'])} are not {R8_GOLDEN_PASSES}"
    for label in R8_LABELS:
        # Every pass gates, the first copy and its events included; only the golden passes fill the profile.
        for which in R8_PASSES:
            found = units.get(f"pm8:{label}:{which}")
            if not found or "error" in found or "class" not in found:
                return f"pm8:{label}:{which} has no class to fill"
        for which in R8_GOLDEN_PASSES:
            found = units[f"pm8:{label}:{which}"]
            profile[f"pm_{label}_{which}"] = profile_pair(found)
            out["pm"][f"pm_{label}_{which}"] = profile[f"pm_{label}_{which}"]
    for key in MG8_LOCATE:
        found = mg8_locate(pool, port, key)
        out["located"][key] = found
        if "error" in found:
            return f"{key}: {found['error']}"
        profile[key] = found["level"]
    profile["org_cache_limit"] = profile["org_cache_split"]
    absent = [key for keys, _ in port["GROUPS"] for key in keys.split() if key not in profile]
    if absent:
        return f"the profile lacks {absent}"
    out["profile"] = profile
    return None


def mg8_unit():
    def go(pool, quiet):
        port = mg8_port()
        out = {"located": {}, "pm": {}}
        failed = mg8_profile(pool, port, out)
        if failed:
            return {"error": failed, **out}
        profile = out["profile"]
        try:
            cases, pairs, moves, drifted = mg8_margin_cases(port, profile)
        except Band8Stop as error:
            return {"error": str(error), **out}
        if drifted:
            return {"error": drifted, **out}
        partial = next(make for keys, make in port["GROUPS"] if keys == "partial_drop")
        cases += partial(profile["partial_drop"]) + port["FIXED"] + port["org_cases"]()
        if len({case[0] for case in cases}) != len(cases):
            return {"error": "two cases share a name", **out}
        port["CASES"] = cases
        version = subprocess.run([NODE, "--version"], capture_output=True, check=True, env={}).stdout.decode().strip()
        port["LABEL"] = f"{version} {HOST}"

        def record(case):
            try:
                return port["reference"](*case)
            except SystemExit:
                return {"name": case[0], "failed": True}

        entries = list(pool.map(record, cases))
        failed = [entry["name"] for entry in entries if entry.get("failed")]
        if failed:
            return {"error": "the reference and cross-check flags answer otherwise", "failed": failed, **out}
        unexpected = [entry["name"] for entry in entries
                      if entry.get("error") is not None and entry["error"].get("name") != "RangeError"]
        if unexpected:
            return {"error": "a case rejects with something other than a RangeError", "unexpected": unexpected, **out}
        by_name = {entry["name"]: {key: entry[key] for key in MG8_ANSWERS} for entry in entries}
        same = [pair for pair in pairs if by_name[pair[0]] == by_name[pair[1]]]
        # The limit pair sits at one level; the split far case at that level holds the same 8193-unit text.
        limit_at = next(names for names in pairs if names[0].startswith("org_cache_limit_"))
        level = int(limit_at[1].split("_")[3])
        split_far = next(names for names in pairs if names[0].startswith("org_cache_split_"))[1]
        limit_consistent = split_far == f"org_cache_split_{level}" and by_name[limit_at[1]] == by_name[split_far]
        out["old"] = mg8_old_gate(entries, moves, port["STARTUP_BYTES"])
        # The recorder observes in a run of its own, so the cases it prepares again need a folder without them.
        shutil.rmtree(port["FOLDER"], ignore_errors=True)
        port["FOLDER"] = tempfile.mkdtemp()
        seen, missed = port["observe"](entries, MG8_OBSERVE_RUNS, MG8_HOSTS[HOST])
        shutil.rmtree(port["FOLDER"], ignore_errors=True)
        out["observed"] = seen
        # Only the old entries kept as committed carry their committed bands; the moved ones are new placements.
        old_names = {name for names in out["old"]["matched"].values() for name in names}
        new_bands = [band["name"] for band in seen["bands"] if band["name"] not in old_names]
        out["body"] = port["body"](entries, [seen])
        out["body_sha256"] = __import__("hashlib").sha256(out["body"].encode()).hexdigest()
        out["cases"] = len(entries)
        if same:
            out["error"] = f"a pair answers the same: {same}"
        elif not limit_consistent:
            out["error"] = "the 8193-unit limit case answers otherwise than the split case at the same level"
        elif out["old"]["missing"]:
            out["error"] = f"committed goldens are missing: {out['old']['missing']}"
        elif [len(out["old"]["moved"]), len(out["old"]["matched"])] != [MG8_OLD_MOVED, MG8_OLD_KEPT]:
            out["error"] = (f"{len(out['old']['moved'])} old entries moved and {len(out['old']['matched'])} stayed, "
                            f"not {MG8_OLD_MOVED} and {MG8_OLD_KEPT}")
        elif missed:
            out["error"] = f"no default-flag run gave the recorded answer of {missed}"
        elif new_bands:
            out["error"] = f"new entries answer more than one way: {new_bands}"
        return out
    return go


# ---- band8: the default-flag margins of every boundary pair (PLAN Update 71) ----
BAND8_RUNS = 25
BAND8_STEPS = 8
BAND8_CLEAN_IN_A_ROW = 2
# A side whose reference region holds one level needs as many clean runs there as two levels give elsewhere.
BAND8_RUNS_ALONE = BAND8_CLEAN_IN_A_ROW * BAND8_RUNS
BAND8_ESTABLISHED = os.path.join(HERE, "band8_established.json")
# How far past a class the next class a pass reaches can be: no model step is wider than this.
BAND8_CLASS_REACH = 64
# Pairs a host steps by class with the startup fixed as registered, where one startup byte per level never left
# the default-flag flips within the cap (PLAN Update 72, note A; Update 75). A level is the next class the pass
# reaches on the host, so its size in bytes is a host property the bound line records.
BAND8_CLASS_AXIS = {"darwin arm64": {"pm_remote_cachedcopy": 4410, "pm_parent_cachedcopy": 4394}}
# The pairs one job measures, by id; unset measures every pair.
BAND8_ONLY = set(filter(None, os.environ.get("R3_BAND8_PAIRS", "").split(",")))


class Band8Stop(Exception):
    """A pair that cannot go on: an unexpected answer, a crash, no output, or a gate."""


def band8_shape_pass(keys):
    """The model table, containers above, major axis and leaves of the pass a proto or pm group measures."""
    if keys.startswith("proto_"):
        return MODEL[HOST]["copy"], 2, "a", ["null_array", "empty_array"]
    which = keys.rsplit("_", 1)[1]
    _, _, model_pass, major, above = R8_PASSES[which]
    return MODEL[HOST][model_pass], above, major, REG5_LEAVES[model_pass]


def band8_pairs(port, profile):
    """Every boundary pair GROUPS makes with `profile`: its id, the near and far case, and how a side steps."""
    pairs = []
    for keys, make in port["GROUPS"]:
        if keys == "partial_drop":
            continue
        names = keys.split()
        args = [profile[key] for key in names]
        made = make(*args)
        for index in range(0, len(made), 2):
            near, far = made[index], made[index + 1]
            if keys == "org_cache_limit":
                level = args[0] + 1
                pairs.append({"id": keys, "axis": "level", "near": near, "far": far, "limit": True,
                              "near_level": level, "far_level": level,
                              "both_at": lambda n, make=make: make(n - 1)})
            elif isinstance(args[0], list):
                first, second = args[0]
                at = lambda entry, make=make: make([entry, entry])[0]
                if at(first)[0] != near[0] or at(second)[0] != far[0]:
                    raise Band8Stop(f"{keys}: the stepped cases do not rebuild the pair {[near[0], far[0]]}")
                if first[:3] == second[:3] and keys in BAND8_CLASS_AXIS.get(HOST, ()):
                    # The near case stays; the far case is the first shape of the next class the pass reaches,
                    # at the same startup.
                    if first[3] != BAND8_CLASS_AXIS[HOST][keys]:
                        raise Band8Stop(f"{keys}: the near startup is {first[3]}, not the registered "
                                        f"{BAND8_CLASS_AXIS[HOST][keys]}")
                    table, above, major, leaves = band8_shape_pass(keys)
                    stepped = {"id": keys, "axis": "class", "pass": [table, above, major, leaves]}
                    near_level = cost(table, above, tuple(first[:3]))
                    far_level = band8_next(stepped, near_level, "far")
                    shape_at = lambda c, p=(table, above, major, leaves): shapes_at(p[0], p[1], c, p[2], p[3])[0]
                    pairs.append({**stepped, "near": near, "far": at([*shape_at(far_level), first[3]]),
                                  "near_level": near_level, "far_level": far_level,
                                  "near_at": lambda c, at=at, first=first, shape_at=shape_at:
                                      at([*shape_at(c), first[3]]),
                                  "far_at": lambda c, at=at, first=first, shape_at=shape_at:
                                      at([*shape_at(c), first[3]])})
                elif first[:3] == second[:3]:
                    pairs.append({"id": keys, "axis": "startup", "near": near, "far": far,
                                  "near_level": first[3], "far_level": second[3],
                                  "near_at": lambda n, at=at, first=first: at([*first[:3], n]),
                                  "far_at": lambda n, at=at, first=first: at([*first[:3], n])})
                else:
                    table, above, major, leaves = band8_shape_pass(keys)
                    pairs.append({"id": keys, "axis": "class", "near": near, "far": far,
                                  "near_level": cost(table, above, tuple(first[:3])),
                                  "far_level": cost(table, above, tuple(second[:3])),
                                  "pass": [table, above, major, leaves],
                                  "near_at": lambda c, at=at, first=first, p=(table, above, major, leaves):
                                      at([*shapes_at(p[0], p[1], c, p[2], p[3])[0], first[3]]),
                                  "far_at": lambda c, at=at, second=second, p=(table, above, major, leaves):
                                      at([*shapes_at(p[0], p[1], c, p[2], p[3])[0], second[3]])})
            else:
                pair_names = [near[0], far[0]]
                stepped = []
                for position in range(len(names)):
                    moved = list(args)
                    moved[position] += 1
                    if [case[0] for case in make(*moved)[index:index + 2]] != pair_names:
                        stepped.append(position)
                if len(stepped) != 1:
                    raise Band8Stop(f"{keys}: {len(stepped)} keys rename the pair {pair_names}, not one")
                position = stepped[0]

                def case_at(n, side, make=make, args=args, position=position, index=index):
                    moved = list(args)
                    moved[position] = n if side == 0 else n - 1
                    return make(*moved)[index + side]
                level = args[position]
                if case_at(level, 0)[0] != near[0] or case_at(level + 1, 1)[0] != far[0]:
                    raise Band8Stop(f"{keys}: the stepped cases do not rebuild the pair {pair_names}")
                pairs.append({"id": near[0].rsplit("_", 1)[0], "axis": "level", "near": near, "far": far,
                              "near_level": level, "far_level": level + 1,
                              "near_at": lambda n, case_at=case_at: case_at(n, 0),
                              "far_at": lambda n, case_at=case_at: case_at(n, 1)})
    ids = [pair["id"] for pair in pairs]
    if len(set(ids)) != len(ids):
        raise Band8Stop(f"two pairs share an id: {ids}")
    return pairs


def band8_next(pair, level, side):
    """The next level of `pair` away from the other side: one level, one startup byte, or the next class the pass
    reaches."""
    step = -1 if side == "near" else 1
    if pair["axis"] != "class":
        return level + step
    table, above, major, leaves = pair["pass"]
    for distance in range(1, BAND8_CLASS_REACH + 1):
        candidate = level + 16 * step * distance
        if shapes_at(table, above, candidate, major, leaves):
            return candidate
    raise Band8Stop(f"{pair['id']}: no class within {BAND8_CLASS_REACH} classes past {level}")


class Band8Runs:
    """Runs one case in a folder of its own and keeps the folder check's tally."""

    def __init__(self, port):
        self.port, self.lock, self.folders, self.refusals, self.runs = port, __import__("threading").Lock(), set(), 0, 0
        self.assertions = 0

    def once(self, case, flags):
        folder = tempfile.mkdtemp()
        with self.lock:
            self.runs += 1
            self.folders.add(folder)
        try:
            answered = self.port["run"](self.port["prepare"](case[0], case[1], folder), flags,
                                        self.port["case_startup"](case))
            return mg8_collapse(answered) if isinstance(answered, dict) else None
        except RuntimeError as error:
            # The folder, flag and PAD checks every probe entry makes: a failed one stops the pair, never counts as a
            # missing answer.
            if "share the working folder" in str(error):
                with self.lock:
                    self.refusals += 1
                raise Band8Stop(f"{case[0]}: {error}") from error
            if str(error).startswith(("a probe", "a padded probe")):
                with self.lock:
                    self.assertions += 1
                raise Band8Stop(f"{case[0]}: {error}") from error
            return None
        except (Exception, SystemExit):
            return None
        finally:
            shutil.rmtree(folder, ignore_errors=True)

    def tally(self):
        return {"runs": self.runs, "distinct_folders": len(self.folders), "refusals": self.refusals,
                "failed_probe_checks": self.assertions, "running_after": len(self.port["RUNNING"])}


def band8_answer(runs, case):
    """The side answer: REF and CROSS must agree."""
    reference = runs.once(case, probe_flags(REF))
    checked = runs.once(case, probe_flags(CROSS))
    if reference is None or checked != reference:
        raise Band8Stop(f"{case[0]}: the reference and cross-check flags answer otherwise")
    return reference


def band8_cases(pair, side, level, answers):
    """The cases a side has at `level`, each with its own answer, the opposite answer and the entry it stands for."""
    if pair.get("limit"):
        made = pair["both_at"](level)
        return [(made[0], answers["near"], answers["far"], "near"), (made[1], answers["far"], answers["near"], "far")]
    own, other = (answers["near"], answers["far"]) if side == "near" else (answers["far"], answers["near"])
    # The pair level is the pair's own case; past it, the case the axis makes there.
    case = pair[side] if level == pair[f"{side}_level"] else pair[f"{side}_at"](level)
    return [(case, own, other, side)]


def band8_region(runs, pair, side, answers, write_event):
    """The levels from L0 outward through L8 whose reference answer is the side's own, from one REF run each, and
    the bound line the events keep for them."""
    level, region, crossing, visited = pair[f"{side}_level"], [], None, []
    for _ in range(BAND8_STEPS + 1):
        visited.append(level)
        for case, own, _other, which in band8_cases(pair, side, level, answers):
            answered = runs.once(case, probe_flags(REF))
            if answered is None:
                raise Band8Stop(f"{case[0]}: the reference run at level {level} crashed or gave no answer")
            if answered != own:
                crossing = {"level": level, "case": case[0], "entry": which, "answer": answered}
                break
        if crossing:
            break
        region.append(level)
        level = band8_next(pair, level, side)
    width = min(len(region), BAND8_STEPS)
    bound = (f"bounded by reference crossing at level {crossing['level']}" if crossing
             else f"no reference crossing through L{BAND8_STEPS}")
    # The size of each step this side took, in the axis's own unit: bytes on the class axis, where it is the host's
    # class granularity for this pass.
    steps = [abs(after - before) for before, after in zip(visited, visited[1:])]
    write_event({"bound": {"host": HOST, "pair": pair["id"], "side": side, "axis": pair["axis"], "width": width,
                           "region": region, "bound": bound, "crossing": crossing, "steps": steps}})
    return region, width


def band8_level(runs, pool, pair, side, level, answers, write_event, hashes, count=None):
    """One measured level: `count` default-flag runs of each case the side has there, plus a REF and a CROSS run
    each, which never count. True when every case shows no opposite answer."""
    cases = band8_cases(pair, side, level, answers)
    count = count or BAND8_RUNS
    clean = True
    for case, own, other, which in cases:
        for flags in (REF, CROSS):
            if runs.once(case, probe_flags(flags)) != own:
                raise Band8Stop(f"{case[0]}: the {'reference' if flags == REF else 'cross-check'} flags do not give "
                                f"the {which} answer at level {level}")
        answered = list(pool.map(lambda _, case=case: runs.once(case, probe_flags(DEFAULT_FLAGS, DEFAULT_FLAGS)),
                                 range(count)))
        same = sum(1 for one in answered if one == own)
        opposite = sum(1 for one in answered if one == other)
        unexpected = len(answered) - same - opposite
        write_event({"host": HOST, "pair": pair["id"], "side": which, "axis": pair["axis"], "level": level,
                     "case": case[0], "startup": runs.port["case_startup"](case), "runs": len(answered),
                     "clean": same, "opposite": opposite, "unexpected": unexpected, "flags": list(DEFAULT_FLAGS),
                     **hashes})
        if unexpected:
            raise Band8Stop(f"{case[0]}: {unexpected} of {len(answered)} default-flag runs gave neither answer or none")
        if 2 * same <= len(answered):
            raise Band8Stop(f"{case[0]}: the default flags do not give the reference answer in most runs at level "
                            f"{level} ({same} of {len(answered)})")
        clean = clean and opposite == 0
    return clean


def band8_side(runs, pool, pair, side, established, answers, write_event, hashes, count=None, strict=False):
    """The margin of one side: 50 consecutive clean default-flag runs inside its reference region. With two levels
    or more there, the outer of the first two consecutive clean levels in L0 .. L8; with one, L0 at 50 runs.
    `strict` is the rule from PLAN Update 75: `count` runs at every level from L0, no level taken as clean
    unmeasured, and a region of one level is a stop."""
    region, width = band8_region(runs, pair, side, answers, write_event)
    if width == 0:
        raise Band8Stop(f"{pair['id']} {side}: the reference answer already differs at L0")
    if strict:
        established = False
        if width == 1:
            raise Band8Stop(f"{pair['id']} {side}: the region holds one level, so no two levels can be clean")
    if width == 1:
        clean = band8_level(runs, pool, pair, side, region[0], answers, write_event, hashes, BAND8_RUNS_ALONE)
        if not clean:
            raise Band8Stop(f"{pair['id']} {side}: L0 is the only level inside the region and it is not clean")
        return {"margin": region[0], "measured": [[region[0], clean]], "established": established, "width": width}
    streak, measured = (1, []) if established else (0, [])
    for level in region[1 if established else 0:BAND8_STEPS + 1]:
        clean = band8_level(runs, pool, pair, side, level, answers, write_event, hashes, count)
        measured.append([level, clean])
        streak = streak + 1 if clean else 0
        if streak == BAND8_CLEAN_IN_A_ROW:
            return {"margin": level, "measured": measured, "established": established, "width": width}
    raise Band8Stop(f"{pair['id']} {side}: no two clean levels in a row inside the region of {width} levels, "
                    f"by L{BAND8_STEPS}")


def band8_unit(strict=False):
    """The band round; `strict` is the PLAN Update 75 round: its own events folder, BAND8C_RUNS runs a level."""
    def go(pool, quiet):
        port = mg8_port()
        out = {"located": {}, "pm": {}}
        failed = mg8_profile(pool, port, out)
        if failed:
            return {"error": failed, **out}
        traced = r8_traced()
        hashes = {"pinned_sha256": r7_sha256(os.path.join(PACKAGE, "sdk.mjs")),
                  "traced_sha256": traced.get("traced_sha256")}
        out["hashes"] = hashes
        with open(BAND8_ESTABLISHED, encoding="utf-8") as handle:
            established = set(json.load(handle).get(HOST, []))
        try:
            pairs = [pair for pair in band8_pairs(port, out["profile"]) if not BAND8_ONLY or pair["id"] in BAND8_ONLY]
            if BAND8_ONLY and {pair["id"] for pair in pairs} != BAND8_ONLY:
                raise Band8Stop(f"no pair for {sorted(BAND8_ONLY - {pair['id'] for pair in pairs})}")
        except Band8Stop as stop:
            return {"error": str(stop), **out}
        folder = os.path.join(os.path.dirname(OUT), "band8c" if strict else "band8")
        os.makedirs(folder, exist_ok=True)
        runs = Band8Runs(port)
        out["pairs"], stopped = {}, []
        with concurrent.futures.ThreadPoolExecutor(max_workers=2 * (os.cpu_count() or 1)) as band_pool:
            for pair in pairs:
                events = os.path.join(folder, f"{pair['id']}.events.jsonl")
                lock = __import__("threading").Lock()

                def write_event(event, events=events, lock=lock):
                    with lock, open(events, "a", encoding="utf-8") as handle:
                        handle.write(json.dumps(event) + "\n")

                summary = {"host": HOST, "pair": pair["id"], "axis": pair["axis"], "near": pair["near"][0],
                           "far": pair["far"][0], "near_level": pair["near_level"], "far_level": pair["far_level"],
                           "flags": list(DEFAULT_FLAGS), **hashes}
                try:
                    if pair["near_level"] >= pair["far_level"] and not pair.get("limit"):
                        raise Band8Stop(f"{pair['id']}: the near level is not below the far level")
                    answers = {"near": band8_answer(runs, pair["near"]), "far": band8_answer(runs, pair["far"])}
                    if answers["near"] == answers["far"]:
                        raise Band8Stop(f"{pair['id']}: the pair answers the same")
                    sides = ["far"] if pair.get("limit") else ["near", "far"]
                    for side in sides:
                        own = pair[side][0] in established and not pair["id"].startswith("org_")
                        summary[side] = band8_side(runs, band_pool, pair, side, own, answers, write_event, hashes,
                                                   BAND8C_RUNS if strict else None, strict)
                except Band8Stop as stop:
                    summary["stop"] = str(stop)
                    stopped.append(pair["id"])
                summary["folder_assertion"] = runs.tally()
                write_event({"summary": summary})
                out["pairs"][pair["id"]] = summary
        out["folder_assertion"] = runs.tally()
        out["stopped"] = stopped
        assertion = out["folder_assertion"]
        if (assertion["refusals"] or assertion["failed_probe_checks"] or assertion["running_after"]
                or assertion["distinct_folders"] != assertion["runs"]):
            out["error"] = f"the folder check did not hold: {assertion}"
        elif stopped:
            out["error"] = f"pairs stopped: {stopped}"
        return out
    return go


# ---- band8c: a pair moved to the class axis, measured from L0 under the strengthened rule (PLAN Update 75) ----
BAND8C_RUNS = 50


# ---- band8x: the sides whose margin entry flipped in the margin record, stepped on (PLAN Update 74) ----
BAND8X_RUNS = 50
BAND8X_POOLED = os.path.join(HERE, "band8_pooled.json")


def band8x_side(runs, pool, pair, pooled, answers, write_event, hashes):
    """The new margin of a side whose pooled level is in band: from the level past it, outward inside the reference
    region through L8, the outer of the first two consecutive levels with no opposite answer in 50 default-flag
    runs."""
    side = pooled["side"]
    write_event({"pooled": {"host": HOST, "pair": pair["id"], **pooled}})
    region, width = band8_region(runs, pair, side, answers, write_event)
    if pooled["level"] not in region:
        raise Band8Stop(f"{pair['id']} {side}: the pooled level {pooled['level']} is outside the reference region "
                        f"{region}")
    others = {case[0] for other in runs.pairs if other["id"] != pair["id"] for case in (other["near"], other["far"])}
    streak, measured = 0, []
    for level in region[region.index(pooled["level"]) + 1:BAND8_STEPS + 1]:
        named = [case[0] for case, _own, _other, _which in band8_cases(pair, side, level, answers)]
        shared = sorted(others.intersection(named))
        if shared:
            raise Band8Stop(f"{pair['id']} {side}: level {level} is another pair's edge case {shared}")
        clean = band8_level(runs, pool, pair, side, level, answers, write_event, hashes, BAND8X_RUNS)
        measured.append([level, clean])
        streak = streak + 1 if clean else 0
        if streak == BAND8_CLEAN_IN_A_ROW:
            return {"margin": level, "measured": measured, "width": width, "from": pooled["level"]}
    raise Band8Stop(f"{pair['id']} {side}: no two clean levels in a row past the pooled level {pooled['level']} "
                    f"inside the region of {width} levels, by L{BAND8_STEPS}")


def band8x_unit():
    def go(pool, quiet):
        port = mg8_port()
        out = {"located": {}, "pm": {}}
        failed = mg8_profile(pool, port, out)
        if failed:
            return {"error": failed, **out}
        traced = r8_traced()
        hashes = {"pinned_sha256": r7_sha256(os.path.join(PACKAGE, "sdk.mjs")),
                  "traced_sha256": traced.get("traced_sha256")}
        out["hashes"] = hashes
        with open(BAND8X_POOLED, encoding="utf-8") as handle:
            wanted = json.load(handle).get(HOST, {})
        with open(MG8_MARGINS, encoding="utf-8") as handle:
            margins = json.load(handle)[HOST]
        runs = Band8Runs(port)
        try:
            runs.pairs = band8_pairs(port, out["profile"])
            pairs = [pair for pair in runs.pairs if pair["id"] in wanted]
            if {pair["id"] for pair in pairs} != set(wanted):
                raise Band8Stop(f"no pair for {sorted(set(wanted) - {pair['id'] for pair in pairs})}")
            for pair in pairs:
                committed, pooled = margins[pair["id"]], wanted[pair["id"]]
                edge = [pair["axis"], pair["near_level"], pair["far_level"]]
                if edge != [committed["axis"], committed["near_level"], committed["far_level"]]:
                    raise Band8Stop(f"the edge moved: {pair['id']} {edge}")
                if pooled["level"] != committed[f"{pooled['side']}_margin"] or pooled["axis"] != pair["axis"]:
                    raise Band8Stop(f"{pair['id']}: the pooled level is not the committed margin")
                if pair[f"{pooled['side']}_at"](pooled["level"])[0] != pooled["case"]:
                    raise Band8Stop(f"{pair['id']}: the pooled level does not make the pooled case")
        except Band8Stop as stop:
            return {"error": str(stop), **out}
        folder = os.path.join(os.path.dirname(OUT), "band8x")
        os.makedirs(folder, exist_ok=True)
        out["pairs"], stopped = {}, []
        with concurrent.futures.ThreadPoolExecutor(max_workers=2 * (os.cpu_count() or 1)) as band_pool:
            for pair in pairs:
                events = os.path.join(folder, f"{pair['id']}.events.jsonl")
                lock = __import__("threading").Lock()

                def write_event(event, events=events, lock=lock):
                    with lock, open(events, "a", encoding="utf-8") as handle:
                        handle.write(json.dumps(event) + "\n")

                pooled = wanted[pair["id"]]
                summary = {"host": HOST, "pair": pair["id"], "axis": pair["axis"], "side": pooled["side"],
                           "near_level": pair["near_level"], "far_level": pair["far_level"],
                           "flags": list(DEFAULT_FLAGS), **hashes}
                try:
                    answers = {"near": band8_answer(runs, pair["near"]), "far": band8_answer(runs, pair["far"])}
                    if answers["near"] == answers["far"]:
                        raise Band8Stop(f"{pair['id']}: the pair answers the same")
                    summary[pooled["side"]] = band8x_side(runs, band_pool, pair, pooled, answers, write_event, hashes)
                except Band8Stop as stop:
                    summary["stop"] = str(stop)
                    stopped.append(pair["id"])
                summary["folder_assertion"] = runs.tally()
                write_event({"summary": summary})
                out["pairs"][pair["id"]] = summary
        out["folder_assertion"] = runs.tally()
        out["stopped"] = stopped
        assertion = out["folder_assertion"]
        if (assertion["refusals"] or assertion["failed_probe_checks"] or assertion["running_after"]
                or assertion["distinct_folders"] != assertion["runs"]):
            out["error"] = f"the folder check did not hold: {assertion}"
        elif stopped:
            out["error"] = f"pairs stopped: {stopped}"
        return out
    return go


# ---- mg8x: the record of the entries the band8x and band8c rounds moved (PLAN Updates 74 and 75) ----
# mg8x_moved.json: host -> pair -> list of {side, axis, level, case}; a pair moved on both sides lists both.
MG8X_RUNS = MG8_OBSERVE_RUNS
MG8X_MOVED = os.path.join(HERE, "mg8x_moved.json")


def mg8x_unit():
    def go(pool, quiet):
        port = mg8_port()
        out = {"located": {}, "pm": {}}
        failed = mg8_profile(pool, port, out)
        if failed:
            return {"error": failed, **out}
        traced = r8_traced()
        hashes = {"pinned_sha256": r7_sha256(os.path.join(PACKAGE, "sdk.mjs")),
                  "traced_sha256": traced.get("traced_sha256")}
        out["hashes"] = hashes
        with open(MG8X_MOVED, encoding="utf-8") as handle:
            wanted = json.load(handle).get(HOST, {})
        with open(MG8_MARGINS, encoding="utf-8") as handle:
            margins = json.load(handle)[HOST]
        runs = Band8Runs(port)
        try:
            runs.pairs = band8_pairs(port, out["profile"])
            pairs = [pair for pair in runs.pairs if pair["id"] in wanted]
            if {pair["id"] for pair in pairs} != set(wanted):
                raise Band8Stop(f"no pair for {sorted(set(wanted) - {pair['id'] for pair in pairs})}")
            for pair in pairs:
                committed, moves = margins[pair["id"]], wanted[pair["id"]]
                edge = [pair["axis"], pair["near_level"], pair["far_level"]]
                if edge != [committed["axis"], committed["near_level"], committed["far_level"]]:
                    raise Band8Stop(f"the edge moved: {pair['id']} {edge}")
                sides = [moved["side"] for moved in moves]
                if not moves or len(set(sides)) != len(sides) or not set(sides) <= {"near", "far"}:
                    raise Band8Stop(f"{pair['id']}: the moved sides are not distinct near or far sides: {sides}")
                for moved in moves:
                    if moved["level"] != committed[f"{moved['side']}_margin"] or moved["axis"] != pair["axis"]:
                        raise Band8Stop(f"{pair['id']} {moved['side']}: the moved level is not the committed margin")
                    if pair[f"{moved['side']}_at"](moved["level"])[0] != moved["case"]:
                        raise Band8Stop(f"{pair['id']} {moved['side']}: the moved level does not make the moved case")
        except Band8Stop as stop:
            return {"error": str(stop), **out}
        folder = os.path.join(os.path.dirname(OUT), "mg8x")
        os.makedirs(folder, exist_ok=True)
        out["pairs"], failed = {}, []
        with concurrent.futures.ThreadPoolExecutor(max_workers=2 * (os.cpu_count() or 1)) as record_pool:
            for pair in pairs:
                events = os.path.join(folder, f"{pair['id']}.events.jsonl")
                lock = __import__("threading").Lock()

                def write_event(event, events=events, lock=lock):
                    with lock, open(events, "a", encoding="utf-8") as handle:
                        handle.write(json.dumps(event) + "\n")

                out["pairs"][pair["id"]] = {}
                try:
                    answers = {"near": band8_answer(runs, pair["near"]), "far": band8_answer(runs, pair["far"])}
                    if answers["near"] == answers["far"]:
                        raise Band8Stop(f"{pair['id']}: the pair answers the same")
                except Band8Stop as stop:
                    summary = {"host": HOST, "pair": pair["id"], "stop": str(stop), "folder_assertion": runs.tally()}
                    write_event({"summary": summary})
                    out["pairs"][pair["id"]]["answers"] = summary
                    failed.append(pair["id"])
                    continue
                for moved in wanted[pair["id"]]:
                    side = moved["side"]
                    summary = {"host": HOST, "pair": pair["id"], "axis": pair["axis"], "side": side,
                               "level": moved["level"], "case": moved["case"], "flags": list(DEFAULT_FLAGS),
                               "answers": answers, **hashes}
                    try:
                        # The golden entry: the recorder's own reference record, whose answer must be the side's.
                        case = band8_cases(pair, side, moved["level"], answers)[0][0]
                        try:
                            entry = port["reference"](*case)
                        except SystemExit as error:
                            raise Band8Stop(f"{case[0]}: the reference and cross-check flags answer otherwise") from error
                        answered = {key: value for key, value in entry.items()
                                    if key not in ("name", "files", "startup_bytes")}
                        if mg8_collapse(answered) != answers[side]:
                            raise Band8Stop(f"{case[0]}: the recorded answer is not the {side} answer")
                        summary["entry"] = entry
                        # The strict rule: every one of the default-flag runs gives the side's answer.
                        if not band8_level(runs, record_pool, pair, side, moved["level"], answers, write_event,
                                           hashes, MG8X_RUNS):
                            raise Band8Stop(f"{case[0]}: a default-flag run gave the other answer at the moved level")
                    except Band8Stop as stop:
                        summary["stop"] = str(stop)
                        failed.append(f"{pair['id']} {side}")
                    summary["folder_assertion"] = runs.tally()
                    write_event({"summary": summary})
                    out["pairs"][pair["id"]][side] = summary
        out["folder_assertion"] = runs.tally()
        out["failed"] = failed
        assertion = out["folder_assertion"]
        if (assertion["refusals"] or assertion["failed_probe_checks"] or assertion["running_after"]
                or assertion["distinct_folders"] != assertion["runs"]):
            out["error"] = f"the folder check did not hold: {assertion}"
        elif failed:
            out["error"] = f"moved entries failed the record: {failed}"
        return out
    return go


def round8_units():
    out = [("rec8:instrument", r8_instrument_unit())]
    out += [(f"rec8:fixture:{name}", rec8_fixture(name)) for name in R8_FIXTURES]
    out += [(f"rec8:events:{R8_EVICTION}", rec8_fixture(R8_EVICTION, True))]
    out += [(f"pm8:{label}:{which}", pm8_unit(label, which)) for label in R8_LABELS for which in R8_PASSES]
    if R8_SECOND:
        # A fresh identity: the second call whose options object the pm WeakMap has not seen (PLAN Update 82 A).
        out += [(f"pm8f:{label}:{which}", pm8_unit(label, which, "fresh")) for label in R8_LABELS for which in R8_PASSES]
    # Last: it fills the handed-over profile keys from the pm8 classes this job found.
    out += [("mg8:goldens", mg8_unit())]
    # After the pm8 units too: the default-flag margins of every pair the profile names.
    out += [("band8:goldens", band8_unit())]
    # The sides whose margin entry flipped in the margin record, from their pooled level outward.
    out += [("band8x:goldens", band8x_unit())]
    out += [("band8c:goldens", band8_unit(True))]
    # The entries moved by band8x and band8c, recorded at their new margins under the strict rule.
    out += [("mg8x:goldens", mg8x_unit())]
    return out



def plutil_depth_unit(shape):
    """The deepest XML plist the real /usr/bin/plutil converts to JSON (the darwin read, sdk.mjs 0.3.323 line 136
    col 804652: `plutil -convert json -o - --`), with `shape` `o` (objects) or `a` (arrays) closed by `true`."""
    def converts(level, folder):
        if shape == "o":
            body = "<dict><key>x</key>" * (level - 1) + "<true/>" + "</dict>" * (level - 1)
        else:
            body = "<array>" * level + "<true/>" + "</array>" * level
        location = os.path.join(folder, f"{level}.plist")
        with open(location, "w", encoding="utf-8") as handle:
            handle.write('<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0">' + body + "</plist>\n")
        try:
            run = subprocess.run(["/usr/bin/plutil", "-convert", "json", "-o", "-", "--", location],
                                 capture_output=True, timeout=60)
        finally:
            os.remove(location)
        return run.returncode == 0, run.stderr.decode("utf-8", "replace")[-300:]

    def go(pool, quiet):
        folder = tempfile.mkdtemp(prefix="p")
        try:
            low, high = 1, 1
            while converts(high, folder)[0]:
                low, high = high, high * 2
                if high > 1 << 20:
                    return {"shape": shape, "max_converted": None, "note": "converts past 2^20 levels"}
            while high - low > 1:
                middle = (low + high) // 2
                if converts(middle, folder)[0]:
                    low = middle
                else:
                    high = middle
            around = {level: converts(level, folder)[0] for level in range(max(1, low - 8), high + 9)}
            return {"shape": shape, "max_converted": low, "first_rejected": high,
                    "rejected_stderr": converts(high, folder)[1], "around": around,
                    "monotone_around": all(ok == (level <= low) for level, ok in around.items())}
        finally:
            shutil.rmtree(folder, ignore_errors=True)
    return go


# ---- Windows native discovery (rulings 35, 36.5b; PLAN Update 82 W) ----
# The machine policy (HKLM) holds a policy; a user hive that answers; a machine hive the stand-in prints past the
# 2097152-byte reg.exe buffer, which the execFile read cannot finish.
W35_POLICY = '{"permissions":{"deny":["Bash(rm:*)"]}}'
W35_UNREAD = '{"permissions":{"deny":["Bash(rm:*)"]},"pad":"' + "p" * 2100000 + '"}'
# row: (hklm document, hkcu document, managed json document, the resume base the cited frames predict)
W35_ROWS = {
    "hklm_policy": (W35_POLICY, None, None, "Tool"),
    "hkcu_absent": (None, None, None, "Tool"),
    "hkcu_json": (None, W35_POLICY, W35_POLICY, "Tool"),
    "hkcu_dir": (None, W35_POLICY, None, "Fs"),
    "hklm_unread_dir": (W35_UNREAD, W35_POLICY, None, "Fs"),
    "hklm_unread_absent": (W35_UNREAD, None, None, "Tool"),
}
# The startup axis: the env+argv size each proto band also runs at, past the default.
W35_STARTUP_STEP = 4096
# Keys no earlier Windows run measured, searched over a range around the Windows levels of the neighbouring keys
# (main_merge 1272, main_read_arrays 1833, sources_copy 1833 in windows_profile_guess.json).
W35_RANGES = {"origin_merge": [600, 2600], "origin_read_arrays": [900, 4000], "machine_read_arrays": [900, 4000],
              "origin_copy": [900, 3600]}


def base35_probe(row, n):
    """Whether the user settings at the home folder, holding `x` with an own `__proto__` `n` levels of arrays deep,
    reach the effective settings after the registry and managed reads of `row`."""
    hklm, hkcu, managed, _ = W35_ROWS[row]
    root = os.path.join(D["SCRATCH"], unique("b"))
    spec_path = root + ".json"
    try:
        for folder in ("home/.claude", "work"):
            os.makedirs(os.path.join(root, folder), exist_ok=True)
        write(os.path.join(root, "home/.claude/settings.json"),
              '{"x":{"__proto__":' + deep(0, n - 1, "null_array") + "}}")
        system32 = os.path.join(root, "mnt", "c", "Windows", "System32")
        D["tool"](system32, "reg.exe", D["REG_SH"])
        for name, document in (("hklm", hklm), ("hkcu", hkcu)):
            if document is not None:
                write(os.path.join(system32, name + ".txt"), D["reg_text"](document))
        if managed is not None:
            write(os.path.join(root, "Program Files", "ClaudeCode", "managed-settings.json"), managed)
        spec = {"root": root, "platform": "win32", "deep": True,
                "env": {"HOME": os.path.join(root, "home"), "SYSTEMROOT": os.path.join(root, "mnt", "c", "Windows")},
                "cases": [{"name": "case", "options": {"cwd": os.path.join(root, "work")}}]}
        with open(spec_path, "w", encoding="utf-8") as handle:
            json.dump(spec, handle)
        answered = run_spec(spec_path, REF, STARTUP)
        if isinstance(answered, dict):
            return False
        return "x" in answered[0]["result"]["effective"]
    finally:
        shutil.rmtree(root, ignore_errors=True)
        if os.path.exists(spec_path):
            os.remove(spec_path)


def base35_unit(row):
    def go(pool, quiet):
        found = windows_band(pool, lambda n: base35_probe(row, n), 50, 6000)
        found["predicted_base"] = W35_ROWS[row][3]
        return found
    return go


def startup35_unit(observable):
    def go(pool, quiet):
        startup = STARTUP + W35_STARTUP_STEP
        found = windows_band(pool, lambda n: proto_probe((0, n - 1, "null_array"), None, startup)[observable], 50, 6000)
        found["startup"] = startup
        return found
    return go


def pm35_unit(label, which):
    """The Windows band of the parent or remote managed settings pass `which`, each level the shallow form's
    answer or not."""
    def go(pool, quiet):
        visible, detail = r8_control(label)
        if not visible:
            return {"error": "the control does not show the document reaching every stage", "control": detail}
        major = R8_PASSES[which][3]
        fits = r8_fits(label, which)

        def shape(n):
            return (n, 0, "null_array") if major == "o" else (0, n, "null_array")
        found = windows_band(pool, lambda n: bool(fits(shape(n), None, STARTUP)), 50, 6000)
        found["major"] = major
        found["control"] = detail
        return found
    return go


def windows35_units():
    out = [(f"base35:{row}", base35_unit(row)) for row in W35_ROWS]
    out += [(f"startup35:{o}", startup35_unit(o)) for o in "ESOM"]
    out += [(f"pm35:{label}:{which}", pm35_unit(label, which)) for label in R8_LABELS for which in R8_GOLDEN_PASSES]
    out += [(f"key:{key}", key_unit(key, guess, windows_guesses())) for key, guess in W35_RANGES.items()]
    return out


def units():
    out = [(f"proto:{o}", proto_unit(o)) for o in "ESOM"]
    if DARWIN and GATE["NATIVE"]:
        out += [(f"plutil_depth:{shape}", plutil_depth_unit(shape)) for shape in "oa"]
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
        out += windows35_units()
    else:
        out += unix_round4_units() + round5_unix_units() + round6_held_units() + round6_xp_units() + round7_units() + round8_units()
        profile = MANAGED["PROFILES"].get(HOST, {})
        # Under a WSL kernel org_copy is measured whatever the profile holds: Gc (136 col 4257) reads /proc/version only
        # on linux, and only a real WSL kernel answers it (no /proc stand-in is fit data, PLAN Update 82 D).
        out += [(f"key:{key}", key_unit(key, guess)) for key, guess in GUESSES.get(HOST, {}).items()
                if key not in profile or (GATE["WSL"] and key == "org_copy")]
        out += [(f"key:{key}", key_unit(key, guess, profile)) for key, guess in PASS_KEYS.get(HOST, {}).items()]
        if os.environ.get("R3_DISCOVERY"):
            out += discovery_units()
    return out


# The reason the native oracle gives for a spec that names another platform than the host's.
PLATFORM_REFUSAL = __import__("re").compile(r"platform \w+ on a \w+ host")


def main():
    version = subprocess.run([NODE, "--version"], capture_output=True, check=True).stdout.decode().strip()
    result = {"host": HOST, "node": version, "shard": SHARD, "shards": SHARDS, "machine": machine(), "units": {},
              "native": {"enabled": GATE["NATIVE"], "call": GATE["CALL"], "context": GATE["CONTEXT"],
                         "platform": sys.platform}}
    R8_STATE["units"] = result["units"]
    started = time.time()
    only = __import__("re").compile(os.environ["R3_ONLY"]) if os.environ.get("R3_ONLY") else None
    matching = [unit for unit in units() if not only or only.search(unit[0])]
    # R3_EVERY_SHARD runs every matching unit in each shard: each shard is one job of a pooled band.
    chosen = matching if os.environ.get("R3_EVERY_SHARD") else matching[SHARD - 1::SHARDS]
    with concurrent.futures.ThreadPoolExecutor(max_workers=WORKERS) as pool, \
            concurrent.futures.ThreadPoolExecutor(max_workers=QUIET) as quiet:
        for name, go in chosen:
            tally = GATE["snapshot"]()
            try:
                result["units"][name] = go(pool, quiet)
            except Exception as error:  # a unit that cannot finish reports why and the next one runs
                result["units"][name] = {"error": repr(error)}
            if GATE["NATIVE"]:
                # What every oracle run of the unit reported; a unit whose runs broke a check failed with them.
                assertion = GATE["delta"](tally, GATE["snapshot"]())
                # A spec written for another platform is refused by the native oracle (resolve_settings_oracle.mjs
                # line 110) before it resolves anything, so it can record no wrong data. On Unix the tool base's
                # `native` place writes a win32 spec, and when that unit stops on the refusal, the probes it still had
                # in flight on the quiet pool end during the next unit and land in its tally. Those are expected
                # platform refusals, not a check the next unit failed.
                platform_refusals = {reason: number for reason, number in assertion["refusals"].items()
                                     if PLATFORM_REFUSAL.fullmatch(reason)}
                assertion["refusals"] = {reason: number for reason, number in assertion["refusals"].items()
                                         if reason not in platform_refusals}
                assertion["expected_platform_refusals"] = platform_refusals
                result["units"][name]["native_assertion"] = assertion
                if (assertion["violations"] or assertion["refusals"]) and "error" not in result["units"][name]:
                    result["units"][name]["error"] = f"native runs failed their checks: {assertion['refusals']} " \
                                                     f"{assertion['violations'][:5]}"
            result["units"][name]["seconds"] = round(time.time() - started)
            with open(OUT, "w", encoding="utf-8") as handle:
                json.dump(result, handle)
            brief = {k: v for k, v in result["units"][name].items() if k in ("class", "startup_max", "budget", "error", "changes", "edge_guess", "profile", "window", "room_at_class")}
            if GATE["NATIVE"]:
                native = result["units"][name]["native_assertion"]
                brief["native"] = {key: native[key] for key in ("runs", "platforms", "spawns", "refusals",
                                                                  "expected_platform_refusals")}
            sys.stderr.write(f"{name} {json.dumps(brief)[:600]}\n")
            sys.stderr.flush()
    shutil.rmtree(D["SCRATCH"], ignore_errors=True)


if os.environ.get("R3_LIST"):
    listed = re.compile(os.environ["R3_ONLY"]) if os.environ.get("R3_ONLY") else None
    for number, (name, _) in enumerate(unit for unit in units() if not listed or listed.search(unit[0])):
        print(number % SHARDS + 1, name)
else:
    main()
