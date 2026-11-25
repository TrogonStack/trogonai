from gate_errors import Code, GateError
from rust_references import require
from rust_generics import generics
from rust_fields import fields, tuple_fields
from rust_values import discriminant, value_item


def data_shape(reader, name, kind, body):
    result = {"kind": kind}
    if kind == "module":
        return result
    if kind in ("type_alias", "constant", "static"):
        reader.type(body["type"])
        if kind == "type_alias":
            generics(reader, body["generics"])
        else:
            value_item(reader, kind, body)
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


def enum_shapes(reader, name, body):
    require(body["has_stripped_variants"] is False, name)
    require(isinstance(body["variants"], list), "rust-variants")
    result = {name: {"kind": "enum", "generics": generics(reader, body["generics"]), "variants": []}}
    for identifier in body["variants"]:
        variant = reader.item(identifier)
        require(set(variant["inner"]) == {"variant"}, name)
        key = name + "::" + variant["name"]
        require(key not in result, "duplicate-variant:" + key)
        result[name]["variants"].append(variant["name"])
        value = variant["inner"]["variant"]
        kind = value["kind"]
        if kind == "plain":
            payload = "plain"
        elif isinstance(kind, dict) and set(kind) == {"tuple"}:
            require(all(i is not None for i in kind["tuple"]), key)
            payload = tuple_fields(reader, kind["tuple"])
        elif isinstance(kind, dict) and set(kind) == {"struct"}:
            require(kind["struct"]["has_stripped_fields"] is False, key)
            payload = fields(reader, kind["struct"]["fields"])
        else:
            raise GateError(Code.FORMAT, key)
        result[key] = {"kind": "variant", "attributes": reader.attributes(variant),
                       "payload": payload, "discriminant": discriminant(value["discriminant"])}
    return result


