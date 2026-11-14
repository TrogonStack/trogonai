#!/usr/bin/env python3
"""Records how the pinned TypeScript `resolveSettings` merges array-like objects into arrays and objects into
arrays, so the arrays of the TypeScript dialect of the settings engine are checked against Node and the pinned
package instead of against a reading of lodash.

usage: python3 -I ts_settings_merge_goldens.py NODE PACKAGE_DIR OUT_FILE [--check]

NODE runs `resolve_settings_oracle.mjs` against PACKAGE_DIR, the unpacked pinned `@anthropic-ai/claude-agent-sdk`,
on linux.

lodash `mergeWith` merges an array into an object it holds under the same key by copying the object with
`copyArray` when the object is array-like, its `length` a number that is a whole number from 0 to 2^53 - 1: an array
of that length (`Array(length)` throws past 2^32 - 1) holding the index members below it and holes elsewhere. An
object merges into an array it holds as into any object: each index member sets an element, past the end leaving
holes, `length` converts its value to a number and truncates or extends the array (a value that does not convert to
a whole number from 0 to 2^32 - 1 throws), and every other member sets a named property no later merge prints. A
hole or `undefined` element prints as `null`, an array merged from one walks every index below its length, and the
settings customizer joins two arrays as a `Set` does, so holes and `undefined` read as one `undefined`. A drop-in
that throws gives up on the rest of the managed folder, as an overflow does. A source holding so many holes that
printing it would build a string longer than V8 holds rejects `resolveSettings` with
`RangeError: Invalid string length`. A fresh array or object merged from a source drops each array or object under
`__proto__`, while the source keeps it.

Each `cases` entry names a resolve and its files as `[place, text]`. `result` is the whole object `resolveSettings`
returns as `JSON.stringify` prints it, with the sandbox root taken off every path so the home and project folders
read as `/home/fixture` and `/work/project`, or `null` when it rejects; `error` is the error it rejects with, or
`null`.
"""

import json
import os
import re
import subprocess
import sys
import tempfile

args = sys.argv[1:]
if len(args) < 3 or any(arg.startswith("-") for arg in args[:3]):
    sys.stderr.write("usage: ts_settings_merge_goldens.py NODE PACKAGE_DIR OUT_FILE [--check]\n")
    sys.exit(2)
node, package, out, flags = args[0], args[1], os.path.abspath(args[2]), args[3:]
ORACLE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "resolve_settings_oracle.mjs")
FOLDER = tempfile.mkdtemp()

PLACES = {
    "userSettings": "home/fixture/.claude/settings.json",
    "projectSettings": "work/project/.claude/settings.json",
    "localSettings": "work/project/.claude/settings.local.json",
    "managed": "etc/claude-code/managed-settings.json",
    "drop_a": "etc/claude-code/managed-settings.d/a.json",
    "drop_b": "etc/claude-code/managed-settings.d/b.json",
}
U, P, L, M, A, B = PLACES


def j(value):
    return json.dumps(value, separators=(",", ":"))


def onto(first, second, then=None):
    """A user file holding `first` under `x`, a project file holding `second` and a local file holding `then`."""
    files = [[U, j({"x": first})], [P, j({"x": second})]]
    return files + ([[L, j({"x": then})]] if then is not None else [])


def policy(first, second, *rest):
    """A managed file holding `first` under `x`, drop-in a holding `second` and further files after them."""
    return [[M, j({"x": first})], [A, j({"x": second})], *rest]


CASES = [
    ("al_basic", onto({"length": 2, "0": "a", "foo": 1}, ["b"])),
    ("al_zero", onto({"length": 0, "0": "a"}, ["b"])),
    ("al_string_len", onto({"length": "2", "0": "a"}, ["b"])),
    ("al_frac", onto({"length": 1.5, "0": "a"}, ["b"])),
    ("al_neg", onto({"length": -1, "0": "a"}, ["b"])),
    ("al_negzero", [[U, '{"x":{"length":-0,"0":"a"}}'], [P, '{"x":["b"]}']]),
    ("al_long", onto({"length": 3, "0": "a", "2": {"k": 1}}, ["b"])),
    ("al_nested_merge", onto({"length": 2, "0": {"k": 1}, "1": [1]}, [{"j": 2}, [2]])),
    ("al_then_null", onto({"length": 2}, [], [None])),
    ("al_safe_max", onto({"length": 9007199254740991}, [])),
    ("al_2_32", onto({"length": 4294967296}, [])),
    ("al_big_unsafe", onto({"length": 9007199254740992}, [])),
    ("al_after_arr_obj", onto({"length": 4, "1": [1], "3": {"a": 1}, "z": 9}, [[2], [3], None, {"b": 2}, 7],
                              {"2": {"c": 1}, "length": "6"})),
    ("arr_obj", onto([1, 2, 3], {"0": "a", "b": 1, "5": 7, "01": 9})),
    ("arr_obj_len", onto([1, 2, 3], {"length": 1})),
    ("arr_obj_len5", onto([1, 2, 3], {"length": 5})),
    ("arr_obj_lenbad", onto([1, 2, 3], {"length": "a"})),
    ("arr_obj_nested", onto([{"k": 1}, [1]], {"0": {"j": 2}, "1": {"0": 5}})),
    ("arr_obj_then", onto([1], {"a": 1}, [2])),
    ("len_true", onto([1, 2, 3], {"length": True})),
    ("len_null", onto([1, 2, 3], {"length": None})),
    ("len_str", onto([1, 2, 3], {"length": " 0x2 "})),
    ("len_arr", onto([1, 2, 3], {"length": ["1"]})),
    ("len_emptyarr", onto([1, 2, 3], {"length": []})),
    ("len_obj", onto([1, 2, 3], {"length": {}})),
    ("len_then_idx", onto([1, 2, 3], {"5": 1, "length": 2})),
    ("len_huge", onto([1], {"length": 4294967295}, {"length": 2})),
    ("huge_idx", onto([1], {"4294967294": 1})),
    ("huge_nonidx", onto([1], {"4294967295": 1})),
    ("huge_then_scalar", onto([1], {"4294967294": 1}, 5)),
    ("huge_then_len", onto([1], {"4294967294": 1}, {"length": 1})),
    ("trunc_after_idx", onto([1, 2, 3], {"length": 1, "2": 9})),
    ("trunc_zero", onto([1, 2, 3], {"length": 0})),
    ("idx_into_held_obj", onto([{"a": 1}, [1]], {"0": [5], "1": {"k": 2}})),
    ("proto_on_array", onto([1], {"__proto__": 5, "0": 2})),
    ("proto_scalar_on_arr", onto([1], {"__proto__": 1})),
    ("picker_prop", onto([1], {"modelPicker": {"a": 1}, "fallbackModel": [1]}, {"fallbackModel": [2]})),
    ("proto_fresh", [[U, '{"x":{"p":{"__proto__":{"z":1},"k":1}}}']]),
    ("proto_fresh_arr", [[U, '{"x":[{"__proto__":[1],"k":1}]}']]),
    ("alias_fb", onto({"fallbackModel": ["a"]}, {"fallbackModel": ["b"]}, {"fallbackModel": {"0": "z"}})),
    ("alias_union", onto([{"a": 1}], [{"b": 1}], {"0": {"c": 1}, "1": {"d": 1}})),
    ("alias_spread", onto({"managedMcpServers": {"s": {"a": 1}}}, {"managedMcpServers": {"t": {"b": 1}}},
                          {"managedMcpServers": {"t": {"c": 1}}})),
    ("alias_union_arr", onto([[1]], [[2]], {"1": {"0": 9}})),
    ("alias_policy", policy([{"a": 1}], [{"b": 1}], [B, j({"x": {"1": {"c": 1}}})])),
    ("m_allow_al", [[M, j({"permissions": {"allow": {"length": 1, "0": "Bash"}}})],
                    [A, j({"permissions": {"allow": ["Read"]}})]]),
    ("m_allow_al_undef", [[M, j({"permissions": {"allow": {"length": 2}}})],
                          [A, j({"permissions": {"allow": ["Read"]}})]]),
    ("m_allow_al_undef2", [[M, j({"permissions": {"allow": {"length": 3, "2": "Edit"}}})],
                           [A, j({"permissions": {"allow": ["Read"]}})]]),
    ("m_allow_arr_obj", [[M, j({"permissions": {"allow": ["Bash"]}})], [A, j({"permissions": {"allow": {"2": "Read"}}})]]),
    ("m_allow_null", [[M, j({"permissions": {"allow": ["Bash", None, "Read"]}})]]),
    ("u_allow_null", [[U, j({"permissions": {"allow": ["Bash", None, "Read"]}})]]),
    ("m_env_arr", [[M, j({"env": ["a", "b"]})], [A, j({"env": {"K": "v", "1": "z"}})]]),
    ("m_x_props", policy([1], {"b": {"q": [1]}})),
    ("m_x_undef", policy({"length": 2}, [1], [B, j({"x": [None]})])),
    ("m_bad_len", [[M, j({"x": {"length": 9007199254740991}, "y": 1})], [A, j({"x": []})], [B, j({"z": 2})],
                   [U, j({"u": 1})]]),
    ("m_bad_len2", [[M, j({"x": [1, 2]})], [A, j({"y": 3, "x": {"length": {}}})], [B, j({"z": 2})], [U, j({"u": 1})]]),
    ("props_src", policy([1], {"b": {"q": [1]}}, [U, j({"x": {"b": {"q": [[2]]}}})])),
    ("src_arr_marker", [[M, j({"x": {"k": [1]}})], [A, j({"x": {"k": {"b": {"q": [1]}, "1": 5}}})],
                        [U, j({"x": {"k": {"length": 3, "0": "m"}}})]]),
    ("src_undef_into_fresh", policy({"length": 2}, [], [U, j({"x": {"q": 1}})])),
    ("run_source", policy([1], {"3": 2}, [U, j({"x": ["a", "b", "c", "d", "e"]})])),
    ("run_onto_arraylike", policy([1], {"3": 2}, [U, j({"x": {"length": 5, "1": "u"}})])),
    ("run_fresh", policy([1], {"3": 2})),
    ("union_holder", policy([1], {"p": 1}, [U, j({"x": [0]})])),
    ("union_runs", policy([1], {"2": 1, "5": 3}, [U, j({"x": [None]})])),
    ("holder_props", policy([1], {"p": {"q": 1}}, [B, j({"x": {"p": {"r": 2}, "length": "1"}})])),
]


def exact(value):
    """`JSON.stringify` of `value`, which escapes each lone surrogate and leaves every other code point as is."""
    text = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    return re.sub("[\ud800-\udfff]", lambda found: f"\\u{ord(found.group()):04x}", text)


def record(name, files):
    root = os.path.join(FOLDER, name)
    os.makedirs(os.path.join(root, "home/fixture/.claude"))
    os.makedirs(os.path.join(root, "work/project/.claude"))
    for place, text in files:
        location = os.path.join(root, PLACES[place])
        os.makedirs(os.path.dirname(location), exist_ok=True)
        with open(location, "w", encoding="utf-8") as handle:
            handle.write(text)
    spec = {
        "root": root,
        "platform": "linux",
        "env": {"HOME": os.path.join(root, "home/fixture")},
        "cases": [{"name": name, "options": {"cwd": os.path.join(root, "work/project")}}],
    }
    spec_path = os.path.join(FOLDER, f"{name}.spec.json")
    with open(spec_path, "w", encoding="utf-8") as handle:
        json.dump(spec, handle)
    run = subprocess.run([node, ORACLE, package, spec_path], capture_output=True)
    entry = {"name": name, "files": files}
    if run.returncode == 0:
        result = json.loads(run.stdout.decode("utf-8"))[0]["result"]
        entry["result"] = exact(result).replace(json.dumps(root)[1:-1], "")
        entry["error"] = None
    else:
        found = re.search(r"^(\w*Error): (.*)$", run.stderr.decode("utf-8"), re.M)
        if not found:
            sys.stderr.write(run.stderr.decode("utf-8"))
            sys.exit(1)
        entry["result"] = None
        entry["error"] = {"name": found.group(1), "message": found.group(2)}
    return entry


version = subprocess.run([node, "--version"], capture_output=True, check=True, env={}).stdout.decode().strip()
entries = [record(name, files) for name, files in CASES]
body = (
    "{\n"
    + f'  "node": {json.dumps(version)},\n'
    + '  "cases": [\n'
    + ",\n".join(f"    {json.dumps(entry)}" for entry in entries)
    + "\n  ]\n}\n"
)

if "--check" in flags:
    with open(out, encoding="utf-8") as handle:
        if handle.read() != body:
            sys.stderr.write(f"{out} is stale: run ts_settings_merge_goldens.py without --check\n")
            sys.exit(1)
else:
    with open(out, "w", encoding="utf-8") as handle:
        handle.write(body)
