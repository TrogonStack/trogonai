"""Validate the shared mise pins before exposing Bazel's Buf executable."""

load("@toml.bzl", "toml")

_TOOLS = ["bazel", "buf", "go", "rust"]
_BUF_PLATFORMS = ["macos-arm64", "linux-x64"]

def _invalid(message):
    fail("%s. Update .config/mise/config.toml as needed, then run `mise run --skip-tools bazel:lock`." % message)

def _tools(document, source):
    tools = document.get("tools")
    if type(tools) != "dict":
        _invalid("%s must contain a tools table" % source)
    return tools

def _version(value, tool):
    if type(value) == "dict":
        value = value.get("version")
    if type(value) == "list":
        if not value:
            _invalid("mise.toml tools.%s must pin at least one version" % tool)
        value = value[0]
    if type(value) == "dict":
        value = value.get("version")
    if type(value) != "string":
        _invalid("mise.toml tools.%s must pin one exact version" % tool)
    parts = value.split(".")
    if len(parts) != 3 or any([not part or part.strip("0123456789") for part in parts]):
        _invalid("mise.toml tools.%s must pin an exact major.minor.patch version" % tool)
    return value

def _locked_tool(tools, tool, version):
    entries = tools.get(tool)
    if type(entries) != "list" or not entries:
        _invalid("mise.lock must contain [[tools.%s]] entries" % tool)
    selected = None
    seen = {}
    for entry in entries:
        if type(entry) != "dict" or type(entry.get("version")) != "string":
            _invalid("mise.lock tools.%s entries must name their versions" % tool)
        entry_version = entry["version"]
        if entry_version in seen:
            _invalid("mise.lock tools.%s contains duplicate version %s" % (tool, entry_version))
        seen[entry_version] = True
        if entry_version == version:
            selected = entry
    if selected == None:
        _invalid("mise.lock tools.%s must include the primary mise.toml pin %s" % (tool, version))
    entry = selected
    if "specifiers" in entry and entry["specifiers"] != [version]:
        _invalid("mise.lock tools.%s must bind the mise.toml pin %s" % (tool, version))
    if type(entry.get("backend")) != "string" or not entry["backend"]:
        _invalid("mise.lock tools.%s must name its backend" % tool)
    return entry

def _validate_buf(entry):
    for platform in _BUF_PLATFORMS:
        data = entry.get("platforms." + platform)
        if type(data) != "dict":
            _invalid("mise.lock Buf must include platform %s" % platform)
        url = data.get("url")
        if type(url) != "string" or not url.startswith("https://") or len(url) <= len("https://"):
            _invalid("mise.lock Buf platform %s must include an HTTPS download URL" % platform)
        checksum = data.get("checksum")
        if type(checksum) != "string" or not checksum.startswith("sha256:"):
            _invalid("mise.lock Buf platform %s must include a SHA256 checksum" % platform)
        digest = checksum[len("sha256:"):]
        if len(digest) != 64 or digest.strip("0123456789abcdefABCDEF"):
            _invalid("mise.lock Buf platform %s has a malformed SHA256 checksum" % platform)

def _mise_lock_impl(ctx):
    config = toml.decode(ctx.read(ctx.attr.config, watch = "yes"))
    lockfile = toml.decode(ctx.read(ctx.attr.lockfile, watch = "yes"))
    if lockfile.get("lockfile_version") != 3:
        _invalid("mise.lock must use lockfile_version = 3 for CI's pinned mise release")
    requested = _tools(config, "mise.toml")
    locked = _tools(lockfile, "mise.lock")
    for tool in _TOOLS:
        entry = _locked_tool(locked, tool, _version(requested.get(tool), tool))
        if tool == "buf":
            _validate_buf(entry)
    ctx.file(
        "BUILD.bazel",
        """alias(
    name = "buf",
    actual = %s,
    visibility = ["//visibility:public"],
)
""" % repr(str(ctx.attr.tool)),
        executable = False,
    )

mise_lock = repository_rule(
    implementation = _mise_lock_impl,
    attrs = {
        "config": attr.label(mandatory = True, allow_single_file = True),
        "lockfile": attr.label(mandatory = True, allow_single_file = True),
        "tool": attr.label(mandatory = True),
    },
)
