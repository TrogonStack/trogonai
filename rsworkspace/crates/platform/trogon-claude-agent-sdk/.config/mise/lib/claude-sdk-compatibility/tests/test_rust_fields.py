import unittest
from gate_errors import GateError
from rust_fields import fields, tuple_fields
from rust_references import RustDocument
from rustdoc_fixture import add, document, node

class FieldSchemaTests(unittest.TestCase):
    def test_named_tuple_empty_and_private_fields(self):
        d = document()
        add(d, node(1, "value", "struct_field", {"primitive": "u8"}), False)
        add(d, node(2, "secret", "struct_field", {"primitive": "bool"}, "default"), False)
        r = RustDocument(d)
        self.assertEqual(fields(r, [1, 2], public_only=True), {"value": {"primitive": "u8"}})
        self.assertEqual(tuple_fields(r, [1, None]), [{"primitive": "u8"}, None])
        self.assertEqual(fields(r, []), {})
        self.assertEqual(tuple_fields(r, []), [])
        for reader in (fields, tuple_fields):
            for value in ({}, "", None, 0):
                with self.subTest(reader=reader, value=value), self.assertRaises(GateError):
                    reader(r, value)
