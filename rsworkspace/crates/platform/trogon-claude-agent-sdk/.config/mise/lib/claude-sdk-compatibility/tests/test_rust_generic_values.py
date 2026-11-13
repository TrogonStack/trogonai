import unittest
from gate_errors import GateError
from rust_generic_values import associated, term
from rust_references import RustDocument
from rustdoc_fixture import document, path

class GenericValueTests(unittest.TestCase):
    def setUp(self):
        self.r = RustDocument(document(), {"1": "Public"})
        self.const = {"expr": "2", "value": "2", "is_literal": True}

    def test_every_argument_and_binding_keeps_supported_scope(self):
        args = {"angle_bracketed": {"args": [{"type": path(1)}, {"lifetime": "'a"}, {"const": self.const}, "infer"], "constraints": []}}
        result = self.r.arguments(args)
        self.assertEqual(result["angle_bracketed"]["args"][1:], args["angle_bracketed"]["args"][1:])
        self.assertEqual(result["angle_bracketed"]["args"][0]["type"]["resolved_path"]["path"], "Public")
        for binding in ({"equality": {"type": path(1)}}, {"equality": {"constant": self.const}}, {"constraint": [{"outlives": "'a"}]}):
            value = {"name": "Item", "args": args, "binding": binding}
            self.assertEqual(associated(self.r, value), self.r.normalize(value))
        self.assertEqual(term(self.r, {"constant": self.const}), {"constant": self.const})
        parenthesized = {"parenthesized": {"inputs": [path(1)], "output": {"primitive": "u8"}}}
        self.assertEqual(self.r.arguments(parenthesized)["parenthesized"]["inputs"][0]["resolved_path"]["path"], "Public")

    def test_unsupported_nested_types_and_argument_schemas_are_rejected(self):
        for value in ({"lifetime": 0}, {"infer": "_"}, {"const": None}, {"type": {"tuple": [{"function_pointer": {}}]}}):
            with self.subTest(value=value), self.assertRaises(GateError):
                self.r.arguments({"angle_bracketed": {"args": [value], "constraints": []}})
        for binding in ({"equality": {"type": {"future_type": {}}}}, {"equality": {"constant": None}}, {"constraint": [{"future_bound": {}}]}):
            with self.subTest(binding=binding), self.assertRaises(GateError):
                self.r.arguments({"angle_bracketed": {"args": [], "constraints": [{"name": "Item", "args": None, "binding": binding}]}})
