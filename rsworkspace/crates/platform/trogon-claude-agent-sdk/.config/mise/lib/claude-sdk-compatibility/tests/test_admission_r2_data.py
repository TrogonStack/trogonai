import unittest
from gate_errors import Code, GateError
from rust_surface import extract
from rustdoc_fixture import add, document, generics, node

class GenericSchemaAdmissionTests(unittest.TestCase):
    def test_predicates_recursively_reject_unsupported_types(self):
        for value in ({"future_type": {}}, {"function_pointer": {}}, {"tuple": [{"slice": {"future_type": {}}}]}):
            g = generics()
            g["where_predicates"] = [{"eq_predicate": {"lhs": value, "rhs": {"type": {"primitive": "u8"}}}}]
            d = document()
            add(d, node(1, "Value", "struct", {"kind": "unit", "generics": g, "impls": []}))
            with self.subTest(value=value):
                with self.assertRaises(GateError) as caught:
                    extract(d)
                self.assertEqual(caught.exception.code, Code.FORMAT)

    def test_constant_and_static_schemas_are_admitted_before_normalizing(self):
        bodies = [("constant", {"const": None}),
                  ("constant", {"const": {"expr": "1", "value": None, "is_literal": 0}}),
                  ("static", {"expr": None, "is_mutable": False, "is_unsafe": False}),
                  ("static", {"expr": "1", "is_mutable": 0, "is_unsafe": False})]
        for kind, body in bodies:
            d = document()
            add(d, node(1, "VALUE", kind, {"type": {"primitive": "u8"}, **body}))
            with self.subTest(kind=kind, body=body):
                with self.assertRaises(GateError) as caught:
                    extract(d)
                self.assertEqual(caught.exception.code, Code.FORMAT)
