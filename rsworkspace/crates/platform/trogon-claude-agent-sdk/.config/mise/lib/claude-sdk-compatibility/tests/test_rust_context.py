import unittest
from gate_errors import GateError
from rust_references import RustDocument
from rustdoc_fixture import document

class ContextTests(unittest.TestCase):
    def test_explicit_local_alias_and_external_identity(self):
        d = document()
        d["paths"]["9"] = {"crate_id": 2, "path": ["alloc", "vec", "Vec"], "kind": "struct"}
        r = RustDocument(d, {"1": "PublicName"})
        self.assertEqual(r.qualified(1), "PublicName")
        self.assertEqual(r.qualified(9), "alloc::vec::Vec")
        self.assertEqual(r.item(0), d["index"]["0"])
        for bad in (None, 0, True, -1, "2", 2.0):
            d["paths"]["9"]["crate_id"] = bad
            with self.subTest(crate=bad), self.assertRaises(GateError):
                r.qualified(9)

    def test_ids_never_coerce_strings_booleans_or_floats(self):
        r = RustDocument(document(), {"9": "Alias"})
        for bad in ("9", True, 9.0, -1):
            with self.subTest(identifier=bad), self.assertRaises(GateError):
                r.qualified(bad)
        d = document()
        d["index"]["0"]["id"] = False
        with self.assertRaises(GateError):
            RustDocument(d)
