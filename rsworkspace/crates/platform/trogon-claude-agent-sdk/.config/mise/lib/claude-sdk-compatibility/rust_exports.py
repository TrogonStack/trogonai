from gate_errors import format_boundary
from rust_references import RustDocument, require


@format_boundary("public-exports")
def inventory(document):
    reader = RustDocument(document)
    exported = {}

    def target(node, seen):
        identifier = node["id"]
        require(identifier not in seen, "public-reexport-cycle")
        if "use" not in node["inner"]:
            return node
        use = node["inner"]["use"]
        require(isinstance(use, dict), "public-reexport")
        require(use.get("is_glob") is False and use.get("id") is not None, "public-reexport")
        return target(reader.item(use["id"]), seen | {identifier})

    def visit(module, prefix, ancestors):
        require(module["id"] not in ancestors, "public-module-cycle")
        body = module["inner"]["module"]
        require(isinstance(body, dict), "public-module")
        require(isinstance(body.get("items"), list), "public-module")
        for identifier in body["items"]:
            node = reader.item(identifier)
            if not reader.is_public(node):
                continue
            if "use" in node["inner"]:
                require(isinstance(node["inner"]["use"], dict), "public-reexport")
            name = node["inner"]["use"].get("name") if "use" in node["inner"] else node.get("name")
            require(isinstance(name, str) and name and "::" not in name, "public-name")
            name = prefix + name
            node = target(node, set())
            require(name not in exported, "duplicate-export:" + name)
            exported[name] = node
            if "module" in node["inner"]:
                visit(node, name + "::", ancestors | {module["id"]})

    visit(reader.item(document["root"]), "", set())
    for name in sorted(exported, key=lambda n: (n.count("::"), n)):
        reader.aliases.setdefault(str(exported[name]["id"]), name)
    return reader, exported
