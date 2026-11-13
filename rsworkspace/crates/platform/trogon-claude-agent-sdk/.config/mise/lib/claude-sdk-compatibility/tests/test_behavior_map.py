import base64
import copy
import hashlib
import io
import re
import tarfile
import unittest
from pathlib import Path

import behavior_map as bm
from gate_errors import Code, GateError

LIB = Path(__file__).resolve().parents[1]
REVIEWED = LIB / "upstream"
CRATE_TESTS = LIB.parents[3] / "tests"
TOP_LEVEL_TEST = re.compile(
    r"^#\[(?:tokio::)?test[^\]]*\]\n(?:(?:#\[.*\]|///.*)\n)*(?:async )?fn (\w+)", re.MULTILINE)


def source_test_selectors():
    """Top-level integration test functions, named the way nextest lists them."""
    return frozenset(f"{path.stem}::{name}"
                     for path in CRATE_TESTS.glob("*.rs")
                     for name in TOP_LEVEL_TEST.findall(path.read_text()))

PY_PATH = "src/pkg/mod.py"
TS_PATH = "package/sdk.mjs"
PY_BYTES = b"def a():\n    return 1\n\ndef b():\n    raise ValueError('x')\n"
TS_BYTES = b'let x=1;function f(e){if(e.type==="ping")return"pong";throw Error("unknown")}'
TARBALL = b"reviewed tarball bytes"
REVISION = "0" * 40


def sha(data):
    return hashlib.sha256(data).hexdigest()


def fixture_pins():
    return {
        "version": 1,
        "python": {
            "repository": "owner/repo",
            "revision": REVISION,
            "files": {PY_PATH: {
                "url": f"https://raw.githubusercontent.com/owner/repo/{REVISION}/{PY_PATH}",
                "sha256": sha(PY_BYTES), "bytes": len(PY_BYTES)}},
        },
        "typescript": {
            "package": "@scope/pkg",
            "version": "1.2.3",
            "url": "https://registry.npmjs.org/@scope/pkg/-/pkg-1.2.3.tgz",
            "integrity": "sha512-" + base64.b64encode(hashlib.sha512(TARBALL).digest()).decode(),
            "members": {TS_PATH: {"sha256": sha(TS_BYTES), "bytes": len(TS_BYTES)}},
        },
    }


def fixture_sources():
    return {"python": {PY_PATH: PY_BYTES}, "typescript": {TS_PATH: TS_BYTES}}


def ts_evidence(anchor='"ping"', before=8, after=16):
    span = bm.typescript_span(TS_BYTES, anchor.encode(), 0, before, after)
    return {"file": TS_PATH, "anchor": anchor, "occurrence": 0,
            "occurrences": len(bm.anchor_offsets(TS_BYTES, anchor.encode())),
            "span_before": before, "span_after": after, "sha256": sha(span)}


def row(row_id="r1", verdict="agree", decision=None):
    return {
        "id": row_id,
        "concept": "ping handling",
        "python": {"file": PY_PATH, "line_start": 4, "line_end": 5,
                   "sha256": sha(bm.python_span(PY_BYTES, 4, 5))},
        "typescript": ts_evidence(),
        "upstream": {"verdict": verdict, "behavior": "unknown input is rejected"},
        "decision": decision or {"kind": "upstream"},
        "contracts": [],
        "rust": ["src/query.rs"],
    }


def fixture_behaviors(*rows):
    return {"version": 1, "rows": list(rows) or [row()]}


class BehaviorMapTests(unittest.TestCase):
    def assertCode(self, code, call, *args, **kwargs):
        with self.assertRaises(GateError) as caught:
            call(*args, **kwargs)
        self.assertEqual(caught.exception.code, code)
        return caught.exception

    def validate(self, behaviors, pins=None, sources=None, **kwargs):
        return bm.validate(behaviors, pins or fixture_pins(), sources or fixture_sources(), **kwargs)

    def test_valid_fixture_returns_row_ids(self):
        self.assertEqual(self.validate(fixture_behaviors(row("a"), row("b"))), ("a", "b"))

    def test_absent_evidence_is_explicit(self):
        cited = row("a")
        absent = row("b")
        absent["typescript"] = {"status": "absent", "reason": "no such guard"}
        self.assertEqual(self.validate(fixture_behaviors(cited, absent)), ("a", "b"))

    def test_unknown_row_key_is_format(self):
        behaviors = fixture_behaviors()
        behaviors["rows"][0]["note"] = "extra"
        self.assertCode(Code.FORMAT, self.validate, behaviors)

    def test_unknown_evidence_key_is_format(self):
        behaviors = fixture_behaviors()
        behaviors["rows"][0]["python"]["column"] = 1
        self.assertCode(Code.FORMAT, self.validate, behaviors)

    def test_wrong_version_is_format(self):
        behaviors = fixture_behaviors()
        behaviors["version"] = 2
        self.assertCode(Code.FORMAT, self.validate, behaviors)

    def test_duplicate_json_keys_are_format(self):
        self.assertCode(Code.FORMAT, bm.load_json, b'{"id": "a", "id": "b"}', "behaviors")

    def test_invalid_utf8_is_format(self):
        self.assertCode(Code.FORMAT, bm.load_json, b'{"id": "\xff"}', "behaviors")

    def test_duplicate_row_id_is_mapping(self):
        self.assertCode(Code.MAPPING, self.validate, fixture_behaviors(row("a"), row("a")))

    def test_unpinned_python_file_is_reference(self):
        behaviors = fixture_behaviors()
        behaviors["rows"][0]["python"]["file"] = "src/pkg/other.py"
        self.assertCode(Code.REFERENCE, self.validate, behaviors)

    def test_line_range_past_end_is_reference(self):
        behaviors = fixture_behaviors()
        behaviors["rows"][0]["python"]["line_end"] = 99
        self.assertCode(Code.REFERENCE, self.validate, behaviors)

    def test_python_span_drift_is_shape(self):
        behaviors = fixture_behaviors()
        behaviors["rows"][0]["python"]["line_start"] = 5
        self.assertCode(Code.SHAPE, self.validate, behaviors)

    def test_missing_anchor_is_reference(self):
        behaviors = fixture_behaviors()
        behaviors["rows"][0]["typescript"]["anchor"] = '"pong!"'
        self.assertCode(Code.REFERENCE, self.validate, behaviors)

    def test_anchor_count_change_is_reference(self):
        behaviors = fixture_behaviors()
        evidence = ts_evidence('"p', before=0, after=4)
        evidence["occurrences"] = 1
        behaviors["rows"][0]["typescript"] = evidence
        error = self.assertCode(Code.REFERENCE, self.validate, behaviors)
        self.assertTrue(error.selector.endswith(".occurrences"))

    def test_span_outside_file_is_reference(self):
        behaviors = fixture_behaviors()
        behaviors["rows"][0]["typescript"]["span_after"] = len(TS_BYTES)
        self.assertCode(Code.REFERENCE, self.validate, behaviors)

    def test_typescript_span_drift_is_shape(self):
        behaviors = fixture_behaviors()
        behaviors["rows"][0]["typescript"]["span_after"] = 15
        self.assertCode(Code.SHAPE, self.validate, behaviors)

    def test_rust_adaptation_without_reason_is_mapping(self):
        behaviors = fixture_behaviors(row(decision={"kind": "rust-adaptation"}))
        self.assertCode(Code.MAPPING, self.validate, behaviors)

    def test_rust_adaptation_with_reason_is_accepted(self):
        behaviors = fixture_behaviors(
            row(decision={"kind": "rust-adaptation", "reason": "bounded queue"}))
        self.assertEqual(self.validate(behaviors), ("r1",))

    def test_upstream_decision_rejects_reason(self):
        behaviors = fixture_behaviors(row(decision={"kind": "upstream", "reason": "x"}))
        self.assertCode(Code.FORMAT, self.validate, behaviors)

    def test_agree_with_follow_decision_is_mapping(self):
        decision = {"kind": "follow-python", "reason": "stricter"}
        self.assertCode(Code.MAPPING, self.validate, fixture_behaviors(row(decision=decision)))

    def test_disagree_without_follow_decision_is_mapping(self):
        for decision in ({"kind": "upstream"}, {"kind": "rust-adaptation", "reason": "x"}):
            with self.subTest(decision=decision["kind"]):
                behaviors = fixture_behaviors(row(verdict="disagree", decision=decision))
                self.assertCode(Code.MAPPING, self.validate, behaviors)

    def test_disagree_with_follow_decision_is_accepted(self):
        decision = {"kind": "follow-typescript", "reason": "registration fails fast"}
        behaviors = fixture_behaviors(row(verdict="disagree", decision=decision))
        self.assertEqual(self.validate(behaviors), ("r1",))

    def test_required_contracts_must_be_nonempty(self):
        self.assertCode(Code.MAPPING, self.validate, fixture_behaviors(), require_contracts=True)

    def test_rust_paths_are_crate_relative(self):
        behaviors = fixture_behaviors()
        behaviors["rows"][0]["rust"] = ["crates/x/src/query.rs"]
        self.assertCode(Code.FORMAT, self.validate, behaviors)

    def test_unused_pin_is_mapping(self):
        behaviors = fixture_behaviors()
        behaviors["rows"][0]["python"] = {"status": "absent", "reason": "not implemented"}
        self.assertCode(Code.MAPPING, self.validate, behaviors)

    def test_changed_source_bytes_are_pin(self):
        sources = fixture_sources()
        sources["typescript"][TS_PATH] = TS_BYTES + b";"
        self.assertCode(Code.PIN, bm.verify_sources, fixture_pins(), sources)

    def test_missing_or_extra_source_is_pin(self):
        missing = fixture_sources()
        del missing["python"][PY_PATH]
        self.assertCode(Code.PIN, bm.verify_sources, fixture_pins(), missing)
        extra = fixture_sources()
        extra["python"]["src/pkg/extra.py"] = b""
        self.assertCode(Code.PIN, bm.verify_sources, fixture_pins(), extra)

    def test_url_not_derived_from_pin_is_pin(self):
        pins = fixture_pins()
        pins["python"]["files"][PY_PATH]["url"] = "https://example.com/mod.py"
        self.assertCode(Code.PIN, bm.verify_sources, pins, fixture_sources())
        pins = fixture_pins()
        pins["typescript"]["url"] = "https://registry.npmjs.org/@scope/pkg/-/pkg-9.9.9.tgz"
        self.assertCode(Code.PIN, bm.verify_sources, pins, fixture_sources())

    def test_tarball_integrity_mismatch_is_pin(self):
        bm.verify_tarball(fixture_pins(), TARBALL)
        self.assertCode(Code.PIN, bm.verify_tarball, fixture_pins(), TARBALL + b"x")

    def test_contracts_must_be_compiled_selectors(self):
        behaviors = fixture_behaviors()
        behaviors["rows"][0]["contracts"] = ["query::rejects_unknown"]
        bm.contracts_check(behaviors, frozenset({"query::rejects_unknown"}))
        self.assertCode(Code.REFERENCE, bm.contracts_check, behaviors, frozenset())

    def test_validation_does_not_mutate_input(self):
        behaviors = fixture_behaviors()
        before = copy.deepcopy(behaviors)
        self.validate(behaviors)
        self.assertEqual(behaviors, before)


class ReviewedMappingTests(unittest.TestCase):
    """Validate the committed mapping against reviewed upstream bytes when present."""

    def setUp(self):
        self.pins = bm.load_json((LIB / "behavior-pins.json").read_bytes(), "pins")
        self.behaviors = bm.load_json((LIB / "behaviors.json").read_bytes(), "behaviors")

    def reviewed_sources(self):
        tarball = REVIEWED / "typescript" / self.pins["typescript"]["url"].rsplit("/", 1)[-1]
        python = {path: REVIEWED / "python" / path for path in self.pins["python"]["files"]}
        if not tarball.is_file() or not all(path.is_file() for path in python.values()):
            self.skipTest("reviewed upstream bytes are not present")
        data = tarball.read_bytes()
        bm.verify_tarball(self.pins, data)
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
            members = {name: archive.extractfile(archive.getmember(name)).read()
                       for name in self.pins["typescript"]["members"]}
        return {"python": {path: file.read_bytes() for path, file in python.items()},
                "typescript": members}

    def test_mapping_is_well_formed(self):
        bm.pinned_files(self.pins)
        bm.contracts_check(self.behaviors, source_test_selectors())

    def test_mapping_matches_reviewed_bytes(self):
        ids = bm.validate(self.behaviors, self.pins, self.reviewed_sources())
        self.assertEqual(len(ids), 87)


if __name__ == "__main__":
    unittest.main()
