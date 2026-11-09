import unittest

from gate_errors import Code, GateError
from rust_surface import extract
from test_rust_methods import method_document
from test_rust_traits import trait_document


class FunctionInputContainerTests(unittest.TestCase):
    def test_empty_nonlist_function_arguments_are_rejected_at_public_extraction(self):
        for factory in (method_document, trait_document):
            for value in ({}, "", (), set(), None, False, 0):
                with self.subTest(factory=factory.__name__, value=value):
                    doc = factory()
                    doc["index"]["3"]["inner"]["function"]["sig"]["inputs"] = value
                    with self.assertRaises(GateError) as caught:
                        extract(doc)
                    self.assertEqual(caught.exception.code, Code.FORMAT)

    def test_valid_empty_function_argument_lists_are_preserved(self):
        for factory, selector in ((method_document, "Client::connect"),
                                  (trait_document, "Convert::convert")):
            with self.subTest(selector=selector):
                doc = factory()
                doc["index"]["3"]["inner"]["function"]["sig"]["inputs"] = []
                self.assertEqual(extract(doc)[selector]["signature"]["inputs"], [])
