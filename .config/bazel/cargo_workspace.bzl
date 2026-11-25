"""Reads the Cargo workspace so Bazel derives every Rust target from Cargo.

The repository rule runs `cargo metadata --no-deps` against the workspace
manifest and writes `crates.bzl`, whose `CRATES` dict maps each member's Bazel
package to what `workspace_crate()` needs: its targets, enabled features, and
workspace dependencies. Members a wasm component reaches also carry the
features and dependencies of their wasm32 build, under `wasm`. Bazel reruns it whenever a member manifest, the
lockfile, or a directory Cargo discovers targets from changes.
"""

_LIBRARY_KINDS = ["lib", "rlib", "dylib", "cdylib", "staticlib"]

# Directories Cargo scans for targets it discovers without a manifest entry.
_TARGET_DIRS = ["src", "src/bin", "tests", "examples", "benches"]

def _as_set(items):
    return {item: True for item in items}

def _dep_key(dep):
    return dep.get("rename") or dep["name"]

def _feature_closure(package, requested):
    """Resolves features the way Cargo does for one package.

    Args:
        package: A `cargo metadata` package.
        requested: Features asked for directly.

    Returns:
        A struct with the enabled features, the activated optional
        dependencies by dependency key, and the `dep/feature` requests as
        (dependency key, feature, weak) tuples.
    """
    features = package["features"]
    optional = _as_set([_dep_key(dep) for dep in package["dependencies"] if dep["optional"]])
    enabled = {}
    activated = {}
    requests = []
    pending = list(requested)

    # Starlark has no while loop; each feature is visited at most once.
    for _ in range(len(features) + len(optional) + len(requested) + 1):
        if not pending:
            break
        next_pending = []
        for feature in pending:
            if feature in enabled:
                continue
            enabled[feature] = True
            if feature in optional and feature not in features:
                activated[feature] = True
            for value in features.get(feature, []):
                if value.startswith("dep:"):
                    activated[value.removeprefix("dep:")] = True
                elif "/" in value:
                    dep, _, dep_feature = value.partition("/")
                    weak = dep.endswith("?")
                    dep = dep.removesuffix("?")
                    if not weak and dep in optional:
                        activated[dep] = True
                    requests.append((dep, dep_feature, weak))
                else:
                    next_pending.append(value)
        pending = next_pending
    if pending:
        fail("feature resolution of {} did not converge".format(package["name"]))
    return struct(enabled = enabled, activated = activated, requests = requests)

def _unified_features(packages, by_name):
    """The features each member gets when Cargo resolves the whole workspace.

    crate_universe resolves the workspace once with every member's default
    features, so an optional dependency reaches its `all_crate_deps` only when
    that resolution activates it. Dev-dependency edges count, as they do for
    Cargo's resolver.
    """
    unified = {
        package["name"]: _as_set(["default"] if "default" in package["features"] else [])
        for package in packages
    }
    for _ in range(len(packages) * 64):
        changed = False
        for package in packages:
            closure = _feature_closure(package, unified[package["name"]].keys())
            for dep in package["dependencies"]:
                if not dep.get("path"):
                    continue
                key = _dep_key(dep)
                if dep["optional"] and key not in closure.activated:
                    continue
                target = by_name[dep["name"]]
                wanted = list(dep["features"])
                if dep["uses_default_features"] and "default" in target["features"]:
                    wanted.append("default")
                for request_key, feature, weak in closure.requests:
                    if request_key == key and (not weak or not dep["optional"] or key in closure.activated):
                        wanted.append(feature)
                for feature in wanted:
                    if feature not in unified[target["name"]]:
                        unified[target["name"]][feature] = True
                        changed = True
        if not changed:
            return unified
    fail("workspace feature unification did not converge")

def _is_proc_macro(package):
    return any([target["kind"][0] == "proc-macro" for target in package["targets"]])

def _is_component(package):
    cdylib = any(["cdylib" in target["crate_types"] for target in package["targets"]])
    return cdylib and "component" in (package.get("metadata") or {})

def _wasm_features(packages, by_name, by_dir):
    """The features each crate gets when Cargo builds the wasm components.

    A component is a member whose library is a cdylib and whose manifest has
    `[package.metadata.component]`. Cargo builds it for wasm32 on its own, so only the normal dependencies it
    reaches count, with the features they ask for. Proc macros run on the host
    and keep their workspace build.
    """
    wanted = {}
    for package in packages:
        if _is_component(package):
            wanted[package["name"]] = _as_set(["default"] if "default" in package["features"] else [])
    for _ in range(len(packages) * 64):
        changed = False
        for name in list(wanted.keys()):
            package = by_name[name]
            closure = _feature_closure(package, wanted[name].keys())
            for dep in package["dependencies"]:
                key = _dep_key(dep)
                if dep["kind"] or not dep.get("path") or (dep["optional"] and key not in closure.activated):
                    continue
                if dep.get("target"):
                    fail("{} has a platform-specific path dependency, which the wasm build does not support yet".format(name))
                target = by_dir[dep["path"]]
                if _is_proc_macro(target):
                    continue
                features = list(dep["features"])
                if dep["uses_default_features"] and "default" in target["features"]:
                    features.append("default")
                for request_key, feature, weak in closure.requests:
                    if request_key == key and (not weak or not dep["optional"] or key in closure.activated):
                        features.append(feature)
                if target["name"] not in wanted:
                    wanted[target["name"]] = {}
                    changed = True
                for feature in features:
                    if feature not in wanted[target["name"]]:
                        wanted[target["name"]][feature] = True
                        changed = True
        if not changed:
            return wanted
    fail("wasm feature resolution did not converge")

def _wasm_crate(package, features, workspace_package, workspace_root, by_dir):
    """What the wasm build of a crate needs: its features and dependencies.

    Workspace dependencies point at their own wasm build, external ones at the
    `@crates_wasm` resolution, and proc macros at their workspace build.
    """
    closure = _feature_closure(package, features.keys())
    deps = []
    proc_macro_deps = []
    aliases = {}
    for dep in package["dependencies"]:
        key = _dep_key(dep)
        if dep["kind"] or (dep["optional"] and key not in closure.activated):
            continue
        if dep.get("target"):
            fail("{} has a platform-specific dependency, which the wasm build does not support yet".format(package["name"]))
        if dep.get("path"):
            member = by_dir[dep["path"]]
            label = "//{}/{}:{}".format(workspace_package, _relative(_dirname(member["manifest_path"]), workspace_root), member["name"])
            if _is_proc_macro(member):
                proc_macro_deps.append(label)
                continue
            label = label + "_wasm"
        else:
            label = "@crates_wasm//:" + dep["name"]
        if dep.get("rename"):
            aliases[label] = dep["rename"].replace("-", "_")
        deps.append(label)
    return {
        "aliases": aliases,
        "crate_features": sorted(closure.enabled.keys()),
        "deps": deps,
        "proc_macro_deps": proc_macro_deps,
    }

def _relative(path, root):
    if path == root:
        return ""
    if not path.startswith(root + "/"):
        fail("{} is outside {}".format(path, root))
    return path.removeprefix(root + "/")

def _dirname(path):
    return path.rpartition("/")[0]

def _crate(package, workspace_package, workspace_root, by_dir, unified, wasm):
    manifest_dir = _dirname(package["manifest_path"])
    bazel_package = workspace_package + "/" + _relative(manifest_dir, workspace_root)

    library = None
    binaries = []
    integration_tests = []
    examples = []
    for target in package["targets"]:
        kind = target["kind"][0]
        if kind == "custom-build":
            fail("{} has a build script, which workspace_crate does not support yet".format(package["name"]))
        if kind == "proc-macro" or kind in _LIBRARY_KINDS:
            library = {
                "crate_name": target["name"].replace("-", "_"),
                "doctest": target["doctest"],
                "proc_macro": kind == "proc-macro",
                "root": _relative(target["src_path"], manifest_dir),
                "test": target["test"],
            }
        elif kind == "bin":
            binaries.append({"name": target["name"], "root": _relative(target["src_path"], manifest_dir), "test": target["test"]})
        elif kind == "test":
            if target["test"]:
                integration_tests.append({"name": target["name"], "root": _relative(target["src_path"], manifest_dir)})
        elif kind == "example":
            examples.append({"name": target["name"], "root": _relative(target["src_path"], manifest_dir)})

    # Bazel builds every feature, as `cargo test --all-features` does.
    closure = _feature_closure(package, package["features"].keys())
    resolved = _feature_closure(package, unified[package["name"]].keys())

    deps = {"build": [], "dev": [], "dev_proc_macro": [], "normal": [], "proc_macro": []}
    aliases = {}
    seen = {}
    for dep in package["dependencies"]:
        key = _dep_key(dep)
        if dep["optional"] and key not in closure.activated:
            continue
        kind = dep["kind"] or "normal"
        if dep.get("path"):
            member = by_dir.get(dep["path"])
            if member == None:
                fail("{} depends on {}, which is not a workspace member".format(package["name"], dep["path"]))
            if member["name"] == package["name"]:
                # A dev-dependency on itself only turns features on, and every
                # feature is on already.
                continue
            member_dir = _dirname(member["manifest_path"])
            label = "//{}/{}:{}".format(workspace_package, _relative(member_dir, workspace_root), member["name"])
            proc_macro = any([t["kind"][0] == "proc-macro" for t in member["targets"]])
        elif dep["optional"] and key not in resolved.activated:
            # crate_universe's resolution leaves this optional dependency out
            # of all_crate_deps, so it is listed explicitly.
            label = "@crates//:" + key
            proc_macro = False
        else:
            continue
        if dep.get("rename"):
            aliases[label] = dep["rename"].replace("-", "_")
        bucket = {
            "build": "build",
            "dev": "dev_proc_macro" if proc_macro else "dev",
            "normal": "proc_macro" if proc_macro else "normal",
        }[kind]
        if (label, kind == "dev") in seen:
            continue
        seen[(label, kind == "dev")] = True
        deps[bucket].append(label)

    # A crate listed as both a dependency and a dev-dependency is linked once.
    deps["dev"] = [label for label in deps["dev"] if label not in deps["normal"]]
    deps["dev_proc_macro"] = [label for label in deps["dev_proc_macro"] if label not in deps["proc_macro"]]

    return bazel_package, {
        "aliases": aliases,
        "binaries": binaries,
        "crate_features": sorted(closure.enabled.keys()),
        "deps": deps["normal"],
        "dev_deps": deps["dev"],
        "dev_proc_macro_deps": deps["dev_proc_macro"],
        "edition": package["edition"],
        "examples": examples,
        "integration_tests": integration_tests,
        "library": library,
        "component": _is_component(package),
        "metadata": package.get("metadata") or {},
        "name": package["name"],
        "proc_macro_deps": deps["proc_macro"],
        "version": package["version"],
        "wasm": _wasm_crate(package, wasm[package["name"]], workspace_package, workspace_root, by_dir) if package["name"] in wasm else None,
    }

def _watch_dir(path):
    if path.exists and path.is_dir:
        return path.readdir(watch = "yes")
    return []

def _cargo_workspace_impl(rctx):
    manifest = rctx.path(rctx.attr.manifest)
    rctx.watch(manifest)
    rctx.watch(rctx.attr.lockfile)
    toolchain_bin = rctx.path(rctx.attr.rust_root).dirname.get_child("bin")
    result = rctx.execute(
        [
            toolchain_bin.get_child("cargo"),
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
            "--manifest-path",
            manifest,
        ],
        environment = {
            "PATH": "{}:{}".format(toolchain_bin, rctx.getenv("PATH", "")),
            "RUSTC": str(toolchain_bin.get_child("rustc")),
        },
        quiet = True,
    )
    if result.return_code != 0:
        fail("cargo metadata failed:\n" + result.stderr)
    metadata = json.decode(result.stdout)

    workspace_root = metadata["workspace_root"]
    workspace_package = rctx.attr.manifest.package
    packages = metadata["packages"]
    by_name = {package["name"]: package for package in packages}
    by_dir = {_dirname(package["manifest_path"]): package for package in packages}
    unified = _unified_features(packages, by_name)
    wasm = _wasm_features(packages, by_name, by_dir)

    crates = {}
    watched_dirs = {}
    for package in packages:
        rctx.watch(package["manifest_path"])
        manifest_dir = _dirname(package["manifest_path"])

        # Member globs pick up new crate directories, so their parents are watched.
        parent = manifest_dir
        for _ in range(manifest_dir.count("/")):
            if parent == workspace_root or not parent.startswith(workspace_root):
                break
            watched_dirs[parent] = True
            parent = _dirname(parent)
        for target_dir in _TARGET_DIRS:
            watched_dirs[manifest_dir + "/" + target_dir] = True

        bazel_package, crate = _crate(package, workspace_package, workspace_root, by_dir, unified, wasm)
        crates[bazel_package] = crate

    for directory in sorted(watched_dirs.keys()):
        for entry in _watch_dir(rctx.path(directory)):
            # Cargo also discovers `tests/<name>/main.rs` and its peers.
            if entry.is_dir:
                _watch_dir(entry)

    rctx.file("BUILD.bazel", "")
    rctx.file("crates.bzl", "\n".join([
        '"""Generated from `cargo metadata`; Bazel rewrites this file when the Cargo workspace changes."""',
        "",
        "CRATES = " + repr(crates),
        "",
    ]))

cargo_workspace_repository = repository_rule(
    implementation = _cargo_workspace_impl,
    attrs = {
        "lockfile": attr.label(mandatory = True, allow_single_file = True),
        "manifest": attr.label(mandatory = True, allow_single_file = True),
        "rust_root": attr.label(mandatory = True, allow_single_file = True),
    },
    doc = "Exposes the Cargo workspace members as `CRATES` in `crates.bzl`.",
)

_workspace = tag_class(attrs = {
    "lockfile": attr.label(mandatory = True, doc = "The workspace Cargo.lock."),
    "manifest": attr.label(mandatory = True, doc = "The workspace root Cargo.toml."),
    "name": attr.string(mandatory = True, doc = "Name of the generated repository."),
    "rust_root": attr.label(mandatory = True, doc = "A file at the root of the Rust toolchain whose bin/ holds cargo."),
})

def _cargo_workspace_extension_impl(module_ctx):
    for module in module_ctx.modules:
        for workspace in module.tags.workspace:
            cargo_workspace_repository(
                name = workspace.name,
                lockfile = workspace.lockfile,
                manifest = workspace.manifest,
                rust_root = workspace.rust_root,
            )

cargo_workspace = module_extension(
    implementation = _cargo_workspace_extension_impl,
    tag_classes = {"workspace": _workspace},
)
