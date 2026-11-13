from gate_errors import Code, GateError


def type_scalars(kind, body):
    if kind in ("borrowed_ref", "raw_pointer") and type(body["is_mutable"]) is not bool:
        raise GateError(Code.FORMAT, "rust-mutability")
    if kind == "borrowed_ref" and body["lifetime"] is not None and not isinstance(body["lifetime"], str):
        raise GateError(Code.FORMAT, "rust-lifetime")
    if kind == "array" and not isinstance(body["len"], str):
        raise GateError(Code.FORMAT, "rust-array-length")
