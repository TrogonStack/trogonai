import unittest

from gate_errors import Code, GateError
from rust_surface import extract
from test_rust_methods import method_document
from test_rust_traits import trait_document


class MemberAttributeTests(unittest.TestCase):
    def test_public_function_constant_and_associated_type_attributes_are_retained(self):
        attributes = [{"must_use": {"reason": "Review this result."}}]
        for factory, identifier, selector in ((method_document, "3", "Client::connect"),
                                               (method_document, "5", "Client::VERSION"),
                                               (trait_document, "3", "Convert::convert"),
                                               (trait_document, "4", "Convert::Output")):
            with self.subTest(selector=selector):
                doc = factory()
                doc["index"][identifier]["attrs"] = attributes
                result = extract(doc)
                self.assertEqual(result[selector].get("attributes"), attributes)
                if factory is trait_document:
                    member = result["Value"]["manual_trait_impls"][0]["items"][selector.split("::")[1]]
                    self.assertEqual(member.get("attributes"), attributes)

    def test_malformed_and_unknown_member_attributes_fail_closed(self):
        for factory in (method_document, trait_document):
            for value in ({}, "", None, False, [{}], [{"future_attribute": {}}], ["unknown"]):
                with self.subTest(factory=factory.__name__, value=value):
                    doc = factory()
                    doc["index"]["3"]["attrs"] = value
                    with self.assertRaises(GateError) as caught:
                        extract(doc)
                    self.assertEqual(caught.exception.code, Code.FORMAT)

    def test_attribute_drift_changes_the_public_inventory(self):
        doc = method_document()
        original = extract(doc)
        doc["index"]["3"]["attrs"].append({"must_use": {"reason": None}})
        self.assertNotEqual(extract(doc), original)

    def test_empty_member_attributes_preserve_the_existing_canonical_shape(self):
        for factory, selector in ((method_document, "Client::connect"),
                                  (trait_document, "Convert::convert")):
            self.assertNotIn("attributes", extract(factory())[selector])
