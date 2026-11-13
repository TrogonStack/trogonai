import importlib.util
import json
from pathlib import Path
import sys
import unittest


HERE = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("reviewed_sdk_freshness", HERE / "check.py")
check = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = check
spec.loader.exec_module(check)


class ReviewedMappingTests(unittest.TestCase):
    def test_approved_core_contracts_override_aggregate_option_mappings(self):
        baseline = check.read_json(HERE / "baseline.json")
        features = check.validate_mapping(check.read_json(HERE / "mapping.json"), baseline)
        contracts = [
            ("configuration-preservation", "ClaudeAgentOptions.cli_path", "Options.pathToClaudeCodeExecutable", "ClaudeAgentOptions.cli_path"),
            ("configuration-preservation", "ClaudeAgentOptions.cwd", "Options.cwd", "ClaudeAgentOptions.cwd"),
            ("configuration-preservation", "ClaudeAgentOptions.model", "Options.model", "ClaudeAgentOptions.model"),
            ("permission-modes", "ClaudeAgentOptions.permission_mode", "Options.permissionMode", "ClaudeAgentOptions.permission_mode"),
            ("permission-modes", "PermissionMode", "PermissionMode", "PermissionMode"),
            ("setting-sources", "ClaudeAgentOptions.setting_sources", "Options.settingSources", "ClaudeAgentOptions.setting_sources"),
            ("setting-sources", "SettingSource", "SettingSource", "SettingSource"),
            ("client-lifecycle", "ClaudeSDKClient.connect", "Query.close", "ClaudeSdkClient.disconnect"),
            ("client-lifecycle", "ClaudeSDKClient.disconnect", "Query.close", "ClaudeSdkClient.disconnect"),
        ]
        for feature_id, python, typescript, rust_api in contracts:
            for language, symbol in [("python", python), ("typescript", typescript), ("typescript", "core." + typescript)]:
                with self.subTest(language=language, symbol=symbol):
                    feature = check.resolve_mapping(features, language, symbol)
                    self.assertIsNotNone(feature)
                    self.assertEqual(feature["id"], feature_id)
                    self.assertIn(rust_api, feature["rust_api"])
                    self.assertTrue(feature["tests"])

    def test_inherited_settings_are_not_reported_as_an_isolation_default(self):
        mapping = check.read_json(HERE / "mapping.json")
        isolation = next(feature for feature in mapping["features"] if feature["id"] == "isolation-defaults")
        self.assertNotIn("SettingSources", isolation["rust_api"])
        self.assertNotIn("ClaudeAgentOptions.setting_sources", isolation["upstream"]["python"]["symbols"])
        self.assertNotIn("Options.settingSources", isolation["upstream"]["typescript"]["symbols"])

    def test_checked_in_baseline_and_rust_evidence_are_valid(self):
        baseline = check.read_json(HERE / "baseline.json")
        check.validate_snapshot(baseline)
        self.assertEqual(baseline["extractor_fingerprint"], check.extractor_fingerprint())
        features = check.validate_mapping(check.read_json(HERE / "mapping.json"), baseline)
        report = check.comparison(baseline, baseline, features)
        self.assertFalse(report["upstream_drift"])
        self.assertTrue(report["needs_attention"])
        self.assertIn("session-convenience", {item["id"] for item in report["declared_known_gaps"]})
        self.assertIn("isolation-defaults", {item["id"] for item in report["intentional_differences"]})
        self.assertTrue(report["unreviewed_rust_mappings"]["python"]["observed"])
        self.assertTrue(report["unreviewed_rust_mappings"]["typescript"]["observed"])

    def test_baseline_pins_verified_official_release_artifacts(self):
        baseline = check.read_json(HERE / "baseline.json")
        python = baseline["sources"]["python"]
        typescript = baseline["sources"]["typescript"]
        self.assertTrue(python["artifact"]["url"].startswith("https://files.pythonhosted.org/"))
        self.assertRegex(python["artifact"]["integrity"], r"^sha256-[0-9a-f]{64}$")
        self.assertTrue(typescript["artifact"]["url"].startswith(
            "https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk/"
        ))
        self.assertTrue(typescript["artifact"]["integrity"].startswith("sha512-"))
        parser = json.loads((HERE / "package.json").read_text())
        self.assertEqual(parser["dependencies"]["typescript"], "6.0.3")


if __name__ == "__main__":
    unittest.main()
