from rust_references import require
from rust_schema import schema, tagged, text, texts


def parameters(reader, values):
    require(isinstance(values, list), "rust-generic-parameters")
    for param in values:
        schema(param, ("name", "kind"), "rust-generic-parameter")
        require(text(param["name"]), "rust-generic-parameter")
        kind, body = tagged(param["kind"], ("type", "const", "lifetime"), "rust-generic-parameter")
        if kind == "type":
            schema(body, ("bounds", "default", "is_synthetic"), "rust-type-parameter")
            require(type(body["is_synthetic"]) is bool, "rust-type-parameter")
            bounds(reader, body["bounds"])
            if body["default"] is not None:
                reader.type(body["default"])
        elif kind == "const":
            schema(body, ("type", "default"), "rust-const-parameter")
            reader.type(body["type"])
            require(text(body["default"], nullable=True), "rust-const-parameter")
        else:
            schema(body, ("outlives",), "rust-lifetime-parameter")
            require(texts(body["outlives"]), "rust-lifetime-parameter")
    return reader.normalize(values)


def bounds(reader, values):
    require(isinstance(values, list), "rust-bounds")
    for bound in values:
        kind, body = tagged(bound, ("trait_bound", "outlives", "use"), "rust-bound")
        if kind == "trait_bound":
            schema(body, ("trait", "generic_params", "modifier"), "rust-trait-bound")
            require(body["modifier"] in ("none", "maybe", "maybe_const"), "rust-trait-bound")
            reader.path(body["trait"])
            parameters(reader, body["generic_params"])
        elif kind == "outlives":
            require(text(body), "rust-outlives")
        else:
            require(isinstance(body, list), "rust-captures")
            for capture in body:
                _, value = tagged(capture, ("lifetime", "param"), "rust-capture")
                require(text(value), "rust-capture")
    return reader.normalize(values)
