from gate_errors import Code, GateError
from rust_scalars import type_scalars


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

    def path(self, value):
        require(isinstance(value, dict) and set(value) == {"path", "id", "args"}, "rust-path")
        require(value["args"] is None, "rust-generics")
        return {"path": self.qualified(value["id"]), "args": None}

    def normalize(self, value):
        if isinstance(value, list):
            return [self.normalize(v) for v in value]
        if not isinstance(value, dict):
            return value
        if "id" in value:
            return self.path(value)
        if "resolved_path" in value:
            return self.type(value)
        return {key: self.normalize(v) for key, v in value.items()}

    def type(self, value):
        require(isinstance(value, dict) and len(value) == 1, "rust-type")
        kind, body = next(iter(value.items()))
        if kind == "resolved_path":
            return {kind: self.path(body)}
        if kind in ("primitive", "generic"):
            require(isinstance(body, str) and body, "rust-type")
            return {kind: body}
        if kind == "tuple":
            require(isinstance(body, list), "rust-tuple")
            return {kind: [self.type(v) for v in body]}
        if kind == "slice":
            return {kind: self.type(body)}
        fields = {"borrowed_ref": {"lifetime", "is_mutable", "type"},
                  "raw_pointer": {"is_mutable", "type"}, "array": {"type", "len"}}
        if kind in fields:
            require(isinstance(body, dict) and set(body) == fields[kind], "rust-type")
            type_scalars(kind, body)
            return {kind: {**body, "type": self.type(body["type"])}}
        # Other type forms require an explicit reader extension before comparison.
        raise GateError(Code.FORMAT, "rust-type:" + kind)

