import copy
import unittest
from gate_errors import GateError
from rust_generics import generics
from rust_references import RustDocument
from rustdoc_fixture import document, path

def generic_body():
    lifetime = {"name": "'a", "kind": {"lifetime": {"outlives": ["'static"]}}}
    bound = {"trait_bound": {"trait": path(9)["resolved_path"], "generic_params": [lifetime], "modifier": "none"}}
    return {"params": [lifetime, {"name": "T", "kind": {"type": {"bounds": [bound], "default": path(9), "is_synthetic": False}}},
                       {"name": "N", "kind": {"const": {"type": {"primitive": "usize"}, "default": None}}}],
            "where_predicates": [{"bound_predicate": {"type": {"generic": "T"}, "bounds": [bound], "generic_params": [lifetime]}},
                                 {"lifetime_predicate": {"lifetime": "'a", "outlives": ["'static"]}},
                                 {"eq_predicate": {"lhs": {"tuple": [path(9)]}, "rhs": {"type": {"slice": path(9)}}}},
                                 {"eq_predicate": {"lhs": {"generic": "N"}, "rhs": {"constant": {"expr": "2", "value": "2", "is_literal": True}}}}]}

class PredicateSchemaTests(unittest.TestCase):
    def setUp(self):
        self.r = RustDocument(document(), {"9": "Public"})

    def test_all_predicates_preserve_types_bounds_scopes_and_constants(self):
        value = generic_body()
        result = generics(self.r, value)
        self.assertEqual(result, self.r.normalize(value))
        self.assertEqual(result["where_predicates"][2]["eq_predicate"]["lhs"]["tuple"][0]["resolved_path"]["path"], "Public")
        self.assertEqual(result["where_predicates"][3], value["where_predicates"][3])

    def test_nested_predicate_and_parameter_types_are_not_passed_through(self):
        for field in ("lhs", "rhs"):
            for bad in ({"future_type": {}}, {"function_pointer": {}}, {"tuple": [{"slice": {"future_type": {}}}]}):
                value = generic_body()
                value["where_predicates"][2]["eq_predicate"][field] = bad if field == "lhs" else {"type": bad}
                with self.subTest(field=field, bad=bad), self.assertRaises(GateError):
                    generics(self.r, value)
        for bad in (None, {}, {"future_predicate": {}}):
            value = generic_body()
            value["where_predicates"] = [bad]
            with self.subTest(bad=bad), self.assertRaises(GateError):
                generics(self.r, value)
