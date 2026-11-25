from gate_errors import Code, GateError


def schema(value, keys, selector):
    if not isinstance(value, dict) or set(value) != set(keys):
        raise GateError(Code.FORMAT, selector)
    return value


def tagged(value, kinds, selector):
    if not isinstance(value, dict) or len(value) != 1 or next(iter(value)) not in kinds:
        raise GateError(Code.FORMAT, selector)
    return next(iter(value.items()))


def text(value, nullable=False):
    return isinstance(value, str) or (nullable and value is None)


def texts(value):
    return isinstance(value, list) and all(text(v) for v in value)
