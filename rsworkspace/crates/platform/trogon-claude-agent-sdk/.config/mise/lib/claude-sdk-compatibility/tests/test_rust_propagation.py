import unittest
from gate_errors import Code, GateError
from rust_surface import extract
from rustdoc_fixture import add, document, generics, node, path
from test_rust_generic_schema import generic_body

class ExtractionPropagationTests(unittest.TestCase):
    def test_all_generic_scopes_remain_supported_at_extract(self):
        d = document()
        add(d, node(9, "Public", "struct", {"kind": "unit", "generics": generics(), "impls": []}))
        body = {"type": {"primitive": "u8"}, "generics": generic_body()}
        add(d, node(1, "Value", "type_alias", body))
        shape = extract(d)["Value"]["shape"]
        self.assertEqual(shape["generics"]["params"][2], body["generics"]["params"][2])
        self.assertEqual(shape["generics"]["where_predicates"][2]["eq_predicate"]["lhs"]["tuple"][0]["resolved_path"]["path"], "Public")

    def test_identity_and_scalar_rejections_propagate_to_extract(self):
        for value, entry in [(path("9"), {"crate_id": 2}), (path(9), {}), (path(9), {"crate_id": 0}),
                             ({"borrowed_ref": {"lifetime": None, "is_mutable": 0, "type": {"primitive": "u8"}}}, {})]:
            d = document()
            d["paths"]["9"] = {"path": ["alloc", "vec", "Vec"], "kind": "struct", **entry}
            add(d, node(1, "Value", "type_alias", {"type": value, "generics": generics()}))
            with self.subTest(value=value, entry=entry):
                with self.assertRaises(GateError) as caught:
                    extract(d)
                self.assertEqual(caught.exception.code, Code.FORMAT)

    def test_empty_containers_and_both_exact_mutability_values_remain_valid(self):
        d = document()
        for i, shape in enumerate(({"plain": {"fields": [], "has_stripped_fields": False}}, {"tuple": []}), 1):
            add(d, node(i, "Empty" + str(i), "struct", {"kind": shape, "generics": generics(), "impls": []}))
        add(d, node(3, "EmptyEnum", "enum", {"generics": generics(), "has_stripped_variants": False, "variants": [], "impls": []}))
        result = extract(d)
        self.assertEqual(result["Empty1"]["fields"], {})
        self.assertEqual(result["Empty2"]["tuple_fields"], [])
        self.assertEqual(result["EmptyEnum"]["variants"], [])
