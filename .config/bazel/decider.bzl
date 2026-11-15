"""Decider components: the wasm32 build, its checks, and the conformance suites.

`workspace_crate()` calls `decider_component()` for every crate whose library is
a cdylib with `[package.metadata.component]`, so each component gets:

- `:component`, the component `cargo component build --release` produces, made
  from the crate's `<name>_wasm` core module with `wasm-tools component new`.
  Tests that list it in `data` find it where Cargo would leave it,
  `rsworkspace/target/wasm32-unknown-unknown/release/<crate>.wasm`, relative to
  the runfiles root.
- `:component_test`, which fails when the component declares an import or does
  not validate.

`decider_suites()` in the decider-test CLI package runs every component's YAML
suite and checks that each component has a suite and a parity test, or says in
its Cargo.toml why it does not.
"""

load("@cargo_workspace//:crates.bzl", "CRATES")
load("@rules_shell//shell:sh_test.bzl", "sh_test")

_WASM_TOOLS = Label("//.config/bazel:wasm-tools")
_CHECK_COMPONENT = Label("//.config/bazel:decider_component_test.sh")
_REPORT_COVERAGE = Label("//.config/bazel:decider_coverage_test.sh")
_RUN_SUITE = Label("//.config/bazel:decider_suite_test.sh")
_COMPONENTS_DIR = "rsworkspace/wasm-components/"
_SIM_PACKAGE = "rsworkspace/crates/decider/trogon-decider-sim"

def _wasm32_release_impl(_settings, _attr):
    return {
        "//command_line_option:compilation_mode": "opt",
        "//command_line_option:platforms": str(Label("//.config/bazel:wasm32-unknown-unknown")),
    }

_wasm32_release = transition(
    implementation = _wasm32_release_impl,
    inputs = [],
    outputs = [
        "//command_line_option:compilation_mode",
        "//command_line_option:platforms",
    ],
)

def _decider_component_impl(ctx):
    modules = [file for file in ctx.files.module if file.extension == "wasm"]
    if len(modules) != 1:
        fail("{} must build exactly one .wasm file, got {}".format(ctx.attr.module[0].label, modules))
    filename = ctx.attr.crate_name + ".wasm"
    component = ctx.actions.declare_file(ctx.label.name + "/" + filename)
    ctx.actions.run(
        executable = ctx.executable._wasm_tools,
        arguments = ["component", "new", modules[0].path, "-o", component.path],
        inputs = modules,
        outputs = [component],
        mnemonic = "WasmComponentNew",
        progress_message = "Making decider component %{output}",
    )
    return [DefaultInfo(
        files = depset([component]),
        runfiles = ctx.runfiles(symlinks = {
            "rsworkspace/target/wasm32-unknown-unknown/release/" + filename: component,
        }),
    )]

_decider_component = rule(
    implementation = _decider_component_impl,
    doc = "A decider component made from a wasm32 core module built in release mode.",
    attrs = {
        "crate_name": attr.string(mandatory = True, doc = "The crate's library name, which names the .wasm file."),
        "module": attr.label(mandatory = True, cfg = _wasm32_release, doc = "The crate's wasm32 cdylib."),
        "_wasm_tools": attr.label(default = _WASM_TOOLS, allow_single_file = True, cfg = "exec", executable = True),
    },
)

# The package path names the component, so the macro takes no name.
# buildifier: disable=unnamed-macro
def decider_component(name, crate_name):
    """Declares `:component` and `:component_test` for the decider crate `name`.

    Args:
        name: The crate name, whose `<name>_wasm` target is the core module.
        crate_name: The crate's library name, which names the .wasm file.
    """
    _decider_component(
        name = "component",
        crate_name = crate_name,
        module = ":" + name + "_wasm",
        visibility = ["//visibility:public"],
    )
    sh_test(
        name = "component_test",
        srcs = [_CHECK_COMPONENT],
        args = [
            "$(rootpath {})".format(_WASM_TOOLS),
            "$(rootpath :component)",
        ],
        data = [
            ":component",
            _WASM_TOOLS,
        ],
    )

def _alias(crate_name):
    return crate_name.removeprefix("trogon-").removesuffix("-decider")

def _exemption(crate, table):
    return crate["metadata"].get(table, {}).get("exempt")

# buildifier: disable=unnamed-macro
def decider_suites(binary):
    """Runs every component's conformance suite and gates suite and parity coverage.

    Every decider component needs `<alias>.yaml` in this package and a
    `parity_<alias>` integration test in trogon-decider-sim, or an
    `exempt = "..."` reason under `[package.metadata.decider-test]` or
    `[package.metadata.decider-parity]` in its Cargo.toml. Each suite runs as
    `:<alias>_suite_test`. `:decider_coverage_test` fails on a component missing
    either without a reason, on a reason given for a suite or test that exists,
    and on a suite or parity test no component claims.

    Args:
        binary: The decider-test binary target.
    """
    sim = CRATES[_SIM_PACKAGE]
    parity_tests = [test["name"] for test in sim["integration_tests"] if test["name"].startswith("parity_")]
    suites = native.glob(["*.yaml"], allow_empty = True)
    components = {
        package: crate
        for package, crate in CRATES.items()
        if crate["component"] and package.startswith(_COMPONENTS_DIR)
    }

    notes = []
    violations = []
    claimed_suites = {}
    claimed_parity = {}
    if not components:
        violations.append("no decider components found under " + _COMPONENTS_DIR)
    for package, crate in sorted(components.items()):
        alias = _alias(crate["name"])

        suite = alias + ".yaml"
        suite_exempt = _exemption(crate, "decider-test")
        if suite in suites:
            claimed_suites[suite] = True
        if suite_exempt and suite in suites:
            violations.append("{}: declares a decider-test exemption but {} exists; drop the exemption so the suite is enforced".format(package, suite))
        elif suite_exempt:
            notes.append("conformance-exempt {}: {}".format(alias, suite_exempt))
        elif suite not in suites:
            violations.append('{}: no conformance suite at {}/{} and no [package.metadata.decider-test] exempt = "..." explaining why'.format(crate["name"], native.package_name(), suite))
        else:
            sh_test(
                name = alias + "_suite_test",
                srcs = [_RUN_SUITE],
                args = [
                    "$(rootpath {})".format(binary),
                    "$(rootpath //{}:component)".format(package),
                    "$(rootpath {})".format(suite),
                ],
                data = [
                    binary,
                    "//{}:component".format(package),
                    suite,
                    _WASM_TOOLS,
                ],
                # decider-test runs the import check through wasm-tools.
                env = {"WASM_TOOLS": "$(rootpath {})".format(_WASM_TOOLS)},
            )

        parity = "parity_" + alias
        parity_exempt = _exemption(crate, "decider-parity")
        if parity in parity_tests:
            claimed_parity[parity] = True
        if parity_exempt and parity in parity_tests:
            violations.append("{}: declares a decider-parity exemption but {} exists; drop the exemption so the parity test is enforced".format(package, parity))
        elif parity_exempt:
            notes.append("parity-exempt {}: {}".format(alias, parity_exempt))
        elif parity not in parity_tests:
            violations.append('{}: no parity test at {}/tests/{}.rs and no [package.metadata.decider-parity] exempt = "..." explaining why'.format(crate["name"], _SIM_PACKAGE, parity))

    for suite in suites:
        if suite not in claimed_suites:
            violations.append("{}/{} is not the suite of any wasm-components/* decider".format(native.package_name(), suite))
    for parity in parity_tests:
        if parity not in claimed_parity:
            violations.append("{}/tests/{}.rs is not the parity test of any wasm-components/* decider".format(_SIM_PACKAGE, parity))

    # Test args are split like a shell command line, so the messages travel in
    # the environment, one per line.
    sh_test(
        name = "decider_coverage_test",
        srcs = [_REPORT_COVERAGE],
        env = {
            "DECIDER_NOTES": "\n".join(notes).replace("$", "$$"),
            "DECIDER_VIOLATIONS": "\n".join(violations).replace("$", "$$"),
        },
    )
