import unittest
from gate_errors import Code, GateError
from rust_exports import inventory
from rustdoc_fixture import add, document, node

class InventoryAdmissionTests(unittest.TestCase):
    def test_direct_inventory_has_typed_malformed_payload_errors(self):
        for kind in ("use", "module"):
            for body in (None, [], "", 0):
                d = document()
                add(d, node(1, "Export", kind, body))
                with self.subTest(kind=kind, body=body):
                    with self.assertRaises(GateError) as caught:
                        inventory(d)
                    self.assertEqual(caught.exception.code, Code.FORMAT)
                    self.assertTrue(caught.exception.selector)
