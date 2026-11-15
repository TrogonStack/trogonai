# mise-bazel

Status: experimental

Bazel uses the tools that mise installed, at the versions pinned in the mise
configuration. Bazel never downloads or installs them itself: mise owns
installation and its lockfile, and Bazel links the installed files into an
external repository and hashes them as build inputs.

## Usage

```starlark
bazel_dep(name = "mise-bazel")
bazel_dep(name = "rules_go", version = "0.65.0")
bazel_dep(name = "rules_rust", version = "0.75.0")
bazel_dep(name = "aspect_rules_js", version = "3.6.1")

mise = use_extension("@mise-bazel//mise:extensions.bzl", "mise")
mise.config(file = "//.config/mise:config.toml")
use_repo(mise, "mise", "mise_go", "mise_node", "mise_rust")

go_sdk = use_extension("@rules_go//go:extensions.bzl", "go_sdk")
go_sdk.wrap(root_file = "@mise_go//:ROOT")

# No downloaded Rust toolchains; the mise one is registered instead.
rust = use_extension("@rules_rust//rust:extensions.bzl", "rust")
rust.toolchain(versions = [])
register_toolchains(
    "@mise_rust//:rust_toolchain",
    "@mise_rust//:cargo_toolchain",
)

# Registered in the root module, ahead of the toolchains rules_nodejs
# declares for itself, so no Node.js is downloaded.
register_toolchains(
    "@mise_node//:node_toolchain",
    "@mise_node//:node_runtime_toolchain",
)
```

Every tool pinned in the configuration gets its own repository, named `mise_`
plus the tool name with other characters replaced by `_`. Bazel resolves a tool
only when a target uses it, so a build needs only the tools it touches
installed. Install them with `mise install`; a missing tool fails with an
installation hint.

| Tool | Provides |
| --- | --- |
| any executable tool | `@mise//tools/<executable>`, for example `@mise//tools/buf` |
| `go` | `@mise_go//:ROOT`, for `go_sdk.wrap` in `rules_go` |
| `node` | `@mise_node//:node_toolchain` and `@mise_node//:node_runtime_toolchain` for `rules_nodejs`, and so for `rules_js` and `rules_ts`, plus `@mise_node//:ROOT` |
| `rust` | `@mise_rust//:rust_toolchain` and `@mise_rust//:cargo_toolchain` for `rules_rust`, plus `@mise_rust//:ROOT` for crate_universe `host_tools` |

The version comes from the tool's entry in the configuration. When an entry
lists several versions, the first one is used. The executable name defaults to
the last segment of the tool name, so `ubi:open-telemetry/weaver` provides
`weaver`, and an `exe` option overrides it.

The Rust toolchain supports macOS ARM64 and Linux x86-64 hosts. The Node.js
toolchain supports macOS and Linux hosts on ARM64 and x86-64.

Bazel does not watch files in repositories that another module extension
creates, so crate_universe keeps its cached resolution when the Rust pin
changes. After changing the pin, delete `MODULE.bazel.lock` and run
`bazel mod deps` so crate_universe resolves again with the new Cargo.
