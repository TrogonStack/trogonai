import unittest

from gate_errors import Code, GateError
from rust_references import RustDocument
from rust_surface import extract
from rustdoc_fixture import add, document, generics, node, path


def inferred_alias(argument):
    doc = document()
    doc["paths"]["2"] = {"crate_id": 1, "path": ["alloc", "vec", "Vec"]}
    args = {"angle_bracketed": {"args": [argument], "constraints": []}}
    add(doc, node(1, "Value", "type_alias", {"type": path(2, args), "generics": generics()}))
    return doc, args


class InferArgumentTests(unittest.TestCase):
    def test_direct_arguments_preserve_the_format57_unit_string(self):
        doc, args = inferred_alias("infer")
        self.assertEqual(RustDocument(doc).arguments(args), args)

    def test_extraction_preserves_the_format57_unit_string(self):
        doc, args = inferred_alias("infer")
        value = extract(doc)["Value"]["shape"]["type"]["resolved_path"]
        self.assertEqual(value, {"path": "alloc::vec::Vec", "args": args})

    def test_direct_arguments_reject_a_tagged_infer_object(self):
        doc, args = inferred_alias({"infer": None})
        with self.assertRaises(GateError) as error:
            RustDocument(doc).arguments(args)
        self.assertIs(error.exception.code, Code.FORMAT)

    def test_extraction_rejects_a_tagged_infer_object(self):
        doc, _ = inferred_alias({"infer": None})
        with self.assertRaises(GateError) as error:
            extract(doc)
        self.assertIs(error.exception.code, Code.FORMAT)
