import contextlib
import hashlib
import io
import json
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

from peer_id_evidence import EvidenceRefused, check_path, digest_of, main, parse_log, unexpected_peers, verify
from profile import registrations

HERE = Path(__file__).parent
EGRESS = "spiffe://production.identity.example.org/platform/egress"
OTHER = "spiffe://production.identity.example.org/platform/other"
NOW = datetime(2026, 11, 24, 12, 0, 0, tzinfo=timezone.utc)


def example():
    return json.loads((HERE / "credential-inventory.example.json").read_text())


def accepted(peer=EGRESS, source="workload_api"):
    fields = {"message": "mTLS connection accepted", "event": "peer_connection_accepted", "peer": "10.0.4.17:44218", "peer.spiffe_id": peer, "credential_source": source}
    return json.dumps({"timestamp": "2026-11-10T09:05:12.123456Z", "level": "INFO", "fields": fields, "target": "x"})


def would_deny(peer=EGRESS):
    fields = {"message": "dry run", "event": "egress_would_deny", "peer.spiffe_id": peer, "agent": "agent-1", "destination": "a.example:443", "mode": "dry_run"}
    return json.dumps({"timestamp": "2026-11-10T09:05:12.123456Z", "level": "WARN", "fields": fields, "target": "x"})


def log(*lines):
    return ("\n".join(lines) + "\n").encode()


class VerifyTests(unittest.TestCase):
    def setUp(self):
        self.inventory = example()

    def refused(self, rule, data, names=(), strict=False):
        with self.assertRaises(EvidenceRefused) as raised:
            verify(self.inventory, data, list(names), strict)
        self.assertIn(rule, str(raised.exception))

    def test_the_fixture_log_is_accepted_for_the_parallel_path_and_its_digest_is_the_file_digest(self):
        data = (HERE / "testdata" / "proxy-peer-events.jsonl").read_bytes()
        facts = verify(self.inventory, data, [], True)
        self.assertEqual(facts["evidence_sha256"], hashlib.sha256(data).hexdigest())
        self.assertEqual(facts["paths"], [{"name": "egress-proxy-to-audit-collector", "caller_spiffe_id": EGRESS, "accepted_connections": 2,
                                           "accepted_by_credential_source": {"workload_api": 2}, "would_deny_events": 0}])
        self.assertEqual(facts["unexpected_peer_ids"], [])

    def test_a_log_without_an_accepted_connection_from_the_expected_id_is_refused(self):
        self.refused("no peer_connection_accepted event names", log(accepted(OTHER)))
        self.refused("no peer_connection_accepted event names", log())

    def test_a_would_deny_event_naming_the_expected_id_is_refused_even_beside_accepted_connections(self):
        self.refused("egress_would_deny event(s) name", log(accepted(), accepted(), would_deny()))

    def test_a_would_deny_naming_another_id_does_not_refuse_the_path_but_is_unexpected(self):
        facts = verify(self.inventory, log(accepted(), would_deny(OTHER)), [], False)
        self.assertEqual(facts["unexpected_peer_ids"], [OTHER])

    def test_strict_refuses_an_observed_peer_that_no_inventory_path_expects(self):
        data = log(accepted(), accepted(OTHER))
        self.assertEqual(verify(self.inventory, data, [], False)["unexpected_peer_ids"], [OTHER])
        self.refused(OTHER, data, strict=True)

    def test_a_malformed_line_is_refused_with_its_line_number(self):
        for bad in ["not json", '{"fields": ', '["fields"]', '{"level": "INFO"}', '{"fields": "x"}', '{"fields": {"event": "x", "event": "y"}}']:
            with self.subTest(line=bad):
                self.refused("line 2", log(accepted(), bad))

    def test_a_truncated_would_deny_line_cannot_be_skipped(self):
        self.refused("line 3", log(accepted(), accepted(), would_deny()[:-20]))

    def test_an_event_without_a_peer_id_is_refused(self):
        for event in ["peer_connection_accepted", "egress_would_deny"]:
            for peer in [None, "", 5]:
                with self.subTest(event=event, peer=peer):
                    line = json.dumps({"fields": {"event": event, "peer.spiffe_id": peer}})
                    self.refused("without a text peer.spiffe_id", log(accepted(), line))

    def test_non_utf8_input_is_refused(self):
        self.refused("not UTF-8", b"\xff\xfe")

    def test_other_events_and_blank_lines_are_ignored(self):
        self.assertEqual(parse_log(log("", '{"fields": {"message": "hello"}}', accepted()))[0][0][1], EGRESS)

    def test_the_default_selection_is_every_parallel_path_and_none_parallel_is_refused(self):
        for record in self.inventory["call_paths"]:
            if record["state"] == "parallel":
                record["state"] = "static"
        self.refused("no call path is in state parallel", log(accepted()))

    def test_a_named_path_must_exist_and_name_a_caller(self):
        self.refused("no such call path", log(accepted()), ["missing"])
        self.refused("names no caller_spiffe_id", log(accepted()), ["batch-exporter-to-object-store"])

    def test_every_parallel_path_is_checked_not_only_the_first(self):
        second = dict(self.inventory["call_paths"][2], name="second-path", caller_spiffe_id=OTHER)
        self.inventory["call_paths"].append(second)
        self.refused(OTHER, log(accepted()))
        self.assertEqual(len(verify(self.inventory, log(accepted(), accepted(OTHER)), [], False)["paths"]), 2)

    def test_check_path_counts_connections_by_credential_source(self):
        record = self.inventory["call_paths"][2]
        facts = check_path(record, parse_log(log(accepted(), accepted(source="static_file"), accepted(OTHER))))
        self.assertEqual(facts["accepted_by_credential_source"], {"static_file": 1, "workload_api": 1})

    def test_unexpected_peers_ignores_every_inventory_caller(self):
        self.assertEqual(unexpected_peers(self.inventory, parse_log(log(accepted()))), [])

    def test_digest_of_is_the_sha256_of_the_bytes(self):
        self.assertEqual(digest_of(b"x"), hashlib.sha256(b"x").hexdigest())


class CommandLineTests(unittest.TestCase):
    def command(self, data, *options, inventory=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "inventory.json").write_text(json.dumps(inventory or example()))
            (root / "registrations.json").write_text(json.dumps(registrations(json.loads((HERE / "deployment.example.json").read_text()))))
            (root / "proxy.jsonl").write_bytes(data)
            argv = ["--inventory", str(root / "inventory.json"), "--registrations", str(root / "registrations.json"), "--proxy-log", str(root / "proxy.jsonl"), *options]
            stdout, stderr = io.StringIO(), io.StringIO()
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                try:
                    status = main(argv, clock=lambda: NOW)
                except SystemExit as raised:
                    status = raised.code
            return status, stdout.getvalue(), stderr.getvalue()

    def test_an_accepted_log_exits_zero_and_prints_the_digest(self):
        data = log(accepted())
        status, out, _ = self.command(data)
        self.assertEqual(status, 0)
        self.assertEqual(json.loads(out)["evidence_sha256"], hashlib.sha256(data).hexdigest())

    def test_a_refused_log_exits_one_prints_nothing_on_stdout_and_names_the_rule(self):
        status, out, err = self.command(log(accepted(), would_deny()))
        self.assertEqual((status, out), (1, ""))
        self.assertIn("peer ID evidence refused: egress-proxy-to-audit-collector", err)

    def test_the_strict_flag_reaches_the_check(self):
        data = log(accepted(), accepted(OTHER))
        self.assertEqual(self.command(data)[0], 0)
        self.assertEqual(self.command(data, "--strict")[0], 1)

    def test_a_named_enforced_path_can_be_rechecked(self):
        self.assertEqual(self.command(log(accepted()), "--path", "egress-proxy-to-policy-engine")[0], 0)

    def test_an_invalid_inventory_is_refused(self):
        inventory = example()
        inventory["call_paths"][2]["caller_spiffe_id"] = OTHER
        status, out, err = self.command(log(accepted()), inventory=inventory)
        self.assertEqual((status, out), (1, ""))
        self.assertIn("inventory refused", err)

    def test_an_unreadable_log_exits_one(self):
        self.assertEqual(self.command(b"{")[0], 1)


if __name__ == "__main__":
    unittest.main()
