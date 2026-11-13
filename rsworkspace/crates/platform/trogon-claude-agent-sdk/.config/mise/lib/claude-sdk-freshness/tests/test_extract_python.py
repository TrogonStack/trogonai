from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from extract_python import ExtractionError, extract


class PythonInventoryTests(unittest.TestCase):
    def package(self, files: dict[str, str]) -> Path:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        for name, contents in files.items():
            path = root / "src" / "claude_agent_sdk" / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(contents, encoding="utf-8")
        return root

    @staticmethod
    def shapes(inventory: dict) -> dict:
        return {name: (record["kind"], record["shape"]) for name, record in inventory["symbols"].items()}

    def test_reexports_dataclass_defaults_and_reachable_forward_references(self):
        root = self.package({
            "__init__.py": "from .api import Config as Options\n__all__ = ['Options']\nraise RuntimeError('upstream code must never run')\n",
            "api.py": """from dataclasses import dataclass, field
@dataclass
class Config:
    required: str | None
    explicit_none: str | None = None
    values: list[str] = field(default_factory=list)
    no_default: str = field()
    nested: 'Nested' = None
class Nested:
    choice: str
""",
        })
        inventory = extract(root)["symbols"]
        self.assertTrue(inventory["Options.required"]["shape"]["required"])
        self.assertFalse(inventory["Options.explicit_none"]["shape"]["required"])
        self.assertFalse(inventory["Options.values"]["shape"]["required"])
        self.assertTrue(inventory["Options.no_default"]["shape"]["required"])
        self.assertIn("api.Nested.choice", inventory)
        self.assertEqual(inventory["Options"]["source"]["path"], "src/claude_agent_sdk/api.py")

    def test_inherited_typed_dict_fields_keep_the_declaring_total(self):
        root = self.package({"__init__.py": """from typing import TypedDict, Required, NotRequired
class Base(TypedDict, total=False):
    inherited: str
    forced: Required[str]
class Child(Base):
    own: str
    optional: NotRequired[str]
__all__ = ['Child']
"""})
        symbols = extract(root)["symbols"]
        self.assertFalse(symbols["Child.inherited"]["shape"]["required"])
        self.assertTrue(symbols["Child.forced"]["shape"]["required"])
        self.assertTrue(symbols["Child.own"]["shape"]["required"])
        self.assertFalse(symbols["Child.optional"]["shape"]["required"])

    def test_async_overloads_aliases_and_literals_are_preserved(self):
        root = self.package({"__init__.py": """from typing import Literal, overload
Mode = Literal['allow', 'deny']
class Client:
    @overload
    async def send(self, message: str) -> str: ...
    @overload
    async def send(self, message: bytes) -> bytes: ...
__all__ = ['Mode', 'Client']
"""})
        symbols = extract(root)["symbols"]
        signatures = symbols["Client.send"]["shape"]["signatures"]
        self.assertEqual(len(signatures), 2)
        self.assertTrue(all(signature["async"] for signature in signatures))
        self.assertIn("'allow'", symbols["Mode"]["shape"]["value"])
        self.assertIn("'deny'", symbols["Mode"]["shape"]["value"])

    def test_formatting_docstrings_and_function_bodies_do_not_change_shapes(self):
        before = self.package({"__init__.py": "__all__=['Options', 'query']\nclass Options:\n    mode: str = 'safe'\ndef query(value: str = 'ok') -> str:\n    return value\n"})
        after = self.package({"__init__.py": '\n"""New documentation."""\n__all__ = ["Options", "query"]\n\nclass Options:\n    """Class prose."""\n    mode : str = "safe"\n\ndef query( value: str = "ok" ) -> str:\n    """Function prose."""\n    return "different implementation"\n'})
        self.assertEqual(self.shapes(extract(before)), self.shapes(extract(after)))

    def test_addition_annotation_and_default_changes_have_stable_public_keys(self):
        before = self.package({"__init__.py": "__all__=['Options']\nclass Options:\n    mode: str = 'safe'\n"})
        after = self.package({"__init__.py": "__all__=['Options']\nclass Options:\n    mode: str | None = None\n    new_budget: float = 0.0\n"})
        first, second = extract(before)["symbols"], extract(after)["symbols"]
        self.assertNotEqual(first["Options.mode"]["shape"], second["Options.mode"]["shape"])
        self.assertIn("Options.new_budget", second.keys() - first.keys())

    def test_static_all_augmentation_is_included(self):
        root = self.package({"__init__.py": "class A: pass\nclass B: pass\nclass C: pass\nclass D: pass\n__all__ = ['A']\n__all__ += ['B']\n__all__.append('C')\n__all__.extend(['D'])\n"})
        self.assertTrue({"A", "B", "C", "D"} <= extract(root)["symbols"].keys())

    def test_dynamic_or_conditional_all_mutations_fail_loudly(self):
        for mutation in ("__all__.extend(discover())", "if enabled:\n    __all__.append('B')", "__all__[0] = 'B'", "__all__ = discover()"):
            with self.subTest(mutation=mutation):
                root = self.package({"__init__.py": f"class A: pass\n__all__=['A']\n{mutation}\n"})
                with self.assertRaises(ExtractionError):
                    extract(root)

    def test_unresolved_local_exports_and_missing_roots_fail(self):
        root = self.package({"__init__.py": "from .missing import A\n__all__ = ['A']\n"})
        with self.assertRaises(ExtractionError):
            extract(root)
        with tempfile.TemporaryDirectory() as empty:
            with self.assertRaises(ExtractionError):
                extract(Path(empty))

    def test_external_binding_changes_are_explicit(self):
        first = self.package({"__init__.py": "from provider_one import Data\n__all__=['Data']\n"})
        second = self.package({"__init__.py": "from provider_two import Data\n__all__=['Data']\n"})
        before, after = extract(first)["symbols"], extract(second)["symbols"]
        self.assertEqual(before["Data"]["kind"], "external-reference")
        self.assertNotEqual(before["Data"]["shape"], after["Data"]["shape"])

    def test_symlinked_source_is_refused(self):
        root = self.package({"__init__.py": "class A: pass\n__all__=['A']\n"})
        with tempfile.TemporaryDirectory() as outside:
            target = Path(outside) / "external.py"
            target.write_text("class External: pass\n__all__=['External']\n", encoding="utf-8")
            source = root / "src" / "claude_agent_sdk" / "__init__.py"
            source.unlink()
            source.symlink_to(target)
            with self.assertRaises(ExtractionError):
                extract(root)


if __name__ == "__main__":
    unittest.main()
