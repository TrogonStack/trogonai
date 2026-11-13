import unittest

from gate_errors import GateError
from rust_surface import extract
from rustdoc_fixture import add, document, function, generics, implementation, node, path


def trait_document():
    doc = document()
    add(doc, node(1, "Value", "struct", {"kind": "unit", "generics": generics(), "impls": [5, 6, 7, 8]}))
    add(doc, node(2, "Convert", "trait", {"is_auto": False, "is_unsafe": False, "is_dyn_compatible": True,
        "items": [3, 4], "generics": generics(), "bounds": [], "implementations": [5]}))
    required = function(path(1))
    required["has_body"] = False
    add(doc, node(3, "convert", "function", required, "default"), False)
    add(doc, node(4, "Output", "assoc_type", {"generics": generics(), "bounds": [], "type": None}, "default"), False)
    for identifier in (5, 6, 7, 8):
        add(doc, node(identifier, None, "impl", implementation(path(1), [3, 4], path(2)["resolved_path"])), False)
    doc["index"]["6"]["inner"]["impl"]["is_synthetic"] = True
    doc["index"]["7"]["inner"]["impl"]["blanket_impl"] = {"generic": "T"}
    doc["index"]["8"]["attrs"] = [{"other": "#[allow(unused_qualifications)]"}, "automatically_derived"]
    return doc


class TraitTests(unittest.TestCase):
    def test_trait_declaration_and_manual_implementation_keep_all_members(self):
        result = extract(trait_document())
        self.assertFalse(result["Convert::convert"]["has_body"])
        self.assertEqual(result["Convert::Output"]["shape"]["bounds"], [])
        traits = result["Value"]["manual_trait_impls"]
        self.assertEqual(len(traits), 1)
        self.assertEqual(traits[0]["trait"]["path"], "Convert")
        self.assertEqual(set(traits[0]["items"]), {"convert", "Output"})
        self.assertEqual(traits[0]["provided_trait_methods"], [])
        self.assertFalse(traits[0]["is_negative"])

    def test_manual_unknown_or_missing_members_fail_closed(self):
        for mutation in (lambda d: d["index"]["5"]["inner"]["impl"]["items"].append(100),
                         lambda d: d["index"]["4"].update(inner={"future_member": {}}),
                         lambda d: d["index"]["4"].update(name="convert")):
            doc = trait_document()
            mutation(doc)
            with self.assertRaises(GateError):
                extract(doc)

    def test_unsupported_trait_bounds_and_nonboolean_impl_flags_fail_closed(self):
        for mutation in (lambda d: d["index"]["2"]["inner"]["trait"].update(bounds=[{"future_bound": {}}]),
                         lambda d: d["index"]["4"]["inner"]["assoc_type"].update(bounds=[{"future_bound": {}}]),
                         lambda d: d["index"]["5"]["inner"]["impl"].update(is_synthetic="false")):
            doc = trait_document()
            mutation(doc)
            with self.assertRaises(GateError):
                extract(doc)
