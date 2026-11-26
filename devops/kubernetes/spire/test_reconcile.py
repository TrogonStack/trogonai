import contextlib
import copy
import hashlib
import io
import json
import tempfile
import unittest
from pathlib import Path

from profile import MIGRATION_TRUST_DOMAIN, REQUESTED_X509_SVID_TTL_SECONDS, registrations, validate
from reconcile import ApplyFailed, ReconcileRefused, apply_plan, main, plan_reconciliation, reconcile

TESTDATA = Path(__file__).with_name("testdata")

TRUST_DOMAIN = "production.identity.example.org"


def configuration():
    return json.loads(Path(__file__).with_name("deployment.example.json").read_text())


def second_registration(config):
    registration = copy.deepcopy(config["registrations"][0])
    registration["spiffe_id"] = f"spiffe://{TRUST_DOMAIN}/platform/ingress"
    registration["service_account"] = "ingress-supervisor"
    registration["uid"] = 10002
    return registration


def identifier(spiffe_id):
    trust_domain, _, path = spiffe_id.removeprefix("spiffe://").partition("/")
    return {"trust_domain": trust_domain, "path": "/" + path}


def server_entry(entry, entry_id):
    """An entry as `spire-server entry show -output json` prints it."""
    return {
        "id": entry_id,
        "spiffe_id": identifier(entry["spiffe_id"]),
        "parent_id": identifier(entry["parent_id"]),
        "selectors": [dict(selector) for selector in reversed(entry["selectors"])],
        "x509_svid_ttl": entry["x509_svid_ttl"],
        "federates_with": [],
        "admin": entry["admin"],
        "downstream": entry["downstream"],
        "expires_at": "0",
        "dns_names": [],
        "revision_number": "0",
        "store_svid": False,
        "jwt_svid_ttl": 0,
        "hint": "",
        "created_at": "1760000000",
    }


def listing(entries):
    return {"entries": [server_entry(entry, f"id-{index}") for index, entry in enumerate(entries)], "next_page_token": ""}


class ReconciliationTests(unittest.TestCase):
    def setUp(self):
        self.config = configuration()
        self.config["registrations"].append(second_registration(self.config))
        self.desired = registrations(self.config)["entries"]

    def plan(self, current, desired=None):
        return plan_reconciliation(desired or self.desired, current)

    def test_unchanged_set_yields_an_empty_plan(self):
        plan = self.plan(listing(self.desired))
        self.assertEqual((plan.create, plan.delete, plan.replaced), ([], [], []))

    def test_empty_server_creates_every_entry(self):
        plan = self.plan({"entries": [], "next_page_token": ""})
        self.assertEqual(plan.create, self.desired)
        self.assertEqual(plan.delete, [])

    def test_removed_registration_yields_a_delete_by_entry_id(self):
        current = listing(self.desired)
        withdrawn = self.desired[:-1]
        plan = self.plan(current, withdrawn)
        self.assertEqual(plan.delete, [current["entries"][-1]["id"]])
        self.assertEqual(plan.create, [])

    def test_renaming_the_trust_domain_changes_every_identity_and_refuses_the_old_servers_listing(self):
        renamed_config = json.loads(json.dumps(self.config).replace(TRUST_DOMAIN, "renamed.identity.example.org"))
        renamed = registrations(renamed_config)["entries"]
        self.assertEqual(len(renamed), len(self.desired))
        self.assertEqual({entry["spiffe_id"] for entry in renamed} & {entry["spiffe_id"] for entry in self.desired}, set())
        for entry in renamed:
            self.assertTrue(entry["spiffe_id"].startswith("spiffe://renamed.identity.example.org/"))
        validate(renamed_config)
        with self.assertRaises(ValueError):
            validate({**self.config, "trust_domain": "renamed.identity.example.org"})
        with self.assertRaises(ReconcileRefused):
            self.plan(listing(self.desired), renamed)

    def test_changed_selector_replaces_the_entry(self):
        current = listing(self.desired)
        changed = copy.deepcopy(self.desired)
        changed[1]["selectors"][0]["value"] = "ns:another-namespace"
        plan = self.plan(current, changed)
        self.assertEqual(plan.delete, [current["entries"][1]["id"]])
        self.assertEqual(plan.create, [changed[1]])
        self.assertEqual(plan.replaced, [changed[1]["spiffe_id"]])

    def test_changed_ttl_or_hidden_attribute_replaces_the_entry(self):
        for field, value in [("x509_svid_ttl", 60), ("jwt_svid_ttl", 60), ("dns_names", ["a.example.org"]), ("federates_with", ["other.example.org"]), ("store_svid", True)]:
            with self.subTest(field=field):
                current = listing(self.desired)
                current["entries"][1][field] = value
                plan = self.plan(current)
                self.assertEqual(plan.delete, [current["entries"][1]["id"]])
                self.assertEqual(plan.create, [self.desired[1]])

    def test_selector_order_and_duplicate_server_entries_are_normalized(self):
        current = listing(self.desired)
        duplicate = copy.deepcopy(current["entries"][1])
        duplicate["id"] = "id-duplicate"
        current["entries"].append(duplicate)
        plan = self.plan(current)
        self.assertEqual(plan.delete, ["id-duplicate"])
        self.assertEqual(plan.create, [])

    def test_foreign_admin_or_downstream_entry_is_refused(self):
        for flag in ["admin", "downstream"]:
            for parent in [f"spiffe://{TRUST_DOMAIN}/spire/server", f"spiffe://{TRUST_DOMAIN}/platform/agent-group/{self.config['cluster']}/epoch-1"]:
                with self.subTest(flag=flag, parent=parent):
                    foreign = {
                        "parent_id": parent, "spiffe_id": f"spiffe://{TRUST_DOMAIN}/operators/break-glass",
                        "selectors": [{"type": "unix", "value": "uid:0"}], "x509_svid_ttl": 0, "admin": False, "downstream": False,
                    }
                    foreign[flag] = True
                    with self.assertRaises(ReconcileRefused):
                        self.plan(listing(self.desired + [foreign]))

    def test_listing_that_cannot_be_trusted_is_refused(self):
        truncated = listing(self.desired)
        truncated["next_page_token"] = "more"
        other_domain = listing(self.desired)
        other_domain["entries"][0]["spiffe_id"]["trust_domain"] = "another.example.org"
        for name, current in {"truncated": truncated, "other trust domain": other_domain, "not an object": [], "no entries": {}}.items():
            with self.subTest(case=name):
                with self.assertRaises(ReconcileRefused):
                    self.plan(current)

    def test_entries_outside_the_owned_namespace_are_never_deleted(self):
        unowned = [
            {"parent_id": f"spiffe://{TRUST_DOMAIN}/spire/server", "spiffe_id": f"spiffe://{TRUST_DOMAIN}/other/alias",
             "selectors": [{"type": "k8s_psat", "value": "cluster:other"}], "x509_svid_ttl": 3590, "admin": False, "downstream": False},
            {"parent_id": f"spiffe://{TRUST_DOMAIN}/spire/server", "spiffe_id": f"spiffe://{TRUST_DOMAIN}/platform/agent-group/{self.config['cluster']}-b/epoch-1",
             "selectors": [{"type": "k8s_psat", "value": "cluster:b"}], "x509_svid_ttl": 3590, "admin": False, "downstream": False},
            {"parent_id": f"spiffe://{TRUST_DOMAIN}/other/alias", "spiffe_id": f"spiffe://{TRUST_DOMAIN}/team/service",
             "selectors": [{"type": "unix", "value": "uid:1"}], "x509_svid_ttl": 3590, "admin": False, "downstream": False},
        ]
        plan = self.plan(listing(self.desired + unowned))
        self.assertEqual((plan.create, plan.delete), ([], []))

    def test_retired_epoch_alias_and_its_workloads_are_owned_and_deleted(self):
        retired_alias = f"spiffe://{TRUST_DOMAIN}/platform/agent-group/{self.config['cluster']}/epoch-0"
        retired = [
            {"parent_id": f"spiffe://{TRUST_DOMAIN}/spire/server", "spiffe_id": retired_alias,
             "selectors": [{"type": "k8s_psat", "value": "cluster:" + self.config["cluster"]}], "x509_svid_ttl": 3590, "admin": False, "downstream": False},
            {"parent_id": retired_alias, "spiffe_id": f"spiffe://{TRUST_DOMAIN}/platform/egress",
             "selectors": [{"type": "unix", "value": "uid:1"}], "x509_svid_ttl": 3590, "admin": False, "downstream": False},
        ]
        current = listing(self.desired + retired)
        plan = self.plan(current)
        self.assertEqual(sorted(plan.delete), sorted(item["id"] for item in current["entries"][-2:]))
        self.assertEqual(plan.create, [])

    def test_desired_set_without_a_node_alias_or_with_mixed_clusters_is_refused(self):
        workloads = [entry for entry in self.desired if not entry["parent_id"].endswith("/spire/server")]
        with self.assertRaises(ReconcileRefused):
            self.plan(listing([]), workloads)
        foreign = copy.deepcopy(self.desired[0])
        foreign["spiffe_id"] = foreign["spiffe_id"].replace(f"/{self.config['cluster']}/", "/another-cluster/")
        with self.assertRaises(ReconcileRefused):
            self.plan(listing([]), self.desired + [foreign])

    def test_one_node_alias_per_node_is_rendered_and_same_spiffe_id_is_one_entry_per_parent(self):
        aliases = [entry for entry in self.desired if entry["parent_id"].endswith("/spire/server")]
        nodes = sorted(selector["value"] for entry in aliases for selector in entry["selectors"] if selector["value"].startswith("agent_node_name:"))
        self.assertEqual(nodes, sorted(f"agent_node_name:{node}" for node in self.config["node_names"]))
        egress = [entry for entry in self.desired if entry["spiffe_id"].endswith("/platform/egress")]
        self.assertEqual(len(egress), len(self.config["registrations"][0]["node_names"]))
        self.assertEqual(len({entry["parent_id"] for entry in egress}), len(egress))
        plan = self.plan({"entries": [], "next_page_token": ""})
        self.assertEqual(plan.create, self.desired)
        self.assertEqual(plan.delete, [])

    def test_dropping_a_node_from_a_registration_deletes_only_that_parents_entry(self):
        current = listing(self.desired)
        egress = [index for index, entry in enumerate(self.desired) if entry["spiffe_id"].endswith("/platform/egress")]
        withdrawn = [entry for index, entry in enumerate(self.desired) if index != egress[-1]]
        plan = self.plan(current, withdrawn)
        self.assertEqual(plan.delete, [current["entries"][egress[-1]]["id"]])
        self.assertEqual(plan.create, [])

    def test_upgrade_from_the_single_alias_converges_deleting_workloads_before_the_old_alias(self):
        old_alias = f"spiffe://{TRUST_DOMAIN}/platform/agent-group/{self.config['cluster']}/epoch-1"
        base = {"x509_svid_ttl": 3590, "admin": False, "downstream": False}
        legacy = [
            {**base, "parent_id": f"spiffe://{TRUST_DOMAIN}/spire/server", "spiffe_id": old_alias, "selectors": [{"type": "k8s_psat", "value": "cluster:" + self.config["cluster"]}]},
            {**base, "parent_id": old_alias, "spiffe_id": f"spiffe://{TRUST_DOMAIN}/platform/egress", "selectors": [{"type": "unix", "value": "uid:1"}]},
        ]
        current = listing(legacy)
        plan = self.plan(current)
        self.assertEqual(sorted(plan.delete), ["id-0", "id-1"])
        steps = plan.steps()
        deletes = [item for kind, item in steps if kind == "delete"]
        self.assertEqual(deletes, ["id-1", "id-0"])
        self.assertEqual(len(plan.create), len(self.desired))
        self.assertEqual([kind for kind, _ in steps[:len(plan.create)]], ["create"] * len(plan.create))

    def test_live_entries_with_fewer_than_two_selectors_are_listed_whoever_owns_them(self):
        owned = copy.deepcopy(self.desired)
        owned[-1]["selectors"] = owned[-1]["selectors"][:1]
        foreign = {"parent_id": f"spiffe://{TRUST_DOMAIN}/other/alias", "spiffe_id": f"spiffe://{TRUST_DOMAIN}/team/service",
                   "selectors": [{"type": "unix", "value": "uid:1"}], "x509_svid_ttl": 0, "admin": False, "downstream": False}
        plan = self.plan(listing(owned + [foreign]))
        self.assertEqual(len(plan.broad), 2)
        self.assertEqual(len(json.loads(plan.to_json())["broad_selectors"]), 2)
        self.assertTrue(plan.drifted())
        self.assertFalse(self.plan(listing(self.desired)).drifted())
        self.assertEqual(self.plan(listing(self.desired)).broad, ())

    def test_plan_is_deterministic_and_serializes_for_entry_create(self):
        current = listing(self.desired[:1])
        first, second = self.plan(current), self.plan(copy.deepcopy(current))
        self.assertEqual(first.to_json(), second.to_json())
        self.assertEqual(json.loads(first.to_json())["create"]["entries"], self.desired[1:])


def fixture(name):
    return json.loads((TESTDATA / name).read_text())


LIVE_TRUST_DOMAIN = "test.example.org"


class RealServerListingTests(unittest.TestCase):
    """Output captured from a SPIRE 1.16.0 server by `spire-server entry show -output json`."""

    def setUp(self):
        self.desired = fixture("test-registrations.json")["entries"]

    def test_listing_matching_the_rendered_set_plans_nothing(self):
        plan = plan_reconciliation(self.desired, fixture("spire-1.16.0-rendered-listing.json"))
        self.assertEqual((plan.create, plan.delete, plan.replaced, plan.steps()), ([], [], [], []))

    def test_empty_listing_creates_everything_with_the_node_alias_first(self):
        plan = plan_reconciliation(self.desired, fixture("spire-1.16.0-empty-listing.json"))
        self.assertEqual(len(plan.create), 3)
        self.assertEqual(plan.steps()[0][1]["parent_id"], f"spiffe://{LIVE_TRUST_DOMAIN}/spire/server")

    def test_listing_with_admin_or_downstream_entries_is_refused(self):
        with self.assertRaises(ReconcileRefused):
            plan_reconciliation(self.desired, fixture("spire-1.16.0-privileged-listing.json"))

    def test_an_extra_selector_on_the_server_replaces_the_entry(self):
        listing = fixture("spire-1.16.0-rendered-listing.json")
        listing["entries"][0]["selectors"].append({"type": "k8s", "value": "pod-uid:extra"})
        plan = plan_reconciliation(self.desired, listing)
        self.assertEqual((len(plan.create), len(plan.delete)), (1, 1))

    def test_a_non_empty_page_token_is_refused(self):
        listing = fixture("spire-1.16.0-rendered-listing.json")
        listing["next_page_token"] = "next"
        with self.assertRaises(ReconcileRefused):
            plan_reconciliation(self.desired, listing)


class FakeServer:
    """A runner that records commands and plays a fixed listing; `fail_on` fails that call number."""

    def __init__(self, listing, fail_on=None, bundles=()):
        self.listing, self.fail_on, self.calls = listing, fail_on, []
        self.bundles = list(bundles)

    def __call__(self, argv, stdin=None):
        self.calls.append((argv[1:], stdin))
        if argv[1:3] == ["entry", "show"]:
            return 0, json.dumps(self.listing), ""
        if argv[1:3] == ["bundle", "list"]:
            return 0, json.dumps({"bundles": [{"trust_domain": name} for name in self.bundles], "next_page_token": ""}), ""
        if argv[1:3] == ["bundle", "delete"]:
            self.bundles.remove(argv[argv.index("-id") + 1].removeprefix("spiffe://"))
            return 0, "bundle deleted.\n", ""
        if self.fail_on == len(self.calls):
            return 1, "", "boom"
        return 0, "", ""

    def mutations(self):
        return [call for call in self.calls if call[0][:2] not in (["entry", "show"], ["bundle", "list"])]


class ApplyTests(unittest.TestCase):
    def setUp(self):
        self.desired = fixture("test-registrations.json")["entries"]
        self.current = fixture("spire-1.16.0-rendered-listing.json")
        self.empty = fixture("spire-1.16.0-empty-listing.json")

    def test_unchanged_set_runs_no_mutation(self):
        server = FakeServer(self.current)
        reconcile(self.desired, ["spire-server"], server)
        self.assertEqual(server.mutations(), [])

    def test_order_is_new_then_each_replacement_then_retirements(self):
        ids = {entry["spiffe_id"]["path"].rsplit("/", 1)[1]: entry["id"] for entry in self.current["entries"]}
        desired = [copy.deepcopy(entry) for entry in self.desired if not entry["spiffe_id"].endswith("/ingress")]
        egress = next(entry for entry in desired if entry["spiffe_id"].endswith("/egress"))
        egress["x509_svid_ttl"] = 60
        added = copy.deepcopy(egress)
        added["spiffe_id"] = f"spiffe://{LIVE_TRUST_DOMAIN}/platform/new"
        steps = plan_reconciliation(desired + [added], self.current).steps()
        self.assertEqual([(kind, item if kind == "delete" else item["spiffe_id"].rsplit("/", 1)[1]) for kind, item in steps], [
            ("create", "new"), ("delete", ids["egress"]), ("create", "egress"), ("delete", ids["ingress"]),
        ])

    def test_retired_workloads_are_deleted_before_their_retired_node_alias(self):
        alias = f"spiffe://{LIVE_TRUST_DOMAIN}/platform/agent-group/test-cluster/epoch-0"
        server = f"spiffe://{LIVE_TRUST_DOMAIN}/spire/server"
        template = self.current["entries"][0]
        retired = []
        for entry_id, spiffe, parent in [("a-alias", alias, server), ("z-workload", f"spiffe://{LIVE_TRUST_DOMAIN}/platform/old", alias)]:
            entry = copy.deepcopy(template)
            entry["id"] = entry_id
            for field, value in [("spiffe_id", spiffe), ("parent_id", parent)]:
                trust_domain, _, path = value.removeprefix("spiffe://").partition("/")
                entry[field] = {"trust_domain": trust_domain, "path": "/" + path}
            retired.append(entry)
        plan = plan_reconciliation(self.desired, {"entries": self.current["entries"] + retired, "next_page_token": ""})
        self.assertEqual(plan.steps(), [("delete", "z-workload"), ("delete", "a-alias")])

    def test_apply_sends_one_entry_per_create_through_the_server_command(self):
        server = FakeServer(self.empty)
        plan = plan_reconciliation(self.desired, self.empty)
        apply_plan(plan, ["kubectl", "exec", "-i", "pod", "--", "spire-server"], server)
        create = ["exec", "-i", "pod", "--", "spire-server", "entry", "create", "-data", "-"]
        self.assertEqual([call[0] for call in server.calls], [create] * 3)
        self.assertEqual([json.loads(call[1])["entries"] for call in server.calls], [[entry] for entry in plan.create])

    def test_deletes_use_the_entry_id_without_json_output(self):
        listing = copy.deepcopy(self.current)
        server = FakeServer(listing)
        apply_plan(plan_reconciliation(self.desired[:2], listing), ["spire-server"], server)
        self.assertEqual(server.calls, [(["entry", "delete", "-entryID", listing["entries"][-1]["id"]], None)])

    def test_a_failed_step_stops_with_a_message_and_a_rerun_plans_the_rest(self):
        server = FakeServer(self.empty, fail_on=4)
        with self.assertRaises(ApplyFailed) as raised:
            reconcile(self.desired, ["spire-server"], server)
        self.assertIn("step 2 of 3", str(raised.exception))
        self.assertIn("boom", str(raised.exception))
        self.assertEqual(len(server.mutations()), 2)
        partial = {"entries": self.current["entries"][:1], "next_page_token": ""}
        self.assertEqual(len(plan_reconciliation(self.desired, partial).create), 2)

    def test_a_listing_that_does_not_converge_is_a_failure(self):
        with self.assertRaises(ApplyFailed):
            reconcile(self.desired, ["spire-server"], FakeServer(self.empty))

    def test_a_rendered_entry_with_admin_or_downstream_is_refused_before_any_call(self):
        for flag in ["admin", "downstream"]:
            with self.subTest(flag=flag):
                desired = copy.deepcopy(self.desired)
                desired[1][flag] = True
                server = FakeServer(self.current)
                with self.assertRaises(ReconcileRefused):
                    reconcile(desired, ["spire-server"], server)
                self.assertEqual(server.mutations(), [])

    def test_refusals_hold_before_any_mutation_on_the_live_path(self):
        server = FakeServer(fixture("spire-1.16.0-privileged-listing.json"))
        with self.assertRaises(ReconcileRefused):
            reconcile(self.desired, ["spire-server"], server)
        self.assertEqual(server.mutations(), [])

    def test_each_deleted_entry_is_logged_with_the_time_the_server_accepted_it(self):
        listing = copy.deepcopy(self.current)
        plan = plan_reconciliation(self.desired[:2], listing)
        self.assertEqual(plan.delete, [listing["entries"][-1]["id"]])
        server, lines = FakeServer(listing), []
        apply_plan(plan, ["spire-server"], server, log=lines.append, clock=lambda: "2026-11-24T12:00:00Z")
        self.assertEqual([line for line in lines if line.startswith("deleted entry ")], [
            f"deleted entry {plan.delete[0]} at 2026-11-24T12:00:00Z x509_svid_ttl_seconds=3590 "
            "last_svid_expires_at=2026-11-24T12:59:50Z time_to_revoke_seconds=3590"])

    def deleted_everything(self, ttls):
        listing = copy.deepcopy(self.current)
        for entry, ttl in zip(listing["entries"], ttls):
            if ttl is None:
                entry.pop("x509_svid_ttl", None)
            else:
                entry["x509_svid_ttl"] = ttl
        plan = plan_reconciliation(self.desired[:1], listing)
        server, lines = FakeServer(listing), []
        apply_plan(plan, ["spire-server"], server, log=lines.append, clock=lambda: "2026-11-24T12:00:00Z")
        return plan, lines

    def test_time_to_revoke_is_the_entry_ttl_and_the_summary_carries_the_maximum(self):
        plan, lines = self.deleted_everything([3590, 60, 7200])
        self.assertEqual(len(plan.delete), 2)
        deleted = [line for line in lines if line.startswith("deleted entry ")]
        self.assertEqual(len(deleted), 2)
        for line in deleted:
            fields = dict(part.split("=") for part in line.split()[5:])
            self.assertEqual(fields["time_to_revoke_seconds"], fields["x509_svid_ttl_seconds"])
        self.assertEqual(sorted(int(line.split("time_to_revoke_seconds=")[1]) for line in deleted), [60, 7200])
        self.assertEqual(lines[-1], "time to revoke maximum_seconds=7200 entries=2")

    def test_an_entry_without_a_ttl_revokes_by_the_rendered_server_default(self):
        _, lines = self.deleted_everything([None, 0, 0])
        deleted = [line for line in lines if line.startswith("deleted entry ")]
        self.assertTrue(deleted)
        for line in deleted:
            self.assertIn(f"x509_svid_ttl_seconds={REQUESTED_X509_SVID_TTL_SECONDS} ", line)
            self.assertIn("last_svid_expires_at=2026-11-24T12:59:50Z", line)

    def test_a_plan_that_deletes_nothing_logs_a_zero_summary(self):
        lines = []
        apply_plan(plan_reconciliation(self.desired, self.current), ["spire-server"], FakeServer(self.current), log=lines.append)
        self.assertEqual(lines, ["time to revoke maximum_seconds=0 entries=0"])

    def test_a_negative_listed_ttl_is_refused(self):
        listing = copy.deepcopy(self.current)
        listing["entries"][0]["x509_svid_ttl"] = -1
        with self.assertRaises(ReconcileRefused):
            plan_reconciliation(self.desired, listing)

    def test_dry_run_prints_the_plan_and_runs_no_mutation(self):
        server, lines = FakeServer(self.empty), []
        reconcile(self.desired, ["spire-server"], server, dry_run=True, log=lines.append)
        self.assertEqual(server.mutations(), [])
        self.assertIn('"steps"', lines[0])

    def command(self, *options, runner):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "registrations.json"
            path.write_text(json.dumps({"entries": self.desired}))
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                try:
                    return main(["--registrations", str(path), "--server-command", "spire-server", *options], runner=runner)
                except SystemExit as raised:
                    return raised.code

    def test_apply_refuses_without_or_with_a_wrong_approved_digest_before_contacting_the_server(self):
        digest = hashlib.sha256(json.dumps({"entries": self.desired}).encode()).hexdigest()
        for label, options in {"missing": [], "wrong": ["--approved-digest", "0" * 64], "not hex": ["--approved-digest", "approved"],
                               "uppercase": ["--approved-digest", digest.upper()], "empty": ["--approved-digest", ""]}.items():
            with self.subTest(label):
                server = FakeServer(self.empty)
                self.assertEqual(self.command(*options, runner=server), 1)
                self.assertEqual(server.calls, [])

    def test_apply_with_the_digest_of_the_registrations_bytes_converges(self):
        digest = hashlib.sha256(json.dumps({"entries": self.desired}).encode()).hexdigest()
        server = FakeServer(self.current)
        self.assertEqual(self.command("--approved-digest", digest, runner=server), 0)
        self.assertEqual([call[0][:2] for call in server.calls], [["entry", "show"], ["bundle", "list"]] * 2)

    def test_dry_run_check_and_current_need_no_digest_and_a_stale_digest_does_not_block_them(self):
        for option in ["--dry-run", "--check"]:
            with self.subTest(option):
                self.assertEqual(self.command(option, runner=FakeServer(self.current)), 0)
                self.assertEqual(self.command(option, "--approved-digest", "0" * 64, runner=FakeServer(self.current)), 0)

    def test_socket_path_follows_the_subcommand_on_every_call(self):
        server = FakeServer(self.empty)
        reconcile(self.desired, ["spire-server"], server, dry_run=True, socket_path="/run/spire/admin/api.sock")
        self.assertEqual(server.calls[0][0], ["entry", "show", "-socketPath", "/run/spire/admin/api.sock", "-output", "json"])
        server = FakeServer(self.current)
        plan = plan_reconciliation(self.desired[:2], self.current)
        apply_plan(plan, ["spire-server"], server, socket_path="/run/spire/admin/api.sock")
        self.assertEqual(server.calls[0][0][:4], ["entry", "delete", "-socketPath", "/run/spire/admin/api.sock"])
        self.assertEqual(self.command("--socket-path", "relative.sock", "--dry-run", runner=FakeServer(self.empty)), 1)

    def test_check_exits_zero_when_converged_two_on_drift_and_one_on_refusal(self):
        self.assertEqual(self.command("--check", runner=FakeServer(self.current)), 0)
        server = FakeServer(self.empty)
        self.assertEqual(self.command("--check", runner=server), 2)
        self.assertEqual(server.mutations(), [])
        truncated = listing(self.desired)
        truncated["next_page_token"] = "more"
        self.assertEqual(self.command("--check", runner=FakeServer(truncated)), 1)

    def command_with_stderr(self, *options, runner):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "registrations.json"
            path.write_text(json.dumps({"entries": self.desired}))
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()) as stderr:
                try:
                    code = main(["--registrations", str(path), "--server-command", "spire-server", *options], runner=runner)
                except SystemExit as raised:
                    code = raised.code
            return code, stderr.getvalue()

    def test_check_exits_three_naming_every_admin_and_downstream_entry(self):
        code, message = self.command_with_stderr("--check", runner=FakeServer(fixture("spire-1.16.0-privileged-listing.json")))
        self.assertEqual(code, 3)
        self.assertIn("privileged grant present", message)
        self.assertIn("a80522fd-0ccf-43e8-9477-673f11bfded2 spiffe://test.example.org/downstream (downstream)", message)
        self.assertIn("6bafa542-98a3-4280-b1db-7d36efed6f13 spiffe://test.example.org/operators/break-glass (admin)", message)

    def test_a_privileged_entry_wins_over_drift_and_every_mode_refuses_with_three(self):
        both = listing(self.desired[:1])
        both["entries"][0]["admin"] = both["entries"][0]["downstream"] = True
        for options in [["--check"], ["--dry-run"]]:
            with self.subTest(options=options):
                code, message = self.command_with_stderr(*options, runner=FakeServer(both))
                self.assertEqual(code, 3)
                self.assertIn("(admin and downstream)", message)
        self.assertEqual(self.command("--check", runner=FakeServer(self.empty)), 2)

    def test_the_exit_codes_are_distinct_and_stable(self):
        from reconcile import PRIVILEGED_GRANT_EXIT
        self.assertEqual(PRIVILEGED_GRANT_EXIT, 3)
        with self.assertRaises(ReconcileRefused):
            plan_reconciliation(self.desired, fixture("spire-1.16.0-privileged-listing.json"))

    def test_check_fails_on_a_broad_live_entry_and_dry_run_still_exits_zero(self):
        broad = copy.deepcopy(self.current)
        broad["entries"][0]["selectors"] = broad["entries"][0]["selectors"][:1]
        converged = copy.deepcopy(self.current)
        for entry in converged["entries"]:
            entry["selectors"] = entry["selectors"][:1]
        self.assertEqual(self.command("--check", runner=FakeServer(converged)), 2)
        self.assertEqual(self.command("--dry-run", runner=FakeServer(self.empty)), 0)
        self.assertEqual(self.command("--dry-run", runner=FakeServer(converged)), 0)

    def test_command_line_exits_non_zero_when_the_server_command_fails(self):
        def failing(argv, stdin=None):
            return 1, "", "no route to server"
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "registrations.json"
            path.write_text(json.dumps({"entries": self.desired}))
            with self.assertRaises(SystemExit) as raised, contextlib.redirect_stderr(io.StringIO()) as stderr:
                main(["--registrations", str(path), "--server-command", "kubectl exec -i pod -- spire-server",
                      "--approved-digest", hashlib.sha256(path.read_bytes()).hexdigest()], runner=failing)
        self.assertEqual(raised.exception.code, 1)
        self.assertIn("no route to server", stderr.getvalue())


class BundleTests(unittest.TestCase):
    """Stored federated bundles: the migration Job's is deleted, any other is reported and kept."""

    def setUp(self):
        self.desired = fixture("test-registrations.json")["entries"]
        self.current = fixture("spire-1.16.0-rendered-listing.json")
        self.migration = MIGRATION_TRUST_DOMAIN

    command = ApplyTests.command

    def test_apply_deletes_the_migration_bundle_with_restrict_and_the_socket_after_the_subcommand(self):
        server = FakeServer(self.current, bundles=[self.migration])
        plan = reconcile(self.desired, ["spire-server"], server, socket_path="/run/spire/admin/api.sock")
        self.assertEqual(plan.undeclared_bundles, (f"spiffe://{self.migration}",))
        self.assertEqual(server.mutations(), [(["bundle", "delete", "-socketPath", "/run/spire/admin/api.sock",
                                               "-id", f"spiffe://{self.migration}", "-mode", "restrict"], None)])
        self.assertEqual(server.calls[1][0], ["bundle", "list", "-socketPath", "/run/spire/admin/api.sock", "-output", "json"])
        self.assertEqual(server.bundles, [])

    def test_second_apply_after_cleanup_changes_nothing(self):
        server = FakeServer(self.current, bundles=[self.migration])
        reconcile(self.desired, ["spire-server"], server)
        server.calls.clear()
        plan = reconcile(self.desired, ["spire-server"], server)
        self.assertEqual((plan.steps(), plan.undeclared_bundles, plan.bundles_inspected), ([], (), True))
        self.assertEqual(server.mutations(), [])

    def test_an_unknown_foreign_bundle_is_refused_by_name_and_never_deleted(self):
        server = FakeServer(self.current, bundles=["partner.example.org", self.migration])
        with self.assertRaises(ReconcileRefused) as raised:
            reconcile(self.desired, ["spire-server"], server)
        self.assertIn("spiffe://partner.example.org", str(raised.exception))
        self.assertNotIn(self.migration, str(raised.exception))
        self.assertEqual(server.bundles, ["partner.example.org"])
        self.assertTrue(all("partner.example.org" not in " ".join(call[0]) for call in server.mutations()))
        server = FakeServer(self.current, bundles=["partner.example.org"])
        self.assertEqual(self.command(runner=server), 1)
        self.assertEqual(server.mutations(), [])

    def test_check_exits_two_while_any_bundle_is_stored_and_zero_when_none_remain(self):
        for bundles in (["migration.invalid"], ["partner.example.org"]):
            server = FakeServer(self.current, bundles=bundles)
            self.assertEqual(self.command("--check", runner=server), 2)
            self.assertEqual(server.mutations(), [])
        self.assertEqual(self.command("--check", runner=FakeServer(self.current)), 0)

    def test_dry_run_lists_the_bundle_and_changes_nothing(self):
        server, lines = FakeServer(self.current, bundles=[self.migration]), []
        reconcile(self.desired, ["spire-server"], server, dry_run=True, log=lines.append)
        printed = json.loads(lines[0])
        self.assertEqual(printed["undeclared_bundles"], [f"spiffe://{self.migration}"])
        self.assertEqual(printed["steps"], [f"delete bundle spiffe://{self.migration}"])
        self.assertEqual(server.mutations(), [])
        self.assertEqual(self.command("--dry-run", runner=FakeServer(self.current, bundles=["partner.example.org"])), 0)

    def test_offline_current_says_bundles_were_not_inspected(self):
        plan = plan_reconciliation(self.desired, self.current)
        self.assertEqual((plan.bundles_inspected, plan.undeclared_bundles), (False, ()))
        self.assertFalse(json.loads(plan.to_json())["bundles_inspected"])
        with tempfile.TemporaryDirectory() as directory:
            registrations_path, current_path = Path(directory) / "registrations.json", Path(directory) / "current.json"
            registrations_path.write_text(json.dumps({"entries": self.desired}))
            current_path.write_text(json.dumps(self.current))
            with contextlib.redirect_stdout(io.StringIO()) as stdout:
                self.assertEqual(main(["--registrations", str(registrations_path), "--current", str(current_path)]), 0)
        self.assertFalse(json.loads(stdout.getvalue())["bundles_inspected"])

    def test_a_bundle_listing_that_cannot_be_trusted_is_refused(self):
        for bad in [[], {"bundles": "x"}, {"bundles": [{}]}, {"bundles": [], "next_page_token": "more"}]:
            with self.subTest(bad=bad), self.assertRaises(ReconcileRefused):
                plan_reconciliation(self.desired, self.current, bad)


if __name__ == "__main__":
    unittest.main()
