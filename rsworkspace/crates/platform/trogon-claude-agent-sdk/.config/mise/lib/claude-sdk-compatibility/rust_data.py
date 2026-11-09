from gate_errors import Code, GateError
from rust_references import require


def generics(reader, body):
    require(isinstance(body, dict) and set(body) == {"params", "where_predicates"}, "rust-generics")
    require(isinstance(body["params"], list) and isinstance(body["where_predicates"], list), "rust-generics")
    for param in body["params"]:
        require(set(param) == {"name", "kind"} and len(param["kind"]) == 1, "rust-generic-parameter")
        kind, value = next(iter(param["kind"].items()))
        require(kind in ("type", "const", "lifetime"), "rust-generic-parameter")
        if kind == "type" and value["default"] is not None:
            reader.type(value["default"])
        if kind == "const":
            reader.type(value["type"])
    for predicate in body["where_predicates"]:
        require(len(predicate) == 1 and next(iter(predicate)) in
                ("bound_predicate", "lifetime_predicate", "eq_predicate"), "rust-where-predicate")
    return reader.normalize(body)


def fields(reader, identifiers, public_only=False):
    result = {}
    for identifier in identifiers:
        item = reader.item(identifier)
        require(set(item["inner"]) == {"struct_field"}, "rust-field")
        if public_only and item["visibility"] != "public":
            continue
        name = item["name"]
        require(isinstance(name, str) and name not in result, "rust-field-name")
        result[name] = reader.type(item["inner"]["struct_field"])
    return result


def tuple_fields(reader, identifiers):
    result = []
    for identifier in identifiers:
        if identifier is None:
            result.append(None)
        else:
            item = reader.item(identifier)
            require(set(item["inner"]) == {"struct_field"}, "rust-field")
            result.append(reader.type(item["inner"]["struct_field"]))
    return result


def data_shape(reader, name, kind, body):
    result = {"kind": kind}
    if kind == "module":
        return result
    if kind in ("type_alias", "constant", "static"):
        reader.type(body["type"])
        if kind == "type_alias":
            generics(reader, body["generics"])
        result["shape"] = reader.normalize(body)
        return result
    if kind != "struct":
        raise GateError(Code.FORMAT, "rust-item:" + kind)
    result["generics"] = generics(reader, body["generics"])
    shape = body["kind"]
    if shape == "unit":
        result["struct_kind"] = "unit"
    elif isinstance(shape, dict) and set(shape) == {"plain"}:
        plain = shape["plain"]
        require(type(plain["has_stripped_fields"]) is bool, name)
        result.update(struct_kind="plain", has_stripped_fields=plain["has_stripped_fields"],
                      fields=fields(reader, plain["fields"], public_only=True))
        require(name != "ClaudeAgentOptions" or not plain["has_stripped_fields"], name)
    elif isinstance(shape, dict) and set(shape) == {"tuple"}:
        result.update(struct_kind="tuple", tuple_fields=tuple_fields(reader, shape["tuple"]))
    else:
        raise GateError(Code.FORMAT, name)
    require(name != "ClaudeAgentOptions" or result["struct_kind"] == "plain", name)
    return result
