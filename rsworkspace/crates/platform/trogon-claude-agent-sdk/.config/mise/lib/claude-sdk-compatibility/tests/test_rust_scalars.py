import unittest
from gate_errors import Code, GateError
from rust_scalars import type_scalars

class TypeScalarTests(unittest.TestCase):
    def test_supported_scalars(self):
        for kind, body in [("borrowed_ref", {"lifetime": None, "is_mutable": False}),
                           ("borrowed_ref", {"lifetime": "'a", "is_mutable": True}),
                           ("raw_pointer", {"is_mutable": True}), ("array", {"len": "N + 1"})]:
            self.assertIsNone(type_scalars(kind, body))

    def test_exact_booleans_and_scalar_types(self):
        for kind, body in [("borrowed_ref", {"lifetime": None, "is_mutable": 0}),
                           ("borrowed_ref", {"lifetime": False, "is_mutable": False}),
                           ("raw_pointer", {"is_mutable": 1}), ("array", {"len": 4})]:
            with self.subTest(kind=kind):
                with self.assertRaises(GateError) as caught:
                    type_scalars(kind, body)
                self.assertEqual(caught.exception.code, Code.FORMAT)
