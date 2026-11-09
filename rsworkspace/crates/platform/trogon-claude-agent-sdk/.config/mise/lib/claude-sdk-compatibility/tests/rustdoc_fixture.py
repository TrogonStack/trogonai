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
