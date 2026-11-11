"""Repository rules that link tools installed by mise."""

# Bazel must never trigger installs; mise owns installation.
_NO_INSTALL = {
    "MISE_AUTO_INSTALL": "false",
    "MISE_EXEC_AUTO_INSTALL": "false",
    "MISE_NOT_FOUND_SYSTEM_FALLBACK": "false",
}

_ENVIRON = ["HOME", "PATH", "MISE_DATA_DIR", "MISE_CONFIG_DIR", "RUSTUP_HOME", "MISE_RUSTUP_HOME", "CARGO_HOME", "MISE_CARGO_HOME"]

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

def _mise(rctx, arguments):
    mise = rctx.which("mise")
    if mise == None:
        fail("mise must be on PATH to locate installed tools")
    rctx.watch(mise.realpath)
    result = rctx.execute(
        [str(mise), "--no-config", "--no-env", "--no-hooks"] + arguments,
        environment = _NO_INSTALL,
        quiet = True,
    )
    if result.return_code:
        fail("mise {} failed; install the configured tools with `mise install`.\n{}".format(" ".join(arguments), result.stderr))
    return result.stdout.strip()

def _link_installed(rctx):
    """Links the installed tool into the repository root and returns its real path."""
    source = rctx.path(_mise(rctx, ["where", rctx.attr.tool + "@" + rctx.attr.version])).realpath
    if not source.exists:
        fail("{}@{} is not installed; run `mise install`".format(rctx.attr.tool, rctx.attr.version))
    rctx.watch_tree(source)
    for entry in source.readdir():
        if entry.basename not in _SKIPPED:
            rctx.symlink(entry, entry.basename)
    return source

def _mise_tool_impl(rctx):
    source = _link_installed(rctx)
    executable = None
    for directory in _mise(rctx, ["bin-paths", rctx.attr.tool + "@" + rctx.attr.version]).splitlines():
        candidate = rctx.path(directory).get_child(rctx.attr.executable)
        if candidate.exists:
            executable = str(candidate.realpath).removeprefix(str(source) + "/")
            break
    if executable == None:
        fail("{}@{} provides no `{}` executable".format(rctx.attr.tool, rctx.attr.version, rctx.attr.executable))
    rctx.file("BUILD.bazel", """\
alias(
    name = "{name}",
    actual = "{executable}",
    visibility = ["//visibility:public"],
)
""".format(name = rctx.attr.executable, executable = executable))

def _mise_go_impl(rctx):
    source = _link_installed(rctx)
    lines = rctx.read(source.get_child("VERSION")).splitlines()
    if not lines or lines[0] != "go" + rctx.attr.version:
        fail("The Go SDK at {} is not go{}".format(source, rctx.attr.version))
    rctx.file("ROOT", "go{}\n".format(rctx.attr.version))
    rctx.file("BUILD.bazel", 'exports_files(["ROOT"])\n')

def _mise_rust_impl(rctx):
    version = rctx.attr.version
    host = _RUST_HOSTS.get((rctx.os.name, _ARCH_ALIASES.get(rctx.os.arch, rctx.os.arch)))
    if host == None:
        fail("The mise Rust toolchain supports macOS ARM64 and Linux x86-64 only")

    # mise installs Rust through rustup; the real sysroot lives under RUSTUP_HOME.
    # Resolve it the way mise does instead of asking `mise env`, which installs
    # missing tools passed as arguments.
    rustup_home = rctx.getenv("MISE_RUSTUP_HOME") or rctx.getenv("RUSTUP_HOME") or "~/.rustup"
    if rustup_home == "~" or rustup_home.startswith("~/"):
        home = rctx.getenv("HOME")
        if not home:
            fail("Set HOME, RUSTUP_HOME, or MISE_RUSTUP_HOME to locate rust@{}".format(version))
        rustup_home = home + rustup_home.removeprefix("~")
    source = rctx.path("{}/toolchains/{}-{}".format(rustup_home, version, host.triple))
    if not source.exists:
        fail("rust@{} for {} is not installed; run `mise install rust`".format(version, host.triple))
    source = source.realpath
    rctx.watch_tree(source)
    for relative in ["bin/rustc", "bin/cargo", "lib/rustlib/{}/bin/llvm-cov".format(host.triple)]:
        if not source.get_child(relative).exists:
            fail("The Rust toolchain at {} lacks {}; check the configured components".format(source, relative))
    for entry in source.readdir():
        if entry.basename not in _SKIPPED:
            rctx.symlink(entry, entry.basename)
    rctx.file("ROOT", "rust{}\n".format(version))
    rctx.template("BUILD.bazel", rctx.attr._build, substitutions = {
        "%{rust_version}": version,
        "%{rust_triple}": host.triple,
        "%{constraints}": repr(host.constraints),
        "%{dylib_ext}": host.dylib_ext,
        "%{stdlib_linkflags}": repr(host.stdlib_linkflags),
        "%{strip_level}": repr(host.strip_level),
    })

def _mise_hub_impl(rctx):
    rctx.file("BUILD.bazel", "")
    for executable, repository in rctx.attr.tools.items():
        rctx.file("tools/{}/BUILD.bazel".format(executable), """\
alias(
    name = "{executable}",
    actual = "@{repository}//:{executable}",
    visibility = ["//visibility:public"],
)
""".format(executable = executable, repository = repository))

_INSTALLED = {
    "tool": attr.string(mandatory = True),
    "version": attr.string(mandatory = True),
}

mise_tool = repository_rule(
    implementation = _mise_tool_impl,
    attrs = _INSTALLED | {"executable": attr.string(mandatory = True)},
    environ = _ENVIRON,
    local = True,
    configure = True,
)

mise_go = repository_rule(
    implementation = _mise_go_impl,
    attrs = _INSTALLED,
    environ = _ENVIRON,
    local = True,
    configure = True,
)

mise_rust = repository_rule(
    implementation = _mise_rust_impl,
    attrs = _INSTALLED | {"_build": attr.label(default = Label("//mise/private:rust.BUILD.bazel.tpl"))},
    environ = _ENVIRON,
    local = True,
    configure = True,
)

mise_hub = repository_rule(
    implementation = _mise_hub_impl,
    attrs = {"tools": attr.string_dict(mandatory = True)},
)
