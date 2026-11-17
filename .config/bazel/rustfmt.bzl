"""Checks Rust formatting over the sources `cargo fmt` reaches.

The `rules_rust` rustfmt aspect hands rustfmt every source file directly, so it
also checks generated code that `cargo fmt` never reaches because it sits
behind `include!` or a `#[rustfmt::skip]` module. This aspect runs the same
check but skips files whose first line marks them `@generated`.
"""

load("@rules_rust//rust:defs.bzl", "rust_common")

_RUSTFMT_TOOLCHAIN = "@rules_rust//rust/rustfmt:toolchain_type"

def _crate_info(target):
    if rust_common.crate_info in target:
        return target[rust_common.crate_info]
    if rust_common.test_crate_info in target:
        return target[rust_common.test_crate_info].crate
    return None

def _rustfmt_aspect_impl(target, ctx):
    crate_info = _crate_info(target)
    if target.label.workspace_name or not crate_info:
        return []

    srcs = depset(getattr(ctx.rule.files, "srcs", []), transitive = [crate_info.srcs])
    srcs = [src for src in srcs.to_list() if src.is_source]
    if not srcs:
        return [OutputGroupInfo(rustfmt_checks = depset())]

    toolchain = ctx.toolchains[_RUSTFMT_TOOLCHAIN]
    marker = ctx.actions.declare_file(ctx.label.name + ".rustfmt.ok")
    args = ctx.actions.args()
    args.add(marker)
    args.add(toolchain.rustfmt)
    args.add(ctx.file._config)
    args.add(crate_info.edition)
    args.add_all(srcs)
    ctx.actions.run(
        executable = ctx.executable._check,
        inputs = srcs + [ctx.file._config],
        outputs = [marker],
        tools = [toolchain.all_files],
        arguments = [args],
        mnemonic = "Rustfmt",
        progress_message = "Rustfmt %{label}",
        toolchain = _RUSTFMT_TOOLCHAIN,
    )
    return [OutputGroupInfo(rustfmt_checks = depset([marker]))]

rustfmt_aspect = aspect(
    implementation = _rustfmt_aspect_impl,
    doc = "Fails when a hand-written Rust source of the target is not formatted.",
    attrs = {
        "_check": attr.label(
            allow_single_file = True,
            cfg = "exec",
            default = Label("//.config/bazel:rustfmt_check.sh"),
            executable = True,
        ),
        "_config": attr.label(
            allow_single_file = True,
            default = Label("//rsworkspace:rustfmt.toml"),
        ),
    },
    required_providers = [
        [rust_common.crate_info],
        [rust_common.test_crate_info],
    ],
    toolchains = [_RUSTFMT_TOOLCHAIN],
)
