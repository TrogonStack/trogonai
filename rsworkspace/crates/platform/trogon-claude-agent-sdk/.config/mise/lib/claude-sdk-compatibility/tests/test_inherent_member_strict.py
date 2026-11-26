import unittest

from gate_errors import Code, GateError
from rust_surface import extract
from test_rust_methods import method_document


class InherentMemberStrictTests(unittest.TestCase):
    def test_empty_nonlist_inputs_fail_at_public_extraction(self):
        for value in ({}, "", (), set(), None, False, 0):
            doc = method_document()
            doc["index"]["3"]["inner"]["function"]["sig"]["inputs"] = value
            with self.subTest(value=value), self.assertRaises(GateError) as caught:
                extract(doc)
            self.assertEqual(caught.exception.code, Code.FORMAT)

    def test_valid_empty_inputs_and_attributes_preserve_member_shapes(self):
        doc = method_document()
        doc["index"]["3"]["inner"]["function"]["sig"]["inputs"] = []
        result = extract(doc)
        self.assertEqual(result["Client::connect"]["signature"]["inputs"], [])
        for selector in ("Client::connect", "Client::VERSION"):
            self.assertNotIn("attributes", result[selector])

    def test_supported_method_and_constant_attributes_change_inventory(self):
        for identifier, selector in (("3", "Client::connect"), ("5", "Client::VERSION")):
            doc = method_document()
            original = extract(doc)
            attributes = [{"must_use": {"reason": "Review this result."}}]
            doc["index"][identifier]["attrs"] = attributes
            result = extract(doc)
            with self.subTest(selector=selector):
                self.assertNotEqual(result, original)
                self.assertEqual(result[selector].get("attributes"), attributes)

    def test_malformed_method_and_constant_attributes_fail_closed(self):
        for identifier in ("3", "5"):
            for value in ({}, "", None, False, [{}], [{"future_attribute": {}}], ["unknown"]):
                doc = method_document()
                doc["index"][identifier]["attrs"] = value
                with self.subTest(identifier=identifier, value=value), self.assertRaises(GateError) as caught:
                    extract(doc)
                self.assertEqual(caught.exception.code, Code.FORMAT)
