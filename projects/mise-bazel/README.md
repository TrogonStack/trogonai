# mise-bazel

Status: experimental

Bazel uses the tools that mise installed, at the versions pinned in the mise
configuration. Bazel never downloads or installs them itself: mise owns
installation and its lockfile, and Bazel links the installed files into an
external repository and hashes them as build inputs.

## Usage

```starlark
bazel_dep(name = "mise-bazel")

mise = use_repo_rule("@mise-bazel//mise:repositories.bzl", "mise")

mise(
    name = "mise",
    config = "//.config/mise:config.toml",
    tools = ["buf", "go", "rust"],
)
```

Install the tools first with `mise install`. A missing tool fails the build
with an installation hint.

| Tool | Provides |
| --- | --- |
| any executable tool | `@mise//tools/<tool>` |
| `go` | `@mise//go:ROOT`, for `go_sdk.wrap(root_file = ...)` in `rules_go` |
| `rust` | `@mise//:rust_toolchain` and `@mise//:cargo_toolchain` for `rules_rust`, plus `@mise//rust:ROOT` for crate_universe `host_tools` |

The version comes from the tool's entry in the configuration. When an entry
lists several versions, the first one is used.

The Rust toolchain supports macOS ARM64 and Linux x86-64 hosts.
