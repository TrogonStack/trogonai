from gate_errors import Code, GateError


def require(condition, selector):
    if not condition:
        raise GateError(Code.FORMAT, selector)


class RustDocument:
    def __init__(self, document, aliases=None):
        require(isinstance(document, dict) and type(document.get("format_version")) is int
                and document["format_version"] == 57, "rustdoc")
        self.document = document
        self.aliases = aliases or {}
        require(isinstance(document.get("index"), dict) and isinstance(document.get("paths"), dict), "rustdoc-index")
        root = self.item(document.get("root"))
        require(root.get("name") == "trogon_claude_agent_sdk" and "module" in root["inner"], "crate")

    def qualified(self, identifier):
        require(type(identifier) is int and identifier >= 0, "rust-id")
        key = str(identifier)
        if key in self.aliases:
            return self.aliases[key]
        entry = self.document["paths"].get(key)
        require(isinstance(entry, dict) and type(entry.get("crate_id")) is int
                and entry["crate_id"] > 0, "rust-type-path")
        segments = entry.get("path")
        require(isinstance(segments, list) and segments and all(isinstance(s, str) and s for s in segments), "rust-type-path")
        return "::".join(segments)

    def item(self, identifier):
        require(type(identifier) is int and identifier >= 0, "rust-id")
        item = self.document["index"].get(str(identifier))
        require(isinstance(item, dict) and type(item.get("id")) is int and item["id"] == identifier
                and isinstance(item.get("inner"), dict) and len(item["inner"]) == 1, "rust-node")
        return item
