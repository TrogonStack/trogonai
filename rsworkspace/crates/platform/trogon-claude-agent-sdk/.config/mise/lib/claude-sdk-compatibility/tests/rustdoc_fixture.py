def generics():
    return {"params": [], "where_predicates": []}


def document():
    root = node(0, "trogon_claude_agent_sdk", "module", {
        "items": [], "is_crate": True, "is_stripped": False,
    })
    return {"format_version": 57, "root": 0, "index": {"0": root}, "paths": {}}


def node(identifier, name, kind, body, visibility="public"):
    return {"id": identifier, "crate_id": 0, "name": name,
            "visibility": visibility, "attrs": [], "inner": {kind: body}}


def add(doc, item, exported=True):
    doc["index"][str(item["id"])] = item
    if exported:
        doc["index"]["0"]["inner"]["module"]["items"].append(item["id"])
    return item


def path(identifier, arguments=None):
    return {"resolved_path": {"path": "compiler_spelling", "id": identifier, "args": arguments}}


def function(output=None):
    return {"sig": {"inputs": [["self", {"generic": "Self"}]], "output": output, "is_c_variadic": False},
            "generics": generics(), "header": {"is_const": False, "is_unsafe": False, "is_async": True, "abi": "Rust"}, "has_body": True}


def implementation(for_type, items, trait=None):
    return {"is_unsafe": False, "generics": generics(), "provided_trait_methods": [],
            "trait": trait, "for": for_type, "items": items, "is_negative": False,
            "is_synthetic": False, "blanket_impl": None}
