from rust_references import require
from rust_schema import schema, text


def constant(value):
    schema(value, ("expr", "value", "is_literal"), "rust-constant")
    require(text(value["expr"]) and text(value["value"], nullable=True)
            and type(value["is_literal"]) is bool, "rust-constant")
    return value


def discriminant(value):
    if value is not None:
        schema(value, ("expr", "value"), "rust-discriminant")
        require(text(value["expr"]) and text(value["value"]), "rust-discriminant")
    return value


def value_item(reader, kind, body):
    if kind == "constant":
        schema(body, ("type", "const"), "rust-constant")
        constant(body["const"])
    else:
        schema(body, ("type", "expr", "is_mutable", "is_unsafe"), "rust-static")
        require(text(body["expr"]) and type(body["is_mutable"]) is bool
                and type(body["is_unsafe"]) is bool, "rust-static")
    reader.type(body["type"])
    return reader.normalize(body)
