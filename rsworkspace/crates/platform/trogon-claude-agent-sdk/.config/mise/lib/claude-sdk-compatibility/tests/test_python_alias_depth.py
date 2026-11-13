import unittest

from gate_errors import Code, GateError
from test_python_alias_operators import read_alias


class AliasDepthTests(unittest.TestCase):
    def test_bounded_deep_unions_keep_typed_refusals(self):
        for size in (500, 1500):
            with self.subTest(size=size):
                try:
                    shape = read_alias(" | ".join(["str"] * size))["Message"]
                except Exception as error:
                    self.assertIsInstance(error, GateError)
                    self.assertIn(error.code, (Code.FORMAT, Code.EXTRACTION))
                else:
                    self.assertEqual(shape, {"kind": "alias", "annotation": " | ".join(["str"] * size)})
