from __future__ import annotations

import base64
from copy import deepcopy
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import Mock, patch


MODULE_PATH = Path(__file__).resolve().parents[1] / "check.py"
SPEC = importlib.util.spec_from_file_location("claude_sdk_freshness_check_tests", MODULE_PATH)
check = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(check)


def symbol(shape="value: str", line=1):
    return {"kind": "field", "shape": shape, "source": {"path": "types.d.ts", "line": line}}


def snapshot():
    return {
        "schema_version": 1,
        "extractor_fingerprint": {"schema_version": 1, "files": {name: "c" * 64 for name in check.FINGERPRINT_FILES}, "typescript_version": "6.0.3", "python_ast_version": "3.14"},
        "sources": {
            language: {
                "version": "1.0.0",
                "artifact": {"kind": "local"},
                "inventory": {"schema_version": 1, "language": language, "symbols": {"Options.value": symbol()}},
                "runtime_hashes": {"runtime.py" if language == "python" else "sdk.mjs": "a" * 64},
            }
            for language in check.LANGUAGES
        },
    }


def feature(name="value", status="supported", symbols=None, prefixes=None):
    return {
        "id": name,
        "upstream": {language: {"symbols": ["Options.value"] if symbols is None else symbols, "prefixes": prefixes or []} for language in check.LANGUAGES},
        "rust_api": ["Options::value"],
        "tests": [{"path": "test.rs", "name": "retains_value"}],
        "status": status,
        "rationale": "The field uses the reviewed Rust value object and public regression.",
    }


def tar_bytes(entries):
    result = io.BytesIO()
    with tarfile.open(fileobj=result, mode="w:gz") as archive:
        for name, contents, kind in entries:
            member = tarfile.TarInfo(name)
            if kind == "symlink":
                member.type = tarfile.SYMTYPE
                member.linkname = "../../outside"
                archive.addfile(member)
            else:
                value = contents.encode()
                member.size = len(value)
                archive.addfile(member, io.BytesIO(value))
    return result.getvalue()


class ComparisonTests(unittest.TestCase):
    def setUp(self):
        self.baseline = snapshot()
        self.mapping = [feature()]

    def test_source_line_movement_is_not_api_drift(self):
        current = deepcopy(self.baseline)
        current["sources"]["python"]["inventory"]["symbols"]["Options.value"]["source"]["line"] = 20
        report = check.comparison(self.baseline, current, self.mapping)
        self.assertFalse(report["upstream_drift"])
        self.assertFalse(report["needs_attention"])

    def test_extractor_change_cannot_be_misreported_as_upstream_drift(self):
        baseline = deepcopy(self.baseline)
        baseline["extractor_fingerprint"]["files"]["extract_python.py"] = "a" * 64
        current = deepcopy(baseline)
        current["extractor_fingerprint"]["files"]["extract_python.py"] = "b" * 64
        with self.assertRaisesRegex(check.SnapshotError, "extractor changed"):
            check.comparison(baseline, current, self.mapping)

    def test_requiredness_changes_need_review_even_when_supported(self):
        current = deepcopy(self.baseline)
        current["sources"]["typescript"]["inventory"]["symbols"]["Options.value"]["shape"] = "value?: str"
        report = check.comparison(self.baseline, current, self.mapping)
        change = report["sources"]["typescript"]["api_changes"][0]
        self.assertEqual(change["change"], "changed")
        self.assertEqual(change["mapping_status"], "supported")
        self.assertTrue(change["needs_review"])

    def test_changed_symbol_report_carries_rust_evidence(self):
        current = deepcopy(self.baseline)
        current["sources"]["python"]["inventory"]["symbols"]["Options.value"]["shape"] = "value: int"
        report = check.comparison(self.baseline, current, self.mapping)
        change = report["sources"]["python"]["api_changes"][0]
        self.assertEqual(change["rust_evidence"]["rust_api"], ["Options::value"])
        self.assertEqual(change["rust_evidence"]["tests"][0]["name"], "retains_value")
        self.assertIn("types.d.ts:1", check.markdown(report))
        self.assertIn("retains_value", check.markdown(report))

    def test_new_symbol_under_supported_prefix_always_needs_review(self):
        current = deepcopy(self.baseline)
        current["sources"]["python"]["inventory"]["symbols"]["Options.new"] = symbol()
        mapping = [feature(symbols=[], prefixes=["Options"])]
        report = check.comparison(self.baseline, current, mapping)
        change = report["sources"]["python"]["api_changes"][0]
        self.assertEqual(change["symbol"], "Options.new")
        self.assertEqual(change["mapping_status"], "supported")
        self.assertTrue(change["needs_review"])

    def test_removal_is_preserved_with_reviewed_mapping(self):
        current = deepcopy(self.baseline)
        current["sources"]["python"]["inventory"]["symbols"] = {"Replacement": symbol()}
        report = check.comparison(self.baseline, current, self.mapping)
        self.assertEqual([entry["change"] for entry in report["sources"]["python"]["api_changes"]], ["removed", "added"])

    def test_version_only_change_is_release_review(self):
        current = deepcopy(self.baseline)
        current["sources"]["python"]["version"] = "1.0.1"
        report = check.comparison(self.baseline, current, self.mapping)
        self.assertTrue(report["upstream_drift"])
        self.assertEqual(report["sources"]["python"]["api_changes"], [])
        self.assertEqual(report["sources"]["python"]["runtime_content_changes"], [])

    def test_runtime_only_change_is_review_signal_not_semantic_claim(self):
        current = deepcopy(self.baseline)
        current["sources"]["typescript"]["runtime_hashes"]["sdk.mjs"] = "b" * 64
        report = check.comparison(self.baseline, current, self.mapping)
        self.assertTrue(report["upstream_drift"])
        body = check.markdown(report)
        self.assertIn("not proven semantic changes", body)
        self.assertEqual(report["sources"]["typescript"]["api_changes"], [])

    def test_known_gaps_are_distinct_from_release_drift(self):
        report = check.comparison(self.baseline, self.baseline, [feature(status="partial")])
        self.assertFalse(report["upstream_drift"])
        self.assertTrue(report["needs_attention"])
        self.assertEqual(report["declared_known_gaps"][0]["status"], "partial")

    def test_unreviewed_symbols_do_not_claim_missing_functionality(self):
        report = check.comparison(self.baseline, self.baseline, [])
        self.assertTrue(report["needs_attention"])
        self.assertFalse(report["upstream_drift"])
        self.assertIn("not proof that functionality is missing", check.markdown(report))
        self.assertEqual(report["unreviewed_rust_mappings"]["python"]["reviewed_baseline"], ["Options.value"])

    def test_explicit_rust_difference_is_not_an_implementation_gap(self):
        report = check.comparison(self.baseline, self.baseline, [feature(status="rust_difference")])
        self.assertFalse(report["needs_attention"])
        self.assertEqual(report["intentional_differences"][0]["status"], "rust_difference")

    def test_equal_version_registry_digest_change_is_visible(self):
        current = deepcopy(self.baseline)
        self.baseline["sources"]["python"]["artifact"] = {"url": "https://files.pythonhosted.org/old.tgz", "integrity": "sha256-" + "a" * 64}
        current["sources"]["python"]["artifact"] = {"url": "https://files.pythonhosted.org/new.tgz", "integrity": "sha256-" + "b" * 64}
        report = check.comparison(self.baseline, current, self.mapping)
        self.assertTrue(report["sources"]["python"]["registry_integrity_changed"])
        self.assertTrue(report["upstream_drift"])

    def test_issue_is_bounded_with_full_artifact_link(self):
        current = deepcopy(self.baseline)
        current["sources"]["typescript"]["inventory"]["symbols"].update({f"Options.new_{index}_{'x' * 300}": symbol() for index in range(1000)})
        report = check.comparison(self.baseline, current, self.mapping)
        run_url = "https://github.com/example/sdk/actions/runs/12"
        body = check.markdown(report, run_url, bounded=True)
        self.assertLessEqual(len(body), check.MAX_ISSUE_CHARS)
        self.assertIn(run_url, body)
        self.assertGreater(len(check.markdown(report)), len(body))


class MappingTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / "test.rs").write_text("#[tokio::test]\nasync fn retains_value() {}\n")
        self.baseline = snapshot()
        self.mapping = {"schema_version": 1, "features": [feature()]}

    def validate(self):
        return check.validate_mapping(self.mapping, self.baseline, self.root)

    def test_valid_supported_mapping_requires_real_named_test(self):
        self.assertEqual(len(self.validate()), 1)

    def test_exact_symbol_overrides_partial_group(self):
        group = feature("options-group", status="partial", symbols=[], prefixes=["Options"])
        exact = feature()
        self.mapping["features"] = [group, exact]
        self.validate()
        self.assertEqual(check.resolve_mapping(self.mapping["features"], "python", "Options.value")["id"], "value")

    def test_longest_prefix_wins_without_matching_unrelated_names(self):
        group = feature("options-group", status="partial", symbols=[], prefixes=["Options"])
        child = feature("nested", status="deferred", symbols=[], prefixes=["Options.value"])
        self.assertEqual(check.resolve_mapping([group, child], "python", "Options.value.child")["id"], "nested")
        self.assertIsNone(check.resolve_mapping([group, child], "python", "Options2.value"))

    def test_ambiguous_exact_claim_is_an_error(self):
        self.mapping["features"].append(feature("other"))
        with self.assertRaises(check.MappingError):
            self.validate()

    def test_unknown_status_is_rejected(self):
        self.mapping["features"][0]["status"] = "probably-supported"
        with self.assertRaises(check.MappingError):
            self.validate()

    def test_missing_rationale_is_rejected(self):
        self.mapping["features"][0]["rationale"] = "  "
        with self.assertRaises(check.MappingError):
            self.validate()

    def test_missing_test_file_is_rejected(self):
        (self.root / "test.rs").unlink()
        with self.assertRaises(check.MappingError):
            self.validate()

    def test_stale_test_function_is_rejected(self):
        self.mapping["features"][0]["tests"][0]["name"] = "removed_test"
        with self.assertRaises(check.MappingError):
            self.validate()

    def test_comment_or_string_cannot_impersonate_a_test_function(self):
        (self.root / "test.rs").write_text('// fn retains_value() {}\nconst TEXT: &str = r#"fn retains_value() {}"#;\n')
        with self.assertRaises(check.MappingError):
            self.validate()

    def test_test_path_escape_is_rejected(self):
        self.mapping["features"][0]["tests"][0]["path"] = "../test.rs"
        with self.assertRaises(check.MappingError):
            self.validate()

    def test_stale_symbol_selector_is_rejected(self):
        self.mapping["features"][0]["upstream"]["python"]["symbols"] = ["Missing"]
        with self.assertRaises(check.MappingError):
            self.validate()

    def test_supported_without_test_evidence_is_rejected(self):
        self.mapping["features"][0]["tests"] = []
        with self.assertRaises(check.MappingError):
            self.validate()

    def test_duplicate_feature_id_is_rejected(self):
        self.mapping["features"].append(feature())
        with self.assertRaises(check.MappingError):
            self.validate()


class FetchAndArchiveTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def archive(self, entries):
        path = self.root / "source.tgz"
        path.write_bytes(tar_bytes(entries))
        return path

    def test_integrity_mismatch_fails_before_any_extraction(self):
        archive = self.archive([("package/sdk.mjs", "export {}", "file")])
        with self.assertRaises(check.FetchError):
            check.verify_archive(archive, "sha512", base64.b64encode(b"wrong").decode())
        self.assertFalse((self.root / "extracted").exists())

    def test_safe_extraction_selects_source_without_executing_it(self):
        archive = self.archive([("package/sdk.mjs", "throw new Error('must not run')", "file"), ("package/package.json", '{"version":"1.0.0"}', "file"), ("package/install.sh", "exit 1", "file")])
        output = check.extract_archive(archive, self.root / "extracted", "typescript")
        self.assertTrue((output / "sdk.mjs").is_file())
        self.assertFalse((output / "install.sh").exists())

    def test_archive_traversal_is_rejected(self):
        archive = self.archive([("package/../../outside.mjs", "bad", "file")])
        with self.assertRaises(check.FetchError):
            check.extract_archive(archive, self.root / "extracted", "typescript")
        self.assertFalse((self.root / "outside.mjs").exists())

    def test_archive_symlink_is_rejected_even_when_not_selected(self):
        archive = self.archive([("package/link", "", "symlink")])
        with self.assertRaises(check.FetchError):
            check.extract_archive(archive, self.root / "extracted", "typescript")

    def test_duplicate_archive_destination_is_rejected(self):
        archive = self.archive([("package/sdk.mjs", "first", "file"), ("package/sdk.mjs", "second", "file")])
        with self.assertRaises(check.FetchError):
            check.extract_archive(archive, self.root / "extracted", "typescript")
        self.assertEqual((self.root / "extracted/sdk.mjs").read_text(), "first")

    def test_archive_expansion_limit_is_enforced(self):
        archive = self.archive([("package/sdk.mjs", "too big", "file")])
        with patch.object(check, "MAX_EXPANDED_BYTES", 3), self.assertRaises(check.FetchError):
            check.extract_archive(archive, self.root / "extracted", "typescript")

    def test_redirect_is_rejected_before_opening_unapproved_host(self):
        redirect = check.RegistryRedirect()
        with self.assertRaises(check.FetchError):
            redirect.redirect_request(Mock(), Mock(), 302, "redirect", {}, "https://example.invalid/package.tgz")

    def test_download_size_is_enforced_without_trusting_content_length(self):
        response = Mock()
        response.url = "https://registry.npmjs.org/package.tgz"
        response.headers = {}
        response.read.side_effect = [b"12345", b""]
        response.__enter__ = Mock(return_value=response)
        response.__exit__ = Mock(return_value=False)
        opener = Mock()
        opener.open.return_value = response
        with patch.object(check, "build_opener", return_value=opener), self.assertRaises(check.FetchError):
            check.download(response.url, self.root / "download.tgz", 4)

    def test_exact_pypi_release_uses_verified_sdist(self):
        archive = tar_bytes([("claude_agent_sdk-1.2.3/pyproject.toml", '[project]\nversion="1.2.3"', "file"), ("claude_agent_sdk-1.2.3/src/claude_agent_sdk/__init__.py", "raise RuntimeError('must not run')", "file")])
        digest = hashlib.sha256(archive).hexdigest()
        metadata = {"info": {"name": "claude-agent-sdk", "version": "1.2.3"}, "urls": [{"packagetype": "sdist", "digests": {"sha256": digest}, "url": "https://files.pythonhosted.org/sdk.tgz"}]}
        with patch.object(check, "fetch_json", return_value=metadata) as fetch, patch.object(check, "download", side_effect=lambda url, destination, limit: destination.write_bytes(archive)):
            root, version, artifact = check.fetch_python(self.root, "1.2.3")
        self.assertIn("/1.2.3/json", fetch.call_args.args[0])
        self.assertEqual(version, "1.2.3")
        self.assertEqual(artifact["integrity"], "sha256-" + digest)
        self.assertTrue((root / "src/claude_agent_sdk/__init__.py").exists())

    def test_exact_npm_release_uses_verified_published_package(self):
        archive = tar_bytes([("package/package.json", '{"version":"1.2.3"}', "file"), ("package/sdk.d.ts", "export type Test = string;", "file"), ("package/sdk.mjs", "throw Error('must not run')", "file")])
        integrity = "sha512-" + base64.b64encode(hashlib.sha512(archive).digest()).decode()
        metadata = {"name": "@anthropic-ai/claude-agent-sdk", "version": "1.2.3", "dist": {"integrity": integrity, "tarball": "https://registry.npmjs.org/sdk.tgz"}}
        with patch.object(check, "fetch_json", return_value=metadata) as fetch, patch.object(check, "download", side_effect=lambda url, destination, limit: destination.write_bytes(archive)):
            root, version, artifact = check.fetch_typescript(self.root, "1.2.3")
        self.assertTrue(fetch.call_args.args[0].endswith("/1.2.3"))
        self.assertEqual(version, "1.2.3")
        self.assertEqual(artifact["integrity"], integrity)
        self.assertTrue((root / "sdk.d.ts").exists())

    def test_version_mismatch_fails_before_archive_download(self):
        metadata = {"name": "@anthropic-ai/claude-agent-sdk", "version": "9.9.9"}
        with patch.object(check, "fetch_json", return_value=metadata), patch.object(check, "download") as download, self.assertRaises(check.FetchError):
            check.fetch_typescript(self.root, "1.2.3")
        download.assert_not_called()

    def test_empty_inventory_cannot_appear_healthy(self):
        with self.assertRaises(check.SnapshotError):
            check.validate_inventory({"schema_version": 1, "language": "python", "symbols": {}}, "python")

    def test_source_path_cannot_leak_local_absolute_paths(self):
        inventory = snapshot()["sources"]["python"]["inventory"]
        inventory["symbols"]["Options.value"]["source"]["path"] = str(self.root / "private.py")
        with self.assertRaises(check.SnapshotError):
            check.validate_inventory(inventory, "python")


class IssueAndCliTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.body = self.root / "issue.md"
        self.body.write_text("review")
        self.report = check.comparison(snapshot(), snapshot(), [])

    def test_clean_report_never_calls_github(self):
        clean = check.comparison(snapshot(), snapshot(), [feature()])
        with patch.object(check, "run_gh") as command:
            self.assertEqual(check.sync_issue("example/sdk", clean, self.body), "not-needed")
        command.assert_not_called()

    def test_existing_marker_across_pages_updates_one_issue(self):
        existing = [[{"number": 2, "body": "unrelated"}], [{"number": 7, "body": check.ISSUE_MARKER + "\nold", "user": {"login": "github-actions[bot]", "type": "Bot"}}]]
        with patch.object(check, "run_gh", side_effect=[json.dumps(existing), ""]) as command:
            self.assertEqual(check.sync_issue("example/sdk", self.report, self.body), "updated")
        args = command.call_args_list[1].args[0]
        self.assertEqual(args[:3], ["issue", "edit", "7"])
        self.assertIn("--body-file", args)
        self.assertFalse(any("create" in call.args[0] for call in command.call_args_list))

    def test_forged_marker_cannot_redirect_or_block_the_workflow_issue(self):
        forged = [
            {"number": 1, "body": check.ISSUE_MARKER, "user": {"login": "unrelated", "type": "User"}},
            {"number": 2, "body": check.ISSUE_MARKER, "user": {"login": "other[bot]", "type": "Bot"}},
            {"number": 3, "body": check.ISSUE_MARKER, "user": None},
            {"number": 4, "body": check.ISSUE_MARKER},
        ]
        owned = {"number": 7, "body": check.ISSUE_MARKER, "user": {"login": "github-actions[bot]", "type": "Bot"}}
        for issues, expected in [(forged, ["issue", "create"]), ([*forged, owned], ["issue", "edit", "7"])]:
            with self.subTest(owned=owned in issues), patch.object(check, "run_gh", side_effect=[json.dumps([issues]), ""]) as command:
                self.assertIn(check.sync_issue("example/sdk", self.report, self.body), ("created", "updated"))
                self.assertEqual(command.call_count, 2)
                self.assertEqual(command.call_args.args[0][:len(expected)], expected)

    def test_first_report_creates_only_one_issue(self):
        with patch.object(check, "run_gh", side_effect=["[[]]", ""]) as command:
            self.assertEqual(check.sync_issue("example/sdk", self.report, self.body), "created")
        self.assertEqual(command.call_count, 2)
        self.assertEqual(command.call_args.args[0][:2], ["issue", "create"])

    def test_ambiguous_existing_issues_do_not_create_more(self):
        existing = [[{"number": 1, "body": check.ISSUE_MARKER, "user": {"login": "github-actions[bot]", "type": "Bot"}}, {"number": 2, "body": check.ISSUE_MARKER, "user": {"login": "github-actions[bot]", "type": "Bot"}}]]
        with patch.object(check, "run_gh", return_value=json.dumps(existing)) as command, self.assertRaises(check.CommandError):
            check.sync_issue("example/sdk", self.report, self.body)
        self.assertEqual(command.call_count, 1)

    def test_gh_failure_preserves_typed_error_without_stderr_values(self):
        with patch.object(check.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "gh", stderr="private local details")), self.assertRaises(check.CommandError) as error:
            check.run_gh(["api", "example"])
        self.assertNotIn("private local details", str(error.exception))

    def test_fetch_failure_cannot_sync_a_drift_issue(self):
        with patch.object(check, "fetch_python", side_effect=check.FetchError("registry unavailable")), patch.object(check, "sync_issue") as sync, patch("sys.stderr", new=io.StringIO()) as error:
            result = check.main(["--out", str(self.root / "out"), "--file-issue", "--repository", "example/sdk"])
        self.assertEqual(result, 1)
        self.assertIn("registry unavailable", error.getvalue())
        sync.assert_not_called()
        self.assertFalse((self.root / "out/report.json").exists())
        self.assertEqual(json.loads((self.root / "out/failure.json").read_text())["status"], "failed")

    def test_baseline_write_and_issue_publication_are_mutually_exclusive(self):
        with patch.object(check, "fetch_python") as fetch, patch("sys.stderr", new=io.StringIO()):
            result = check.main(["--out", str(self.root / "out"), "--write-baseline", str(self.root / "baseline.json"), "--file-issue"])
        self.assertEqual(result, 1)
        fetch.assert_not_called()

    def test_offline_baseline_write_is_explicit_and_never_publishes(self):
        python_root = self.root / "python"
        typescript_root = self.root / "typescript"
        python_root.mkdir()
        typescript_root.mkdir()
        (python_root / "pyproject.toml").write_text('[project]\nversion = "1.0.0"\n')
        (typescript_root / "package.json").write_text('{"version":"1.0.0"}')
        sources = snapshot()["sources"]
        baseline = self.root / "baseline.json"
        with patch.object(check, "extractor_fingerprint", return_value=snapshot()["extractor_fingerprint"]), patch.object(check, "source_snapshot", side_effect=lambda root, language, version, artifact: sources[language]), patch.object(check, "sync_issue") as sync, patch.object(check, "fetch_python") as fetch, patch("sys.stdout", new=io.StringIO()):
            result = check.main(["--out", str(self.root / "out"), "--python-source", str(python_root), "--typescript-package", str(typescript_root), "--write-baseline", str(baseline)])
        self.assertEqual(result, 0)
        sync.assert_not_called()
        fetch.assert_not_called()
        self.assertEqual(json.loads(baseline.read_text()), snapshot())

    def test_periodic_check_does_not_rewrite_reviewed_baseline(self):
        baseline = self.root / "baseline.json"
        baseline.write_text(json.dumps(snapshot()))
        original = baseline.read_bytes()
        mapping = self.root / "mapping.json"
        mapping.write_text('{"schema_version":1,"features":[]}')
        sources = snapshot()["sources"]
        with patch.object(check, "extractor_fingerprint", return_value=snapshot()["extractor_fingerprint"]), patch.object(check, "fetch_python", return_value=(self.root, "1.0.0", {"kind": "local"})), patch.object(check, "fetch_typescript", return_value=(self.root, "1.0.0", {"kind": "local"})), patch.object(check, "source_snapshot", side_effect=lambda root, language, version, artifact: sources[language]), patch("sys.stdout", new=io.StringIO()):
            result = check.main(["--out", str(self.root / "out"), "--baseline", str(baseline), "--mapping", str(mapping)])
        self.assertEqual(result, 0)
        self.assertEqual(baseline.read_bytes(), original)
        self.assertTrue((self.root / "out/report.md").exists())


if __name__ == "__main__":
    unittest.main()
