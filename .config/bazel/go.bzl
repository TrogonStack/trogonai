"""Checks for the Go workspace that rules_go does not provide."""

def _runfile_path(file):
    if file.short_path.startswith("../"):
        return "$TEST_SRCDIR/" + file.short_path[3:]
    return "$TEST_SRCDIR/$TEST_WORKSPACE/" + file.short_path

def _gofmt_test_impl(ctx):
    sdk = ctx.toolchains["@rules_go//go:toolchain"].sdk
    gofmt = [tool for tool in sdk.tools.to_list() if tool.basename in ("gofmt", "gofmt.exe")]
    if len(gofmt) != 1:
        fail("The Go SDK provides no gofmt")

    launcher = ctx.actions.declare_file(ctx.label.name + ".sh")
    ctx.actions.write(
        launcher,
        """#!/usr/bin/env bash
set -euo pipefail
unformatted="$("{gofmt}" -l {srcs})"
if [ -n "$unformatted" ]; then
  echo "gofmt found unformatted files:" >&2
  echo "$unformatted" >&2
  exit 1
fi
""".format(
            gofmt = _runfile_path(gofmt[0]),
            srcs = " ".join([src.short_path for src in ctx.files.srcs]),
        ),
        is_executable = True,
    )
    return [DefaultInfo(
        executable = launcher,
        runfiles = ctx.runfiles(files = gofmt + ctx.files.srcs),
    )]

gofmt_test = rule(
    implementation = _gofmt_test_impl,
    doc = "Fails when gofmt from the registered Go SDK would reformat any of srcs.",
    test = True,
    attrs = {
        "srcs": attr.label_list(allow_files = [".go"], mandatory = True),
    },
    toolchains = ["@rules_go//go:toolchain"],
)
