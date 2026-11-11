"""Expose tools installed by mise to Bazel."""

load("@toml.bzl", "toml")

# Bazel must never trigger installs; mise owns installation.
_NO_INSTALL = {
    "MISE_AUTO_INSTALL": "false",
    "MISE_EXEC_AUTO_INSTALL": "false",
    "MISE_NOT_FOUND_SYSTEM_FALLBACK": "false",
}

_RUST_HOSTS = {
    ("mac os x", "aarch64"): struct(
        triple = "aarch64-apple-darwin",
        constraints = ["@platforms//os:osx", "@platforms//cpu:aarch64"],
        dylib_ext = ".dylib",
        stdlib_linkflags = ["-lSystem"],
        # Stripping proc-macro dylibs corrupts LINKEDIT on macOS.
        strip_level = {"dbg": "none", "fastbuild": "none", "opt": "none"},
    ),
    ("linux", "x86_64"): struct(
        triple = "x86_64-unknown-linux-gnu",
        constraints = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
        dylib_ext = ".so",
        stdlib_linkflags = ["-ldl", "-lpthread"],
        strip_level = None,
    ),
}

_ARCH_ALIASES = {"arm64": "aarch64", "amd64": "x86_64"}

_SKIPPED = ["BUILD", "BUILD.bazel", "MODULE.bazel", "REPO.bazel", "ROOT", "WORKSPACE", "WORKSPACE.bazel"]

def _mise(rctx, mise, arguments):
    result = rctx.execute(
        [mise, "--no-config", "--no-env", "--no-hooks"] + arguments,
        environment = _NO_INSTALL,
        quiet = True,
    )
    if result.return_code:
        fail("mise {} failed; install the configured tools with `mise install`.\n{}".format(" ".join(arguments), result.stderr))
    return result.stdout.strip()

def _version(rctx, tool):
    pin = toml.decode(rctx.read(rctx.attr.config)).get("tools", {}).get(tool)
    if type(pin) == "list":
        pin = pin[0] if pin else None
    if type(pin) == "dict":
        pin = pin.get("version")
    if type(pin) != "string" or not pin:
        fail("{} has no pinned version in {}".format(tool, rctx.attr.config))
    return pin

def _where(rctx, mise, tool, version):
    path = rctx.path(_mise(rctx, mise, ["where", tool + "@" + version])).realpath
    if not path.exists:
        fail("{}@{} is not installed; run `mise install {}`".format(tool, version, tool))
    rctx.watch_tree(path)
    return path

def _link(rctx, source, prefix):
    for entry in source.readdir():
        if entry.basename not in _SKIPPED:
            rctx.symlink(entry, prefix + entry.basename)

def _tool(rctx, mise, tool, version):
    source = _where(rctx, mise, tool, version)
    executable = None
    for directory in _mise(rctx, mise, ["bin-paths", tool + "@" + version]).splitlines():
        candidate = rctx.path(directory).get_child(tool)
        if candidate.exists:
            executable = str(candidate.realpath).removeprefix(str(source) + "/")
            break
    if executable == None:
        fail("{}@{} provides no `{}` executable".format(tool, version, tool))
    _link(rctx, source, "tools/{}/".format(tool))
    rctx.file("tools/{}/BUILD.bazel".format(tool), """\
alias(
    name = "{tool}",
    actual = "{executable}",
    visibility = ["//visibility:public"],
)
""".format(tool = tool, executable = executable))

def _go(rctx, mise, version):
    source = _where(rctx, mise, "go", version)
    lines = rctx.read(source.get_child("VERSION")).splitlines()
    if not lines or lines[0] != "go" + version:
        fail("The Go SDK at {} is not go{}".format(source, version))
    _link(rctx, source, "go/")
    rctx.file("go/ROOT", "go{}\n".format(version))
    rctx.file("go/BUILD.bazel", 'exports_files(["ROOT"])\n')

def _rust(rctx, mise, version):
    host = _RUST_HOSTS.get((rctx.os.name, _ARCH_ALIASES.get(rctx.os.arch, rctx.os.arch)))
    if host == None:
        fail("The mise Rust toolchain supports macOS ARM64 and Linux x86-64 only")

    # mise installs Rust through rustup; the real sysroot lives under RUSTUP_HOME.
    environment = json.decode(_mise(rctx, mise, ["env", "--json", "rust@" + version]))
    if "RUSTUP_HOME" not in environment:
        fail("mise reported no RUSTUP_HOME for rust@{}".format(version))
    source = rctx.path("{}/toolchains/{}-{}".format(environment["RUSTUP_HOME"], version, host.triple))
    if not source.exists:
        fail("rust@{} for {} is not installed; run `mise install rust`".format(version, host.triple))
    source = source.realpath
    rctx.watch_tree(source)
    for relative in ["bin/rustc", "bin/cargo", "lib/rustlib/{}/bin/llvm-cov".format(host.triple)]:
        if not source.get_child(relative).exists:
            fail("The Rust toolchain at {} lacks {}; check the configured components".format(source, relative))

    # rules_rust resolves standard-library paths relative to the repository root.
    _link(rctx, source, "")
    _link(rctx, source, "rust/")
    rctx.file("rust/ROOT", "rust{}\n".format(version))
    rctx.file("rust/BUILD.bazel", 'exports_files(["ROOT"])\n')
    rctx.template("BUILD.bazel", rctx.attr._rust_build, substitutions = {
        "%{rust_version}": version,
        "%{rust_triple}": host.triple,
        "%{constraints}": repr(host.constraints),
        "%{dylib_ext}": host.dylib_ext,
        "%{stdlib_linkflags}": repr(host.stdlib_linkflags),
        "%{strip_level}": repr(host.strip_level),
    })

def _mise_impl(rctx):
    mise = rctx.which("mise")
    if mise == None:
        fail("mise must be on PATH to locate installed tools")
    rctx.watch(mise.realpath)
    mise = str(mise)
    if "rust" not in rctx.attr.tools:
        rctx.file("BUILD.bazel", "")
    for tool in rctx.attr.tools:
        version = _version(rctx, tool)
        if tool == "go":
            _go(rctx, mise, version)
        elif tool == "rust":
            _rust(rctx, mise, version)
        else:
            _tool(rctx, mise, tool, version)

mise = repository_rule(
    implementation = _mise_impl,
    doc = "Links tools installed by mise, at the versions pinned in `config`.",
    attrs = {
        "config": attr.label(mandatory = True, allow_single_file = True, doc = "The mise configuration that pins tool versions."),
        "tools": attr.string_list(mandatory = True, doc = "Configured tools to expose."),
        "_rust_build": attr.label(default = Label("//mise/private:rust.BUILD.bazel.tpl")),
    },
    environ = ["HOME", "PATH", "MISE_DATA_DIR", "MISE_CONFIG_DIR", "RUSTUP_HOME", "MISE_RUSTUP_HOME", "CARGO_HOME", "MISE_CARGO_HOME"],
    local = True,
    configure = True,
)
