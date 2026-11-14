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
        self.assertIn("ResultOk::match_score(value)", output)
        self.assertIn("super::support::unlisted_literals(value, &[(\"status\", &[\"ok\"])])", output)
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

    def generate_from(self, declarations: str, roots: list[str], **extra) -> subprocess.CompletedProcess:
        out = tempfile.TemporaryDirectory()
        self.addCleanup(out.cleanup)
        result = self.run_generator(self.package(declarations), self.config(roots, **extra), Path(out.name))
        result.output = (Path(out.name) / "generated.rs").read_text(encoding="utf-8") if result.returncode == 0 else ""
        return result

    TAGGED = '''
import type { Foreign } from "other";
export type Kind = "unknown" | "known" | null;
export interface Base { type: "base"; payload: Foreign; kind: Kind }
export interface Sub { type: "system"; subtype: "init"; first: Base["payload"]; maybe?: string }
export interface Replay { type: "system"; subtype: "init"; isReplay: true; first: string }
export interface Other { type: "system"; subtype: "other" | "more"; size: Sub["maybe"] }
export type Message = Base | Sub | Replay | Other;
'''

    def test_foreign_types_take_the_configured_wire_shape(self):
        result = self.generate_from(self.TAGGED, ["Message"], foreign={"Foreign": "object"}, tagged={"Message": ["type", "subtype", "isReplay"]})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("pub payload: serde_json::Map<String, serde_json::Value>,", result.output)
        self.assertNotIn("pub struct Foreign", result.output)

    def test_indexed_access_resolves_to_the_named_property(self):
        result = self.generate_from(self.TAGGED, ["Message"], foreign={"Foreign": "string"}, tagged={"Message": ["type", "subtype", "isReplay"]})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("pub first: String,", result.output)
        # An optional property read through an index can be absent, so it is nullable.
        self.assertIn("pub size: Option<String>,", result.output)

    def test_nullable_aliases_and_a_listed_unknown_value_keep_the_fallback_variant(self):
        result = self.generate_from(self.TAGGED, ["Message"], foreign={"Foreign": "string"}, tagged={"Message": ["type", "subtype", "isReplay"]})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("pub type Kind = Option<KindValue>;", result.output)
        self.assertIn("    UnknownValue,", result.output)
        self.assertIn('Self::UnknownValue => "unknown",', result.output)
        self.assertIn("Unknown(String),", result.output)

    def test_tagged_unions_dispatch_on_literal_tags_before_shape(self):
        result = self.generate_from(self.TAGGED, ["Message"], foreign={"Foreign": "string"}, tagged={"Message": ["type", "subtype", "isReplay"]})
        self.assertEqual(result.returncode, 0, result.stderr)
        output = result.output
        self.assertIn('pub const TAGS: &\'static [&\'static str] = &["type", "subtype", "isReplay"];', output)
        self.assertIn('super::support::tag_str(value, "type", &["system"]) && super::support::tag_str(value, "subtype", &["init"]) && super::support::tag_bool(value, "isReplay", &[true])', output)
        self.assertIn('super::support::tag_str(value, "subtype", &["other", "more"])', output)
        self.assertIn("super::support::keep_most_specific(&mut best, 3,", output)
        self.assertIn("None => Self::Unknown(value),", output)

    def test_tagged_unions_reject_members_that_pin_no_tag(self):
        declarations = self.TAGGED + "export type Loose = Base | { other: string };\n"
        result = self.generate_from(declarations, ["Loose"], foreign={"Foreign": "string"}, tagged={"Loose": ["type"]})
        self.assertEqual(result.returncode, 2)
        self.assertIn("must list named members only", result.stderr)
        result = self.generate_from(self.TAGGED, ["Message"], foreign={"Foreign": "unsupported"})
        self.assertEqual(result.returncode, 2)
        self.assertIn("unsupported shape", result.stderr)

    def test_an_inline_property_union_dispatches_on_its_configured_tag(self):
        declarations = self.TAGGED + "export interface Holder { specific?: Base | Other }\n"
        result = self.generate_from(declarations, ["Holder"], foreign={"Foreign": "string"}, tagged={"HolderSpecific": ["type"]})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('super::support::tag_str(value, "type", &["base"])', result.output)
        self.assertIn("super::support::keep_most_specific(&mut best, 1,", result.output)
        result = self.generate_from(declarations, ["Holder"], foreign={"Foreign": "string"}, tagged={"HolderSpecifc": ["type"]})
        self.assertEqual(result.returncode, 2)
        self.assertIn("tagged names HolderSpecifc, which is not a union emitted for", result.stderr)

    INTERSECTED = '''
export interface Base { id: string; agent?: string }
export type Event = Base & { kind: "event"; agent: string; size?: number };
export type Clash = Base & { id: number };
/** The names. */
export declare const NAMES: readonly ["a", "b-c"];
export declare const BAD: readonly [1, 2];
'''

    def test_intersections_merge_their_members_and_a_required_redeclaration_wins(self):
        result = self.generate_from(self.INTERSECTED, ["Event"])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(
            "pub struct Event {\n    pub id: String,\n    pub agent: String,\n    pub kind: EventKind,\n    pub size: Option<serde_json::Number>,",
            result.output,
        )
        result = self.generate_from(self.INTERSECTED, ["Clash"])
        self.assertEqual(result.returncode, 2)
        self.assertIn("Intersection declares id with two types", result.stderr)

    def test_readonly_string_tuple_constants_become_rust_arrays(self):
        result = self.generate_from(self.INTERSECTED, ["NAMES"])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('/// The names.\npub const NAMES: [&str; 2] = ["a", "b-c"];', result.output)
        result = self.generate_from(self.INTERSECTED, ["BAD"])
        self.assertEqual(result.returncode, 2)
        self.assertIn("Constant BAD lists something other than a string literal", result.stderr)

    def test_fixture_synthesis_merges_intersection_members(self):
        out = tempfile.TemporaryDirectory()
        self.addCleanup(out.cleanup)
        config = self.config(["Event"], fixtures={"output": "fixtures.json", "roots": ["Event"]})
        result = subprocess.run(
            ["node", str(LIB / "synthesize_fixtures.mjs"), "--typescript-package", str(self.package(self.INTERSECTED)), "--config", str(config), "--out-root", out.name],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        fixtures = json.loads((Path(out.name) / "fixtures.json").read_text(encoding="utf-8"))
        self.assertEqual(fixtures, {"Event": [{"id": "text", "agent": "text", "kind": "event", "size": 3}]})

    def test_fixture_synthesis_samples_foreign_names_and_indexed_access(self):
        declarations = '''
import type { Outside } from "elsewhere";
export interface Holder { mode: "a" | "b"; count: number }
export interface Carrier { id: ID; body: Outside; part: Outside["part"]; mode: Holder["mode"] }
'''
        out = tempfile.TemporaryDirectory()
        self.addCleanup(out.cleanup)
        config = self.config(
            ["Carrier"],
            foreign={"ID": "string", "Outside": "object"},
            fixtures={"output": "fixtures.json", "roots": ["Carrier"]},
        )
        result = subprocess.run(
            ["node", str(LIB / "synthesize_fixtures.mjs"), "--typescript-package", str(self.package(declarations)), "--config", str(config), "--out-root", out.name],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        fixtures = json.loads((Path(out.name) / "fixtures.json").read_text(encoding="utf-8"))
        self.assertEqual(
            fixtures,
            {"Carrier": [{"id": "text", "body": {"foreign": "object"}, "part": {"any": ["thing", None]}, "mode": "a"}]},
        )

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
