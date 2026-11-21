"""Public diagnostics contain only fixed codes, never input or exception text."""

from enum import Enum
import sys


class Reason(Enum):
    INVALID_BOOLEAN = 'invalid-boolean'
    BASELINE_MISSING = 'baseline-missing'
    MANIFEST_BINDING = 'manifest-binding'
    SOURCE_MISMATCH = 'source-mismatch'
    XML_DECLARATION = 'xml-declaration'
    COMPARISON_REQUIRED = 'comparison-required'
    BASELINE_COUNTS = 'baseline-counts'
    MISSES_INCREASED = 'misses-increased'
    NEW_UNCOVERED = 'new-uncovered'
    RATE_DECREASED = 'rate-decreased'
    THRESHOLD = 'threshold-not-met'
    INVALID_INPUT = 'invalid-input'
    INPUT_MISSING = 'input-missing'
    OPERATION_FAILED = 'operation-failed'


class CoverageError(ValueError):
    def __init__(self, reason):
        self.reason = Reason(reason)
        super().__init__(self.reason.value)


def diagnostic(error):
    if isinstance(error, CoverageError):
        reason = error.reason
    elif isinstance(error, FileNotFoundError):
        reason = Reason.INPUT_MISSING
    elif isinstance(error, (ValueError, KeyError, TypeError)):
        reason = Reason.INVALID_INPUT
    else:
        reason = Reason.OPERATION_FAILED
    print('Coverage: ' + reason.value + '.', file=sys.stderr)


def boolean(value):
    normalized = value.lower()
    if normalized not in ('true', 'false'):
        raise CoverageError(Reason.INVALID_BOOLEAN)
    return normalized == 'true'
