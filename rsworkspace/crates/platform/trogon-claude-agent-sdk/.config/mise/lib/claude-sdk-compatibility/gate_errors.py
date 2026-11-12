from enum import StrEnum
from functools import wraps


class Code(StrEnum):
    FORMAT = "unsupported-format"
    SHAPE = "shape-changed"
    MAPPING = "mapping-coverage"
    REFERENCE = "stale-reference"
    CONTRACT = "contract-changed"
    PIN = "source-pin-mismatch"
    EXTRACTION = "upstream-extraction-failed"


class GateError(ValueError):
    def __init__(self, code: Code, selector: str):
        self.code, self.selector = code, selector
        super().__init__(f"{code}: {selector}")


def format_boundary(selector):
    def decorate(function):
        @wraps(function)
        def checked(*args, **kwargs):
            try:
                return function(*args, **kwargs)
            except (KeyError, TypeError, IndexError, AttributeError, RecursionError) as error:
                raise GateError(Code.FORMAT, selector) from error
        return checked
    return decorate
