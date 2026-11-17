import unittest

from gate_errors import GateError
from rust_surface import extract
from rust_references import RustDocument
from rustdoc_fixture import add, document, node, path


def generic_document():
    doc = document()
    doc["paths"]["9"] = {"crate_id": 1, "path": ["core", "fmt", "Display"], "kind": "trait"}
    bound = {"trait_bound": {"trait": path(9)["resolved_path"], "generic_params": [], "modifier": "none"}}
    params = {"params": [
        {"name": "'a", "kind": {"lifetime": {"outlives": ["'static"]}}},
        {"name": "T", "kind": {"type": {"bounds": [bound], "default": None, "is_synthetic": False}}},
        {"name": "N", "kind": {"const": {"type": {"primitive": "usize"}, "default": "2"}}},
    ], "where_predicates": [{"bound_predicate": {"type": {"generic": "T"}, "bounds": [bound], "generic_params": []}}]}
    add(doc, node(1, "Generic", "struct", {"kind": "unit", "generics": params, "impls": []}))
    return doc


class GenericTests(unittest.TestCase):
    def test_lifetime_const_and_where_bounds_have_canonical_paths(self):
        result = extract(generic_document())["Generic"]["generics"]
        self.assertEqual(result["params"][0]["kind"]["lifetime"]["outlives"], ["'static"])
        self.assertEqual(result["params"][2]["kind"]["const"]["default"], "2")
        bound = result["where_predicates"][0]["bound_predicate"]["bounds"][0]
        self.assertEqual(bound["trait_bound"]["trait"], {"path": "core::fmt::Display", "args": None})

    def test_unknown_generic_bounds_and_missing_external_crate_identity_fail_closed(self):
        for mutation in (lambda d: d["paths"]["9"].pop("crate_id"),
                         lambda d: d["index"]["1"]["inner"]["struct"]["generics"]["params"][1]["kind"]["type"].update(bounds=[{"future_bound": {}}]),
                         lambda d: d["index"]["1"]["inner"]["struct"]["generics"]["where_predicates"][0]["bound_predicate"].update(type={"future_type": {}})):
            doc = generic_document()
            mutation(doc)
            with self.assertRaises(GateError):
                extract(doc)

    def test_invalid_lifetime_const_and_infer_arguments_fail_closed(self):
        for arg in ({"lifetime": 1}, {"const": {}}, {"infer": "invented"}):
            doc = generic_document()
            value = path(9, {"angle_bracketed": {"args": [arg], "constraints": []}})
            with self.assertRaises(GateError):
                RustDocument(doc).type(value)
