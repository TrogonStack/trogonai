import unittest
from gate_errors import Code, GateError
from rust_references import RustDocument
from rustdoc_fixture import document

class ScalarAdmissionTests(unittest.TestCase):
    def test_reference_scalars_do_not_collide_with_valid_shapes(self):
        for kind, field, values in [("borrowed_ref", "is_mutable", (0, 1, None, "false")),
                                    ("raw_pointer", "is_mutable", (0, 1, None, "true")),
                                    ("borrowed_ref", "lifetime", (0, False, [], {})),
                                    ("array", "len", (0, False, None, [], {}))]:
            for value in values:
                body = {"type": {"primitive": "u8"}}
                body.update({"len": "4"} if kind == "array" else {"is_mutable": False})
                if kind == "borrowed_ref":
                    body["lifetime"] = None
                body[field] = value
                with self.subTest(kind=kind, field=field, value=value):
                    with self.assertRaises(GateError) as caught:
                        RustDocument(document()).type({kind: body})
                    self.assertEqual(caught.exception.code, Code.FORMAT)
