import copy
import unittest

from gate_errors import Code, GateError
from rust_references import RustDocument
from rustdoc_fixture import document, path


class ReferenceTests(unittest.TestCase):
    def setUp(self):
        self.doc = document()
        self.doc["paths"]["9"] = {"crate_id": 2, "path": ["alloc", "vec", "Vec"], "kind": "struct"}
        self.args = {"angle_bracketed": {"args": [{"type": path(1)}], "constraints": []}}


    def test_reference_tuple_lifetime_and_const_arguments_are_retained(self):
        value = {"borrowed_ref": {"lifetime": "'a", "is_mutable": True, "type": {
            "tuple": [{"generic": "T"}, {"array": {"type": {"primitive": "u8"}, "len": "N"}}],
        }}}
        self.assertEqual(RustDocument(self.doc).type(value), value)

    def test_unknown_format_missing_nodes_and_dangling_paths_fail_closed(self):
        for version in (56, 58, True, None):
            with self.subTest(version=version), self.assertRaises(GateError):
                RustDocument({**self.doc, "format_version": version})
        for action in (lambda: RustDocument(self.doc).item(40),
                       lambda: RustDocument(self.doc).type(path(40)),
                       lambda: RustDocument(self.doc).type({"future_type": {}}),
                       lambda: RustDocument(self.doc).normalize({"id": 9})):
            with self.assertRaises(GateError) as caught:
                action()
            self.assertEqual(caught.exception.code, Code.FORMAT)

    def test_invalid_type_arguments_are_not_silently_preserved(self):
        for arguments in ({"future_args": []}, {"angle_bracketed": {"args": [{"future": "T"}], "constraints": []}}):
            with self.assertRaises(GateError):
                RustDocument(self.doc).type(path(9, arguments))
