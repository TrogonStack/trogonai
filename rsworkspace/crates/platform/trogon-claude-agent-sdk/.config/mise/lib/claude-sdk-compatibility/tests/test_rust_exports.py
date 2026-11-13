import unittest

from gate_errors import Code, GateError
from rust_exports import inventory
from rustdoc_fixture import add, document, node, path


class ExportTests(unittest.TestCase):
    def test_private_module_reexports_and_nested_public_frontier(self):
        doc = document()
        add(doc, node(1, "hidden", "module", {"items": [2]}, "default"))
        add(doc, node(2, "Original", "struct", {}), False)
        add(doc, node(3, None, "use", {"id": 2, "name": "Visible", "is_glob": False, "source": "hidden::Original"}))
        add(doc, node(4, "nested", "module", {"items": [5], "is_stripped": False, "is_crate": False}))
        add(doc, node(5, "Other", "struct", {}), False)
        reader, exports = inventory(doc)
        self.assertEqual(set(exports), {"Visible", "nested", "nested::Other"})
        self.assertEqual(reader.type(path(2)), {"resolved_path": {"path": "Visible", "args": None}})

    def test_alias_canonical_choice_is_independent_of_item_order(self):
        doc = document()
        add(doc, node(1, "Original", "struct", {}), False)
        for identifier, name in [(2, "Zulu"), (3, "Alpha")]:
            add(doc, node(identifier, None, "use", {"id": 1, "name": name, "is_glob": False}))
        first, _ = inventory(doc)
        doc["index"]["0"]["inner"]["module"]["items"].reverse()
        second, exports = inventory(doc)
        self.assertEqual(set(exports), {"Alpha", "Zulu"})
        self.assertEqual(first.qualified(1), "Alpha")
        self.assertEqual(second.qualified(1), "Alpha")

    def test_glob_external_missing_duplicate_and_cyclic_exports_fail_closed(self):
        bodies = [{"id": 1, "name": "X", "is_glob": True},
                  {"id": None, "name": "X", "is_glob": False},
                  {"id": 99, "name": "X", "is_glob": False},
                  {"id": 2, "name": "X", "is_glob": False}]
        for body in bodies:
            doc = document()
            add(doc, node(2, None, "use", body))
            with self.subTest(body=body), self.assertRaises(GateError) as caught:
                inventory(doc)
            self.assertEqual(caught.exception.code, Code.FORMAT)
        doc = document()
        add(doc, node(1, "X", "struct", {}))
        add(doc, node(2, "X", "struct", {}))
        with self.assertRaises(GateError):
            inventory(doc)

    def test_missing_or_unknown_visibility_is_not_silently_private(self):
        for visibility in (None, "future-visibility"):
            doc = document()
            add(doc, node(1, "Hidden", "struct", {}, visibility))
            with self.assertRaises(GateError):
                inventory(doc)
        doc = document()
        add(doc, node(1, "cycle", "module", {"items": [1]}))
        with self.assertRaises(GateError):
            inventory(doc)
