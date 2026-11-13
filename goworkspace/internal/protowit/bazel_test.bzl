def _runfile_path(file):
    if file.short_path.startswith("../"):
        return "$TEST_SRCDIR/" + file.short_path[3:]
    return "$TEST_SRCDIR/$TEST_WORKSPACE/" + file.short_path

def _protowit_test_impl(ctx):
    buf_config = ctx.actions.declare_file(ctx.label.name + ".buf.yaml")
    # These fixture schemas have no imports. A scoped module keeps Buf offline
    # without pulling in unrelated repository dependencies from the BSR.
    ctx.actions.write(buf_config, "version: v2\nmodules:\n  - path: proto\n")

    launcher = ctx.actions.declare_file(ctx.label.name + ".sh")
    ctx.actions.write(
        launcher,
        """#!/usr/bin/env bash
set -euo pipefail
buf_binary="%s"
export PATH="${buf_binary%%/*}:$PATH"
export BUF_CACHE_DIR="$TEST_TMPDIR/buf"
export GO_TEST_RUN_FROM_BAZEL=1
exec "%s" "$@"
""" % (
            _runfile_path(ctx.executable.buf),
            _runfile_path(ctx.executable.test_binary),
        ),
        is_executable = True,
    )

    runfiles = ctx.runfiles(
        files = [ctx.executable.buf, ctx.executable.test_binary, ctx.file.go_mod] + ctx.files.schemas,
        symlinks = {"buf.yaml": buf_config},
    )
    runfiles = runfiles.merge(ctx.attr.buf[DefaultInfo].default_runfiles)
    runfiles = runfiles.merge(ctx.attr.test_binary[DefaultInfo].default_runfiles)

    return [
        DefaultInfo(executable = launcher, runfiles = runfiles),
        ctx.attr.test_binary[RunEnvironmentInfo],
        coverage_common.instrumented_files_info(
            ctx,
            dependency_attributes = ["test_binary"],
            extensions = ["go"],
        ),
    ]

protowit_test = rule(
    implementation = _protowit_test_impl,
    test = True,
    attrs = {
        "test_binary": attr.label(executable = True, cfg = "target", mandatory = True),
        "buf": attr.label(executable = True, cfg = "exec", allow_single_file = True, mandatory = True),
        "schemas": attr.label(allow_files = True, mandatory = True),
        "go_mod": attr.label(allow_single_file = True, mandatory = True),
    },
)
