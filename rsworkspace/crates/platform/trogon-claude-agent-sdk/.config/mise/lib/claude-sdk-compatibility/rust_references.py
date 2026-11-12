from gate_errors import Code, GateError, format_boundary
from rust_scalars import type_scalars


def require(condition, selector):
    if not condition:
        raise GateError(Code.FORMAT, selector)


class RustDocument:
    @format_boundary("rustdoc")
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

    @format_boundary("rust-path")
    def path(self, value):
        require(isinstance(value, dict) and set(value) == {"path", "id", "args"}, "rust-path")
        return {"path": self.qualified(value["id"]), "args": self.arguments(value["args"])}

    @format_boundary("rust-generics")
    def arguments(self, value):
        from rust_generic_values import argument, associated
        if value is None:
            return None
        require(isinstance(value, dict) and len(value) == 1, "rust-generics")
        if "angle_bracketed" in value:
            body = value["angle_bracketed"]
            require(isinstance(body, dict) and set(body) == {"args", "constraints"}
                    and isinstance(body["args"], list) and isinstance(body["constraints"], list), "rust-generics")
            return {"angle_bracketed": {
                "args": [argument(self, arg) for arg in body["args"]],
                "constraints": [associated(self, value) for value in body["constraints"]],
            }}
        if "parenthesized" in value:
            body = value["parenthesized"]
            require(isinstance(body, dict) and set(body) == {"inputs", "output"} and isinstance(body["inputs"], list), "rust-generics")
            return {"parenthesized": {"inputs": [self.type(v) for v in body["inputs"]],
                                      "output": None if body["output"] is None else self.type(body["output"])}}
        raise GateError(Code.FORMAT, "rust-generics")

    def attributes(self, item):
        values = item.get("attrs")
        require(isinstance(values, list), "rust-attributes")
        for value in values:
            if isinstance(value, str):
                require(value in ("non_exhaustive", "macro_export", "automatically_derived", "no_mangle"), "rust-attribute")
                continue
            require(isinstance(value, dict) and len(value) == 1, "rust-attribute")
            kind, body = next(iter(value.items()))
            if kind in ("other", "export_name", "link_section"):
                require(isinstance(body, str), "rust-attribute")
            elif kind == "must_use":
                require(isinstance(body, dict) and set(body) == {"reason"}
                        and (body["reason"] is None or isinstance(body["reason"], str)), "rust-attribute")
            elif kind == "target_feature":
                require(isinstance(body, dict) and set(body) == {"enable"}
                        and isinstance(body["enable"], list)
                        and all(isinstance(v, str) for v in body["enable"]), "rust-attribute")
            elif kind == "repr":
                require(isinstance(body, dict) and set(body) == {"kind", "align", "packed", "int"}
                        and body["kind"] in ("rust", "c", "transparent", "simd")
                        and (body["int"] is None or isinstance(body["int"], str)), "rust-attribute")
                require(all(body[k] is None or (type(body[k]) is int and 0 <= body[k] < 2 ** 64)
                            for k in ("align", "packed")), "rust-attribute")
            else:
                raise GateError(Code.FORMAT, "rust-attribute")
        return values


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

    @format_boundary("rust-type")
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
        if kind == "qualified_path":
            require(isinstance(body, dict) and set(body) == {"name", "args", "self_type", "trait"}
                    and isinstance(body["name"], str), "rust-qualified-path")
            return {kind: {"name": body["name"], "args": self.arguments(body["args"]),
                           "self_type": self.type(body["self_type"]),
                           "trait": None if body["trait"] is None else self.path(body["trait"])}}
        if kind == "impl_trait":
            require(isinstance(body, list), "rust-bounds")
            for bound in body:
                require(isinstance(bound, dict) and len(bound) == 1, "rust-bound")
                if "trait_bound" in bound:
                    trait = bound["trait_bound"]
                    require(set(trait) == {"trait", "generic_params", "modifier"}
                            and trait["modifier"] in ("none", "maybe", "maybe_const"), "rust-bound")
                    self.path(trait["trait"])
                    require(isinstance(trait["generic_params"], list), "rust-bound")
                else:
                    require(set(bound) == {"outlives"} and isinstance(bound["outlives"], str), "rust-bound")
            return {kind: self.normalize(body)}
        fields = {"borrowed_ref": {"lifetime", "is_mutable", "type"},
                  "raw_pointer": {"is_mutable", "type"}, "array": {"type", "len"}}
        if kind in fields:
            require(isinstance(body, dict) and set(body) == fields[kind], "rust-type")
            type_scalars(kind, body)
            return {kind: {**body, "type": self.type(body["type"])}}
        # Other type forms require an explicit reader extension before comparison.
        raise GateError(Code.FORMAT, "rust-type:" + kind)

    def bounds(self, values):
        from rust_bounds import bounds
        return bounds(self, values)

    def is_public(self, item):
        visibility = item.get("visibility")
        if isinstance(visibility, str):
            require(visibility in ("public", "default", "crate"), "rust-visibility")
        else:
            require(isinstance(visibility, dict) and set(visibility) == {"restricted"}
                    and set(visibility["restricted"]) == {"parent", "path"}, "rust-visibility")
        return visibility == "public"

