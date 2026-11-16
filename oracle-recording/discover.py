#!/usr/bin/env python3
"""Finds every level where the pinned TypeScript resolveSettings changes its answer for the settings depth, managed
settings and registry/plist case families, on the host it runs on.

usage: python3 -I discover.py LIB NODE PACKAGE OUT [FAMILY_REGEX]

LIB is the claude-sdk-freshness folder (the recorders and resolve_settings_oracle.mjs). Each family is scanned over a
grid, every change between grid points is narrowed to the exact level, and each change is re-run twice with the
reference flags and once with the cross-check flags. STUB names a compiled stand-in for reg.exe and plutil on hosts
that cannot run the shell stand-ins.
"""

import concurrent.futures
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import threading

LIB, NODE, PACKAGE, OUT = sys.argv[1:5]
PATTERN = re.compile(sys.argv[5]) if len(sys.argv) > 5 else None
HOST = f"{sys.platform} {platform.machine()}"
WINDOWS = sys.platform == "win32"
STUB = os.environ.get("STUB")
SCRATCH = tempfile.mkdtemp(prefix="d")
sys.setrecursionlimit(20000)


def started(env):
    """Whether Node starts on this host with only `env`; Windows may need `SYSTEMROOT` as well."""
    return subprocess.run([NODE, "-e", "0"], env=env, capture_output=True).returncode == 0


EXTRA_ENV = {}
if WINDOWS and not started({"PAD": "x"}):
    EXTRA_ENV = {"SYSTEMROOT": os.environ["SYSTEMROOT"]}


class Spawner:
    """`subprocess` as the recorders call it, with `EXTRA_ENV` added to every environment they pass."""

    PIPE = subprocess.PIPE

    @staticmethod
    def run(*args, **kwargs):
        if kwargs.get("env") is not None:
            kwargs["env"] = {**kwargs["env"], **EXTRA_ENV}
        return subprocess.run(*args, **kwargs)


def load(name):
    path = os.path.join(LIB, name)
    with open(path, encoding="utf-8") as handle:
        source = handle.read()
    source = source[: source.index("version = subprocess.run(")]
    source = source.replace('TARGET = f"{sys.platform} {platform.machine()}"', 'TARGET = "darwin arm64"')
    space = {"__name__": "recorder", "__file__": path}
    saved = sys.argv
    sys.argv = [path, NODE, PACKAGE, os.path.join(SCRATCH, name + ".json")]
    try:
        exec(compile(source, path, "exec"), space)
    finally:
        sys.argv = saved
    space["subprocess"] = Spawner
    return space


DEPTH = load("ts_settings_depth_goldens.py")
MANAGED = load("ts_managed_settings_goldens.py")
ORACLE = MANAGED["ORACLE"]
REF = MANAGED["REFERENCE_FLAGS"]
CROSS = MANAGED["CROSS_CHECK_FLAGS"]
# What a recording run that wants Node's own defaults starts it with; a run names it, never gets it by omission.
DEFAULT_FLAGS = []


def probe_flags(flags, override=CROSS):
    """`flags`, checked before they reach a command line: the reference flags, or the one override the probe names."""
    if flags is None or (list(flags) != REF and list(flags) != list(override)):
        raise RuntimeError(f"a probe would start Node with {flags!r}, not the reference flags or {override!r}")
    return flags
COUNTER = iter(range(10**9))
LOCK = threading.Lock()


def unique(prefix):
    with LOCK:
        return f"{prefix}{next(COUNTER)}"


def collapse(text):
    if not isinstance(text, str):
        return text
    text = re.sub(r'(?:\{"x":){2,}', '{"x":*', text)
    text = re.sub(r"\[{2,}", "[*", text)
    text = re.sub(r"\}{2,}", "}*", text)
    return re.sub(r"\]{2,}", "]*", text)


def kind(answer):
    return json.dumps({key: collapse(value) if not isinstance(value, dict) else value for key, value in answer.items()}, sort_keys=True)


def recorder_probe(space, files_of):
    def probe(level, flags):
        name = unique("c")
        try:
            spec = space["prepare"](name, files_of(level))
            return kind(space["run"](spec, probe_flags(flags)))
        except SystemExit:
            return json.dumps({"fail": True})
        finally:
            shutil.rmtree(os.path.join(space["FOLDER"], name), ignore_errors=True)

    return probe


# Registry and plist documents, read through the stand-in tools the oracle remaps under the root.
REG_SH = """#!/bin/sh
d=$(dirname "$0")
case "$2" in
HKLM*) f=hklm ;;
HKCU*) f=hkcu ;;
*) f=none ;;
esac
if [ -f "$d/$f.txt" ]; then cat "$d/$f.txt"; exit 0; fi
echo "ERROR: The system was unable to find the specified registry key or value." >&2
exit 1
"""
PLUTIL_SH = """#!/bin/sh
for last; do :; done
if [ "$1" = "-convert" ]; then exec cat "$last"; fi
exit 1
"""
HELD_STEPS = [{"at": 0, "files": {}}, {"at": 1800000, "files": {"proc/version": "Linux version 5.15.153.1-microsoft-standard-WSL2\n"}}]


def deep(shape, n):
    if shape == "o":
        return '{"x":' * (n - 1) + "null" + "}" * (n - 1)
    return "[" * n + "null" + "]" * n


def tool(folder, name, script):
    os.makedirs(folder, exist_ok=True)
    if WINDOWS:
        stem = name[:-4] if name.endswith(".exe") else name
        for copy in {name, stem + ".exe"}:
            shutil.copyfile(STUB, os.path.join(folder, copy))
        return
    location = os.path.join(folder, name)
    with open(location, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(script)
    os.chmod(location, 0o755)


def write(location, text):
    os.makedirs(os.path.dirname(location), exist_ok=True)
    with open(location, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)


def reg_text(document):
    return "\n    Settings    REG_SZ    " + document + "\n\n"


def registry_spec(place, key, shape, n, root):
    for folder in ["home/.claude", "work/.claude"]:
        os.makedirs(os.path.join(root, folder), exist_ok=True)
    write(os.path.join(root, "home/.claude/settings.json"), '{"u":[null]}')
    system32 = os.path.join(root, "mnt/c/Windows/System32")
    env = {"HOME": os.path.join(root, "home")}
    spec = {"root": root, "platform": "linux", "env": env, "deep": True}
    cases = [{"name": "case", "options": {"cwd": os.path.join(root, "work")}}]
    body = '{"wslInheritsWindowsSettings":true,"' + key + '":' + deep(shape, n) + "}"
    if place == "plist":
        spec["platform"] = "darwin"
        tool(os.path.join(root, "usr/bin"), "plutil", PLUTIL_SH)
        write(os.path.join(root, "Library/Managed Preferences/com.anthropic.claudecode.plist"),
              '{"permissions":{"deny":["Bash(rm:*)"]},"' + key + '":' + deep(shape, n) + "}")
    else:
        tool(system32, "reg.exe", REG_SH)
        if place == "hklm":
            write(os.path.join(system32, "hklm.txt"), reg_text(body))
            env["WSL_DISTRO_NAME"] = "Ubuntu"
        elif place == "hkcu":
            write(os.path.join(system32, "hklm.txt"), reg_text('{"wslInheritsWindowsSettings":true}'))
            write(os.path.join(system32, "hkcu.txt"), reg_text(body))
            env["WSL_DISTRO_NAME"] = "Ubuntu"
        else:
            write(os.path.join(root, "etc/claude-code/managed-settings.json"), '{"permissions":{"deny":["Bash(rm:*)"]}}')
            # An organization session keeps only the policy members of a held machine policy, so the depth sits
            # under one: a member of an allowed server entry, which the policy read keeps as it is.
            write(os.path.join(system32, place[5:] + ".txt"),
                  reg_text('{"allowedMcpServers":[{"serverName":"h","' + key + '":' + deep(shape, n) + "}]}"))
            env["CLAUDE_CODE_ENTRYPOINT"] = "remote_cowork"
            spec["process"] = {"euid": 0, "uid": 0, "egid": 0, "gid": 0, "groups": [0]}
            options = {**cases[0]["options"], "settingSources": []}
            cases = [{"name": f"step{i}", "options": options, **step} for i, step in enumerate(HELD_STEPS)]
    spec["cases"] = cases
    return spec


def oracle_run(spec_path, flags):
    command = [NODE, *probe_flags(flags), ORACLE, PACKAGE, spec_path]
    with tempfile.TemporaryFile() as errors:
        process = subprocess.run(command, stdout=subprocess.PIPE, stderr=errors, env={"PAD": "x" * MANAGED["pad"](command), **EXTRA_ENV})
        errors.seek(0)
        stderr = errors.read().decode("utf-8", "replace").replace("\r\n", "\n")
    if process.returncode == 0:
        return [{"name": entry["name"], **MANAGED["single"](entry["result"])} for entry in json.loads(process.stdout.decode("utf-8"))]
    found = re.search(r"^(\w*Error): (.*)$", stderr, re.M)
    if not found:
        return {"fail": stderr[-600:]}
    return {"error": {"name": found.group(1), "message": found.group(2)}}


def registry_probe(place, key, shape):
    def probe(level, flags):
        root = os.path.join(SCRATCH, unique("r"))
        try:
            spec = registry_spec(place, key, shape, level, root)
            spec_path = root + ".json"
            with open(spec_path, "w", encoding="utf-8") as handle:
                json.dump(spec, handle)
            answered = oracle_run(spec_path, flags)
            if isinstance(answered, list):
                return json.dumps([{k: collapse(v) for k, v in entry.items()} for entry in answered], sort_keys=True)
            return json.dumps(answered, sort_keys=True)
        finally:
            shutil.rmtree(root, ignore_errors=True)
            if os.path.exists(root + ".json"):
                os.remove(root + ".json")

    return probe


GRID = sorted(set(range(100, 900, 64)) | set(range(900, 1800, 8)) | set(range(1800, 4300, 16)))
SMALL = list(range(1, 400, 4))


def families():
    found = {}
    for label, space in (("depth", DEPTH), ("managed", MANAGED)):
        for keys, make in space["GROUPS"]:
            if " " in keys:
                continue
            files_of = (lambda make: lambda n: make(n)[0][1])(make)
            signature = json.dumps(files_of(500))
            name = f"{label}:{signature}"
            entry = found.setdefault(name, {"keys": [], "probe": recorder_probe(space, files_of), "grid": GRID, "files": files_of(500)})
            entry["keys"].append(f"{label}.{keys}")
    for place in ("hklm", "hkcu", "held_hklm", "held_hkcu", "plist"):
        for key in ("x", "model"):
            for shape in ("o", "a"):
                found[f"registry:{place}:{key}:{shape}"] = {"keys": [f"registry.{place}.{key}.{shape}"], "probe": registry_probe(place, key, shape), "grid": GRID}
    return found


def scan(pool, name, entry):
    probe = entry["probe"]
    cache = {}

    def at(level):
        if level not in cache:
            cache[level] = probe(level, REF)
        return cache[level]

    grid = entry["grid"]
    for level, answered in zip(grid, pool.map(at, grid)):
        cache[level] = answered
    changes = []
    for low, high in zip(grid, grid[1:]):
        if at(low) == at(high):
            continue
        levels = list(range(low + 1, high))
        for level, answered in zip(levels, pool.map(at, levels)):
            cache[level] = answered
        for level in range(low, high):
            if at(level) != at(level + 1):
                changes.append(level)
    transitions = []
    for level in changes:
        near, far = at(level), at(level + 1)
        checks = list(pool.map(lambda job: probe(job[0], job[1]), [(level, REF), (level + 1, REF), (level, REF), (level + 1, REF), (level, CROSS), (level + 1, CROSS)]))
        transitions.append({
            "level": level,
            "near": near,
            "far": far,
            "repeatable": checks[0] == near and checks[1] == far and checks[2] == near and checks[3] == far,
            "cross_check": checks[4] == near and checks[5] == far,
        })
    return {"keys": entry["keys"], "files": entry.get("files"), "first": at(grid[0]), "transitions": transitions}


def main():
    version = subprocess.run([NODE, "--version"], capture_output=True, check=True).stdout.decode().strip()
    chosen = {name: entry for name, entry in families().items() if not PATTERN or PATTERN.search(name)}
    result = {"host": HOST, "node": version, "flags": REF, "cross_check_flags": CROSS, "extra_env": sorted(EXTRA_ENV), "families": {}}
    workers = max(2, os.cpu_count() or 2)
    with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as pool:
        for name, entry in chosen.items():
            result["families"][name] = scan(pool, name, entry)
            done = result["families"][name]
            sys.stderr.write(f"{name} {entry['keys']}: {[(t['level'], t['repeatable'], t['cross_check']) for t in done['transitions']]}\n")
            sys.stderr.flush()
            with open(OUT, "w", encoding="utf-8") as handle:
                json.dump(result, handle, indent=1)
    shutil.rmtree(SCRATCH, ignore_errors=True)


main()
