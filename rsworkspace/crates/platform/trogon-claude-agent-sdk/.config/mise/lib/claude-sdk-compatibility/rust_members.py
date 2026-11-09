from gate_errors import Code, GateError
from rust_data import generics
from rust_references import require


def function(reader, body):
    signature = body["sig"]
    require(set(signature) == {"inputs", "output", "is_c_variadic"}, "rust-signature")
    require(type(signature["inputs"]) is list, "rust-function-inputs")
    inputs = []
    for argument in signature["inputs"]:
        require(isinstance(argument, list) and len(argument) == 2 and isinstance(argument[0], str), "rust-argument")
        inputs.append([argument[0], reader.type(argument[1])])
    header = body["header"]
    require(set(header) == {"is_const", "is_unsafe", "is_async", "abi"}, "rust-header")
    require(all(type(header[k]) is bool for k in ("is_const", "is_unsafe", "is_async")), "rust-header")
    require(type(body["has_body"]) is bool and type(signature["is_c_variadic"]) is bool, "rust-function")
    return {"kind": "function", "header": reader.normalize(header), "has_body": body["has_body"],
            "signature": {"inputs": inputs, "output": None if signature["output"] is None else reader.type(signature["output"]),
                          "is_c_variadic": signature["is_c_variadic"]},
            "generics": generics(reader, body["generics"])}


def member_shape(reader, item):
    attributes = reader.attributes(item)
    metadata = {"attributes": attributes} if attributes else {}
    kind, body = next(iter(item["inner"].items()))
    if kind == "function":
        return {**function(reader, body), **metadata}
    if kind in ("assoc_const", "assoc_type"):
        if body["type"] is not None:
            reader.type(body["type"])
        if kind == "assoc_type":
            generics(reader, body["generics"])
        return {"kind": kind, "shape": reader.normalize(body), **metadata}
    raise GateError(Code.FORMAT, "rust-member:" + kind)


def inherent_members(reader, name, body):
    result = {}
    for identifier in body["impls"]:
        item = reader.item(identifier)
        require(set(item["inner"]) == {"impl"}, "rust-impl")
        impl = item["inner"]["impl"]
        if impl["trait"] is not None:
            continue
        context = {"impl_generics": generics(reader, impl["generics"]),
                   "impl_for": reader.type(impl["for"]), "impl_is_unsafe": impl["is_unsafe"]}
        for member_id in impl["items"]:
            member = reader.item(member_id)
            if member["visibility"] != "public":
                continue
            key = name + "::" + member["name"]
            require(key not in result, "duplicate-member:" + key)
            result[key] = {**member_shape(reader, member), **context}
    return result
