import unittest

from gate_errors import Code, GateError
from rust_references import RustDocument
from rustdoc_fixture import document


class ArgumentAdmissionTests(unittest.TestCase):
    def test_arguments_admit_nested_schemas_before_normalizing(self):
        arguments = [{"const": None}, {"lifetime": 0}, {"infer": "_"},
                     {"const": {"expr": False, "value": "2", "is_literal": 0}}]
        constraints = [None, {},
                       {"name": "Item", "args": None, "binding": {"equality": {"type": {"function_pointer": {}}}}},
                       {"name": "Item", "args": None, "binding": {"future_binding": {}}}]
        for args, bindings in [([value], []) for value in arguments] + [([], [value]) for value in constraints]:
            value = {"angle_bracketed": {"args": args, "constraints": bindings}}
            with self.subTest(value=value):
                with self.assertRaises(GateError) as error:
                    RustDocument(document()).arguments(value)
                self.assertIs(error.exception.code, Code.FORMAT)
