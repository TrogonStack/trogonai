import unittest

from gate_errors import Code, GateError
from rust_surface import extract
from rustdoc_fixture import add, document, generics, node, path


def alias_document(value):
    doc = document()
    doc["paths"]["9"] = {"crate_id": 1, "path": ["alloc", "vec", "Vec"], "kind": "struct"}
    add(doc, node(1, "Opaque", "type_alias", {"type": value, "generics": generics()}))
    return doc


class PublicReferenceFormTests(unittest.TestCase):
    def test_impl_trait_bounds_are_normalized_at_public_extraction(self):
        value = {"impl_trait": [{"trait_bound": {"trait": path(9)["resolved_path"],
            "generic_params": [], "modifier": "none"}}, {"outlives": "'a"}]}
        expected = {"impl_trait": [{"trait_bound": {"trait": {"path": "alloc::vec::Vec", "args": None},
            "generic_params": [], "modifier": "none"}}, {"outlives": "'a"}]}
        self.assertEqual(extract(alias_document(value))["Opaque"]["shape"]["type"], expected)
        with self.assertRaises(GateError) as caught:
            extract(alias_document({"impl_trait": [{"future_bound": {}}]}))
        self.assertEqual(caught.exception.code, Code.FORMAT)
