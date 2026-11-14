"""Bazel targets for a Cargo workspace crate."""

load("@crates//:defs.bzl", "all_crate_deps")
load("@rules_rust//rust:defs.bzl", "rust_binary", "rust_doc_test", "rust_library", "rust_proc_macro", "rust_test")

_RUSTC_FLAGS = ["-Dwarnings"]

def _without(labels, others):
    """Drops from `labels` every label already in `others`.

    A Cargo crate may list a dependency both as optional, turned on by one of
    its features, and as a dev-dependency. Targets that take both lists would
    then name it twice, which Bazel rejects. A `select` cannot be inspected,
    so `labels` is kept whole when `others` is one.
    """
    if type(others) != "list":
        return labels
    seen = {str(Label(other)): True for other in others}
    return [label for label in labels if str(Label(label)) not in seen]

def workspace_crate(
        name,
        version = "0.1.0",
        edition = "2024",
        library = True,
        proc_macro = False,
        binaries = {},
        crate_features = [],
        compile_data = [],
        data = [],
        rustc_env = {},
        unit_test_args = [],
        deps = [],
        proc_macro_deps = [],
        dev_deps = [],
        examples = {},
        integration_tests = {},
        unit_test_tags = []):
    """Declares a library, its unit tests, doctests, integration tests, and examples.

    External dependencies come from the crate's Cargo.toml through crate_universe.

    Args:
        name: Cargo package name; the crate name replaces dashes with underscores.
        version: Cargo package version.
        edition: Cargo package edition.
        library: Whether the package has a library target at `src/lib.rs`.
        proc_macro: Whether the library is a procedural macro crate.
        binaries: Maps each Cargo binary name to extra `rust_binary` attributes;
            `crate_root` defaults to `src/main.rs`. The target is the binary name,
            with a `_bin` suffix when it matches the package name of a crate that
            also has a library. Each binary also gets a unit test target, as
            `cargo test` runs the tests inside binaries too; a `test_args` key
            sets that test's arguments.
        crate_features: Cargo features enabled for the library and its tests.
        compile_data: Files read at compile time, such as `include_str!` inputs.
        data: Files read at test runtime.
        rustc_env: Environment for compiling the library, which its unit tests
            inherit.
        deps: Workspace crate labels the library depends on.
        proc_macro_deps: Workspace procedural macro crate labels the library uses.
        dev_deps: Workspace crate labels only the tests and examples depend on.
        examples: Maps each `examples/<name>.rs` to extra `rust_binary` attributes.
        integration_tests: Maps each `tests/<name>.rs` to extra `rust_test` attributes.
        unit_test_args: Arguments for the unit test binary, such as `--skip` filters.
        unit_test_tags: Tags for the unit test target.
    """
    crate_name = name.replace("-", "_")
    lib_deps = [":" + name] if library else []
    library_rule = rust_proc_macro if proc_macro else rust_library

    test_deps = all_crate_deps(normal_dev = True) + dev_deps
    test_proc_macro_deps = all_crate_deps(proc_macro_dev = True)

    for binary, attrs in binaries.items():
        binary_attrs = dict({"crate_root": "src/main.rs"}, **attrs)
        binary_test_args = binary_attrs.pop("test_args", [])

        # A binary named after its package would clash with the library target.
        target = binary + "_bin" if library and binary == name else binary
        rust_binary(
            name = target,
            srcs = native.glob(["src/**/*.rs"]),
            compile_data = compile_data,
            crate_features = crate_features,
            crate_name = binary.replace("-", "_"),
            edition = edition,
            proc_macro_deps = all_crate_deps(proc_macro = True) + proc_macro_deps,
            rustc_flags = _RUSTC_FLAGS,
            version = version,
            visibility = ["//visibility:public"],
            deps = lib_deps + all_crate_deps(normal = True) + deps,
            **binary_attrs
        )
        rust_test(
            name = target + "_unit_test",
            args = binary_test_args,
            crate = ":" + target,
            compile_data = compile_data,
            crate_features = crate_features,
            data = data,
            proc_macro_deps = test_proc_macro_deps,
            rustc_flags = _RUSTC_FLAGS,
            tags = unit_test_tags,
            deps = test_deps,
        )

    for test, attrs in integration_tests.items():
        rust_test(
            name = name + "_" + test + "_test",
            srcs = native.glob(["tests/" + test + ".rs", "tests/" + test + "/**/*.rs"], allow_empty = True),
            crate_features = crate_features,
            crate_root = "tests/" + test + ".rs",
            edition = edition,
            proc_macro_deps = all_crate_deps(proc_macro = True) + proc_macro_deps + test_proc_macro_deps,
            rustc_flags = _RUSTC_FLAGS,
            deps = lib_deps + all_crate_deps(normal = True) + _without(deps, test_deps) + test_deps,
            **dict({"data": data, "version": version}, **attrs)
        )

    for example, attrs in examples.items():
        rust_binary(
            name = name + "_" + example + "_example",
            srcs = native.glob(["examples/" + example + ".rs", "examples/" + example + "/**/*.rs"], allow_empty = True),
            crate_features = crate_features,
            crate_root = "examples/" + example + ".rs",
            edition = edition,
            proc_macro_deps = all_crate_deps(proc_macro = True) + proc_macro_deps + test_proc_macro_deps,
            rustc_flags = _RUSTC_FLAGS,
            # Examples link dev-dependencies, which only test code may use.
            testonly = True,
            deps = lib_deps + all_crate_deps(normal = True) + _without(deps, test_deps) + test_deps,
            **dict({"version": version}, **attrs)
        )

    if not library:
        return

    library_rule(
        name = name,
        srcs = native.glob(["src/**/*.rs"]),
        compile_data = compile_data,
        crate_features = crate_features,
        crate_name = crate_name,
        crate_root = "src/lib.rs",
        edition = edition,
        proc_macro_deps = all_crate_deps(proc_macro = True) + proc_macro_deps,
        rustc_env = rustc_env,
        rustc_flags = _RUSTC_FLAGS,
        version = version,
        visibility = ["//visibility:public"],
        deps = all_crate_deps(normal = True) + deps,
    )

    rust_test(
        name = name + "_test",
        args = unit_test_args,
        crate = ":" + name,
        compile_data = compile_data,
        # A test compiles the crate again and takes features from its own attributes.
        crate_features = crate_features,
        data = data,
        proc_macro_deps = test_proc_macro_deps,
        rustc_flags = _RUSTC_FLAGS,
        tags = unit_test_tags,
        deps = test_deps,
    )

    rust_doc_test(
        name = name + "_doc_test",
        crate = ":" + name,
        crate_features = crate_features,
        proc_macro_deps = test_proc_macro_deps,
        deps = test_deps,
    )
