import hashlib
import inspect
import unittest

from gate_errors import GateError
from upstream_python import extract


HEADER = "class ClaudeAgentOptions:\n    pass\nPermissionMode = Literal['default']\nSettingSource = Literal['user']\n"


def read_alias(value):
    data = (HEADER + "Message = " + value + "\n").encode()
    selected = {"ClaudeAgentOptions", "PermissionMode", "SettingSource", "Message"}
    options = {"selected": selected} if "selected" in inspect.signature(extract).parameters else {}
    return extract(data, hashlib.sha256(data).hexdigest(), **options)


class AliasOperatorTests(unittest.TestCase):
    def test_arithmetic_alias_nodes_are_rejected_at_every_depth(self):
        for value in ("str + int", "str - int", "str * int", "list[str + int]", "str | (int + str)"):
            with self.subTest(value=value), self.assertRaises(GateError):
                read_alias(value)

    def test_union_aliases_remain_supported(self):
        for value in ("str | int", "list[str | int]", "str | list[int]"):
            with self.subTest(value=value):
                self.assertEqual(read_alias(value)["Message"], {"kind": "alias", "annotation": value})
