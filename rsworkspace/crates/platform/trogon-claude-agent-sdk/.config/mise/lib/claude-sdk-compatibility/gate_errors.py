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
    pass
