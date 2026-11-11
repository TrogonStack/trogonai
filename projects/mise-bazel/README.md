# mise-bazel

Status: experimental

Bazel uses the tools that mise installed, at the versions pinned in the mise
configuration. Bazel never downloads or installs them itself: mise owns
installation and its lockfile, and Bazel links the installed files into an
external repository and hashes them as build inputs.

## Usage

```starlark
bazel_dep(name = "mise-bazel")

mise = use_extension("@mise-bazel//mise:extensions.bzl", "mise")
mise.config(file = "//.config/mise:config.toml")
use_repo(mise, "mise", "mise_go", "mise_rust")

go_sdk.wrap(root_file = "@mise_go//:ROOT")

register_toolchains(
    "@mise_rust//:rust_toolchain",
    "@mise_rust//:cargo_toolchain",
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
| `rust` | `@mise_rust//:rust_toolchain` and `@mise_rust//:cargo_toolchain` for `rules_rust`, plus `@mise_rust//:ROOT` for crate_universe `host_tools` |

The version comes from the tool's entry in the configuration. When an entry
lists several versions, the first one is used. The executable name defaults to
the last segment of the tool name, so `ubi:open-telemetry/weaver` provides
`weaver`, and an `exe` option overrides it.

The Rust toolchain supports macOS ARM64 and Linux x86-64 hosts.
