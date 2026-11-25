"""Run the Rust JWT bundle source and JWT-SVID verifier against a real SPIRE agent.

Opt-in: skipped unless SPIRE_JWT_LIVE=1 and `docker` is available, so default
unit runs and Bazel stay hermetic. It starts the pinned SPIRE server and agent
on a private network, registers a `unix:uid` entry, mints a JWT-SVID with the
agent's own CLI, and runs rsworkspace/crates/platform/trogon-workload-identity/
tests/live_spire_agent.rs in a Linux Rust container that shares the agent's
socket volume and PID namespace (Unix sockets do not cross the Docker VM). A
holder container owns the namespace so that stopping the agent, which the harness
does when the test asks for it, does not kill the test. Every container, the
network and the socket volume are removed afterwards; the cargo build cache
volumes stay.

    SPIRE_JWT_LIVE=1 mise exec -- python3 -m unittest discover -v -s devops/kubernetes/spire -p 'test_jwt_bundle_live.py'
"""

import json
import os
import shutil
import subprocess
import tempfile
import threading
import time
import unittest
import uuid
from pathlib import Path

from render import IMAGES, VERSION

SERVER_IMAGE = "ghcr.io/spiffe/spire-server:" + VERSION + "@" + IMAGES["server"].split("@")[1]
AGENT_IMAGE = "ghcr.io/spiffe/spire-agent:" + VERSION + "@" + IMAGES["agent"].split("@")[1]
HOLDER_IMAGE = "busybox:1.37@sha256:a7dbc319b1438115a207e11930dc4bb3e643dcb3ec0bdd694bb36cd8fad289ec"
RUST_IMAGE = "rust:1.96.0-bookworm@sha256:5e2214abe154fe26e39f64488952e5c991eeed1d6d6da7cc8381ae83927f0cfc"
SERVER_BINARY = "/opt/spire/bin/spire-server"
AGENT_BINARY = "/opt/spire/bin/spire-agent"
TRUST_DOMAIN = "jwt-live.example.org"
AGENT_ID = f"spiffe://{TRUST_DOMAIN}/live/agent"
WORKLOAD_ID = f"spiffe://{TRUST_DOMAIN}/live/workload"
AUDIENCE = "https://gateway.example.test/"
SOCKET_DIRECTORY = "/run/spire/sockets"
SOCKET = SOCKET_DIRECTORY + "/agent.sock"
WORKLOAD_UID = "10001"
TARGET_VOLUME = "trogon-jwt-live-target"
REGISTRY_VOLUME = "trogon-jwt-live-cargo-registry"
STOP_MARKER = "LIVE_STOP_THE_AGENT"
REPOSITORY = Path(__file__).resolve().parents[3]
ENABLED = os.environ.get("SPIRE_JWT_LIVE") == "1" and shutil.which("docker") is not None

SERVER_CONF = f"""server {{
  bind_address = "0.0.0.0"
  bind_port = "8081"
  trust_domain = "{TRUST_DOMAIN}"
  data_dir = "/tmp/spire-data"
}}
plugins {{
  DataStore "sql" {{ plugin_data {{ database_type = "sqlite3"  connection_string = "/tmp/spire-data/datastore.sqlite3" }} }}
  KeyManager "memory" {{ plugin_data {{}} }}
  NodeAttestor "join_token" {{ plugin_data {{}} }}
}}
"""
AGENT_CONF = """agent {{
  data_dir = "/tmp/spire-agent"
  server_address = "{server}"
  server_port = "8081"
  socket_path = "{socket}"
  trust_domain = "{trust_domain}"
  insecure_bootstrap = true
  join_token = "{token}"
}}
plugins {{
  KeyManager "memory" {{ plugin_data {{}} }}
  NodeAttestor "join_token" {{ plugin_data {{}} }}
  WorkloadAttestor "unix" {{ plugin_data {{}} }}
}}
"""


def docker(*arguments, check=True, timeout=180):
    return subprocess.run(["docker", *arguments], capture_output=True, text=True, check=check, timeout=timeout)


def find_svid(document):
    if isinstance(document, dict):
        if isinstance(document.get("svid"), str):
            return document["svid"]
        document = list(document.values())
    for item in document if isinstance(document, list) else []:
        found = find_svid(item)
        if found:
            return found
    return None


@unittest.skipUnless(ENABLED, "set SPIRE_JWT_LIVE=1 with Docker available to run against a real SPIRE agent")
class LiveJwtBundleTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        suffix = uuid.uuid4().hex[:12]
        cls.server, cls.agent = f"spire-jwt-server-{suffix}", f"spire-jwt-agent-{suffix}"
        cls.holder, cls.test = f"spire-jwt-holder-{suffix}", f"spire-jwt-test-{suffix}"
        cls.network, cls.sockets = f"spire-jwt-net-{suffix}", f"spire-jwt-sockets-{suffix}"
        cls.addClassCleanup(cls.remove_resources)
        cls.build()
        docker("network", "create", cls.network)
        docker("volume", "create", cls.sockets)
        # The holder owns the PID namespace, so stopping the agent does not kill the test process.
        docker("run", "-d", "--name", cls.holder, HOLDER_IMAGE, "sleep", "3600")
        cls.start_server()
        cls.start_agent()

    @classmethod
    def remove_resources(cls):
        docker("rm", "-f", "-v", cls.test, cls.agent, cls.server, cls.holder, check=False)
        docker("network", "rm", cls.network, check=False)
        docker("volume", "rm", "-f", cls.sockets, check=False)

    @classmethod
    def build(cls):
        """Builds the test binary into the cache volume and records its path."""
        build = subprocess.run(
            ["docker", "run", "--rm", "-v", f"{REPOSITORY}:/src:ro", "-v", f"{TARGET_VOLUME}:/target",
             "-v", f"{REGISTRY_VOLUME}:/usr/local/cargo/registry", "-w", "/src/rsworkspace",
             "-e", "CARGO_TARGET_DIR=/target", "-e", "CARGO_INCREMENTAL=0", "-e", "RUSTUP_TOOLCHAIN=1.96.0",
             RUST_IMAGE, "cargo", "test", "-p", "trogon-workload-identity", "--all-features", "--locked",
             "--test", "live_spire_agent", "--no-run", "--message-format", "json"],
            capture_output=True, text=True, timeout=3000)
        if build.returncode != 0:
            raise AssertionError("the Rust build failed:\n" + build.stderr[-4000:])
        for line in build.stdout.splitlines():
            message = json.loads(line) if line.startswith("{") else {}
            if message.get("executable") and message["target"]["name"] == "live_spire_agent":
                cls.binary = message["executable"]
        if not hasattr(cls, "binary"):
            raise AssertionError("cargo did not report the live_spire_agent test binary")

    @classmethod
    def create_with_config(cls, name, image, binary, config, *options):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "spire.conf"
            path.write_text(config)
            docker("create", "--name", name, "--network", cls.network, *options, "--entrypoint", binary, image,
                   "run", "-config", "/tmp/spire.conf")
            docker("cp", str(path), name + ":/tmp/spire.conf")
        docker("start", name)

    @classmethod
    def wait_until(cls, ready, what):
        for _ in range(120):
            if ready():
                return
            time.sleep(0.5)
        raise AssertionError(what + " did not become ready")

    @classmethod
    def server_call(cls, *arguments):
        return docker("exec", cls.server, SERVER_BINARY, *arguments, check=False)

    @classmethod
    def start_server(cls):
        cls.create_with_config(cls.server, SERVER_IMAGE, SERVER_BINARY, SERVER_CONF)
        cls.wait_until(lambda: cls.server_call("entry", "show").returncode == 0, "the SPIRE server")

    @classmethod
    def start_agent(cls):
        generated = cls.server_call("token", "generate", "-spiffeID", AGENT_ID, "-output", "json")
        assert generated.returncode == 0, generated.stderr
        token = json.loads(generated.stdout)["value"]
        cls.create_with_config(cls.agent, AGENT_IMAGE, AGENT_BINARY,
                               AGENT_CONF.format(server=cls.server, socket=SOCKET, trust_domain=TRUST_DOMAIN, token=token),
                               "--cap-add", "SYS_PTRACE", f"--pid=container:{cls.holder}", "-v", f"{cls.sockets}:{SOCKET_DIRECTORY}")
        cls.wait_until(lambda: docker("exec", cls.agent, AGENT_BINARY, "healthcheck", "-socketPath", SOCKET, check=False).returncode == 0,
                       "the SPIRE agent")
        created = cls.server_call("entry", "create", "-parentID", AGENT_ID, "-spiffeID", WORKLOAD_ID,
                                  "-selector", "unix:uid:" + WORKLOAD_UID)
        assert created.returncode == 0, created.stderr

    def mint(self, audience):
        """A JWT-SVID from the agent's CLI, run as the registered uid inside the agent's PID namespace."""
        mint = ["docker", "exec", "--user", WORKLOAD_UID, self.agent, AGENT_BINARY, "api", "fetch", "jwt",
                "-audience", audience, "-socketPath", SOCKET, "-output", "json"]
        for _ in range(60):
            fetched = subprocess.run(mint, capture_output=True, text=True, timeout=60)
            token = find_svid(json.loads(fetched.stdout)) if fetched.returncode == 0 and fetched.stdout.strip() else None
            if token:
                return token
            time.sleep(1)
        raise AssertionError("the agent did not issue a JWT-SVID for the registered entry")

    def test_real_agent_bundle_delivery_verification_audience_and_staleness(self):
        environment = {**os.environ, "SPIRE_JWT_LIVE_TOKEN": self.mint(AUDIENCE)}
        test = ["docker", "run", "--rm", "--name", self.test, "--network", "none",
                f"--pid=container:{self.holder}", "--user", WORKLOAD_UID, "-v", f"{self.sockets}:{SOCKET_DIRECTORY}",
                "-v", f"{TARGET_VOLUME}:/target:ro", "-e", "SPIRE_JWT_LIVE_TOKEN",
                "-e", f"SPIRE_JWT_LIVE_SOCKET={SOCKET}", "-e", f"SPIRE_JWT_LIVE_AUDIENCE={AUDIENCE}",
                "-e", f"SPIRE_JWT_LIVE_SPIFFE_ID={WORKLOAD_ID}", "-e", f"SPIRE_JWT_LIVE_TRUST_DOMAIN={TRUST_DOMAIN}",
                "--entrypoint", self.binary, RUST_IMAGE, "--nocapture", "--test-threads=1"]
        process = subprocess.Popen(test, env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        watchdog = threading.Timer(300, process.kill)
        watchdog.start()
        lines = []
        try:
            for line in process.stdout:
                lines.append(line.rstrip())
                if "LIVE_" in line:
                    print(line[line.index("LIVE_"):].rstrip(), flush=True)
                if STOP_MARKER in line:
                    docker("stop", self.agent)
            status = process.wait()
        finally:
            watchdog.cancel()
            process.stdout.close()
        output = "\n".join(lines)
        self.assertEqual(status, 0, output[-4000:])
        for marker in ["LIVE_A bundle_delivered", "LIVE_B accepted", "LIVE_C refused", "LIVE_D stale"]:
            self.assertIn(marker, output)
        self.assertIn("1 passed", output)


if __name__ == "__main__":
    unittest.main()
