import copy
import unittest

from gate_errors import GateError
from rust_surface import extract
from rustdoc_fixture import add, document, generics, node, path


class DataShapeTests(unittest.TestCase):
    def test_named_tuple_and_unit_structs_keep_fields_and_generics(self):
        doc = document()
        params = {"params": [{"name": "T", "kind": {"type": {
            "bounds": [], "default": {"primitive": "u8"}, "is_synthetic": False,
        }}}], "where_predicates": []}
        add(doc, node(1, "Record", "struct", {"kind": {"plain": {"fields": [2, 3], "has_stripped_fields": True}}, "generics": params, "impls": []}))
        add(doc, node(2, "value", "struct_field", {"generic": "T"}), False)
        add(doc, node(3, "secret", "struct_field", {"primitive": "u8"}, "default"), False)
        add(doc, node(4, "Pair", "struct", {"kind": {"tuple": [5, None]}, "generics": generics(), "impls": []}))
        add(doc, node(5, "0", "struct_field", path(1)), False)
        add(doc, node(6, "Marker", "struct", {"kind": "unit", "generics": generics(), "impls": []}))
        result = extract(doc)
        self.assertEqual(result["Record"]["fields"], {"value": {"generic": "T"}})
        self.assertTrue(result["Record"]["has_stripped_fields"])
        self.assertEqual(result["Record"]["generics"], params)
        self.assertEqual(result["Pair"]["tuple_fields"], [{"resolved_path": {"path": "Record", "args": None}}, None])
        self.assertEqual(result["Marker"]["struct_kind"], "unit")

    def test_alias_constant_and_static_shapes_keep_types_and_values(self):
        doc = document()
        alias = {"type": {"primitive": "u64"}, "generics": generics()}
        const = {"type": {"primitive": "u64"}, "const": {"expr": "42", "value": "42", "is_literal": True}}
        static = {"type": {"primitive": "u8"}, "is_mutable": False, "expr": "1", "is_unsafe": False}
        for i, name, kind, body in [(1, "Count", "type_alias", alias), (2, "LIMIT", "constant", const), (3, "FLAG", "static", static)]:
            add(doc, node(i, name, kind, body))
        result = extract(doc)
        self.assertEqual(result["Count"]["shape"], alias)
        self.assertEqual(result["LIMIT"]["shape"], const)
        self.assertEqual(result["FLAG"]["shape"], static)

    def test_unsupported_items_fields_and_stripped_options_are_rejected(self):
        for kind, body in [("union", {}), ("struct", {"kind": {"future": []}, "generics": generics(), "impls": []}),
                           ("struct", {"kind": {"plain": {"fields": [20], "has_stripped_fields": False}}, "generics": generics(), "impls": []}),
                           ("struct", {"kind": {"plain": {"fields": [], "has_stripped_fields": True}}, "generics": generics(), "impls": []})]:
            doc = document()
            add(doc, node(1, "ClaudeAgentOptions", kind, body))
            with self.subTest(body=body), self.assertRaises(GateError):
                extract(doc)

    def test_non_exhaustive_and_other_type_attributes_change_the_contract(self):
        doc = document()
        add(doc, node(1, "Record", "struct", {"kind": "unit", "generics": generics(), "impls": []}))
        add(doc, node(2, "Choice", "enum", {"variants": [3], "has_stripped_variants": False,
                                          "generics": generics(), "impls": []}))
        add(doc, node(3, "One", "variant", {"kind": "plain", "discriminant": None}), False)
        baseline = extract(doc)
        for identifier in (1, 2, 3):
            for attribute in ("non_exhaustive", {"must_use": {"reason": "keep the result"}},
                              {"repr": {"kind": "c", "align": None, "packed": None, "int": None}},
                              {"other": "#[serde(untagged)]"}):
                changed = copy.deepcopy(doc)
                changed["index"][str(identifier)]["attrs"] = [attribute]
                with self.subTest(identifier=identifier, attribute=attribute):
                    self.assertNotEqual(baseline, extract(changed))

    def test_malformed_or_unknown_attributes_fail_closed(self):
        for attributes in (None, {}, [42], ["future_attribute"], [{"other": 1}],
                           [{"must_use": {"reason": 42}}], [{"repr": {"kind": "c"}}]):
            doc = document()
            item = add(doc, node(1, "Record", "struct", {"kind": "unit", "generics": generics(), "impls": []}))
            item["attrs"] = attributes
            with self.subTest(attributes=attributes), self.assertRaises(GateError):
                extract(doc)
