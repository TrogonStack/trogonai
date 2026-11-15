load("@rules_rust//cargo:cargo_toolchain.bzl", "cargo_toolchain")
load("@rules_rust//rust:rust_stdlib_filegroup.bzl", "rust_stdlib_filegroup")
load("@rules_rust//rust:rust_toolchain.bzl", "rust_toolchain")

package(default_visibility = ["//visibility:public"])

filegroup(
    name = "rustc_lib",
    srcs = glob(
        [
            "bin/*.dylib*",
            "bin/*.so*",
            "lib/*.dylib*",
            "lib/*.so*",
            "lib/rustlib/%{rust_triple}/codegen-backends/**",
            "lib/rustlib/%{rust_triple}/lib/*.dylib*",
            "lib/rustlib/%{rust_triple}/lib/*.so*",
            "lib/rustlib/%{rust_triple}/lib/*.rmeta",
        ],
        allow_empty = True,
    ),
)

rust_stdlib_filegroup(
    name = "rust_std",
    srcs = glob(["lib/rustlib/%{rust_triple}/lib/**"]),
)

filegroup(
    name = "rust_lld",
    srcs = ["lib/rustlib/%{rust_triple}/bin/rust-lld"],
    data = glob(
        ["lib/rustlib/%{rust_triple}/bin/**"],
        exclude = [
            "lib/rustlib/%{rust_triple}/bin/rust-lld",
            "lib/rustlib/%{rust_triple}/bin/rust-objcopy",
        ],
    ),
)

filegroup(
    name = "llvm_lib",
    srcs = glob(
        [
            "lib/libLLVM*.so*",
            "lib/libLLVM*.dylib*",
            "lib/rustlib/%{rust_triple}/lib/libLLVM*.so*",
            "lib/rustlib/%{rust_triple}/lib/libLLVM*.dylib*",
        ],
        allow_empty = True,
    ),
)

rust_toolchain(
    name = "rust_impl",
    binary_ext = "",
    cargo = "bin/cargo",
    cargo_clippy = "bin/cargo-clippy",
    channel = "stable",
    clippy_driver = "bin/clippy-driver",
    default_edition = "2024",
    dylib_ext = "%{dylib_ext}",
    exec_triple = "%{rust_triple}",
    linker = ":rust_lld",
    linker_type = "direct",
    llvm_cov = "lib/rustlib/%{rust_triple}/bin/llvm-cov",
    llvm_lib = ":llvm_lib",
    llvm_profdata = "lib/rustlib/%{rust_triple}/bin/llvm-profdata",
    rust_doc = "bin/rustdoc",
    rust_objcopy = "lib/rustlib/%{rust_triple}/bin/rust-objcopy",
    rust_std = ":rust_std",
    rustc = "bin/rustc",
    rustc_lib = ":rustc_lib",
    rustfmt = "bin/rustfmt",
    staticlib_ext = ".a",
    stdlib_linkflags = %{stdlib_linkflags},
    strip_level = %{strip_level},
    target_triple = "%{rust_triple}",
    version = "%{rust_version}",
)

toolchain(
    name = "rust_toolchain",
    exec_compatible_with = %{constraints},
    target_compatible_with = %{constraints},
    toolchain = ":rust_impl",
    toolchain_type = "@rules_rust//rust:toolchain_type",
)

cargo_toolchain(
    name = "cargo_impl",
    cargo = "bin/cargo",
    channel = "stable",
    version = "%{rust_version}",
)

toolchain(
    name = "cargo_toolchain",
    exec_compatible_with = %{constraints},
    target_compatible_with = %{constraints},
    toolchain = ":cargo_impl",
    toolchain_type = "@rules_rust//cargo:toolchain_type",
)

exports_files(["ROOT"])
