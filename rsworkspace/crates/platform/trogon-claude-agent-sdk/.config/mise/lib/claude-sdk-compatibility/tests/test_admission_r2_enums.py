import unittest
from gate_errors import Code, GateError
from rust_surface import extract
from test_rust_enums import enum_document

class DiscriminantAdmissionTests(unittest.TestCase):
    def test_discriminant_schema_is_admitted_before_normalizing(self):
        for value in ({"future_expression": {}}, {"expr": "2", "value": 2}, {"expr": None, "value": "2"}):
            d = enum_document()
            d["index"]["2"]["inner"]["variant"]["discriminant"] = value
            with self.subTest(value=value):
                with self.assertRaises(GateError) as caught:
                    extract(d)
                self.assertEqual(caught.exception.code, Code.FORMAT)
