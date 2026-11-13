import unittest

from gate_errors import GateError
from rust_surface import extract
from rustdoc_fixture import add, document, function, generics, implementation, node, path


def method_document():
    doc = document()
    add(doc, node(1, "Client", "struct", {"kind": "unit", "generics": generics(), "impls": [2]}))
    add(doc, node(2, None, "impl", implementation(path(1), [3, 4, 5])), False)
    add(doc, node(3, "connect", "function", function(path(1))), False)
    add(doc, node(4, "private", "function", function(), "default"), False)
    add(doc, node(5, "VERSION", "assoc_const", {"type": {"primitive": "u8"}, "value": "1"}), False)
    return doc


class MethodTests(unittest.TestCase):
    def test_public_method_header_signature_context_and_associated_constant(self):
        result = extract(method_document())
        self.assertIn("Client::connect", result)
        method = result["Client::connect"]
        self.assertTrue(method["header"]["is_async"])
        self.assertEqual(method["signature"]["output"], {"resolved_path": {"path": "Client", "args": None}})
        self.assertEqual(method["impl_generics"], generics())
        self.assertEqual(method["impl_for"], {"resolved_path": {"path": "Client", "args": None}})
        self.assertEqual(result["Client::VERSION"]["shape"]["value"], "1")
        self.assertNotIn("Client::private", result)

    def test_free_function_is_not_omitted(self):
        doc = document()
        body = function({"primitive": "u8"})
        body["sig"]["inputs"] = []
        add(doc, node(1, "version", "function", body))
        self.assertEqual(extract(doc)["version"]["signature"]["output"], {"primitive": "u8"})

    def test_missing_unknown_duplicate_members_and_unsupported_types_fail_closed(self):
        for mutation in (lambda d: d["index"].pop("3"),
                         lambda d: d["index"]["3"].update(inner={"future_member": {}}),
                         lambda d: d["index"]["5"].update(name="connect"),
                         lambda d: d["index"]["3"]["inner"]["function"]["sig"].update(output={"future_type": {}})):
            doc = method_document()
            mutation(doc)
            with self.assertRaises(GateError):
                extract(doc)
