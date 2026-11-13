load("@rules_nodejs//nodejs:toolchain.bzl", "nodejs_toolchain")

package(default_visibility = ["//visibility:public"])

# rules_js does not run npm, but the toolchain provider requires its entry point.
nodejs_toolchain(
    name = "node_impl",
    node = "bin/node",
    npm = "bin/npm",
)

toolchain(
    name = "node_toolchain",
    exec_compatible_with = %{constraints},
    toolchain = ":node_impl",
    toolchain_type = "@rules_nodejs//nodejs:toolchain_type",
)

toolchain(
    name = "node_runtime_toolchain",
    target_compatible_with = %{constraints},
    toolchain = ":node_impl",
    toolchain_type = "@rules_nodejs//nodejs:runtime_toolchain_type",
)

exports_files(["ROOT"])
