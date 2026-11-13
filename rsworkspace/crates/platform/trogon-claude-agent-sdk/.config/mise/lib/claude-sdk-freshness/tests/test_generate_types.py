from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path


LIB = Path(__file__).resolve().parents[1]
GENERATOR = LIB / "generate_types.mjs"

DECLARATIONS = '''
export type Mode = "fast" | "slow-mode" | "acceptEdits";
export interface Sample {
  /** The name. */
  name: string;
  count?: number;
  note: string | null;
  tri?: string | null;
  anything: unknown;
  maybe?: unknown;
  "-B"?: number;
  flag?: true;
  mode?: Mode;
  inline?: "a" | "b";
  tags?: [string, ...string[]];
  questions: [{ q: string }] | [{ q: string }, { q: string }];
  lookup?: { [k: string]: string };
  open?: { type: string; [k: string]: unknown };
  interval?: 0 | number;
}
export interface Empty {}
export type Result =
  | { status: "ok"; value: number }
  | { status: "failed" | "skipped"; reason: string };
export type Loose = string | { [k: string]: unknown }[] | { [k: string]: unknown };
'''


class GenerateTypesTests(unittest.TestCase):
    def package(self, declarations: str = DECLARATIONS, version: str = "1.2.3") -> Path:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        (root / "package.json").write_text(json.dumps({"name": "fixture-sdk", "version": version}), encoding="utf-8")
        (root / "sdk-tools.d.ts").write_text(declarations, encoding="utf-8")
        return root

    def config(self, roots: list[str], **extra) -> Path:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        path = Path(directory.name) / "types.json"
        target = {"source": "sdk-tools.d.ts", "output": "generated.rs", "roots": roots, **extra}
        path.write_text(json.dumps({"package": "fixture-sdk", "version": "1.2.3", "targets": [target]}), encoding="utf-8")
        return path

    def run_generator(self, package: Path, config: Path, out: Path, *extra: str) -> subprocess.CompletedProcess:
        return subprocess.run(
            ["node", str(GENERATOR), "--typescript-package", str(package), "--config", str(config), "--out-root", str(out), *extra],
            capture_output=True,
            text=True,
            check=False,
        )

    def generate(self, roots: list[str], **extra) -> str:
        out = tempfile.TemporaryDirectory()
        self.addCleanup(out.cleanup)
        result = self.run_generator(self.package(), self.config(roots, **extra), Path(out.name))
        self.assertEqual(result.returncode, 0, result.stderr)
        return (Path(out.name) / "generated.rs").read_text(encoding="utf-8")

    ROOTS = ["Sample", "Empty", "Result", "Loose"]

    def test_output_is_deterministic_and_carries_provenance_without_local_paths(self):
        first = self.generate(self.ROOTS)
        self.assertEqual(first, self.generate(self.ROOTS))
        self.assertIn("fixture-sdk 1.2.3 sdk-tools.d.ts", first)
        self.assertNotIn("/Users/", first)
        self.assertNotIn("/Volumes/", first)

    def test_optional_nullable_and_unknown_fields_keep_distinct_shapes(self):
        output = self.generate(self.ROOTS)
        self.assertIn("pub count: Option<serde_json::Number>,", output)
        self.assertIn('#[serde(default, skip_serializing_if = "Option::is_none")]\n    pub count', output)
        self.assertIn("    #[serde(default)]\n    pub note: Option<String>,", output)
        self.assertIn('with = "super::support::double_option"', output)
        self.assertIn("pub tri: Option<Option<String>>,", output)
        self.assertIn("    #[serde(default)]\n    pub anything: serde_json::Value,", output)
        self.assertIn('deserialize_with = "super::support::optional_value"', output)
        self.assertIn('#[serde(rename = "-B", default', output)
        self.assertIn("pub dash_b: Option<serde_json::Number>,", output)
        self.assertIn("pub flag: Option<bool>,", output)

    def test_string_literal_unions_become_round_tripping_enums(self):
        output = self.generate(self.ROOTS)
        self.assertIn("pub enum Mode {", output)
        for variant in ("Fast,", "SlowMode,", "AcceptEdits,"):
            self.assertIn(f"    {variant}", output)
        self.assertIn("Unknown(String),", output)
        self.assertIn('Self::SlowMode => "slow-mode",', output)
        self.assertIn("_ => Self::Unknown(value),", output)
        self.assertIn("pub enum SampleInline {", output)

    def test_every_object_keeps_undeclared_keys_and_its_key_order(self):
        output = self.generate(self.ROOTS)
        self.assertIn("pub struct SampleOpen {", output)
        self.assertIn("pub struct Empty {", output)
        mirrors = output.count("#[serde(remote = ")
        self.assertGreater(mirrors, 0)
        self.assertEqual(output.count("#[serde(flatten)]"), mirrors)
        self.assertEqual(output.count("#[serde(skip)]"), mirrors)
        self.assertIn("pub extra: serde_json::Map<String, serde_json::Value>,", output)
        self.assertIn("pub const KNOWN_KEYS: &'static [&'static str] = &[];", output)
        self.assertIn("decoded.key_order = super::support::KeyOrder::of(&value);", output)

    def test_typed_maps_are_insertion_ordered(self):
        output = self.generate(self.ROOTS)
        self.assertIn("pub lookup: Option<indexmap::IndexMap<String, String>>,", output)
        self.assertNotIn("BTreeMap", output)

    def test_extern_references_are_not_generated_again(self):
        declarations = DECLARATIONS + "export type Wrapper = Sample | Result;\n"
        out = tempfile.TemporaryDirectory()
        self.addCleanup(out.cleanup)
        config = self.config(["Wrapper"], extern={"Result": "super::other::Result"})
        result = self.run_generator(self.package(declarations), config, Path(out.name))
        self.assertEqual(result.returncode, 0, result.stderr)
        output = (Path(out.name) / "generated.rs").read_text(encoding="utf-8")
        self.assertNotIn("pub enum Result {", output)
        self.assertIn("super::other::Result::best_match(value)", output)
        self.assertIn("pub struct Sample {", output)

    def test_numeric_literal_types_are_numbers(self):
        output = self.generate(self.ROOTS)
        self.assertIn("pub interval: Option<serde_json::Number>,", output)

    def test_bounded_tuple_unions_collapse_to_one_vector(self):
        output = self.generate(self.ROOTS)
        self.assertIn("pub questions: Vec<SampleQuestionsItem>,", output)
        self.assertIn("pub tags: Option<Vec<String>>,", output)
        self.assertEqual(output.count("pub struct SampleQuestionsItem"), 1)

    def test_object_unions_round_trip_or_keep_the_raw_value(self):
        output = self.generate(self.ROOTS)
        self.assertIn("pub enum Result {", output)
        self.assertIn("    Ok(ResultOk),", output)
        self.assertIn("    FailedOrSkipped(ResultFailedOrSkipped),", output)
        self.assertIn("Unknown(serde_json::Value),", output)
        self.assertIn("pub fn best_match(value: &serde_json::Value) -> Option<(usize, Self)>", output)
        self.assertIn("None => Self::Unknown(value),", output)
        self.assertIn("super::support::unknown_keys(value, ResultOk::KNOWN_KEYS)", output)
        # A plain map declares no keys, so any struct that does declare them is more specific.
        self.assertIn("super::support::unknown_keys(value, &[]), Self::Map(inner)", output)
        self.assertIn("pub enum Loose {", output)
        self.assertIn("    Text(String),", output)

    def test_omit_members_filters_a_union_without_generating_the_omitted_types(self):
        declarations = DECLARATIONS + 'export type All = Sample | Empty | Result;\n'
        out = tempfile.TemporaryDirectory()
        self.addCleanup(out.cleanup)
        config = self.config(["All"], omit_members={"All": ["Result"]})
        result = self.run_generator(self.package(declarations), config, Path(out.name))
        self.assertEqual(result.returncode, 0, result.stderr)
        output = (Path(out.name) / "generated.rs").read_text(encoding="utf-8")
        self.assertIn("pub enum All {", output)
        self.assertNotIn("pub enum Result", output)

    def test_unsupported_declarations_fail_instead_of_guessing(self):
        for declarations in ("export interface Bad { run(): void }", "export type Bad = Foo<string>;", "export interface Bad { x: Missing }"):
            out = tempfile.TemporaryDirectory()
            self.addCleanup(out.cleanup)
            result = self.run_generator(self.package(declarations), self.config(["Bad"]), Path(out.name))
            self.assertEqual(result.returncode, 2, declarations)
            self.assertIn("generate_types:", result.stderr)

    def test_package_version_mismatch_fails(self):
        out = tempfile.TemporaryDirectory()
        self.addCleanup(out.cleanup)
        result = self.run_generator(self.package(version="9.9.9"), self.config(self.ROOTS), Path(out.name))
        self.assertEqual(result.returncode, 2)
        self.assertIn("Expected fixture-sdk 1.2.3, found 9.9.9", result.stderr)

    def test_check_mode_reports_drift_and_accepts_matching_output(self):
        out = tempfile.TemporaryDirectory()
        self.addCleanup(out.cleanup)
        package, config = self.package(), self.config(self.ROOTS)
        self.assertEqual(self.run_generator(package, config, Path(out.name), "--check").returncode, 1)
        self.assertEqual(self.run_generator(package, config, Path(out.name)).returncode, 0)
        self.assertEqual(self.run_generator(package, config, Path(out.name), "--check").returncode, 0)
        generated = Path(out.name) / "generated.rs"
        generated.write_text(generated.read_text(encoding="utf-8") + "// edited\n", encoding="utf-8")
        result = self.run_generator(package, config, Path(out.name), "--check")
        self.assertEqual(result.returncode, 1)
        self.assertIn("generated.rs", result.stderr)

    def test_fixture_synthesis_sets_every_field_and_splits_families(self):
        out = tempfile.TemporaryDirectory()
        self.addCleanup(out.cleanup)
        config = self.config(
            ["Sample"],
            fixtures={"output": "fixtures.json", "roots": ["Sample", "Result"], "families": {"first": ["name", "count"]}},
        )
        result = subprocess.run(
            ["node", str(LIB / "synthesize_fixtures.mjs"), "--typescript-package", str(self.package()), "--config", str(config), "--out-root", out.name],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        fixtures = json.loads((Path(out.name) / "fixtures.json").read_text(encoding="utf-8"))
        self.assertEqual(set(fixtures), {"first", "other", "Result"})
        self.assertEqual(set(fixtures["first"][0]), {"name", "count"})
        self.assertNotIn("name", fixtures["other"][0])
        self.assertEqual({sample["status"] for sample in fixtures["Result"]}, {"ok", "skipped"})
        self.assertEqual(fixtures["other"][0]["open"]["extra_key"], {"any": ["thing", None]})

    def test_checked_in_generator_config_is_pinned_to_the_reviewed_baseline(self):
        config = json.loads((LIB / "types.json").read_text(encoding="utf-8"))
        baseline = json.loads((LIB / "baseline.json").read_text(encoding="utf-8"))
        pinned = baseline["sources"]["typescript"]
        self.assertEqual(config["package"], "@anthropic-ai/claude-agent-sdk")
        self.assertEqual(config["version"], pinned["version"])
        crate = LIB.parents[3]
        for target in config["targets"]:
            output = (crate / target["output"]).read_text(encoding="utf-8")
            self.assertIn(f"{config['package']} {config['version']} {target['source']}", output.splitlines()[0])


if __name__ == "__main__":
    unittest.main()
