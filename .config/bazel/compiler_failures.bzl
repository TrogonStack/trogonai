"""Compiler-failure actions using the pinned rules_rust providers."""

load("@rules_rust//rust:defs.bzl", "rust_common")

def _compiler_failure_impl(ctx):
    toolchain = ctx.toolchains[rust_common.toolchain_type]
    result = ctx.actions.declare_file(ctx.label.name + ".json")
    args = ctx.actions.args()
    args.add("--capture", result)
    args.add("--")
    args.add(toolchain.rustc)
    args.add(ctx.file.src)
    args.add_all([
        "--crate-name=" + ctx.attr.crate_name,
        "--edition=" + ctx.attr.edition,
        "--crate-type=bin",
        "--emit=metadata",
        "--error-format=json",
        "--color=never",
        "--diagnostic-width=140",
        "--cfg=trybuild",
        "-Adead_code",
        "--sysroot=" + toolchain.sysroot,
        "--target=" + toolchain.target_flag_value,
    ])
    args.add("-o", result.path + ".rmeta")
    inputs = [toolchain.all_files]
    outputs = []
    for target in ctx.attr.deps + ctx.attr.proc_macro_deps:
        crate = target[rust_common.crate_info]
        dep = target[rust_common.dep_info]
        args.add("--extern", ctx.attr.aliases.get(target.label, crate.name) + "=" + crate.output.path)
        outputs.append(crate.output)
        inputs.extend([
            dep.transitive_crate_outputs,
            dep.transitive_proc_macro_data,
            crate.compile_data,
            crate.data,
            crate.srcs,
        ])
        for transitive in dep.transitive_crates.to_list():
            inputs.append(transitive.srcs)
    crate_outputs = depset(outputs, transitive = inputs)
    directories = {file.dirname: True for file in crate_outputs.to_list() if file.extension in ["rlib", "rmeta", "so", "dylib", "dll"]}
    args.add_all(sorted(directories.keys()), format_each = "-Ldependency=%s")
    ctx.actions.run(
        executable = ctx.executable._runner,
        arguments = [args],
        inputs = depset([ctx.file.src] + ctx.files.compile_data, transitive = [crate_outputs]),
        outputs = [result],
        env = toolchain.env | {"CARGO_MANIFEST_DIR": ctx.label.package},
        mnemonic = "RustCompilerFailure",
        progress_message = "Compiling failure fixture %{label}",
    )
    return [DefaultInfo(files = depset([result]))]

compiler_failure = rule(
    implementation = _compiler_failure_impl,
    attrs = {
        "src": attr.label(allow_single_file = [".rs"], mandatory = True),
        "crate_name": attr.string(mandatory = True),
        "edition": attr.string(mandatory = True),
        "deps": attr.label_list(providers = [rust_common.crate_info, rust_common.dep_info]),
        "proc_macro_deps": attr.label_list(cfg = "exec", providers = [rust_common.crate_info, rust_common.dep_info]),
        "aliases": attr.label_keyed_string_dict(),
        "compile_data": attr.label_list(allow_files = True),
        "_runner": attr.label(default = Label("//.config/bazel:compiler_failures"), executable = True, cfg = "exec"),
    },
    toolchains = [rust_common.toolchain_type],
)
