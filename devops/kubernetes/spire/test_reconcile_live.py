"""Apply reconciliation to a real SPIRE server in a throwaway container.

Opt-in: skipped unless SPIRE_RECONCILE_LIVE=1 and `docker` can start the pinned
server image, so default unit runs and Bazel stay hermetic. The container uses
a sqlite datastore inside itself and a test trust domain, and is removed with
its volumes when the tests finish.

    SPIRE_RECONCILE_LIVE=1 mise exec -- python3 -m unittest -v test_reconcile_live
"""

import contextlib
import copy
import hashlib
import io
import json
import os
import shutil
import subprocess
import tempfile
import time
import unittest
import uuid
from pathlib import Path

from profile import MIGRATION_TRUST_DOMAIN
from reconcile import ApplyFailed, ReconcileRefused, main, reconcile, run_command
from render import VERSION

# The multi-architecture index for the pinned release, pulled by digest.
IMAGE = "ghcr.io/spiffe/spire-server:" + VERSION + "@sha256:ff546f4c62dafc479a0cd79eae9a7b6df2975754d75efd31706098e0d7548837"
TRUST_DOMAIN = "test.example.org"
BINARY = "/opt/spire/bin/spire-server"
SERVER_CONF = """server {
  bind_address = "127.0.0.1"
  bind_port = "8081"
  trust_domain = "test.example.org"
  data_dir = "/tmp/spire-data"
}
plugins {
  DataStore "sql" { plugin_data { database_type = "sqlite3"  connection_string = "/tmp/spire-data/datastore.sqlite3" } }
  KeyManager "memory" { plugin_data {} }
  NodeAttestor "join_token" { plugin_data {} }
}
"""
ENABLED = os.environ.get("SPIRE_RECONCILE_LIVE") == "1" and shutil.which("docker") is not None


def docker(*arguments, check=True):
    return subprocess.run(["docker", *arguments], capture_output=True, text=True, check=check, timeout=180)


@unittest.skipUnless(ENABLED, "set SPIRE_RECONCILE_LIVE=1 with Docker available to run against a real SPIRE server")
class LiveReconciliationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.name = "spire-reconcile-test-" + uuid.uuid4().hex[:12]
        cls.server = ["docker", "exec", "-i", cls.name, BINARY]
        cls.addClassCleanup(lambda: docker("rm", "-f", "-v", cls.name, check=False))
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / "server.conf"
            config.write_text(SERVER_CONF)
            docker("create", "--name", cls.name, "--entrypoint", BINARY, IMAGE, "run", "-config", "/tmp/server.conf")
            docker("cp", str(config), cls.name + ":/tmp/server.conf")
        docker("start", cls.name)
        for _ in range(60):
            if run_command([*cls.server, "entry", "show", "-output", "json"])[0] == 0:
                return
            time.sleep(0.5)
        raise AssertionError("the SPIRE server did not become ready")

    def setUp(self):
        self.desired = json.loads(Path(__file__).with_name("testdata").joinpath("test-registrations.json").read_text())["entries"]
        self.calls = []
        self.empty_server()

    def runner(self, argv, stdin=None):
        self.calls.append(argv[len(self.server):])
        return run_command(argv, stdin)

    def listing(self):
        status, out, err = run_command([*self.server, "entry", "show", "-output", "json"])
        self.assertEqual(status, 0, err)
        return json.loads(out)["entries"]

    def empty_server(self):
        for entry in self.listing():
            run_command([*self.server, "entry", "delete", "-entryID", entry["id"]])

    def by_spiffe_id(self):
        return {"spiffe://" + entry["spiffe_id"]["trust_domain"] + entry["spiffe_id"]["path"]: entry for entry in self.listing()}

    def mutations(self):
        return [call for call in self.calls if call[:2] not in (["entry", "show"], ["bundle", "list"])]

    def test_apply_change_retire_and_rerun_converge(self):
        egress, ingress = (f"spiffe://{TRUST_DOMAIN}/platform/{name}" for name in ["egress", "ingress"])

        reconcile(self.desired, self.server, self.runner)
        first = self.by_spiffe_id()
        self.assertEqual(len(first), 3)
        self.assertFalse(any(entry["admin"] or entry["downstream"] for entry in first.values()))

        self.calls.clear()
        self.assertEqual(reconcile(self.desired, self.server, self.runner).steps(), [])
        self.assertEqual(self.mutations(), [])

        changed = copy.deepcopy(self.desired)
        replaced = next(entry for entry in changed if entry["spiffe_id"] == ingress)
        replaced["selectors"] = [dict(item, value="uid:10003") if item["value"].startswith("uid:") else item for item in replaced["selectors"]]
        added = copy.deepcopy(replaced)
        added["spiffe_id"] = f"spiffe://{TRUST_DOMAIN}/platform/audit"
        changed.append(added)
        self.calls.clear()
        plan = reconcile(changed, self.server, self.runner)
        self.assertEqual(plan.replaced, [ingress])
        self.assertEqual([call[1] for call in self.mutations()], ["create", "delete", "create"])
        second = self.by_spiffe_id()
        self.assertEqual(second[egress]["id"], first[egress]["id"])
        self.assertNotEqual(second[ingress]["id"], first[ingress]["id"])
        self.assertIn({"type": "unix", "value": "uid:10003"}, second[ingress]["selectors"])
        self.assertEqual(len(second), 4)

        retired = [entry for entry in changed if entry["spiffe_id"] != egress]
        self.calls.clear()
        reconcile(retired, self.server, self.runner)
        self.assertEqual(self.mutations(), [["entry", "delete", "-entryID", second[egress]["id"]]])
        self.assertNotIn(egress, self.by_spiffe_id())
        self.assertEqual(len(self.listing()), 3)

        self.calls.clear()
        self.assertEqual(reconcile(retired, self.server, self.runner).steps(), [])
        self.assertEqual(self.mutations(), [])

    def test_unowned_entries_survive_and_a_failed_step_is_resumed(self):
        unowned = [*self.server, "entry", "create", "-spiffeID", f"spiffe://{TRUST_DOMAIN}/team/service",
                   "-parentID", f"spiffe://{TRUST_DOMAIN}/team/alias", "-selector", "unix:uid:1"]
        self.assertEqual(run_command(unowned)[0], 0)
        reconcile(self.desired[:2], self.server, self.runner)
        failures = []

        def flaky(argv, stdin=None):
            if argv[len(self.server):][:2] == ["entry", "create"] and not failures:
                failures.append(argv)
                return 1, "", "injected failure"
            return run_command(argv, stdin)

        with self.assertRaises(ApplyFailed) as raised:
            reconcile(self.desired, self.server, flaky)
        self.assertIn("rerun to converge", str(raised.exception))
        self.assertEqual(len(self.listing()), 3)
        reconcile(self.desired, self.server, self.runner)
        self.assertEqual(len(self.listing()), 4)
        self.assertIn(f"spiffe://{TRUST_DOMAIN}/team/service", self.by_spiffe_id())

    def test_server_rejection_fails_with_a_message(self):
        broken = copy.deepcopy(self.desired)
        broken[1]["selectors"] = []
        with self.assertRaises(ApplyFailed) as raised:
            reconcile(broken, self.server, self.runner)
        self.assertIn("selector list is empty", str(raised.exception))

    def bundles(self):
        status, out, err = run_command([*self.server, "bundle", "list", "-output", "json"])
        self.assertEqual(status, 0, err)
        return [bundle["trust_domain"] for bundle in json.loads(out)["bundles"]]

    def set_bundle(self, trust_domain):
        """Store a federated bundle, using the server's own bundle as the payload."""
        status, own, err = run_command([*self.server, "bundle", "show"])
        self.assertEqual(status, 0, err)
        self.assertEqual(run_command([*self.server, "bundle", "set", "-id", "spiffe://" + trust_domain], own)[0], 0)

    def test_migration_bundle_is_deleted_and_a_foreign_bundle_is_refused_and_kept(self):
        self.addCleanup(lambda: [run_command([*self.server, "bundle", "delete", "-id", "spiffe://" + name]) for name in self.bundles()])
        self.assertEqual(self.bundles(), [])
        self.set_bundle(MIGRATION_TRUST_DOMAIN)
        self.set_bundle("partner.example.org")
        self.assertEqual(sorted(self.bundles()), [MIGRATION_TRUST_DOMAIN, "partner.example.org"])
        with self.assertRaises(ReconcileRefused) as raised:
            reconcile(self.desired, self.server, self.runner)
        self.assertIn("spiffe://partner.example.org", str(raised.exception))
        self.assertEqual(self.bundles(), ["partner.example.org"])
        self.assertEqual(len(self.listing()), 3)

        self.calls.clear()
        with self.assertRaises(ReconcileRefused):
            reconcile(self.desired, self.server, self.runner)
        self.assertEqual(self.mutations(), [])
        with tempfile.TemporaryDirectory() as directory:
            registrations = Path(directory) / "registrations.json"
            registrations.write_text(json.dumps({"entries": self.desired}))
            command = " ".join(self.server)
            self.enterContext(contextlib.redirect_stdout(io.StringIO()))
            self.enterContext(contextlib.redirect_stderr(io.StringIO()))
            approved = ["--approved-digest", hashlib.sha256(registrations.read_bytes()).hexdigest()]
            for options, code in [(["--check"], 2), (["--dry-run"], 0), (approved, 1)]:
                with self.subTest(options=options):
                    try:
                        status = main(["--registrations", str(registrations), "--server-command", command, *options])
                    except SystemExit as exited:
                        status = exited.code
                    self.assertEqual(status, code)
            self.assertEqual(self.bundles(), ["partner.example.org"])
            run_command([*self.server, "bundle", "delete", "-id", "spiffe://partner.example.org"])
            self.assertEqual(main(["--registrations", str(registrations), "--server-command", command, "--check"]), 0)

    def test_restrict_mode_keeps_a_migration_bundle_that_an_entry_federates_with(self):
        self.addCleanup(lambda: [run_command([*self.server, "bundle", "delete", "-id", "spiffe://" + name, "-mode", "dissociate"]) for name in self.bundles()])
        self.set_bundle(MIGRATION_TRUST_DOMAIN)
        federated = [*self.server, "entry", "create", "-spiffeID", f"spiffe://{TRUST_DOMAIN}/team/federated",
                     "-parentID", f"spiffe://{TRUST_DOMAIN}/team/alias", "-selector", "unix:uid:1", "-federatesWith", "spiffe://" + MIGRATION_TRUST_DOMAIN]
        self.assertEqual(run_command(federated)[0], 0)
        with self.assertRaises(ApplyFailed) as raised:
            reconcile(self.desired, self.server, self.runner)
        self.assertIn("bundle delete", str(raised.exception))
        self.assertEqual(self.bundles(), [MIGRATION_TRUST_DOMAIN])

    def test_command_line_dry_run_changes_nothing_and_apply_converges(self):
        with tempfile.TemporaryDirectory() as directory:
            registrations = Path(directory) / "registrations.json"
            registrations.write_text(json.dumps({"entries": self.desired}))
            command = " ".join(self.server)
            approved = ["--approved-digest", hashlib.sha256(registrations.read_bytes()).hexdigest()]
            self.enterContext(contextlib.redirect_stdout(io.StringIO()))
            self.assertEqual(main(["--registrations", str(registrations), "--server-command", command, "--dry-run"]), 0)
            self.assertEqual(self.listing(), [])
            with self.assertRaises(SystemExit) as refused, contextlib.redirect_stderr(io.StringIO()):
                main(["--registrations", str(registrations), "--server-command", command])
            self.assertEqual(refused.exception.code, 1)
            self.assertEqual(self.listing(), [])
            self.assertEqual(main(["--registrations", str(registrations), "--server-command", command, *approved]), 0)
            self.assertEqual(len(self.listing()), 3)
            with self.assertRaises(SystemExit) as raised:
                main(["--registrations", str(registrations), "--server-command", "docker exec -i no-such-container " + BINARY, *approved])
            self.assertEqual(raised.exception.code, 1)


@unittest.skipUnless(ENABLED and shutil.which("openssl") is not None,
                     "set SPIRE_RECONCILE_LIVE=1 with Docker and openssl available to probe a real SPIRE server")
class LiveServerTlsFloorTests(unittest.TestCase):
    """The rendered `tls_config` is honoured by the pinned server, and it is what refuses TLS 1.2."""

    def start(self, tls_config):
        name = "spire-tls-floor-test-" + uuid.uuid4().hex[:12]
        self.addCleanup(lambda: docker("rm", "-f", "-v", name, check=False))
        server = {"bind_address": "0.0.0.0", "bind_port": 8081, "trust_domain": TRUST_DOMAIN, "data_dir": "/tmp/spire-data"}
        if tls_config is not None:
            server["tls_config"] = tls_config
        document = {"server": server, "plugins": {
            "DataStore": {"sql": {"plugin_data": {"database_type": "sqlite3", "connection_string": "/tmp/spire-data/datastore.sqlite3"}}},
            "KeyManager": {"memory": {"plugin_data": {}}},
            "NodeAttestor": {"join_token": {"plugin_data": {}}},
        }}
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / "server.conf"
            config.write_text(json.dumps(document))
            docker("create", "--name", name, "-p", "127.0.0.1::8081", "--entrypoint", BINARY, IMAGE, "run", "-config", "/tmp/server.conf")
            docker("cp", str(config), name + ":/tmp/server.conf")
        docker("start", name)
        for _ in range(60):
            if docker("exec", name, BINARY, "healthcheck", check=False).returncode == 0:
                return docker("port", name, "8081/tcp").stdout.strip().splitlines()[0]
            time.sleep(0.5)
        raise AssertionError("the SPIRE server did not become ready: " + docker("logs", name, check=False).stderr[-2000:])

    def handshake(self, address, version):
        done = subprocess.run(["openssl", "s_client", "-connect", address, version], stdin=subprocess.DEVNULL,
                              capture_output=True, text=True, timeout=30)
        return "Cipher is (NONE)" not in done.stdout and "BEGIN CERTIFICATE" in done.stdout, done.stdout + done.stderr

    def test_the_rendered_tls_floor_refuses_tls_1_2_on_a_real_server(self):
        from profile import server_config

        deployment = json.loads(Path(__file__).with_name("deployment.example.json").read_text())
        rendered = server_config(deployment)["server"]["tls_config"]
        self.assertEqual(rendered, {"min_tls_version": "VersionTLS13"})

        floor = self.start(rendered)
        accepted, output = self.handshake(floor, "-tls1_3")
        self.assertTrue(accepted, output[-2000:])
        accepted, output = self.handshake(floor, "-tls1_2")
        self.assertFalse(accepted, output[-2000:])
        self.assertIn("protocol version", output)

        default = self.start(None)
        accepted, output = self.handshake(default, "-tls1_2")
        self.assertTrue(accepted, "without tls_config the pinned server accepts TLS 1.2, so the setting is what refuses it: " + output[-2000:])


if __name__ == "__main__":
    unittest.main()
