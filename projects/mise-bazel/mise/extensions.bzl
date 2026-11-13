"""Module extension that exposes every tool pinned in a mise configuration."""

load("@toml.bzl", "toml")
load(":repositories.bzl", "mise_go", "mise_hub", "mise_node", "mise_rust", "mise_tool")

def _pin(tool, pin):
    if type(pin) == "list":
        pin = pin[0] if pin else None
    if type(pin) == "string":
        pin = {"version": pin}
    if type(pin) != "dict" or type(pin.get("version")) != "string":
        fail("{} has no pinned version".format(tool))

    # Backend tools such as `ubi:owner/name` install an executable named after the project.
    return pin["version"], pin.get("exe", tool.split(":")[-1].split("/")[-1])

def _repository(tool):
    return "mise_" + "".join([c if c.isalnum() else "_" for c in tool.elems()])

def _mise_impl(mctx):
    configs = [tag.file for module in mctx.modules if module.is_root for tag in module.tags.config]
    if len(configs) != 1:
        fail("The root module must call mise.config exactly once")
    executables = {}
    for tool, pin in toml.decode(mctx.read(configs[0])).get("tools", {}).items():
        version, executable = _pin(tool, pin)
        name = _repository(tool)
        if tool == "go":
            mise_go(name = name, tool = tool, version = version)
        elif tool == "node":
            mise_node(name = name, tool = tool, version = version)
        elif tool == "rust":
            mise_rust(name = name, tool = tool, version = version)
        else:
            mise_tool(name = name, tool = tool, version = version, executable = executable)
            executables[executable] = name
    mise_hub(name = "mise", tools = executables)

mise = module_extension(
    implementation = _mise_impl,
    doc = "Creates one lazily fetched repository per tool pinned in the mise configuration.",
    tag_classes = {
        "config": tag_class(attrs = {"file": attr.label(mandatory = True, allow_single_file = True)}),
    },
)
