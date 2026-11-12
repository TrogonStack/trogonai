from enum import StrEnum


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
