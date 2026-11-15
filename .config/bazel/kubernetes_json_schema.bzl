"""Repository rule for the Kubernetes JSON schemas that kubeconform validates against."""

def _kubernetes_json_schema_impl(rctx):
    # Laid out as in yannh/kubernetes-json-schema, so the same kubeconform
    # schema location template works against this copy and the upstream URL.
    directory = "v{}-standalone-strict".format(rctx.attr.kubernetes_version)
    for name, sha256 in rctx.attr.schemas.items():
        rctx.download(
            url = "https://raw.githubusercontent.com/yannh/kubernetes-json-schema/{}/{}/{}.json".format(
                rctx.attr.commit,
                directory,
                name,
            ),
            output = "{}/{}.json".format(directory, name),
            sha256 = sha256,
        )
    rctx.file("COMMIT", rctx.attr.commit + "\n")
    rctx.file("BUILD.bazel", """\
exports_files(["COMMIT"])

filegroup(
    name = "schemas",
    srcs = ["COMMIT"] + glob(["*/*.json"]),
    visibility = ["//visibility:public"],
)
""")

kubernetes_json_schema = repository_rule(
    implementation = _kubernetes_json_schema_impl,
    doc = "Downloads the named strict standalone schemas of one Kubernetes version, pinned by commit and checksum.",
    attrs = {
        "commit": attr.string(mandatory = True),
        "kubernetes_version": attr.string(mandatory = True),
        "schemas": attr.string_dict(
            mandatory = True,
            doc = "Schema file name without .json, as kubeconform names it, to its sha256.",
        ),
    },
)
