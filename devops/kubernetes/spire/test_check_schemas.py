import copy
import unittest

from check_schemas import require_complete_validation


class CompleteSchemaValidationTests(unittest.TestCase):
    def test_accepts_only_complete_successful_validation(self):
        report = {"resources": [], "summary": {"valid": 25, "invalid": 0, "errors": 0, "skipped": 0}}
        require_complete_validation(report, 25)

        for field, value in [
            ("valid", 24), ("valid", 26), ("invalid", 1), ("errors", 1),
            ("skipped", 1), ("skipped", False), ("errors", "0"),
        ]:
            with self.subTest(field=field, value=value):
                changed = copy.deepcopy(report)
                changed["summary"][field] = value
                with self.assertRaises(ValueError):
                    require_complete_validation(changed, 25)

        for malformed in [{}, {"summary": {}}, {"summary": None}, []]:
            with self.subTest(report=malformed):
                with self.assertRaises(ValueError):
                    require_complete_validation(malformed, 25)

        with self.assertRaises(ValueError):
            require_complete_validation({"summary": {"valid": 0, "invalid": 0, "errors": 0, "skipped": 0}}, 0)


if __name__ == "__main__":
    unittest.main()
