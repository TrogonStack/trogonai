from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path


EXTRACTOR = Path(__file__).resolve().parents[1] / "extract_typescript.mjs"


class TypeScriptInventoryTests(unittest.TestCase):
    def package(self, files: dict[str, str], exports: dict | None = None) -> Path:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        manifest = {"name": "extractor-fixture", "type": "module", "types": "sdk.d.ts", "exports": exports or {".": {"types": "./sdk.d.ts"}}}
        (root / "package.json").write_text(json.dumps(manifest), encoding="utf-8")
        for name, contents in files.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(contents, encoding="utf-8")
        return root

    def run_extract(self, root: Path) -> subprocess.CompletedProcess:
        return subprocess.run(["node", str(EXTRACTOR), str(root)], capture_output=True, text=True, check=False)

    def extract(self, root: Path) -> dict:
        result = self.run_extract(root)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    @staticmethod
    def shapes(inventory: dict) -> dict:
        return {name: (record["kind"], record["shape"]) for name, record in inventory["symbols"].items()}

    def test_reexports_reachable_types_optional_fields_and_overloads(self):
        root = self.package({
            "sdk.d.ts": 'export { Config as Options, Client } from "./types.js";',
            "types.d.ts": """interface Nested { choice: 'allow' | 'deny'; }
export interface Config<T extends string = string> { required: T; budget?: number; nested: Nested; }
export interface Client { send(input: string): Promise<string>; send(input: Uint8Array): Promise<Uint8Array>; }
""",
        })
        symbols = self.extract(root)["symbols"]
        self.assertEqual(symbols["Options.budget"]["shape"]["optional"], [True])
        self.assertEqual(symbols["Options.required"]["shape"]["optional"], [False])
        self.assertEqual(len(symbols["Client.send"]["shape"]["declarations"]), 2)
        self.assertIn("types.Nested.choice", symbols)
        self.assertIn("T extends string = string", symbols["Options"]["shape"]["declarations"][0])
        self.assertEqual(symbols["Options"]["source"]["path"], "types.d.ts")

    def test_alias_union_and_added_members_change_shapes(self):
        first = self.package({"sdk.d.ts": "export type Mode = 'allow' | 'deny'; export type Options = { budget?: number };"})
        second = self.package({"sdk.d.ts": "export type Mode = 'allow' | 'deny' | 'ask'; export type Options = { budget: number; model?: string };"})
        before, after = self.extract(first)["symbols"], self.extract(second)["symbols"]
        self.assertNotEqual(before["Mode"]["shape"], after["Mode"]["shape"])
        self.assertNotEqual(before["Options.budget"]["shape"], after["Options.budget"]["shape"])
        self.assertIn("Options.model", after.keys() - before.keys())

    def test_formatting_and_comments_do_not_change_shapes(self):
        first = self.package({"sdk.d.ts": "export type Mode='safe'|'fast'; export interface Options { model?: string; }"})
        second = self.package({"sdk.d.ts": '\n/** Updated docs. */\nexport type Mode = "safe" | "fast";\n\nexport interface Options {\n/** field docs */ model ? : string\n}\n'})
        self.assertEqual(self.shapes(self.extract(first)), self.shapes(self.extract(second)))

    def test_all_declared_package_entrypoints_are_inspected(self):
        root = self.package({"sdk.d.ts": "export type Options = { model: string };", "browser.d.ts": "export interface Remote { connect(): Promise<void>; }"}, exports={".": {"types": "./sdk.d.ts"}, "./browser": {"types": "./browser.d.ts"}})
        symbols = self.extract(root)["symbols"]
        self.assertIn("browser.Remote.connect", symbols)
        self.assertIn("./browser", symbols["package.exports"]["shape"]["exports"])

    def test_external_reexports_stay_explicit(self):
        root = self.package({"sdk.d.ts": "export type { Data } from 'external-provider';"})
        symbols = self.extract(root)["symbols"]
        self.assertEqual(symbols["Data"]["kind"], "external-reference")
        self.assertIn("external-provider", symbols["modules.sdk.d.ts.imports"]["shape"]["declarations"][0])

    def test_invalid_syntax_missing_local_exports_and_missing_entrypoints_fail(self):
        for contents in ("export interface Broken { field:", 'export { Missing } from "./missing.js";'):
            with self.subTest(contents=contents):
                root = self.package({"sdk.d.ts": contents})
                result = self.run_extract(root)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")
        root = self.package({"sdk.d.ts": "export interface Options {}"}, exports={".": {"types": "./sdk.d.ts"}, "./new": {"types": "./missing.d.ts"}})
        result = self.run_extract(root)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")

    def test_symlinked_declaration_is_refused(self):
        root = self.package({"sdk.d.ts": "export interface Options {}"})
        with tempfile.TemporaryDirectory() as outside:
            target = Path(outside) / "external.d.ts"
            target.write_text("export interface External {}", encoding="utf-8")
            source = root / "sdk.d.ts"
            source.unlink()
            source.symlink_to(target)
            result = self.run_extract(root)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()
