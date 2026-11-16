"""LLVM tools from the Rust toolchain used by collector regressions."""

load("@rules_rust//rust:defs.bzl", "rust_common")

def _rust_coverage_tools_impl(ctx):
    toolchain = ctx.toolchains[rust_common.toolchain_type]
    manifest = ctx.actions.declare_file(ctx.label.name + ".json")
    ctx.actions.write(manifest, json.encode({
        "llvm_cov": toolchain.llvm_cov.short_path,
        "llvm_profdata": toolchain.llvm_profdata.short_path,
    }))
    return [DefaultInfo(
        files = depset([manifest]),
        runfiles = ctx.runfiles(files = [toolchain.llvm_cov, toolchain.llvm_profdata] + toolchain.llvm_lib),
    )]

rust_coverage_tools = rule(
    implementation = _rust_coverage_tools_impl,
    toolchains = [rust_common.toolchain_type],
)
