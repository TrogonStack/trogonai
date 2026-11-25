"""Guards that keep SPIFFE federation unrendered and unimplemented.

The repository carries the federation contract (proto and fixtures) but no
bundle endpoint fetcher or poller. These tests fail the moment either appears
without the contract checks having their own tests."""

import json
import os
import unittest
from pathlib import Path

from render import render
from test_render import configuration, walk

REPOSITORY = Path(__file__).resolve().parents[3]
FEDERATION_KEYS = {"federation", "federates_with", "bundle_endpoint", "bundle_endpoint_url", "federates_with_bundle_endpoint"}
CONTRACT_TYPES = ["SpiffeBundleFetch", "SpiffeTrustProfile", "SpiffeHttpsWeb", "SpiffeHttpsSpiffe", "TrustRegistration", "spiffe_refresh_hint"]
SOURCE_SUFFIXES = {".rs", ".go", ".py", ".ts", ".tsx", ".js"}
SCANNED_TREES = ["rsworkspace", "goworkspace", "devops"]
SKIPPED_DIRECTORIES = {".git", "target", "node_modules", "bazel-bin", "bazel-out", "bazel-testlogs", ".trogonai"}
REQUIRED_CHECKS = (
    "a bundle fetcher must carry tests for: https only (no redirects), refresh interval <= 300s and <= the bundle "
    "spiffe_refresh_hint, rotation overlap between 3x and 5x the refresh interval, the authenticated endpoint SPIFFE ID "
    "equal to the expected one, no fallback from one endpoint profile to another, and bootstrap roots scoped to the "
    "endpoint identity. See proto/trogonai/platform/workload_federation/v1alpha1/trust_registration.proto and "
    "tests/proto/workload_federation/fixtures"
)


def federation_keys(files):
    """Every (file, key) of a rendered JSON document that configures federation."""
    return sorted((name, key) for name, text in files.items() for key in set(walk(json.loads(text))) if key in FEDERATION_KEYS)


def contract_references(root):
    """Every non-proto, non-fixture source file that mentions a federation contract type."""
    own = Path(__file__).resolve()
    hits = []
    for tree in SCANNED_TREES:
        for directory, names, files in os.walk(Path(root, tree)):
            names[:] = [name for name in names if name not in SKIPPED_DIRECTORIES]
            for file in files:
                path = Path(directory, file)
                if path.suffix not in SOURCE_SUFFIXES or path.resolve() == own:
                    continue
                text = path.read_text(errors="replace")
                hits.extend((str(path.relative_to(root)), marker) for marker in CONTRACT_TYPES if marker in text)
    return sorted(hits)


class FederationAbsentTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.files = {name: text for name, text in render(configuration(), lambda image: []).items() if name.endswith(".json") or name.endswith(".conf")}

    def test_no_rendered_document_configures_federation(self):
        self.assertEqual(federation_keys(self.files), [])

    def test_the_rendered_guard_fails_when_a_federation_key_is_injected_into_any_document(self):
        for name in self.files:
            for key in sorted(FEDERATION_KEYS):
                with self.subTest(file=name, key=key):
                    document = json.loads(self.files[name])
                    (document[0] if isinstance(document, list) else document)[key] = {}
                    self.assertEqual(federation_keys({**self.files, name: json.dumps(document)}), [(name, key)])

    def test_no_source_file_consumes_the_federation_contract_so_there_is_no_runtime_to_diverge(self):
        if not (REPOSITORY / "rsworkspace").is_dir():
            self.skipTest("the repository source tree is not available to this runner")
        self.assertEqual(contract_references(REPOSITORY), [], REQUIRED_CHECKS)

    def test_the_source_guard_fails_when_a_consumer_is_added(self):
        import tempfile

        with tempfile.TemporaryDirectory() as root:
            crate = Path(root, "rsworkspace", "crates", "fetcher", "src")
            crate.mkdir(parents=True)
            crate.joinpath("lib.rs").write_text("use x::SpiffeBundleFetchEvidence;")
            Path(root, "goworkspace").mkdir()
            Path(root, "goworkspace", "poll.go").write_text("var hint = spiffe_refresh_hint")
            self.assertEqual(
                contract_references(Path(root)),
                [("goworkspace/poll.go", "spiffe_refresh_hint"), ("rsworkspace/crates/fetcher/src/lib.rs", "SpiffeBundleFetch")],
            )


if __name__ == "__main__":
    unittest.main()
