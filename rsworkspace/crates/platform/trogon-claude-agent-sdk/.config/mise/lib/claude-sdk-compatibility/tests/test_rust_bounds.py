import copy
import unittest
from gate_errors import GateError
from rust_bounds import bounds, parameters
from rust_references import RustDocument
from rustdoc_fixture import document, path

class BoundSchemaTests(unittest.TestCase):
    def setUp(self):
        self.r = RustDocument(document(), {"9": "Trait"})
        self.bound = {"trait_bound": {"trait": path(9)["resolved_path"], "generic_params": [], "modifier": "maybe"}}
        self.params = [{"name": "T", "kind": {"type": {"bounds": [self.bound], "default": {"array": {"type": {"primitive": "u8"}, "len": "N"}}, "is_synthetic": False}}},
                       {"name": "N", "kind": {"const": {"type": {"primitive": "usize"}, "default": "2"}}},
                       {"name": "'a", "kind": {"lifetime": {"outlives": ["'static"]}}}]

    def test_generic_scope_and_nested_paths_are_preserved(self):
        result = parameters(self.r, self.params)
        self.assertEqual(result[0]["kind"]["type"]["bounds"][0]["trait_bound"]["trait"], {"path": "Trait", "args": None})
        self.assertEqual(result[1:], self.params[1:])
        for modifier in ("none", "maybe", "maybe_const"):
            self.bound["trait_bound"]["modifier"] = modifier
            self.assertEqual(bounds(self.r, [self.bound])[0]["trait_bound"]["modifier"], modifier)
        captures = [{"outlives": "'a"}, {"use": [{"lifetime": "'a"}, {"param": "T"}]}]
        self.assertEqual(bounds(self.r, captures), captures)

    def test_nested_unsupported_types_flags_and_bounds_are_rejected(self):
        for value in ({"future_type": {}}, {"function_pointer": {}}, {"slice": {"future_type": {}}}):
            params = copy.deepcopy(self.params)
            params[0]["kind"]["type"]["default"] = value
            with self.subTest(value=value), self.assertRaises(GateError):
                parameters(self.r, params)
        for value in ([{"future_bound": {}}], [{"outlives": 0}], [{"use": [{"param": False}]}], {}):
            with self.subTest(value=value), self.assertRaises(GateError):
                bounds(self.r, value)
