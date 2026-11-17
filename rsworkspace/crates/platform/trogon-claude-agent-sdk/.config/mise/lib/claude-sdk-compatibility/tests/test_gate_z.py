import json
import unittest
from pathlib import Path

import gate_z
from gate_errors import Code, GateError

LIB = Path(__file__).resolve().parents[1]
CRATE = LIB.parents[3]
WORKSPACE = CRATE.parents[2]
MAPPING = CRATE / ".config" / "mise" / "lib" / "claude-sdk-freshness" / "mapping.json"
TS_ABSENT = {"status": "absent", "reason": "TypeScript has no such form."}
TS_PRESENT = {"file": "package/sdk.mjs", "anchor": "x"}


def row(row_id, kind="upstream", reason=None, contracts=("tests_binary::pins",), typescript=None):
    decision = {"kind": kind} if reason is None else {"kind": kind, "reason": reason}
    return {"id": row_id, "decision": decision, "contracts": list(contracts),
            "typescript": TS_PRESENT if typescript is None else typescript}


def behaviors(*rows):
    return {"version": 1, "rows": list(rows)}


def lock(*packages):
    blocks = []
    for name, version, dependencies, *source in packages:
        listed = "".join(f' "{dependency}",\n' for dependency in dependencies)
        origin = f'source = "{source[0]}"\n' if source else ""
        blocks.append(f'[[package]]\nname = "{name}"\nversion = "{version}"\n{origin}dependencies = [\n{listed}]\n')
    return "version = 4\n\n" + "\n".join(blocks)


class MappingTests(unittest.TestCase):
    def test_supported_features_pass(self):
        mapping = {"features": [{"id": "a", "status": "supported"}, {"id": "b", "status": "supported"}]}
        self.assertEqual(gate_z.unsupported_features(mapping), [])

    def test_every_other_status_is_a_gap(self):
        statuses = ("partial", "deferred", "out_of_scope", "rust_difference")
        mapping = {"features": [{"id": status, "status": status} for status in statuses]}
        self.assertEqual(gate_z.unsupported_features(mapping), [f"{status}.status" for status in statuses])

    def test_a_feature_without_a_status_is_format(self):
        with self.assertRaises(GateError) as caught:
            gate_z.unsupported_features({"features": [{"id": "a"}]})
        self.assertEqual(caught.exception.code, Code.FORMAT)


class BehaviorTests(unittest.TestCase):
    def test_upstream_and_followed_rows_pass(self):
        rows = behaviors(row("s01-a"), row("s02-b", "follow-typescript", "TypeScript wins."),
                         row("s43-c", "follow-python", "Only Python has it.", typescript=TS_ABSENT))
        self.assertEqual(gate_z.behavior_gaps(rows), [])

    def test_following_python_over_a_typescript_form_is_a_gap(self):
        rows = behaviors(row("s44-d", "follow-python", "Python wins."))
        self.assertEqual(gate_z.behavior_gaps(rows), ["s44-d.decision"])

    def test_tls_rows_may_adapt_with_the_owner_reason(self):
        reason = "Rust verifies with rustls. " + gate_z.TLS_REASON
        rows = behaviors(*(row(f"m{number}-tls-case", gate_z.RUST_ADAPTATION, reason) for number in range(19, 26)),
                         row("m26-python-tls-trust-directory-lookup", gate_z.RUST_ADAPTATION, reason))
        self.assertEqual(gate_z.behavior_gaps(rows), [])

    def test_a_tls_row_without_the_owner_reason_is_a_gap(self):
        rows = behaviors(row("m19-tls-case", gate_z.RUST_ADAPTATION, gate_z.TLS_REASON + " More text."),
                         row("m20-tls-case", gate_z.RUST_ADAPTATION, "Rust verifies with rustls."))
        self.assertEqual(gate_z.behavior_gaps(rows), ["m19-tls-case.decision", "m20-tls-case.decision"])

    def test_a_row_adapting_an_upstream_defect_passes_with_the_owner_reason(self):
        reason = "Rust zeroes the buffer. " + gate_z.DEFECT_REASON
        rows = behaviors(row("m18-python-system-proxy-undefined-reads", gate_z.RUST_ADAPTATION, reason))
        self.assertEqual(gate_z.behavior_gaps(rows), [])

    def test_a_row_adapting_an_upstream_defect_without_the_owner_reason_is_a_gap(self):
        row_id = "m18-python-system-proxy-undefined-reads"
        rows = behaviors(row(row_id, gate_z.RUST_ADAPTATION, "Rust zeroes the buffer."),
                         row(row_id, gate_z.RUST_ADAPTATION, gate_z.TLS_REASON),
                         row(row_id, gate_z.RUST_ADAPTATION, gate_z.DEFECT_REASON + " More text."))
        self.assertEqual(gate_z.behavior_gaps(rows), [f"{row_id}.decision"] * 3)

    def test_the_defect_reason_does_not_admit_any_other_row(self):
        ids = ("m18-python-system-proxy", "m17-python-system-proxy-undefined-reads", "m19-python-tls-hello-retry-groups")
        rows = behaviors(*(row(row_id, gate_z.RUST_ADAPTATION, gate_z.DEFECT_REASON) for row_id in ids))
        self.assertEqual(gate_z.behavior_gaps(rows), [f"{row_id}.decision" for row_id in ids])

    def test_adapting_any_other_row_is_a_gap(self):
        ids = ("c24-launch-directory", "m18-tls-case", "m26-python-tls-other", "m19", "xm19-tls-case", "m190-tls-case")
        rows = behaviors(*(row(row_id, gate_z.RUST_ADAPTATION, gate_z.TLS_REASON) for row_id in ids))
        self.assertEqual(gate_z.behavior_gaps(rows), [f"{row_id}.decision" for row_id in ids])

    def test_an_unknown_decision_is_a_gap(self):
        self.assertEqual(gate_z.behavior_gaps(behaviors(row("s01-a", "follow-rust", "No."))), ["s01-a.decision"])

    def test_a_row_without_contracts_is_a_gap(self):
        rows = behaviors(row("s01-a", contracts=()), row("s02-b", gate_z.RUST_ADAPTATION, "No.", contracts=()))
        self.assertEqual(gate_z.behavior_gaps(rows), ["s01-a.contracts", "s02-b.decision", "s02-b.contracts"])

    def test_an_adaptation_without_a_reason_is_format(self):
        with self.assertRaises(GateError) as caught:
            gate_z.behavior_gaps(behaviors(row("m19-tls-case", gate_z.RUST_ADAPTATION)))
        self.assertEqual(caught.exception.code, Code.FORMAT)


class DependencyTests(unittest.TestCase):
    def test_a_tree_without_openssl_passes(self):
        text = lock(("trogon-claude-agent-sdk", "0.1.0", ["rustls"]), ("rustls", "0.23.0", []),
                    ("openssl", "0.10.0", []), ("other", "0.1.0", ["native-tls"]), ("native-tls", "0.2.0", []))
        self.assertEqual(gate_z.openssl_dependencies(text), [])

    def test_every_reachable_openssl_crate_is_reported(self):
        text = lock(("trogon-claude-agent-sdk", "0.1.0", ["reqwest 0.12.0", "tls"]),
                    ("reqwest", "0.12.0", ["native-tls"]), ("reqwest", "0.11.0", ["openssl-src"]),
                    ("native-tls", "0.2.0", ["openssl 0.10.0 (registry+https://github.com/rust-lang/crates.io-index)"]),
                    ("openssl", "0.10.0", ["openssl-sys"], "registry+https://github.com/rust-lang/crates.io-index"),
                    ("openssl-sys", "0.9.0", []), ("openssl-src", "300.0.0", []),
                    ("tls", "1.0.0", ["native-tls"]))
        self.assertEqual(gate_z.openssl_dependencies(text), ["native-tls 0.2.0", "openssl 0.10.0", "openssl-sys 0.9.0"])

    def test_a_source_qualifier_picks_the_package_it_names(self):
        crates_io = "registry+https://github.com/rust-lang/crates.io-index"
        mirror = "registry+https://example.com/mirror-index"
        text = lock(("trogon-claude-agent-sdk", "0.1.0", [f"tls 1.0.0 ({mirror})"]),
                    ("tls", "1.0.0", ["rustls"], crates_io), ("tls", "1.0.0", ["native-tls"], mirror),
                    ("rustls", "0.23.0", [], crates_io), ("native-tls", "0.2.0", [], crates_io))
        self.assertEqual(gate_z.openssl_dependencies(text), ["native-tls 0.2.0"])

    def test_an_ambiguous_reference_walks_every_package_it_could_name(self):
        text = lock(("trogon-claude-agent-sdk", "0.1.0", ["tls 1.0.0"]),
                    ("tls", "1.0.0", ["rustls"], "registry+https://example.com/a"),
                    ("tls", "1.0.0", ["native-tls"], "registry+https://example.com/b"),
                    ("rustls", "0.23.0", []), ("native-tls", "0.2.0", []))
        self.assertEqual(gate_z.openssl_dependencies(text), ["native-tls 0.2.0"])

    def test_a_source_that_names_no_locked_package_is_a_stale_reference(self):
        text = lock(("trogon-claude-agent-sdk", "0.1.0", ["tls 1.0.0 (registry+https://example.com/gone)"]),
                    ("tls", "1.0.0", [], "registry+https://example.com/a"))
        with self.assertRaises(GateError) as caught:
            gate_z.openssl_dependencies(text)
        self.assertEqual(caught.exception.code, Code.REFERENCE)

    def test_a_replaced_package_is_walked_through_its_replacement(self):
        crates_io = "registry+https://github.com/rust-lang/crates.io-index"
        text = (lock(("trogon-claude-agent-sdk", "0.1.0", [f"tls 1.0.0 ({crates_io})"]),
                     ("tls", "1.0.0", ["native-tls"]), ("native-tls", "0.2.0", []))
                + f'\n[[package]]\nname = "tls"\nversion = "1.0.0"\nsource = "{crates_io}"\nreplace = "tls 1.0.0"\n')
        self.assertEqual(gate_z.openssl_dependencies(text), ["native-tls 0.2.0"])

    def test_a_replacement_missing_from_the_lock_is_a_stale_reference(self):
        crates_io = "registry+https://github.com/rust-lang/crates.io-index"
        text = (lock(("trogon-claude-agent-sdk", "0.1.0", [f"tls 1.0.0 ({crates_io})"]))
                + f'\n[[package]]\nname = "tls"\nversion = "1.0.0"\nsource = "{crates_io}"\nreplace = "tls 2.0.0"\n')
        with self.assertRaises(GateError) as caught:
            gate_z.openssl_dependencies(text)
        self.assertEqual(caught.exception.code, Code.REFERENCE)

    def test_a_dependency_missing_from_the_lock_is_a_stale_reference(self):
        with self.assertRaises(GateError) as caught:
            gate_z.openssl_dependencies(lock(("trogon-claude-agent-sdk", "0.1.0", ["gone 1.0.0"])))
        self.assertEqual(caught.exception.code, Code.REFERENCE)

    def test_a_lock_without_the_sdk_is_a_stale_reference(self):
        with self.assertRaises(GateError) as caught:
            gate_z.openssl_dependencies(lock(("rustls", "0.23.0", [])))
        self.assertEqual(caught.exception.code, Code.REFERENCE)


class RepositoryGateZTests(unittest.TestCase):
    """The committed mapping, behavior map and lockfile must clear gate Z."""

    def test_every_mapping_feature_is_supported(self):
        self.assertEqual(gate_z.unsupported_features(json.loads(MAPPING.read_text())), [])

    def test_every_behavior_follows_upstream_with_a_test(self):
        self.assertEqual(gate_z.behavior_gaps(json.loads((LIB / "behaviors.json").read_text())), [])

    def test_the_sdk_does_not_depend_on_openssl(self):
        self.assertEqual(gate_z.openssl_dependencies((WORKSPACE / "Cargo.lock").read_text()), [])


if __name__ == "__main__":
    unittest.main()
