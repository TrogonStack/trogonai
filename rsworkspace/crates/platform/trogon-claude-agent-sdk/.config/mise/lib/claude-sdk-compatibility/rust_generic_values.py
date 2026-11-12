from rust_bounds import bounds
from rust_references import require
from rust_schema import schema, tagged, text
from rust_values import constant


def term(reader, value):
    kind, body = tagged(value, ("type", "constant"), "rust-term")
    if kind == "type":
        reader.type(body)
    else:
        constant(body)
    return reader.normalize(value)


def argument(reader, value):
    if isinstance(value, str) and value == "infer":
        return "infer"
    kind, body = tagged(value, ("type", "lifetime", "const"), "rust-generic-argument")
    if kind == "type":
        reader.type(body)
    elif kind == "const":
        constant(body)
    elif kind == "lifetime":
        require(text(body), "rust-lifetime-argument")
    return reader.normalize(value)


def associated(reader, value):
    schema(value, ("name", "args", "binding"), "rust-associated-constraint")
    require(text(value["name"]), "rust-associated-constraint")
    reader.arguments(value["args"])
    kind, body = tagged(value["binding"], ("equality", "constraint"), "rust-associated-binding")
    if kind == "equality":
        term(reader, body)
    else:
        bounds(reader, body)
    return reader.normalize(value)
