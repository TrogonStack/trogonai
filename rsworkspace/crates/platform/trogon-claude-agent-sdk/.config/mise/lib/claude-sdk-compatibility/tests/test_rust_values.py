import unittest
from gate_errors import Code, GateError
from rust_references import RustDocument
from rust_values import constant, discriminant, value_item
from rustdoc_fixture import document

class ValueSchemaTests(unittest.TestCase):
    def test_constant_optional_value_and_discriminants_are_retained(self):
        for value in (None, "42"):
            body = {"expr": "N + 1", "value": value, "is_literal": False}
            self.assertEqual(constant(body), body)
            item = {"type": {"primitive": "u8"}, "const": body}
            self.assertEqual(value_item(RustDocument(document()), "constant", item), item)
        self.assertIsNone(discriminant(None))
        self.assertEqual(discriminant({"expr": "2", "value": "2"}), {"expr": "2", "value": "2"})
        item = {"type": {"primitive": "u8"}, "expr": "1", "is_mutable": True, "is_unsafe": False}
        self.assertEqual(value_item(RustDocument(document()), "static", item), item)

    def test_malformed_values_and_unknown_schemas_are_typed(self):
        for action in (lambda: constant(None), lambda: constant({"expr": "1", "value": None, "is_literal": 0}),
                       lambda: discriminant({"future_expression": {}}), lambda: discriminant({"expr": "1", "value": 1}),
                       lambda: value_item(RustDocument(document()), "static", {"type": {"primitive": "u8"}, "expr": None, "is_mutable": False, "is_unsafe": False})):
            with self.subTest(action=action):
                with self.assertRaises(GateError) as caught:
                    action()
                self.assertEqual(caught.exception.code, Code.FORMAT)
