"""Exercise the Rust collector against an executed public helper binary."""

def _instrument_children_impl(_settings, _attr):
    return {
        "//command_line_option:collect_code_coverage": True,
        "//command_line_option:instrument_test_targets": True,
        "//command_line_option:instrumentation_filter": "^//rsworkspace/crates/platform/sandbox-command-supervisor[/:]",
    }

_instrument_children = transition(
    implementation = _instrument_children_impl,
    inputs = [],
    outputs = [
        "//command_line_option:collect_code_coverage",
        "//command_line_option:instrument_test_targets",
        "//command_line_option:instrumentation_filter",
    ],
)

def _target(value):
    return value[0] if type(value) == "list" else value

def _runfile(file):
    return '"${TEST_SRCDIR}/${TEST_WORKSPACE}/' + file.short_path + '"'

def _rust_child_coverage_test_impl(ctx):
    helper = _target(ctx.attr._helper)
    subject = _target(ctx.attr._subject)
    collector = ctx.attr._collector
    runner = ctx.attr._runner
    toolchain = ctx.toolchains["@rules_rust//rust:toolchain_type"]
    files = [
        collector[DefaultInfo].files_to_run.executable,
        subject[DefaultInfo].files_to_run.executable,
        helper[DefaultInfo].files_to_run.executable,
        toolchain.llvm_cov,
        toolchain.llvm_profdata,
    ]
    executable = ctx.actions.declare_file(ctx.label.name + ".sh")
    ctx.actions.write(
        output = executable,
        content = "#!/usr/bin/env bash\nset -euo pipefail\nexec " + _runfile(runner[DefaultInfo].files_to_run.executable) + " " + " ".join([_runfile(file) for file in files]) + "\n",
        is_executable = True,
    )
    runfiles = ctx.runfiles(files = files + toolchain.llvm_lib)
    for target in [helper, subject, collector, runner]:
        runfiles = runfiles.merge(target[DefaultInfo].default_runfiles)
    return [DefaultInfo(executable = executable, runfiles = runfiles)]

rust_child_coverage_test = rule(
    implementation = _rust_child_coverage_test_impl,
    test = True,
    attrs = {
        "_helper": attr.label(
            default = "//rsworkspace/crates/platform/sandbox-command-supervisor:sandbox-command-supervisor_bin",
            cfg = _instrument_children,
        ),
        "_subject": attr.label(
            default = "//rsworkspace/crates/platform/sandbox-command-supervisor:supervisor-overflow-fixture",
            cfg = _instrument_children,
        ),
        "_collector": attr.label(
            default = "@rules_rust//rust/coverage:collect_rust_coverage",
            executable = True,
            cfg = "exec",
        ),
        "_runner": attr.label(
            default = ":collector_test_runner",
            executable = True,
            cfg = "exec",
        ),
        "_allowlist_function_transition": attr.label(
            default = "@bazel_tools//tools/allowlists/function_transition_allowlist",
        ),
    },
    toolchains = ["@rules_rust//rust:toolchain_type"],
)
