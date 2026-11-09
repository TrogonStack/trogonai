import unittest
from gate_errors import Code, GateError
from rust_surface import extract
from test_rust_enums import enum_document

class VariantContainerAdmissionTests(unittest.TestCase):
    def test_variant_and_payload_containers_require_lists(self):
        for container in ("variants", "tuple", "fields"):
            for value in ({}, ""):
                d = enum_document()
                if container == "variants":
                    d["index"]["1"]["inner"]["enum"][container] = value
                elif container == "tuple":
                    d["index"]["3"]["inner"]["variant"]["kind"][container] = value
                else:
                    d["index"]["4"]["inner"]["variant"]["kind"]["struct"][container] = value
                with self.subTest(container=container, value=value):
                    with self.assertRaises(GateError) as caught:
                        extract(d)
                    self.assertEqual(caught.exception.code, Code.FORMAT)
