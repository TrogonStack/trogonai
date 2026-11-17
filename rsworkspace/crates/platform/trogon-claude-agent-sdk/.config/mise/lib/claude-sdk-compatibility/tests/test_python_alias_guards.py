import hashlib
import inspect
import unittest

from gate_errors import GateError
from upstream_python import extract


def read(declaration):
    contents = ("class ClaudeAgentOptions:\n    pass\n"
                "PermissionMode = Literal['default']\nSettingSource = Literal['user']\n"
                + declaration).encode()
    selection = {"selected": {"Message"}} if "selected" in inspect.signature(extract).parameters else {}
    return extract(contents, hashlib.sha256(contents).hexdigest(), **selection)


class AliasGuardTests(unittest.TestCase):
    def test_missing_and_unsupported_alias_values_are_typed_failures(self):
        for declaration in ("Message: object\n", "Message = 42\n", "Message = factory()\n"):
            with self.subTest(declaration=declaration), self.assertRaises(GateError):
                read(declaration)

    def test_selected_class_kind_is_explicit_or_rejected(self):
        try:
            result = read("class Message:\n    pass\n")
        except GateError:
            return
        self.assertIn("Message", result)
        self.assertEqual(result["Message"]["kind"], "class")
