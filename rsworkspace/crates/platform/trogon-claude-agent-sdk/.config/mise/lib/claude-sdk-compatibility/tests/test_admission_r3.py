import unittest
from gate_errors import Code, GateError
from rust_references import RustDocument
from rustdoc_fixture import document, path

class IdentityAdmissionTests(unittest.TestCase):
    def test_external_identity_requires_positive_integer_crate(self):
        for crate in (None, False, True, 0, -1, "2", 2.0):
            d = document()
            d["paths"]["9"] = {"path": ["alloc", "vec", "Vec"], "kind": "struct"}
            if crate is not None:
                d["paths"]["9"]["crate_id"] = crate
            with self.subTest(crate=crate):
                with self.assertRaises(GateError) as caught:
                    RustDocument(d).type(path(9))
                self.assertEqual(caught.exception.code, Code.FORMAT)

    def test_reference_ids_are_exact_integers_even_for_aliases(self):
        for identifier in ("9", True, 9.0, -1):
            with self.subTest(identifier=identifier), self.assertRaises(GateError):
                RustDocument(document(), {str(identifier): "Alias"}).type(path(identifier))
