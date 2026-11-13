import unittest
from gate_errors import Code, GateError
from rust_surface import extract
from rustdoc_fixture import add, document, generics, node

class FieldContainerAdmissionTests(unittest.TestCase):
    def test_struct_field_containers_require_lists(self):
        for value in ({}, ""):
            for shape in ({"plain": {"fields": value, "has_stripped_fields": False}}, {"tuple": value}):
                d = document()
                add(d, node(1, "Value", "struct", {"kind": shape, "generics": generics(), "impls": []}))
                with self.subTest(shape=shape):
                    with self.assertRaises(GateError) as caught:
                        extract(d)
                    self.assertEqual(caught.exception.code, Code.FORMAT)
