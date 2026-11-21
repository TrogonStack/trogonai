"""Bazel targets for a Cargo workspace crate, derived from its Cargo.toml.

`workspace_crate()` looks its package up in `CRATES`, which the cargo_workspace
module extension reads from `cargo metadata`, so a crate's BUILD.bazel holds no
dependency, feature, or target lists. Bazel builds each crate with all of its
features, as `cargo test --all-features` does, plus any feature sets the crate lists
in `feature_variants`.

What Cargo cannot express, such as test tags, skipped tests, and extra files,
is passed to `workspace_crate()` in the crate's BUILD.bazel, so Cargo.toml stays
Cargo-only.

A crate that a wasm component reaches through its normal dependencies also gets
a `<name>_wasm` library, built with the features and dependencies Cargo would
use for the component's wasm32 build. A component's own `<name>_wasm` is its
core module; `decider.bzl` turns it into the component.
"""

load("@cargo_workspace//:crates.bzl", "CRATES")
load("@crates//:defs.bzl", "all_crate_deps", crate_aliases = "aliases")
load("@rules_python//python:py_test.bzl", "py_test")
load("@rules_rust//cargo:defs.bzl", "extract_cargo_lints")
load("@rules_rust//rust:defs.bzl", "rust_binary", "rust_doc_test", "rust_library", "rust_proc_macro", "rust_shared_library", "rust_test")
load("//.config/bazel:compiler_failures.bzl", "compiler_failure")
load("//.config/bazel:decider.bzl", "decider_component")

_RUSTC_FLAGS = ["-Dwarnings"]

_TARGET_KEYS = {
    "binaries": ["compile_data", "rustc_env", "tags", "test_args"],
    "examples": ["compile_data", "rustc_env", "tags"],
    "integration_tests": ["args", "compile_data", "data", "rustc_env", "tags"],
}

def workspace_compile_fail_library(crate_features, dependency_overrides = {}):
    """Isolates a fixture's feature set without reconfiguring its dependency graph.

    Args:
        crate_features: The library features used by the diagnostic goldens.
        dependency_overrides: Workspace dependencies requiring fixture libraries.
    """
    crate = CRATES[native.package_name()]
    library = crate["library"]
    rust_library(
        name = crate["name"] + "_compile_fail",
        srcs = native.glob(["src/**/*.rs"]),
        aliases = crate_aliases(normal = True, proc_macro = True) | crate["aliases"],
        crate_features = crate_features,
        crate_name = library["crate_name"],
        crate_root = library["root"],
        edition = crate["edition"],
        proc_macro_deps = all_crate_deps(proc_macro = True) + crate["proc_macro_deps"],
        rustc_env = {"CARGO_PKG_NAME": crate["name"]},
        lint_config = ":" + crate["name"] + "_lints",
        rustc_flags = _RUSTC_FLAGS,
        version = crate["version"],
        visibility = ["//visibility:public"],
        deps = all_crate_deps(normal = True) + [dependency_overrides.get(dep, dep) for dep in crate["deps"]],
    )

def workspace_compile_fail_tests(name, fixtures, compile_data = [], dependency_overrides = {}):
    """Checks every fixture against its existing trybuild diagnostic golden.

    Args:
        name: The Bazel test name.
        fixtures: Rust fixture sources, each with a sibling `.stderr` file.
        compile_data: Files read by procedural macros while compiling fixtures.
        dependency_overrides: Workspace dependencies requiring fixture libraries.
    """
    crate = CRATES[native.package_name()]
    deps = all_crate_deps(normal = True, normal_dev = True) + [dependency_overrides.get(dep, dep) for dep in crate["deps"] + crate["dev_deps"]]
    proc_macro_deps = all_crate_deps(proc_macro = True, proc_macro_dev = True) + crate["proc_macro_deps"] + crate["dev_proc_macro_deps"]
    aliases = crate_aliases(normal = True, normal_dev = True, proc_macro = True, proc_macro_dev = True) | crate["aliases"]
    args = []
    data = []
    for fixture in fixtures:
        stem = fixture.rpartition("/")[2].removesuffix(".rs")
        result = name + "_" + stem
        golden = fixture.removesuffix(".rs") + ".stderr"
        compiler_failure(
            name = result,
            src = fixture,
            crate_name = stem,
            edition = crate["edition"],
            deps = deps,
            proc_macro_deps = proc_macro_deps,
            aliases = aliases,
            compile_data = compile_data,
            testonly = True,
        )
        args.extend([
            "--fixture=$(rootpath {})".format(fixture),
            "--case",
            "$(rootpath {})".format(fixture),
            "$(rootpath {})".format(golden),
            "$(rootpath :{})".format(result),
        ])
        data.extend([fixture, golden, ":" + result])
    py_test(
        name = name,
        srcs = ["//.config/bazel:compiler_failures.py"],
        main = "//.config/bazel:compiler_failures.py",
        args = args,
        data = data,
    )
    py_test(
        name = name + "_contract_test",
        srcs = ["//.config/bazel:compiler_failures_test.py"],
        main = "//.config/bazel:compiler_failures_test.py",
        args = args,
        data = data,
        deps = ["//.config/bazel:compiler_failures_lib"],
    )

def _files(entries):
    """Resolves `compile_data` and `data` entries to globs and labels."""
    labels = [entry for entry in entries if entry.startswith(("//", ":", "@"))]
    patterns = [entry for entry in entries if entry not in labels]
    return labels + (native.glob(patterns) if patterns else [])

def _env(env):
    return {key: value.replace("{package}", native.package_name()) for key, value in env.items()}

def _target_settings(kind, entries, targets):
    """Each target's settings, with the `*` entry merged under it."""
    entries = dict(entries)
    names = [target["name"] for target in targets]
    for name, entry in entries.items():
        if name != "*" and name not in names:
            fail("workspace_crate({}) names {}, which Cargo does not declare".format(kind, name))
        unknown = [key for key in entry if key not in _TARGET_KEYS[kind]]
        if unknown:
            fail("workspace_crate({}) entry {} has unsupported keys {}; supported: {}".format(kind, name, unknown, _TARGET_KEYS[kind]))
    shared = entries.pop("*", {})
    settings = {}
    for name in names:
        entry = entries.get(name, {})
        settings[name] = {
            key: shared.get(key, {}) | entry.get(key, {}) if key == "rustc_env" else shared.get(key, []) + entry.get(key, [])
            for key in _TARGET_KEYS[kind]
        }
    return settings

def _target_srcs(root):
    """A target's root file plus the module directory beside it."""
    if root.endswith("/main.rs"):
        return native.glob([root.rpartition("/")[0] + "/**/*.rs"])
    support = native.glob(["tests/support/**/*.rs"], allow_empty = True) if root.startswith("tests/") else []
    return _dedupe([root] + native.glob([root.removesuffix(".rs") + "/**/*.rs"], allow_empty = True) + support)

def _dedupe(items):
    return {item: True for item in items}.keys()

# The package path names the crate, so the macro takes no name.
# buildifier: disable=unnamed-macro
def workspace_crate(
        compile_data = [],
        data = [],
        env = {},
        rustc_env = {},
        library_tags = [],
        unit_test_args = [],
        unit_test_tags = [],
        binaries = {},
        integration_tests = {},
        examples = {},
        feature_variants = {}):
    """Declares the library, binaries, tests, doctests, and examples Cargo builds for this package.

    Dependencies, features, and targets come from Cargo. The arguments carry only
    what Bazel needs on top of that.

    Args:
        compile_data: Files read at compile time by every target.
        data: Files read at run time by every test.
        env: Environment for running every test. Values may use `$(rootpath ...)`
            on a `data` label.
        rustc_env: Environment for compiling every target.
        library_tags: Tags for the library target.
        unit_test_args: Arguments for the library's unit test binary.
        unit_test_tags: Tags for every unit test target, library and binaries.
        binaries: Maps a binary name to `compile_data`, `rustc_env`, `tags`, and
            `test_args` for its unit test.
        integration_tests: Maps a test name to `args`, `compile_data`, `data`,
            `rustc_env`, and `tags`.
        examples: Maps an example name to `compile_data`, `rustc_env`, and `tags`.
        feature_variants: Maps a variant name to another set of Cargo features.
            Each variant declares every target again with those features, named
            with a `_<variant>` suffix, for feature sets Cargo checks separately.

    Under `binaries`, `integration_tests`, and `examples`, the `*` entry applies
    to every target of that kind. `compile_data` and `data` entries are globs
    relative to the crate, or labels when they start with `//`, `:`, or `@`.
    `{package}` in a `rustc_env` value becomes the crate's Bazel package path.
    """
    package = native.package_name()
    crate = CRATES.get(package)
    if crate == None:
        fail("{} is not a member of the Cargo workspace".format(package))

    name = crate["name"]
    compile_data = _files(compile_data)
    data = _files(data)
    test_env = _env(env)
    rustc_env = _env(rustc_env)

    # crate_aliases may return a select, which only `|` can merge into.
    aliases = crate_aliases(normal = True, normal_dev = True, proc_macro = True, proc_macro_dev = True) | crate["aliases"]
    normal_deps = all_crate_deps(normal = True) + crate["deps"]
    normal_proc_macro_deps = all_crate_deps(proc_macro = True) + crate["proc_macro_deps"]
    test_deps = all_crate_deps(normal_dev = True) + crate["dev_deps"]
    test_proc_macro_deps = all_crate_deps(proc_macro_dev = True) + crate["dev_proc_macro_deps"]

    # Lint levels come from the crate's Cargo.toml, as Cargo applies them only
    # to crates that set or inherit a [lints] table.
    extract_cargo_lints(
        name = name + "_lints",
        manifest = "Cargo.toml",
        workspace = "//rsworkspace:Cargo.toml",
    )

    variants = {"": crate["crate_features"]}
    for variant, variant_features in feature_variants.items():
        variants["_" + variant] = variant_features
    for suffix, crate_features in variants.items():
        _crate_targets(
            crate = crate,
            suffix = suffix,
            crate_features = crate_features,
            compile_data = compile_data,
            data = data,
            test_env = test_env,
            rustc_env = rustc_env,
            aliases = aliases,
            normal_deps = normal_deps,
            normal_proc_macro_deps = normal_proc_macro_deps,
            test_deps = test_deps,
            test_proc_macro_deps = test_proc_macro_deps,
            library_tags = library_tags,
            unit_test_args = unit_test_args,
            unit_test_tags = unit_test_tags,
            binaries = binaries,
            integration_tests = integration_tests,
            examples = examples,
        )

def _crate_targets(
        crate,
        suffix,
        crate_features,
        compile_data,
        data,
        test_env,
        rustc_env,
        aliases,
        normal_deps,
        normal_proc_macro_deps,
        test_deps,
        test_proc_macro_deps,
        library_tags,
        unit_test_args,
        unit_test_tags,
        binaries,
        integration_tests,
        examples):
    """Declares every target of one feature set, with `suffix` on each name."""
    name = crate["name"]
    version = crate["version"]
    edition = crate["edition"]
    library = crate["library"]
    lint_config = ":" + name + "_lints"
    lib_target = name + suffix
    lib_deps = [":" + lib_target] if library and not library["proc_macro"] else []
    lib_proc_macro_deps = [":" + lib_target] if library and library["proc_macro"] else []

    # A binary named after its package would clash with the library target.
    binary_targets = {
        binary["name"]: (binary["name"] + "_bin" if library and binary["name"] == name else binary["name"]) + suffix
        for binary in crate["binaries"]
    }

    binary_settings = _target_settings("binaries", binaries, crate["binaries"])
    for binary in crate["binaries"]:
        settings = binary_settings[binary["name"]]
        target = binary_targets[binary["name"]]
        root = binary["root"]
        rust_binary(
            name = target,
            srcs = _dedupe(native.glob(["src/**/*.rs"], allow_empty = True) + _target_srcs(root)),
            aliases = aliases,
            compile_data = compile_data + _files(settings["compile_data"]),
            crate_features = crate_features,
            crate_name = binary["name"].replace("-", "_"),
            crate_root = root,
            edition = edition,
            proc_macro_deps = lib_proc_macro_deps + normal_proc_macro_deps,
            rustc_env = rustc_env | _env(settings["rustc_env"]),
            lint_config = lint_config,
            rustc_flags = _RUSTC_FLAGS,
            tags = settings["tags"],
            version = version,
            visibility = ["//visibility:public"],
            deps = lib_deps + normal_deps,
        )
        if not binary["test"]:
            continue
        rust_test(
            name = target + "_unit_test",
            aliases = aliases,
            args = settings["test_args"],
            crate = ":" + target,
            compile_data = compile_data + _files(settings["compile_data"]),
            crate_features = crate_features,
            data = data,
            env = test_env,
            proc_macro_deps = test_proc_macro_deps,
            lint_config = lint_config,
            rustc_flags = _RUSTC_FLAGS,
            tags = unit_test_tags,
            deps = test_deps,
        )

    # Cargo hands every integration test the path of each binary in the package.
    binary_data = [":" + target for target in binary_targets.values()]
    binary_env = {
        "CARGO_BIN_EXE_" + binary: "$(rootpath :{})".format(target)
        for binary, target in binary_targets.items()
    }
    binary_coverage_env = {"RUST_COVERAGE_OBJECTS": "\n".join(binary_env.values())} if binary_targets else {}

    test_settings = _target_settings("integration_tests", integration_tests, crate["integration_tests"])
    for test in crate["integration_tests"]:
        settings = test_settings[test["name"]]
        rust_test(
            name = name + "_" + test["name"] + suffix + "_test",
            aliases = aliases,
            args = settings["args"],
            srcs = _target_srcs(test["root"]),
            compile_data = compile_data + _files(settings["compile_data"]),
            crate_features = crate_features,
            crate_root = test["root"],
            data = _dedupe(data + binary_data + _files(settings["data"])),
            edition = edition,
            env = test_env | binary_coverage_env,
            proc_macro_deps = lib_proc_macro_deps + normal_proc_macro_deps + test_proc_macro_deps,
            rustc_env = rustc_env | binary_env | _env(settings["rustc_env"]),
            lint_config = lint_config,
            rustc_flags = _RUSTC_FLAGS,
            tags = settings["tags"],
            version = version,
            deps = lib_deps + normal_deps + test_deps,
        )

    example_settings = _target_settings("examples", examples, crate["examples"])
    for example in crate["examples"]:
        settings = example_settings[example["name"]]
        rust_binary(
            name = name + "_" + example["name"] + suffix + "_example",
            aliases = aliases,
            srcs = _target_srcs(example["root"]),
            compile_data = compile_data + _files(settings["compile_data"]),
            crate_features = crate_features,
            crate_root = example["root"],
            edition = edition,
            proc_macro_deps = lib_proc_macro_deps + normal_proc_macro_deps + test_proc_macro_deps,
            rustc_env = rustc_env | _env(settings["rustc_env"]),
            lint_config = lint_config,
            rustc_flags = _RUSTC_FLAGS,
            tags = settings["tags"],
            # Examples link dev-dependencies, which only test code may use.
            testonly = True,
            version = version,
            deps = lib_deps + normal_deps + test_deps,
        )

    if not library:
        return

    library_root = library["root"]
    library_srcs = native.glob(["src/**/*.rs"], allow_empty = True)
    if library_root not in library_srcs:
        library_srcs = library_srcs + _target_srcs(library_root)
    library_rule = rust_proc_macro if library["proc_macro"] else rust_library
    library_rule(
        name = lib_target,
        aliases = aliases,
        srcs = library_srcs,
        compile_data = compile_data,
        crate_features = crate_features,
        crate_name = library["crate_name"],
        crate_root = library_root,
        edition = edition,
        proc_macro_deps = normal_proc_macro_deps,
        rustc_env = rustc_env,
        lint_config = lint_config,
        rustc_flags = _RUSTC_FLAGS,
        tags = library_tags,
        version = version,
        visibility = ["//visibility:public"],
        deps = normal_deps,
    )

    # The wasm build takes its features from Cargo's wasm32 resolution, so
    # feature variants do not declare it again.
    wasm = crate["wasm"] if not suffix else None
    if wasm:
        # Only buildable for wasm32; `bazel build //...` on the host skips it.
        wasm_rule = rust_shared_library if crate["component"] else rust_library
        wasm_rule(
            name = name + "_wasm",
            aliases = wasm["aliases"],
            srcs = library_srcs,
            compile_data = compile_data,
            crate_features = wasm["crate_features"],
            crate_name = library["crate_name"],
            crate_root = library_root,
            edition = edition,
            proc_macro_deps = wasm["proc_macro_deps"],
            rustc_env = rustc_env,
            lint_config = lint_config,
            rustc_flags = _RUSTC_FLAGS,
            target_compatible_with = ["@platforms//cpu:wasm32"],
            version = version,
            visibility = ["//visibility:public"],
            deps = wasm["deps"],
        )
        if crate["component"]:
            decider_component(name = name, crate_name = library["crate_name"])

    if library["test"]:
        rust_test(
            name = lib_target + "_test",
            aliases = aliases,
            args = unit_test_args,
            crate = ":" + lib_target,
            compile_data = compile_data,
            # A test compiles the crate again and takes features from its own attributes.
            crate_features = crate_features,
            data = data,
            env = test_env,
            proc_macro_deps = test_proc_macro_deps,
            lint_config = lint_config,
            rustc_flags = _RUSTC_FLAGS,
            tags = unit_test_tags,
            deps = test_deps,
        )

    if library["doctest"]:
        rust_doc_test(
            name = lib_target + "_doc_test",
            crate = ":" + lib_target,
            crate_features = crate_features,
            proc_macro_deps = test_proc_macro_deps,
            deps = test_deps,
        )
