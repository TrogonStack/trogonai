import hashlib
import unittest

from gate_errors import Code, GateError
from upstream_python import extract


SOURCE = b'''from typing import Literal
raise RuntimeError("declarations must never execute")
PermissionMode = Literal["default", "plan"]
SettingSource: object = Literal["user", "project"]
class ClaudeAgentOptions:
    """Options declaration."""
    model: str | None = None
    settings: list[SettingSource]
    env: dict[str, str] = field(default_factory=dict)
'''


def read(source=SOURCE):
    return extract(source, hashlib.sha256(source).hexdigest())


class PythonDeclarationTests(unittest.TestCase):
    def test_annotations_defaults_and_literal_aliases_without_execution(self):
        result = read()
        self.assertEqual(result["ClaudeAgentOptions.model"], {"annotation": "str | None", "default": "None"})
        self.assertEqual(result["ClaudeAgentOptions.settings"]["default"], None)
        self.assertEqual(result["ClaudeAgentOptions.env"]["default"], "field(default_factory=dict)")
        self.assertEqual(result["PermissionMode"]["literals"], ["default", "plan"])
        self.assertEqual(result["SettingSource.project"], {"kind": "literal"})

    def test_pin_mismatch_and_size_limit_fail_before_parsing(self):
        for source, pin in [(SOURCE, "0" * 64), (b" " * (2 * 1024 * 1024 + 1), "0" * 64)]:
            with self.assertRaises(GateError) as caught:
                extract(source, pin)
            self.assertEqual(caught.exception.code, Code.PIN)

    def test_invalid_bytes_and_syntax_are_typed_extraction_errors(self):
        for source in (b"\xff", b"class broken("):
            with self.assertRaises(GateError) as caught:
                read(source)
            self.assertEqual(caught.exception.code, Code.EXTRACTION)

    def test_missing_duplicate_and_unsupported_selected_declarations_fail_closed(self):
        for source in (SOURCE.replace(b'SettingSource: object = Literal["user", "project"]', b''),
                       SOURCE + b'\nPermissionMode = Literal["other"]\n',
                       SOURCE + b'\nclass ClaudeAgentOptions: pass\n',
                       SOURCE + b'    model: str = "again"\n',
                       SOURCE + b'    def future(self): pass\n',
                       SOURCE.replace(b'Literal["default", "plan"]', b'Literal[1, "plan"]'),
                       SOURCE.replace(b'Literal["default", "plan"]', b'Literal["plan", "plan"]')):
            with self.subTest(source=source), self.assertRaises(GateError):
                read(source)

    def test_selected_rebindings_and_inherited_options_cannot_be_silently_ignored(self):
        for source in (SOURCE + b'\nPermissionMode = Other = Literal["again"]\n',
                       SOURCE + b'\nClaudeAgentOptions = object\n',
                       SOURCE + b'\ndef PermissionMode(): pass\n',
                       SOURCE + b'\ntype SettingSource = str\n',
                       SOURCE.replace(b'class ClaudeAgentOptions:', b'class ClaudeAgentOptions(Base):')):
            with self.subTest(source=source), self.assertRaises(GateError):
                read(source)
