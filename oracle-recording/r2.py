#!/usr/bin/env python3
"""Records, on the host it runs on, where the pinned TypeScript resolveSettings changes its answer for every settings
depth, managed settings and registry/plist family, from each stack base the SDK can resume its synchronous settings
evaluation on: the base a file request leaves (fs), the base a spawned program's end leaves through the fake clock
(timer), through a file request (file) and through its real end (real), and the base each upstream await site leaves.

usage: python3 -I r2.py LIB NODE PACKAGE OUT SHARD SHARDS PASSES

Each unit is one case family under one site. Its timer and file deliveries are scanned over a grid and every change
narrowed to the exact level and re-run twice with the reference flags and once with the cross-check flags. Its real
delivery, and on Windows every delivery, is then run PASSES times at each level of a window around those changes.
Units are dealt round robin to SHARDS shards; this run takes shard SHARD (1-based).

With R2_HELD_CHECK=1, only the units of the managed folder check run: the check that decides whether a Windows user
policy applies parses each managed settings file it finds on a stack of its own, outside the settings evaluation.

With R2_REAL_PASSES set, only the units with a real delivery run, and their real delivery runs that many times at each
level of a wider window: a spawned program's real end resumes from a base of its own, a level or two off both the
timer and the file base, and its rarer outcomes only show over many passes.
"""

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
LIB, NODE, PACKAGE, OUT, SHARD, SHARDS, PASSES = sys.argv[1:8]
SHARD, SHARDS, PASSES = int(SHARD), int(SHARDS), int(PASSES)
REAL_PASSES = int(os.environ.get("R2_REAL_PASSES") or 0)
HELD_ONLY = os.environ.get("R2_HELD_CHECK") == "1"
with open(os.path.join(HERE, "discover.py"), encoding="utf-8") as handle:
    SOURCE = handle.read()
SOURCE = SOURCE[: SOURCE.index("\nmain()")]
D = {"__name__": "discover", "__file__": os.path.join(HERE, "discover.py")}
saved = sys.argv
sys.argv = [D["__file__"], LIB, NODE, PACKAGE, OUT + ".discover"]
try:
    exec(compile(SOURCE, D["__file__"], "exec"), D)
finally:
    sys.argv = saved

DEPTH, MANAGED = D["DEPTH"], D["MANAGED"]
WINDOWS = D["WINDOWS"]
REF, CROSS = D["REF"], D["CROSS"]
ORACLE = MANAGED["ORACLE"]
collapse, write, reg_text, tool, unique = D["collapse"], D["write"], D["reg_text"], D["tool"], D["unique"]
REG_SH, PLUTIL_SH = D["REG_SH"], D["PLUTIL_SH"]
FLAG = '{"wslInheritsWindowsSettings":true}'
POLICY = '{"permissions":{"deny":["Bash(rm:*)"]}}'
SYSTEM32 = "mnt/c/Windows/System32"
SCRATCH = D["SCRATCH"]
GRID = D["GRID"]
WORKERS = max(2, os.cpu_count() or 2)
REAL_WORKERS = 2
FAILS = {}


def dumps(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def oracle(spec_path, flags):
    """The raw result entries of the oracle run of `spec_path`, or the error it rejects with."""
    command = [NODE, *D["probe_flags"](flags), ORACLE, PACKAGE, spec_path]
    with tempfile.TemporaryFile() as errors:
        process = D["spawn"](command, {"PAD": "x" * MANAGED["pad"](command)}, stdout=subprocess.PIPE, stderr=errors)
        errors.seek(0)
        stderr = errors.read().decode("utf-8", "replace").replace("\r\n", "\n")
    if process.returncode == 0:
        return json.loads(process.stdout.decode("utf-8"))
    found = re.search(r"^(\w*Error): (.*)$", stderr, re.M)
    if not found:
        return {"fail": stderr[-800:]}
    return {"error": {"name": found.group(1), "message": collapse(found.group(2))}}


def project(answered, root):
    """What a run answers, with nested runs collapsed and the root left out: effective, machine policy and sources."""
    if isinstance(answered, dict):
        if "fail" in answered:
            return dumps({"fail": True}), answered["fail"]
        return dumps(answered), None
    out = []
    for entry in answered:
        result = entry["result"]
        out.append({
            "effective": collapse(dumps(result.get("effective"))),
            "machine": collapse(dumps(result.get("machinePolicy"))),
            "sources": [[source.get("source"), source.get("policyOrigin"), collapse(dumps(source.get("settings")))] for source in result.get("sources", [])],
        })
    text = dumps(out)
    for form in {root, root.replace("\\", "\\\\"), root.replace("\\", "/")}:
        text = text.replace(form, "R")
    return text, None


# Sites: what the host holds beside the case files, which sets the last thing resolveSettings awaits before it
# evaluates the settings. Each names the deliveries of a spawned program's end it is run with.
def wsl(root, spec, hklm, hkcu=None, held=None):
    # WSL is a linux path: another host refuses it natively instead of reading the case as its own platform.
    spec["platform"] = "linux"
    spec["env"]["WSL_DISTRO_NAME"] = "Ubuntu"
    folder = os.path.join(root, SYSTEM32)
    tool(folder, "reg.exe", REG_SH)
    write(os.path.join(folder, "hklm.txt"), reg_text(hklm))
    if hkcu is not None:
        write(os.path.join(folder, "hkcu.txt"), reg_text(hkcu))
    if held is not None:
        write(os.path.join(root, "mnt/c/Program Files/ClaudeCode/managed-settings.json"), held)


def win32(root, spec, hkcu=None):
    spec["platform"] = "win32"
    # Natively the SYSTEMROOT a run reads the registry through is its own, under the root.
    spec["env"]["SYSTEMROOT"] = os.path.join(root, "mnt", "c", "Windows") if WINDOWS else "/mnt/c/Windows"
    if WINDOWS:
        folder, name = os.path.join(root, SYSTEM32), "reg.exe"
    else:
        folder, name = os.path.join(root, "mnt/c"), "Windows\\System32\\reg.exe"
    tool(folder, name, REG_SH)
    if hkcu is not None:
        write(os.path.join(folder, "hkcu.txt"), reg_text(hkcu))


def darwin(root, spec, plist=None):
    spec["platform"] = "darwin"
    if plist is not None:
        tool(os.path.join(root, "usr/bin"), "plutil", PLUTIL_SH)
        write(os.path.join(root, "Library/Managed Preferences/com.anthropic.claudecode.plist"), plist)


SITES = {
    "plain": (lambda root, spec: None, ["timer"]),
    "fs": (lambda root, spec: wsl(root, spec, FLAG, "{}"), ["timer", "file"]),
    "tool": (lambda root, spec: wsl(root, spec, FLAG), ["timer", "file", "real"]),
    "yet": (lambda root, spec: wsl(root, spec, POLICY), ["timer", "file", "real"]),
    "unread": (lambda root, spec: wsl(root, spec, '{"wslInheritsWindowsSettings":"maybe"}'), ["timer", "file", "real"]),
    "short": (lambda root, spec: wsl(root, spec, FLAG, "{}", '{"model":"m"}'), ["timer", "file", "real"]),
    "w32": (lambda root, spec: win32(root, spec), ["timer", "file", "real"]),
    "w32_hkcu": (lambda root, spec: win32(root, spec, "{}"), ["timer", "file", "real"]),
    "plist_none": (lambda root, spec: darwin(root, spec), ["timer"]),
    "plist_empty": (lambda root, spec: darwin(root, spec, "{}"), ["timer", "file", "real"]),
    "plist_policy": (lambda root, spec: darwin(root, spec, POLICY), ["timer", "file", "real"]),
    # The organization steps: the first resolve as soon as the package loads, or once the clock reaches 0.
    "noat": (lambda root, spec: spec["cases"][0].pop("at", None), ["timer"]),
    "at": (lambda root, spec: spec["cases"][0].update({"at": 0}), ["timer"]),
}


def recorder_probe(space, files_of, prefix, site, delivery):
    def probe(level, flags):
        name = unique(prefix)
        folder = os.path.join(space["FOLDER"], name)
        spec_path = None
        try:
            spec_path = space["prepare"](name, files_of(level))
            with open(spec_path, encoding="utf-8") as handle:
                spec = json.load(handle)
            SITES[site][0](spec["root"], spec)
            if delivery != "timer":
                spec["tools"] = delivery
            with open(spec_path, "w", encoding="utf-8") as handle:
                json.dump(spec, handle)
            answered, failure = project(oracle(spec_path, flags), spec["root"])
            if failure is not None:
                FAILS.setdefault(f"{prefix}:{site}:{delivery}", failure)
            return answered
        finally:
            shutil.rmtree(folder, ignore_errors=True)
            if spec_path and os.path.exists(spec_path):
                os.remove(spec_path)

    return probe


def registry_probe(place, key, shape, site, delivery):
    def probe(level, flags):
        root = os.path.join(SCRATCH, unique("r"))
        try:
            spec = D["registry_spec"](place, key, shape, level, root)
            SITES[site][0](root, spec)
            if delivery != "timer":
                spec["tools"] = delivery
            spec_path = root + ".json"
            with open(spec_path, "w", encoding="utf-8") as handle:
                json.dump(spec, handle)
            answered, failure = project(oracle(spec_path, flags), root)
            if failure is not None:
                FAILS.setdefault(f"registry:{place}:{site}:{delivery}", failure)
            return answered
        finally:
            shutil.rmtree(root, ignore_errors=True)
            if os.path.exists(root + ".json"):
                os.remove(root + ".json")

    return probe


# The managed folder check: a managed settings file nests `n` arrays or objects under a member that carries no policy, so
# the file only counts as held, and the Windows user policy (HKCU) only stops applying, once its parse runs out of
# stack. Under WSL the Windows folder is checked first, right after reg.exe answers, and the platform folder after the
# Windows folder's drop-in listing; native Windows checks its own folder right after reg.exe answers.
HELD_CHECK_DOC = '{"managedSourcesBehavior":%s}'
HELD_CHECK_HKCU = '{"wslInheritsWindowsSettings":true,"effortLevel":"high"}'
HELD_CHECK_SHAPES = {"o": MANAGED["nested_objects"], "a": MANAGED["nested_arrays"]}
WINDOWS_MANAGED = "Program Files/ClaudeCode/managed-settings.json"


def held_check_probe(place, shape, delivery):
    def probe(level, flags):
        name = unique("held_check")
        folder = os.path.join(MANAGED["FOLDER"], name)
        text = HELD_CHECK_DOC % HELD_CHECK_SHAPES[shape](level)
        spec_path = None
        try:
            spec_path = MANAGED["prepare"](name, [["managed", text]] if place == "platform" else [])
            with open(spec_path, encoding="utf-8") as handle:
                spec = json.load(handle)
            root = spec["root"]
            if place == "platform":
                wsl(root, spec, FLAG, HELD_CHECK_HKCU)
            elif place == "windows":
                wsl(root, spec, FLAG, HELD_CHECK_HKCU, text)
            else:
                win32(root, spec, HELD_CHECK_HKCU)
                write(os.path.join(root, WINDOWS_MANAGED), text)
            if delivery != "timer":
                spec["tools"] = delivery
            with open(spec_path, "w", encoding="utf-8") as handle:
                json.dump(spec, handle)
            answered, failure = project(oracle(spec_path, flags), root)
            if failure is not None:
                FAILS.setdefault(f"held_check:{place}:{shape}:{delivery}", failure)
            return answered
        finally:
            shutil.rmtree(folder, ignore_errors=True)
            if spec_path and os.path.exists(spec_path):
                os.remove(spec_path)

    return probe


def held_check_units():
    out = []
    for place, site in (("platform", "held_check_wsl_platform"), ("windows", "held_check_wsl_windows"), ("native", "held_check_win32")):
        for shape in HELD_CHECK_SHAPES:
            make = (lambda p, sh: lambda delivery: held_check_probe(p, sh, delivery))(place, shape)
            out.append((f"held_check:{place}:{shape}|{site}", [f"held_check.{place}.{shape}"], site, ["timer", "file", "real"], make))
    return out


def prefix_of(keys):
    if keys.startswith("org_"):
        return "org_c"
    if keys.startswith("effective_"):
        return "effective_c"
    return "c"


def recorder_families():
    """Every case family of the two recorders, once per set of files, as (name, keys, space, files_of, prefix)."""
    found = {}
    for label, space in (("depth", DEPTH), ("managed", MANAGED)):
        groups = [(keys, (lambda make: lambda n: make(n)[0][1])(make)) for keys, make in space["GROUPS"] if " " not in keys]
        if label == "managed":
            groups.append(("effective_ask effective_ask_machine", space["effective_ask"]))
        for keys, files_of in groups:
            prefix = prefix_of(keys)
            signature = f"{label}:{prefix}:{json.dumps(files_of(500))}"
            shifted = f"{label}:{prefix}:{json.dumps(files_of(501))}"
            twin = next((name for name, entry in found.items() if entry["shifted"] == signature or name == shifted), None)
            if twin is not None and twin != signature:
                found[twin]["keys"].append(f"{label}.{keys} (shifted)")
                continue
            entry = found.setdefault(signature, {"keys": [], "space": space, "files_of": files_of, "prefix": prefix, "label": label, "shifted": shifted})
            entry["keys"].append(f"{label}.{keys}")
    return found


def units():
    """Every (unit name, keys, site, deliveries, probe maker) this recording covers, in a fixed order."""
    out = []
    for signature, entry in recorder_families().items():
        label, prefix = entry["label"], entry["prefix"]
        if prefix == "org_c":
            sites = ["noat", "at"]
        elif label == "depth":
            sites = ["plain", "fs"]
        else:
            sites = ["plain", "fs", "tool"]
        for site in sites:
            make = (lambda e, s: lambda delivery: recorder_probe(e["space"], e["files_of"], e["prefix"], s, delivery))(entry, site)
            out.append((f"{signature}|{site}", entry["keys"], site, SITES[site][1], make))
    # The user settings file nested `n` deep under every site, which tells the base each site leaves.
    # The user settings file holding `n` nested objects or arrays under every site, which tells the base each site
    # leaves: arrays cost the reader the fewest bytes per level, so they tell bases apart the finest.
    shapes = {"user_objects": lambda n: [("userSettings", n, 0, 0)], "user_arrays": lambda n: [("userSettings", 1, n, 0)]}
    for shape, files_of in shapes.items():
        for site in ["yet", "unread", "short", "w32", "w32_hkcu", "plist_none", "plist_empty", "plist_policy"]:
            make = (lambda s, f: lambda delivery: recorder_probe(DEPTH, f, "c", s, delivery))(site, files_of)
            out.append((f"site:{shape}|{site}", [f"site.{shape}.{site}"], site, SITES[site][1], make))
    for place in ("hklm", "plist", "hkcu", "held_hklm", "held_hkcu"):
        # A held machine policy keeps only its policy members, so the member holding the depth is dropped whatever
        # its name: one name covers it.
        for key in (("x",) if place.startswith("held_") else ("x", "model")):
            for shape in ("o", "a"):
                if not place.startswith("held_"):
                    sites = [("plain", ["timer", "file", "real"])]
                else:
                    # The held document loads in the second resolve, under WSL, after reg.exe answers: its end is
                    # delivered every way there, and the first resolve starts either way.
                    sites = [("at", ["timer"]), ("noat", ["timer", "file", "real"])]
                for site, deliveries in sites:
                    make = (lambda p, k, sh, s: lambda delivery: registry_probe(p, k, sh, s, delivery))(place, key, shape, site)
                    out.append((f"registry:{place}:{key}:{shape}|{site}", [f"registry.{place}.{key}.{shape}"], site, deliveries, make))
    return out


def recorded_units():
    if HELD_ONLY:
        return held_check_units()
    return [unit for unit in units() if not REAL_PASSES or "real" in unit[3]]


def scan(pool, probe):
    cache = {}

    def at(level):
        if level not in cache:
            cache[level] = probe(level, REF)
        return cache[level]

    for level, answered in zip(GRID, pool.map(at, GRID)):
        cache[level] = answered
    changes = []
    for low, high in zip(GRID, GRID[1:]):
        if at(low) == at(high):
            continue
        levels = list(range(low + 1, high))
        for level, answered in zip(levels, pool.map(at, levels)):
            cache[level] = answered
        for level in range(low, high):
            if at(level) != at(level + 1):
                changes.append(level)
    answers = []

    def index(answered):
        if answered not in answers:
            answers.append(answered)
        return answers.index(answered)

    transitions = []
    for level in changes:
        near, far = at(level), at(level + 1)
        checks = list(pool.map(lambda job: probe(job[0], job[1]), [(level, REF), (level + 1, REF), (level, REF), (level + 1, REF), (level, CROSS), (level + 1, CROSS)]))
        transitions.append({
            "level": level,
            "near": index(near),
            "far": index(far),
            "repeatable": checks[0] == near and checks[1] == far and checks[2] == near and checks[3] == far,
            "cross_check": checks[4] == near and checks[5] == far,
        })
    return {"first": index(at(GRID[0])), "transitions": transitions, "answers": answers}


def windows_around(levels, reach):
    spans = sorted((max(1, level - reach), level + 1 + reach) for level in levels)
    merged = []
    for low, high in spans:
        if merged and low <= merged[-1][1] + 1:
            merged[-1][1] = max(merged[-1][1], high)
        else:
            merged.append([low, high])
    return merged


def band(pool, probe, windows, answers, flags_list, passes):
    levels = {}
    jobs = [(level, flags) for low, high in windows for level in range(low, high + 1) for flags in flags_list]
    for number in range(passes):
        for (level, flags), answered in zip(jobs, pool.map(lambda job: probe(job[0], REF if job[1] == "ref" else CROSS), jobs)):
            if answered not in answers:
                answers.append(answered)
            levels.setdefault(str(level), {}).setdefault(flags, []).append(answers.index(answered))
    return levels


def save(result):
    result["fails"] = FAILS
    with open(OUT, "w", encoding="utf-8") as handle:
        json.dump(result, handle)


def main():
    version = subprocess.run([NODE, "--version"], capture_output=True, check=True).stdout.decode().strip()
    only = re.compile(os.environ["R2_ONLY"]) if os.environ.get("R2_ONLY") else None
    chosen = [unit for number, unit in enumerate(recorded_units()) if (only.search(unit[0]) if only else number % SHARDS == SHARD - 1)]
    result = {"host": D["HOST"], "node": version, "flags": REF, "cross_check_flags": CROSS, "passes": PASSES, "real_passes": REAL_PASSES or PASSES,
              "workers": WORKERS, "real_workers": REAL_WORKERS, "shard": SHARD, "shards": SHARDS, "units": {}}
    started = time.time()
    with concurrent.futures.ThreadPoolExecutor(max_workers=WORKERS) as pool, \
            concurrent.futures.ThreadPoolExecutor(max_workers=REAL_WORKERS) as quiet_pool:
        for name, keys, site, deliveries, make in chosen:
            unit = result["units"][name] = {"keys": keys, "site": site, "deliveries": {}}
            scanned = [delivery for delivery in deliveries if delivery != "real"]
            for delivery in scanned:
                unit["deliveries"][delivery] = scan(pool, make(delivery))
            near = sorted({t["level"] for delivery in scanned for t in unit["deliveries"][delivery]["transitions"]})
            # The timer and file bands on Windows come from the full recording; a real-only run of the other units
            # leaves them out.
            if WINDOWS and (HELD_ONLY or not REAL_PASSES):
                for delivery in scanned:
                    found = unit["deliveries"][delivery]
                    reach_levels = [t["level"] for t in found["transitions"]]
                    found["bands"] = band(pool, make(delivery), windows_around(reach_levels, 6), found["answers"], ["ref", "cross"], PASSES)
            if "real" in deliveries:
                answers = list(unit["deliveries"][scanned[0]]["answers"])
                windows = windows_around(near, 6 if WINDOWS else 4 if REAL_PASSES else 2)
                unit["deliveries"]["real"] = {"windows": windows, "answers": answers,
                                              "bands": band(quiet_pool, make("real"), windows, answers, ["ref"], REAL_PASSES or PASSES)}
            unit["seconds"] = round(time.time() - started)
            save(result)
            summary = {d: [(t["level"], t["repeatable"], t["cross_check"]) for t in v.get("transitions", [])] for d, v in unit["deliveries"].items()}
            sys.stderr.write(f"{name[:120]} {summary}\n")
            sys.stderr.flush()
    save(result)
    shutil.rmtree(SCRATCH, ignore_errors=True)


if os.environ.get("R2_LIST"):
    for number, (name, keys, site, deliveries, _) in enumerate(recorded_units()):
        print(number % SHARDS + 1, name[:140], keys, deliveries)
else:
    main()
