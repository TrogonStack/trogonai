"""Cargo build scripts respect Bazel's native coverage filter."""

load("@bazel_skylib//lib:unittest.bzl", "analysistest", "asserts")

_COVERAGE_FLAGS = [
    "--coverage",
    "-fprofile-arcs",
    "-ftest-coverage",
    "-fprofile-instr-generate",
    "-fcoverage-mapping",
]

def _native_coverage_test_impl(ctx):
    env = analysistest.begin(ctx)
    actions = [a for a in analysistest.target_actions(env) if a.mnemonic == "CargoBuildScriptRun"]
    asserts.equals(env, 1, len(actions))
    for action in actions:
        for name in ["CFLAGS", "CXXFLAGS", "LDFLAGS"]:
            flags = action.env[name].split(" ")
            instrumented = any([flag in flags for flag in _COVERAGE_FLAGS])
            asserts.equals(env, ctx.attr.expected_coverage, instrumented, name + " must follow the instrumentation filter")
    return analysistest.end(env)

excluded_coverage_test = analysistest.make(
    _native_coverage_test_impl,
    attrs = {"expected_coverage": attr.bool(mandatory = True)},
    config_settings = {
        "//command_line_option:collect_code_coverage": True,
        "//command_line_option:instrumentation_filter": "^//rsworkspace[/:]",
    },
)

included_coverage_test = analysistest.make(
    _native_coverage_test_impl,
    attrs = {"expected_coverage": attr.bool(mandatory = True)},
    config_settings = {
        "//command_line_option:collect_code_coverage": True,
        "//command_line_option:instrumentation_filter": "^//tests/bazel/native_coverage[/:]",
    },
)
