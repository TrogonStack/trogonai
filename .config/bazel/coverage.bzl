"""Toolchain executables used by coverage collector regressions."""

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

def _node_coverage_tools_impl(ctx):
    node = ctx.attr.node_toolchain[platform_common.ToolchainInfo].nodeinfo.node
    if not node:
        fail("coverage regressions require a hermetic Node executable")
    manifest = ctx.actions.declare_file(ctx.label.name + ".json")
    ctx.actions.write(manifest, json.encode({"node": node.short_path}))
    return [DefaultInfo(
        files = depset([manifest]),
        runfiles = ctx.runfiles(files = [node]),
    )]

node_coverage_tools = rule(
    implementation = _node_coverage_tools_impl,
    attrs = {
        "node_toolchain": attr.label(
            default = Label("@mise_node//:node_impl"),
            cfg = "exec",
            providers = [platform_common.ToolchainInfo],
        ),
    },
)
