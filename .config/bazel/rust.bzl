"""Bazel targets for a Cargo workspace crate."""

load("@crates//:defs.bzl", "all_crate_deps")
load("@rules_rust//rust:defs.bzl", "rust_library", "rust_test")

_RUSTC_FLAGS = ["-Dwarnings"]

def workspace_crate(
        name,
        version = "0.1.0",
        crate_features = [],
        compile_data = [],
        data = [],
        unit_test_args = [],
        deps = [],
        dev_deps = [],
        integration_tests = {},
        unit_test_tags = []):
    """Declares a library, its unit tests, and its integration tests.

    External dependencies come from the crate's Cargo.toml through crate_universe.

    Args:
        name: Cargo package name; the crate name replaces dashes with underscores.
        version: Cargo package version.
        crate_features: Cargo features enabled for the library and its tests.
        compile_data: Files read at compile time, such as `include_str!` inputs.
        data: Files read at test runtime.
        deps: Workspace crate labels the library depends on.
        dev_deps: Workspace crate labels only the tests depend on.
        integration_tests: Maps each `tests/<name>.rs` to extra `rust_test` attributes.
        unit_test_args: Arguments for the unit test binary, such as `--skip` filters.
        unit_test_tags: Tags for the unit test target.
    """
    crate_name = name.replace("-", "_")

    rust_library(
        name = name,
        srcs = native.glob(["src/**/*.rs"]),
        compile_data = compile_data,
        crate_features = crate_features,
        crate_name = crate_name,
        crate_root = "src/lib.rs",
        edition = "2024",
        proc_macro_deps = all_crate_deps(proc_macro = True),
        rustc_flags = _RUSTC_FLAGS,
        version = version,
        visibility = ["//visibility:public"],
        deps = all_crate_deps(normal = True) + deps,
    )

    test_deps = all_crate_deps(normal_dev = True) + dev_deps
    test_proc_macro_deps = all_crate_deps(proc_macro_dev = True)

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

    for test, attrs in integration_tests.items():
        rust_test(
            name = name + "_" + test + "_test",
            srcs = native.glob(["tests/" + test + ".rs", "tests/" + test + "/**/*.rs"], allow_empty = True),
            crate_features = crate_features,
            crate_root = "tests/" + test + ".rs",
            edition = "2024",
            proc_macro_deps = all_crate_deps(proc_macro = True) + test_proc_macro_deps,
            rustc_flags = _RUSTC_FLAGS,
            deps = [":" + name] + all_crate_deps(normal = True) + deps + test_deps,
            **dict({"data": data, "version": version}, **attrs)
        )
