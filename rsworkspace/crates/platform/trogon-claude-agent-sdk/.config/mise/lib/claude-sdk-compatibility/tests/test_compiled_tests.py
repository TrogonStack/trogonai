import copy
import unittest

from compiled_tests import compiled_tests
from gate_errors import GateError


def listing():
    return {"rust-suites": {"sdk::contract": {
        "package-name": "trogon-claude-agent-sdk", "binary-name": "contract", "status": "listed",
        "testcases": {"passes": {"kind": "test", "ignored": False, "filter-match": {"status": "matches"}},
                      "skipped": {"kind": "test", "ignored": True, "filter-match": {"status": "matches"}},
                      "excluded": {"kind": "test", "ignored": False, "filter-match": {"status": "mismatch", "reason": "expression"}}},
    }}}


class InventoryTests(unittest.TestCase):
    def test_nonignored_listed_selectors_do_not_claim_execution(self):
        result = compiled_tests(listing())
        self.assertEqual(result.selectors, {"contract::passes"})
        self.assertEqual(result.ignored, {"contract::skipped"})
        self.assertEqual(result.filtered, {"contract::excluded"})
        self.assertEqual(result.evidence, "compiled-and-listed")
        self.assertFalse(hasattr(result, "executed"))

    def test_unlisted_malformed_and_duplicate_sdk_suites_fail_closed(self):
        for mutation in (lambda d: d["rust-suites"]["sdk::contract"].update(status="skipped"),
                         lambda d: d["rust-suites"]["sdk::contract"]["testcases"]["passes"].pop("ignored"),
                         lambda d: d["rust-suites"]["sdk::contract"]["testcases"]["passes"].update(ignored="false"),
                         lambda d: d["rust-suites"]["sdk::contract"]["testcases"]["passes"]["filter-match"].update(status="future"),
                         lambda d: d["rust-suites"].update(duplicate=copy.deepcopy(d["rust-suites"]["sdk::contract"]))):
            doc = listing()
            mutation(doc)
            with self.assertRaises(GateError):
                compiled_tests(doc)

    def test_other_packages_cannot_supply_sdk_selectors(self):
        doc = listing()
        doc["rust-suites"]["sdk::contract"]["package-name"] = "another-package"
        self.assertEqual(compiled_tests(doc).selectors, set())
