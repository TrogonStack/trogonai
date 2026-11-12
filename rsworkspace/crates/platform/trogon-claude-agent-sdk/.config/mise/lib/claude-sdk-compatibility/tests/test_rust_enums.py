import unittest

from gate_errors import GateError
from rust_surface import extract
from rustdoc_fixture import add, document, generics, node


def enum_document():
    doc = document()
    add(doc, node(1, "Event", "enum", {"generics": generics(), "has_stripped_variants": False, "variants": [2, 3, 4], "impls": []}))
    add(doc, node(2, "Idle", "variant", {"kind": "plain", "discriminant": {"expr": "2", "value": "2"}}, "default"), False)
    add(doc, node(3, "Value", "variant", {"kind": {"tuple": [5]}, "discriminant": None}, "default"), False)
    add(doc, node(4, "Named", "variant", {"kind": {"struct": {"fields": [6], "has_stripped_fields": False}}, "discriminant": None}, "default"), False)
    add(doc, node(5, "0", "struct_field", {"primitive": "u8"}, "default"), False)
    add(doc, node(6, "value", "struct_field", {"primitive": "bool"}, "default"), False)
    return doc


class EnumTests(unittest.TestCase):
    def test_every_variant_payload_and_discriminant_is_retained(self):
        result = extract(enum_document())
        self.assertEqual(result["Event"]["variants"], ["Idle", "Value", "Named"])
        self.assertEqual(result["Event::Idle"]["discriminant"], {"expr": "2", "value": "2"})
        self.assertEqual(result["Event::Value"]["payload"], [{"primitive": "u8"}])
        self.assertEqual(result["Event::Named"]["payload"], {"value": {"primitive": "bool"}})

    def test_stripped_unknown_missing_and_duplicate_variants_are_rejected(self):
        for mutation in (lambda d: d["index"]["1"]["inner"]["enum"].update(has_stripped_variants=True),
                         lambda d: d["index"]["2"]["inner"]["variant"].update(kind={"future": []}),
                         lambda d: d["index"].pop("5"),
                         lambda d: d["index"]["4"]["inner"]["variant"]["kind"]["struct"].update(has_stripped_fields=True),
                         lambda d: d["index"]["3"].update(name="Idle")):
            doc = enum_document()
            mutation(doc)
            with self.assertRaises(GateError):
                extract(doc)
