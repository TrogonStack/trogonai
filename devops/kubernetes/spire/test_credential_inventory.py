import contextlib
import copy
import hashlib
import io
import json
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path

from credential_inventory import InventoryRefused, main, summarize, workload_identities
from profile import CA_TTL_SECONDS, registrations, server_config

HERE = Path(__file__).parent
DIGEST = "e703cc872711a515a51563ebf0b3439dc3f140f7ded15ca87a18382c373109b5"
EGRESS = "spiffe://production.identity.example.org/platform/egress"
NOW = datetime(2026, 11, 24, 12, 0, 0, tzinfo=timezone.utc)


def configuration():
    return json.loads((HERE / "deployment.example.json").read_text())


def example():
    return json.loads((HERE / "credential-inventory.example.json").read_text())


def stamp(moment):
    return moment.strftime("%Y-%m-%dT%H:%M:%SZ")


class InventoryTests(unittest.TestCase):
    def setUp(self):
        self.registered = workload_identities(registrations(configuration()))
        self.inventory = example()

    def path(self, state):
        return next(record for record in self.inventory["call_paths"] if record["state"] == state)

    def refused(self, rule, mutate):
        mutate(self.inventory)
        with self.assertRaises(InventoryRefused) as raised:
            summarize(self.inventory, self.registered, NOW)
        self.assertIn(rule, str(raised.exception))
        return str(raised.exception)

    def test_the_example_validates_against_the_registrations_rendered_from_the_example_deployment(self):
        self.assertEqual(self.registered, {EGRESS})
        summary = summarize(self.inventory, self.registered, NOW)
        self.assertEqual(summary["by_state"], {"static": 1, "parallel": 1, "enforced": 1, "retired": 1})

    def test_summary_counts_the_inventory_the_retired_against_it_and_the_paths_still_on_a_static_credential(self):
        self.assertEqual(summarize(self.inventory, self.registered, NOW), {
            "inventory_size": 4, "by_state": {"static": 1, "parallel": 1, "enforced": 1, "retired": 1},
            "retired": {"count": 1, "of": 4}, "static_paths": ["batch-exporter-to-object-store"],
            "static_credential_still_live": ["batch-exporter-to-object-store", "egress-proxy-to-audit-collector"],
        })

    def test_every_state_is_present_in_the_example(self):
        self.assertEqual({record["state"] for record in self.inventory["call_paths"]}, {"static", "parallel", "enforced", "retired"})

    def test_a_record_with_a_field_that_could_hold_the_credential_is_refused_naming_the_path(self):
        for field in ["secret", "value", "password", "api_key", "token"]:
            with self.subTest(field=field):
                self.inventory = example()
                message = self.refused("never stores a credential value", lambda inventory: inventory["call_paths"][0].update({field: "x"}))
                self.assertIn("egress-proxy-to-tenant-gateway", message)

    def test_a_credential_object_with_a_value_field_is_refused(self):
        self.refused("never stores a credential value", lambda inventory: inventory["call_paths"][0]["credential"].update(value="hunter2"))

    def test_a_locator_that_is_the_value_rather_than_a_reference_is_refused(self):
        for locator in ["file:/etc/x y", "hunter2", "k8s-secret:ns/" + "A" * 48, "api_key=abc", ""]:
            with self.subTest(locator=locator):
                self.inventory = example()
                self.refused("not its value", lambda inventory: inventory["call_paths"][0]["credential"].update(locator=locator))

    def test_a_missing_field_is_refused(self):
        self.refused("missing field owner", lambda inventory: inventory["call_paths"][0].pop("owner"))

    def test_duplicate_names_are_refused(self):
        self.refused("not unique", lambda inventory: inventory["call_paths"][1].update(name=inventory["call_paths"][0]["name"]))

    def test_an_empty_owner_caller_or_callee_is_refused(self):
        for field in ["owner", "caller", "callee"]:
            for value in ["", "   ", None]:
                with self.subTest(field=field, value=value):
                    self.inventory = example()
                    self.refused(field, lambda inventory: inventory["call_paths"][0].update({field: value}))

    def test_an_unknown_credential_kind_or_state_is_refused(self):
        self.refused("credential kind", lambda inventory: inventory["call_paths"][0]["credential"].update(kind="bearer"))
        self.inventory = example()
        self.refused("state must be one of", lambda inventory: inventory["call_paths"][0].update(state="done"))

    def test_an_empty_inventory_or_an_unknown_top_level_field_is_refused(self):
        for inventory in [{"call_paths": []}, {"call_paths": [], "extra": 1}, [], {}]:
            with self.subTest(inventory=inventory), self.assertRaises(InventoryRefused):
                summarize(inventory, self.registered, NOW)

    def test_a_static_path_must_not_name_a_spiffe_id_or_carry_timestamps(self):
        self.refused("must not name", lambda inventory: self.path("static").update(caller_spiffe_id=EGRESS))
        self.inventory = example()
        self.refused("must not carry enforcement_started_at", lambda inventory: self.path("static").update(enforcement_started_at="2026-11-01T00:00:00Z"))

    def test_a_path_past_static_without_a_spiffe_id_is_refused(self):
        for state in ["parallel", "enforced", "retired"]:
            with self.subTest(state=state):
                self.inventory = example()
                self.refused("requires caller_spiffe_id", lambda inventory: self.path(state).update(caller_spiffe_id=None))

    def test_a_path_past_static_must_name_a_spiffe_id_present_in_the_rendered_registrations(self):
        for state in ["parallel", "enforced", "retired"]:
            with self.subTest(state=state):
                self.inventory = example()
                self.refused("not a workload registered", lambda inventory: self.path(state).update(caller_spiffe_id="spiffe://production.identity.example.org/platform/other"))

    def test_a_node_alias_is_not_an_acceptable_caller(self):
        alias = next(entry["spiffe_id"] for entry in registrations(configuration())["entries"] if entry["parent_id"].endswith("/spire/server"))
        self.refused("not a workload registered", lambda inventory: self.path("enforced").update(caller_spiffe_id=alias))

    def test_enforced_and_retired_paths_require_the_enforcement_timestamp(self):
        for state in ["enforced", "retired"]:
            with self.subTest(state=state):
                self.inventory = example()
                self.refused("requires enforcement_started_at", lambda inventory: self.path(state).update(enforcement_started_at=None))

    def test_a_parallel_path_must_not_carry_an_enforcement_timestamp(self):
        self.refused("must not carry enforcement_started_at", lambda inventory: self.path("parallel").update(enforcement_started_at="2026-11-01T00:00:00Z"))

    def test_a_retired_path_requires_the_retirement_timestamp_and_no_other_state_carries_it(self):
        self.refused("requires retired_at", lambda inventory: self.path("retired").update(retired_at=None))
        self.inventory = example()
        self.refused("must not carry retired_at", lambda inventory: self.path("enforced").update(retired_at="2026-11-21T00:00:00Z"))

    def test_retirement_inside_one_full_ca_rotation_of_enforcement_is_refused(self):
        record = self.path("retired")
        started = datetime(2026, 3, 2, 9, tzinfo=timezone.utc)
        record["retired_at"] = stamp(started + timedelta(seconds=CA_TTL_SECONDS - 1))
        with self.assertRaises(InventoryRefused) as raised:
            summarize(self.inventory, self.registered, NOW)
        self.assertIn("one full CA rotation", str(raised.exception))

    def test_retirement_after_exactly_one_ca_lifetime_is_accepted(self):
        record = self.path("retired")
        started = datetime(2026, 3, 2, 9, tzinfo=timezone.utc)
        record["retired_at"] = stamp(started + timedelta(seconds=CA_TTL_SECONDS))
        self.assertEqual(summarize(self.inventory, self.registered, NOW)["retired"], {"count": 1, "of": 4})

    def test_retirement_before_enforcement_is_refused(self):
        self.path("retired")["retired_at"] = "2026-03-01T09:00:00Z"
        with self.assertRaises(InventoryRefused):
            summarize(self.inventory, self.registered, NOW)

    def test_the_rotation_window_is_the_rendered_ca_lifetime(self):
        self.assertEqual(server_config(configuration())["server"]["ca_ttl"], f"{CA_TTL_SECONDS // 3600}h")

    def test_timestamps_must_be_rfc_3339_utc(self):
        for value in ["2026-03-02", "2026-03-02T09:00:00", "2026-03-02T09:00:00+01:00", "2026-03-02 09:00:00Z", "2026-13-02T09:00:00Z", "yesterday", 5]:
            with self.subTest(value=value):
                self.inventory = example()
                self.refused("enforcement_started_at", lambda inventory: self.path("enforced").update(enforcement_started_at=value))

    def test_a_timestamp_in_the_future_of_the_injected_clock_is_refused(self):
        self.path("enforced")["enforcement_started_at"] = stamp(NOW + timedelta(seconds=1))
        with self.assertRaises(InventoryRefused) as raised:
            summarize(self.inventory, self.registered, NOW)
        self.assertIn("in the future", str(raised.exception))
        self.assertEqual(summarize(self.inventory, self.registered, NOW + timedelta(seconds=1))["inventory_size"], 4)

    def test_a_retirement_timestamp_in_the_future_is_refused(self):
        self.path("retired")["retired_at"] = stamp(NOW + timedelta(days=1))
        with self.assertRaises(InventoryRefused) as raised:
            summarize(self.inventory, self.registered, NOW)
        self.assertIn("retired_at is in the future", str(raised.exception))


    def test_an_enforced_or_retired_path_requires_the_peer_id_evidence_digest(self):
        for state in ["enforced", "retired"]:
            for value in [None, "", "abc", DIGEST.upper(), DIGEST[:-1], 5]:
                with self.subTest(state=state, value=value):
                    self.inventory = example()
                    self.refused("requires peer_id_evidence_sha256", lambda inventory: self.path(state).update(peer_id_evidence_sha256=value))

    def test_a_static_or_parallel_path_must_not_carry_a_digest(self):
        for state in ["static", "parallel"]:
            with self.subTest(state=state):
                self.inventory = example()
                self.refused("must not carry peer_id_evidence_sha256", lambda inventory: self.path(state).update(peer_id_evidence_sha256=DIGEST))

    def test_the_example_digest_matches_the_fixture_log_and_every_enforced_path_passes_it_again(self):
        data = (HERE / "testdata" / "proxy-peer-events.jsonl").read_bytes()
        self.assertEqual(hashlib.sha256(data).hexdigest(), DIGEST)
        summarize(self.inventory, self.registered, NOW, {DIGEST: data})

    def test_with_evidence_a_digest_that_matches_no_file_is_refused(self):
        with self.assertRaises(InventoryRefused) as raised:
            summarize(self.inventory, self.registered, NOW, {"0" * 64: b""})
        self.assertIn("matches no file", str(raised.exception))

    def test_with_evidence_a_file_that_no_longer_passes_the_peer_check_is_refused(self):
        denied = json.dumps({"fields": {"event": "egress_would_deny", "peer.spiffe_id": EGRESS}}).encode()
        for data in [denied, b"", b"not json"]:
            with self.subTest(data=data), self.assertRaises(InventoryRefused) as raised:
                summarize(self.inventory, self.registered, NOW, {DIGEST: data})
            self.assertIn("no longer passes peer_id_evidence", str(raised.exception))


class CommandLineTests(unittest.TestCase):
    def command(self, inventory, clock=lambda: NOW, registered=None, *options):
        with tempfile.TemporaryDirectory() as directory:
            inventory_path, registrations_path = Path(directory) / "inventory.json", Path(directory) / "registrations.json"
            inventory_path.write_text(inventory if isinstance(inventory, str) else json.dumps(inventory))
            registrations_path.write_text(json.dumps(registered or registrations(configuration())))
            stdout, stderr = io.StringIO(), io.StringIO()
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                try:
                    status = main(["--inventory", str(inventory_path), "--registrations", str(registrations_path), *options], clock=clock)
                except SystemExit as raised:
                    status = raised.code
            return status, stdout.getvalue(), stderr.getvalue()

    def test_a_valid_inventory_exits_zero_and_prints_the_json_summary(self):
        status, out, _ = self.command(example())
        self.assertEqual(status, 0)
        self.assertEqual(json.loads(out)["retired"], {"count": 1, "of": 4})

    def test_a_refused_inventory_exits_one_and_names_the_path_and_the_rule(self):
        inventory = example()
        inventory["call_paths"][1]["caller_spiffe_id"] = None
        status, out, err = self.command(inventory)
        self.assertEqual((status, out), (1, ""))
        self.assertIn("egress-proxy-to-policy-engine: state enforced requires caller_spiffe_id", err)

    def test_the_evidence_directory_is_checked_when_given(self):
        with tempfile.TemporaryDirectory() as empty:
            self.assertEqual(self.command(example(), lambda: NOW, None, "--evidence-dir", empty)[0], 1)
        self.assertEqual(self.command(example(), lambda: NOW, None, "--evidence-dir", str(HERE / "testdata"))[0], 0)

    def test_the_clock_is_injected(self):
        self.assertEqual(self.command(example(), clock=lambda: datetime(2026, 11, 1, tzinfo=timezone.utc))[0], 1)

    def test_unreadable_or_duplicate_field_input_exits_one(self):
        self.assertEqual(self.command("not json")[0], 1)
        self.assertEqual(self.command('{"call_paths": [], "call_paths": []}')[0], 1)
        self.assertEqual(self.command(example(), registered={"entries": [{}]})[0], 1)


if __name__ == "__main__":
    unittest.main()
