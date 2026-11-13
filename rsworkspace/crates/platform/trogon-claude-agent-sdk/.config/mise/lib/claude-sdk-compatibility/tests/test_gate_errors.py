import unittest

from gate_errors import Code, GateError


class ErrorProtocolTests(unittest.TestCase):
    def test_error_retains_typed_code_and_selector(self):
        error = GateError(Code.FORMAT, "rustdoc")
        self.assertEqual(getattr(error, "code", None), Code.FORMAT)
        self.assertEqual(error.selector, "rustdoc")
        self.assertEqual(str(error), "unsupported-format: rustdoc")

    def test_codes_are_stable_wire_values(self):
        self.assertEqual([str(code) for code in Code], [
            "unsupported-format", "shape-changed", "mapping-coverage",
            "stale-reference", "contract-changed", "source-pin-mismatch",
            "upstream-extraction-failed",
        ])
