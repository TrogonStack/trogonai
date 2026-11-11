"""Import installed mise SDKs as declared native Bazel toolchains."""

load("@toml.bzl", "toml")

_NO_INSTALL = {
    "MISE_AUTO_INSTALL": "false",
    "MISE_EXEC_AUTO_INSTALL": "false",
    "MISE_NOT_FOUND_SYSTEM_FALLBACK": "false",
}

def _execute(ctx, arguments, description, environment = {}):
    result = ctx.execute(arguments, environment = environment, quiet = True)
    if result.return_code:
        fail("{} failed. Install the configured SDKs with mise first.\n{}".format(description, result.stderr))
    return result.stdout.strip()

def _mise(ctx, executable, arguments, description):
    return _execute(
        ctx,
        [str(executable), "--no-config", "--no-env", "--no-hooks"] + arguments,
        description,
        environment = _NO_INSTALL,
    )

def _pin(ctx, mise, key):
    configured = toml.decode(ctx.read(ctx.attr.config))["tools"][key.removeprefix("tools.")]
    if type(configured) == "dict":
        key += ".version"
    value = _mise(
        ctx,
        mise,
        ["config", "get", "--file", str(ctx.path(ctx.attr.config)), key],
        "Reading {} from mise.toml".format(key),
    )
    if value.startswith("["):
        value = json.decode(value)
        if type(value) != "list" or not value:
            fail("{} must pin at least one version in mise.toml".format(key))
        value = value[0]
    if type(value) == "dict":
        value = value.get("version")
    if type(value) != "string":
        fail("{} must pin an exact numeric release in mise.toml".format(key))
    parts = value.split(".")
    if len(parts) != 3 or any([not part or part.strip("0123456789") for part in parts]):
        fail("{} must pin an exact numeric release in mise.toml".format(key))
    return value

def _installed(ctx, mise, tool, version):
    location = _mise(ctx, mise, ["where", tool + "@" + version], "Locating installed " + tool)
    if not location.startswith("/"):
        fail("mise did not report an absolute installed path for " + tool)
    path = ctx.path(location)
    if not path.exists:
        fail("The configured {} SDK is absent; run mise install {}".format(tool, tool))
    ctx.watch(path)
    return path

def _host(ctx):
    if ctx.os.name == "mac os x" and ctx.os.arch in ["aarch64", "arm64"]:
        return struct(
            rust_triple = "aarch64-apple-darwin",
            go_platform = "darwin/arm64",
            constraints = ["@platforms//os:osx", "@platforms//cpu:aarch64"],
            dylib_ext = ".dylib",
            stdlib_linkflags = ["-lSystem"],
            # Stripping proc-macro dylibs corrupts LINKEDIT on the pinned macOS toolchain.
            strip_level = {"dbg": "none", "fastbuild": "none", "opt": "none"},
        )
    if ctx.os.name == "linux" and ctx.os.arch in ["x86_64", "amd64"]:
        return struct(
            rust_triple = "x86_64-unknown-linux-gnu",
            go_platform = "linux/amd64",
            constraints = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
            dylib_ext = ".so",
            stdlib_linkflags = ["-ldl", "-lpthread"],
            strip_level = None,
        )
    fail("Installed mise SDKs currently support macOS ARM64 and Linux x86-64 only")

def _overlay(ctx, source, prefix):
    for entry in source.readdir():
        if entry.basename in ["BUILD", "BUILD.bazel", "WORKSPACE", "WORKSPACE.bazel", "MODULE.bazel", "ROOT"]:
            continue
        ctx.symlink(entry, prefix + entry.basename)

def _go_sdk(ctx, mise, version, host):
    source = _installed(ctx, mise, "go", version).realpath
    ctx.watch_tree(source)
    version_file = source.get_child("VERSION")
    version_lines = ctx.read(version_file).splitlines() if version_file.exists else []
    if not version_lines or version_lines[0] != "go" + version:
        fail("The installed Go SDK VERSION does not match mise.toml")
    output = _execute(
        ctx,
        [str(source.get_child("bin/go")), "version"],
        "Validating installed Go",
        environment = {"GOTOOLCHAIN": "local", "GOENV": "off", "GOROOT": str(source)},
    )
    if output != "go version go{} {}".format(version, host.go_platform):
        fail("The installed Go compiler version or host platform does not match mise.toml and this host")
    _overlay(ctx, source, "go/")
    ctx.file("go/ROOT", "go{}\n".format(version))
    ctx.file("go/BUILD.bazel", 'exports_files(["ROOT"])\n')

def _rust_sdk(ctx, mise, version, host):
    installed = _installed(ctx, mise, "rust", version)
    environment = json.decode(_mise(ctx, mise, ["env", "--json", "rust@" + version], "Resolving installed Rust homes"))
    rust_environment = {key: environment[key] for key in ["CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN"] if key in environment}
    rust_environment["RUSTUP_TOOLCHAIN"] = version
    rustup = installed.get_child("rustup")
    if not rustup.exists:
        fail("The mise Rust installation must provide its installed rustup executable")
    ctx.watch(rustup)
    ctx.watch(rustup.realpath)
    rustc = ctx.path(_execute(
        ctx,
        [str(rustup), "which", "--toolchain", version, "rustc"],
        "Resolving the installed Rust compiler",
        environment = rust_environment,
    )).realpath
    ctx.watch(rustc)
    details = _execute(ctx, [str(rustc), "-vV"], "Validating installed Rust")
    metadata = {}
    for line in details.splitlines():
        key, separator, value = line.partition(":")
        if separator:
            metadata[key] = value.strip()
    if metadata.get("release") != version or metadata.get("host") != host.rust_triple:
        fail("The installed Rust compiler version or host platform does not match mise.toml and this host")
    source = ctx.path(_execute(ctx, [str(rustc), "--print", "sysroot"], "Locating the real Rust sysroot")).realpath
    if source.get_child("bin/rustc").realpath != rustc:
        fail("The installed Rust compiler does not belong to its reported sysroot")
    ctx.watch_tree(source)
    for tool in ["cargo", "rustdoc", "rustfmt", "clippy-driver", "cargo-clippy"]:
        selected = ctx.path(_execute(
            ctx,
            [str(rustup), "which", "--toolchain", version, tool],
            "Resolving installed Rust " + tool,
            environment = rust_environment,
        )).realpath
        if not selected.exists or selected != source.get_child("bin/" + tool).realpath:
            fail("The installed Rust {} is missing or belongs to another sysroot".format(tool))
        ctx.watch(selected)
    cargo_version = _execute(ctx, [str(source.get_child("bin/cargo")), "--version"], "Validating installed Cargo")
    if not cargo_version.startswith("cargo " + version + " "):
        fail("The installed Cargo version does not match the configured Rust release")
    for relative in ["lib/rustlib/{}/lib".format(host.rust_triple)] + [
        "lib/rustlib/{}/bin/{}".format(host.rust_triple, tool)
        for tool in ["rust-lld", "rust-objcopy", "llvm-cov", "llvm-profdata"]
    ]:
        if not source.get_child(relative).exists:
            fail("The installed Rust SDK is incomplete: {} is missing; run mise install rust".format(relative))

    # rules_rust preserves repository-relative library paths when constructing its sysroot.
    _overlay(ctx, source, "")
    _overlay(ctx, source, "rust/")
    ctx.file("rust/ROOT", "rust{}\n".format(version))
    ctx.file("rust/BUILD.bazel", 'exports_files(["ROOT"])\n')
    ctx.template("BUILD.bazel", ctx.attr._rust_build, substitutions = {
        "%{rust_version}": version,
        "%{rust_triple}": host.rust_triple,
        "%{constraints}": repr(host.constraints),
        "%{dylib_ext}": host.dylib_ext,
        "%{stdlib_linkflags}": repr(host.stdlib_linkflags),
        "%{strip_level}": repr(host.strip_level),
    })

def _mise_sdks_impl(ctx):
    mise = ctx.which("mise")
    if mise == None:
        fail("mise is required to locate the configured installed SDKs")
    ctx.watch(mise)
    ctx.watch(mise.realpath)
    ctx.watch(ctx.path(ctx.attr.config))
    host = _host(ctx)
    _go_sdk(ctx, mise, _pin(ctx, mise, "tools.go"), host)
    _rust_sdk(ctx, mise, _pin(ctx, mise, "tools.rust"), host)

mise_sdks = repository_rule(
    implementation = _mise_sdks_impl,
    attrs = {
        "config": attr.label(mandatory = True, allow_single_file = True),
        "_rust_build": attr.label(default = Label("//.config/bazel:rust_sdk.BUILD.bazel.tpl")),
    },
    environ = [
        "HOME",
        "PATH",
        "MISE_DATA_DIR",
        "MISE_CONFIG_DIR",
        "CARGO_HOME",
        "MISE_CARGO_HOME",
        "RUSTUP_HOME",
        "MISE_RUSTUP_HOME",
    ],
    local = True,
    configure = True,
)
