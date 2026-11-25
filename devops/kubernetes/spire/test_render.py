import contextlib
import hashlib
import io
import tempfile
import unittest
import copy
import json
import re
from pathlib import Path
from unittest.mock import patch

import alert_tests
import profile
from render import input_commit, main, render, verify_image, IMAGES, COMMIT, IDENTITY, ISSUER


def configuration():
    return json.loads(Path(__file__).with_name("deployment.example.json").read_text())


def is_alias(entry):
    return entry["parent_id"].endswith("/spire/server")


def aliases(entries):
    return [entry for entry in entries if is_alias(entry)]


def workloads(entries):
    return [entry for entry in entries if not is_alias(entry)]


class ReleaseTrustTests(unittest.TestCase):
    def test_refuses_deployment_when_release_signature_is_unavailable(self):
        def unavailable(image):
            raise RuntimeError("release verification unavailable")

        with self.assertRaisesRegex(ValueError, "release verification failed"):
            render({}, unavailable)

    def test_verification_binds_immutable_digest_and_release_workflow(self):
        image = IMAGES["server"]
        response = type("Response", (), {
            "returncode": 0,
            "stdout": json.dumps([{"critical": {"image": {
                "docker-manifest-digest": "sha256:" + "b" * 64,
            }}}]),
        })()
        with patch("render.subprocess.run", return_value=response) as run:
            with self.assertRaisesRegex(ValueError, "release verification failed"):
                verify_image(image)
        command = run.call_args.args[0]
        for flag, value in [
            ("--certificate-oidc-issuer", ISSUER),
            ("--certificate-identity", IDENTITY),
            ("--certificate-github-workflow-sha", COMMIT),
        ]:
            self.assertEqual(command[command.index(flag) + 1], value)
        self.assertEqual(command[-1], image)


class DeploymentTests(unittest.TestCase):
    def render(self, config=None):
        return render(config or configuration(), lambda image: [])

    def test_pinned_issuer_backdating_keeps_generated_svid_validity_within_the_consumer_ceiling(self):
        files = self.render()
        server = json.loads(files["server.conf"])["server"]
        entries = json.loads(files["registrations.json"])["entries"]
        requested = []
        for field in ["default_x509_svid_ttl", "agent_ttl"]:
            duration = re.fullmatch(r"([0-9]+)([smh])", server[field])
            self.assertIsNotNone(duration, "the generated duration must use an explicit supported unit")
            amount, unit = duration.groups()
            requested.append((field, int(amount) * {"s": 1, "m": 60, "h": 3600}[unit]))
        requested.extend((entry["spiffe_id"], entry["x509_svid_ttl"]) for entry in entries)
        # The pinned issuer backdates NotBefore without shortening the requested TTL.
        for source, ttl_seconds in requested:
            with self.subTest(source=source):
                self.assertGreater(ttl_seconds, 0)
                self.assertLessEqual(ttl_seconds + 10, 3600, "the complete X.509 validity must fit the consumer ceiling")

    def test_secure_profile_renders_real_issuer_and_attestor_boundaries(self):
        files = self.render()
        resources = json.loads(files["manifests.json"])["items"]
        server = next(item for item in resources if item["kind"] == "StatefulSet")
        agent = next(item for item in resources if item["kind"] == "DaemonSet")
        self.assertEqual(server["spec"]["replicas"], 3)
        server_pod = server["spec"]["template"]["spec"]
        self.assertFalse(server_pod["automountServiceAccountToken"])
        self.assertTrue(server_pod["containers"][0]["securityContext"]["readOnlyRootFilesystem"])
        self.assertEqual(server_pod["containers"][0]["image"], IMAGES["server"])
        self.assertTrue(agent["spec"]["template"]["spec"]["hostPID"])
        self.assertNotIn("hostNetwork", agent["spec"]["template"]["spec"])
        server_config = json.loads(files["server.conf"])
        plugins = server_config["plugins"]
        self.assertEqual(set(plugins["NodeAttestor"][0]), {"k8s_psat"})
        kms = plugins["KeyManager"][0]["aws_kms"]["plugin_data"]
        self.assertTrue(kms["enable_tag_based_key_discovery"])
        self.assertNotIn("access_key_id", kms)
        database = plugins["DataStore"][0]["sql"]["plugin_data"]
        self.assertIn("sslmode=verify-full", database["connection_string"])
        self.assertIn("connect_timeout=5", database["connection_string"])
        self.assertTrue(database["disable_migration"])
        self.assertTrue(server_config["server"]["audit_log_enabled"])
        self.assertEqual(server_config["server"]["default_x509_svid_ttl"], "3590s")
        self.assertEqual(server_config["server"]["tls_config"]["min_tls_version"], "VersionTLS13")
        agent_config = json.loads(files["agent.conf"])
        attestor = agent_config["plugins"]["WorkloadAttestor"][0]["k8s"]["plugin_data"]
        self.assertEqual(attestor["kubelet_secure_port"], 10250)
        self.assertFalse(attestor["skip_kubelet_verification"])
        self.assertFalse(attestor["disable_container_selectors"])
        self.assertFalse(attestor["use_anonymous_authentication"])
        self.assertTrue(agent_config["agent"]["disable_sds_api"])

    def test_registration_requires_the_actual_supervisor_container(self):
        files = self.render()
        entries = json.loads(files["registrations.json"])["entries"]
        workload = workloads(entries)[0]
        self.assertFalse(workload["admin"])
        self.assertEqual(workload["x509_svid_ttl"], 3590)
        selectors = {(item["type"], item["value"]) for item in workload["selectors"]}
        registration = configuration()["registrations"][0]
        self.assertIn(("k8s", "container-name:supervisor"), selectors)
        self.assertIn(("k8s", "container-image:" + registration["container_image"]), selectors)
        self.assertIn(("unix", "uid:10001"), selectors)
        self.assertEqual(workload["parent_id"], aliases(entries)[0]["spiffe_id"])
        self.assertEqual(aliases(entries)[0]["parent_id"], "spiffe://production.identity.example.org/spire/server")
        self.assertIn("epoch-1", aliases(entries)[0]["spiffe_id"])
        mapping = json.loads(files["registration-plan.json"])
        self.assertEqual(mapping["recovery_epoch"], 1)
        self.assertEqual(mapping["registrations"][0]["tenant"], registration["tenant"])

    def test_application_identity_is_independent_of_the_transport_path(self):
        config = configuration()
        workload = config["registrations"][0]
        workload.pop("agent_id", None)
        workload["role"] = "agent"
        workload["application_id"] = "did:key:z6MkiTBz1ymuepAQ4UdtEKAAk1r94sSZ73KSXtrcfT3qQKVp"
        files = self.render(config)
        plan = json.loads(files["registration-plan.json"])
        self.assertEqual(plan["registrations"][0]["application_id"], workload["application_id"])
        self.assertEqual(plan["registrations"][0]["spiffe_id"], workload["spiffe_id"])

    def test_socket_admission_gate_is_enforced_and_never_matches_an_untrusted_default(self):
        resources = json.loads(self.render()["manifests.json"])["items"]
        policy = next(item for item in resources if item["kind"] == "ValidatingAdmissionPolicy" and item["metadata"]["name"].endswith("-identity-sockets"))
        binding = next(item for item in resources if item["kind"] == "ValidatingAdmissionPolicyBinding" and item["metadata"]["name"].endswith("-identity-sockets"))
        self.assertEqual(policy["spec"]["failurePolicy"], "Fail")
        self.assertEqual(binding["spec"]["validationActions"], ["Deny"])
        expressions = " ".join(item["expression"] for item in policy["spec"]["validations"])
        self.assertIn("ephemeralContainers", expressions)
        self.assertIn("initContainers", expressions)
        self.assertIn("readOnly", expressions)
        self.assertNotIn("namespaceObject", expressions)

    def test_refuses_unsafe_or_ambiguous_deployment_inputs(self):
        mutations = [
            lambda c: c.update(environment=[]),
            lambda c: c.update(node_names=[{}, {}, {}]),
            lambda c: c.update(api_server_cidrs=[42]),
            lambda c: c.pop("api_server_port"),
            lambda c: c.update(api_server_port=0),
            lambda c: c.update(api_server_port=65536),
            lambda c: c.update(api_server_port=True),
            lambda c: c.update(api_server_port="443"),
            lambda c: c.update(api_server_port=443.0),
            lambda c: c["registrations"][0].pop("node_names"),
            lambda c: c["registrations"][0].update(node_names=[]),
            lambda c: c["registrations"][0].update(node_names=["identity-node-a", "identity-node-a"]),
            lambda c: c["registrations"][0].update(node_names=["identity-node-z"]),
            lambda c: c["registrations"][0].update(node_names=["identity-control-a"]),
            lambda c: c["registrations"][0].update(node_names="identity-node-a"),
            lambda c: c["registrations"][0].update(node_names=[{}]),
            lambda c: c.update(server_replicas=1),
            lambda c: c.update(recovery_epoch=0),
            lambda c: c.update(trust_domain=c["other_environment_trust_domain"]),
            lambda c: c.update(server_role_arn=c["key_admin_role_arn"]),
            lambda c: c.update(api_server_cidrs=["0.0.0.0/0"]),
            lambda c: c.update(aws_endpoint_cidrs=["::/0"]),
            lambda c: c.update(password="not-a-credential"),
            lambda c: c["registrations"][0].update(container_image="registry.example.org/egress:latest"),
            lambda c: c["registrations"][0].update(uid=0),
            lambda c: c["registrations"][0].update(spiffe_id="spiffe://another.example.org/workload"),
            lambda c: c["registrations"].append(copy.deepcopy(c["registrations"][0])),
            lambda c: c.update(upstream_authority=None),
            lambda c: c.update(assurance_level="unrestricted"),
            lambda c: c["upstream_authority"].update(type="disk"),
            lambda c: c.update(upstream_authority={"type": "disk", "key_file_path": "/run/keys/ca.key", "cert_file_path": "/run/keys/ca.crt"}),
            lambda c: c["upstream_authority"].update(certificate_authority_arn="arn:aws:acm-pca:eu-west-1:999999999999:certificate-authority/12ac02bc-d425-49f7-ab78-570a44972772"),
            lambda c: c["upstream_authority"].update(ca_signing_template_arn="arn:aws:acm-pca:::template/SubordinateCACertificate_PathLen1/V1"),
            lambda c: c["upstream_authority"].update(signing_algorithm="SHA1WITHRSA"),
            lambda c: c["monitoring"].update(namespace=c["namespace"]),
            lambda c: c["monitoring"].update(namespace=c["registrations"][0]["namespace"]),
            lambda c: c.pop("monitoring"),
            lambda c: c.pop("server_operator_groups"),
            lambda c: c.update(server_operator_groups=[]),
            lambda c: c.update(server_operator_groups="identity-operators"),
            lambda c: c.update(server_operator_groups=["identity-operators", "identity-operators"]),
            lambda c: c.update(server_operator_groups=[""]),
            lambda c: c.update(server_operator_groups=["system:authenticated"]),
            lambda c: c.update(server_operator_groups=["system:unauthenticated"]),
            lambda c: c.update(server_operator_groups=["system:serviceaccounts"]),
            lambda c: c.update(server_operator_groups=[7]),
        ]
        for mutate in mutations:
            with self.subTest(mutation=mutate.__code__.co_firstlineno):
                config = configuration()
                mutate(config)
                with self.assertRaises(ValueError):
                    self.render(config)


def walk(value):
    """Yield every key and every string value in a nested JSON document."""
    if isinstance(value, dict):
        for key, item in value.items():
            yield key
            yield from walk(item)
    elif isinstance(value, list):
        for item in value:
            yield from walk(item)
    elif isinstance(value, str):
        yield value


ABSENT_INTEGRATIONS = ["allow_insecure_scheme", "oidc-discovery-provider", "oidc_discovery_provider", "envoy", "istio", "linkerd", "service-mesh"]


def integration_hits(files):
    """Every (file, marker) where a rendered file mentions an integration this deployment must not have."""
    return sorted((name, marker) for name, text in files.items() for marker in ABSENT_INTEGRATIONS if marker in text.lower())


def seconds(duration):
    amount, unit = re.fullmatch(r"([0-9]+)([smh])", duration).groups()
    return int(amount) * {"s": 1, "m": 60, "h": 3600}[unit]


class RenderedSafetyPinTests(unittest.TestCase):
    """Pin safe-by-omission behavior so a regression fails here, not in review."""

    @classmethod
    def setUpClass(cls):
        cls.files = render(configuration(), lambda image: [])
        cls.server = json.loads(cls.files["server.conf"])
        cls.agent = json.loads(cls.files["agent.conf"])
        cls.entries = json.loads(cls.files["registrations.json"])["entries"]
        cls.resources = json.loads(cls.files["manifests.json"])["items"]

    def test_agent_runs_per_node_and_serves_the_workload_api_on_the_reviewed_socket_not_the_spire_default(self):
        agent_socket = self.agent["agent"]["socket_path"]
        self.assertEqual(agent_socket, "/run/platform-spire/sockets/api.sock")
        self.assertNotEqual(agent_socket, "/tmp/spire-agent/public/api.sock")
        daemon_sets = [item for item in self.resources if item["kind"] == "DaemonSet"]
        self.assertEqual(len(daemon_sets), 1)
        container = daemon_sets[0]["spec"]["template"]["spec"]["containers"][0]
        self.assertIn(agent_socket.rsplit("/", 1)[0], [mount["mountPath"] for mount in container["volumeMounts"]])

    def test_forbidden_settings_are_never_rendered(self):
        documents = {"server.conf": self.server, "agent.conf": self.agent, "registrations.json": self.entries}
        forbidden = {
            "admin_ids", "authorized_delegates", "admin_socket_path", "log_selectors", "federation",
            "federates_with", "bundle_endpoint", "oidc-discovery-provider", "join_token", "insecure_bootstrap",
            "plugin_cmd",
        }
        for name, document in documents.items():
            with self.subTest(document=name):
                self.assertEqual(forbidden & set(walk(document)), set())
        for name in ["server.conf", "agent.conf"]:
            with self.subTest(document=name):
                self.assertFalse(
                    [key for key in walk(documents[name]) if key.startswith("allow_nonconforming") and key],
                    "nonconforming agent IDs must not be enabled",
                )

    def test_no_rendered_file_carries_an_insecure_scheme_discovery_provider_or_mesh_integration(self):
        self.assertEqual(integration_hits(self.files), [])
        self.assertTrue(self.agent["agent"]["disable_sds_api"])

    def test_the_integration_guard_fails_when_each_marker_is_injected_into_any_rendered_file(self):
        for name in self.files:
            for marker in ABSENT_INTEGRATIONS:
                with self.subTest(file=name, marker=marker):
                    self.assertEqual(integration_hits({**self.files, name: self.files[name] + marker.upper()}), [(name, marker)])

    def test_the_injected_manifest_forms_of_the_integrations_are_caught(self):
        manifests = json.loads(self.files["manifests.json"])
        manifests["items"][0].setdefault("spec", {})["containers"] = [{"name": "envoy", "args": ["--allow_insecure_scheme"]}]
        self.assertEqual(integration_hits({**self.files, "manifests.json": json.dumps(manifests)}), [("manifests.json", "allow_insecure_scheme"), ("manifests.json", "envoy")])

    def test_workload_attestors_are_exactly_k8s_and_unix_with_the_reviewed_selectors(self):
        attestors = self.agent["plugins"]["WorkloadAttestor"]
        self.assertEqual([set(item) for item in attestors], [{"k8s", "unix"}])
        self.assertEqual(set(self.agent["plugins"]), {"KeyManager", "NodeAttestor", "WorkloadAttestor"})

    def test_node_attestation_and_key_custody_are_exactly_the_reviewed_plugins(self):
        self.assertEqual({kind: [set(item) for item in items] for kind, items in self.agent["plugins"].items() if kind == "NodeAttestor"}, {"NodeAttestor": [{"k8s_psat"}]})
        self.assertEqual(self.agent["plugins"]["KeyManager"], [{"disk": {"plugin_data": {"directory": "/var/lib/spire/keys"}}}])
        self.assertEqual([set(item) for item in self.server["plugins"]["KeyManager"]], [{"aws_kms"}])
        self.assertEqual([set(item) for item in self.server["plugins"]["NodeAttestor"]], [{"k8s_psat"}])
        psat = self.server["plugins"]["NodeAttestor"][0]["k8s_psat"]["plugin_data"]["clusters"]["production-eu1"]
        self.assertEqual(psat["service_account_allow_list"], ["identity-agents:spire-agent"])
        self.assertEqual(psat["audience"], ["spire-server"])
        datastore = self.server["plugins"]["DataStore"]
        self.assertEqual(len(datastore), 1)
        self.assertEqual(datastore[0]["sql"]["plugin_data"]["database_type"], "postgres")

    def test_token_and_ca_lifetimes_are_bounded(self):
        server = self.server["server"]
        self.assertEqual(server["default_jwt_svid_ttl"], "5m")
        self.assertLessEqual(seconds(server["default_jwt_svid_ttl"]), 300)
        self.assertEqual(server["ca_ttl"], "24h")
        self.assertLessEqual(seconds(server["ca_ttl"]), 24 * 3600, "raising the CA lifetime needs a reviewed rotation plan")

    def test_every_entry_is_unprivileged_and_selected_by_namespace_and_service_account(self):
        required = {"k8s_psat": ("agent_ns:", "agent_sa:", "agent_node_name:"), "k8s": ("ns:", "sa:")}
        for entry in self.entries:
            with self.subTest(entry=entry["spiffe_id"]):
                self.assertIs(entry["admin"], False)
                self.assertIs(entry["downstream"], False)
                self.assertNotIn("federates_with", entry)
                self.assertGreaterEqual(len(entry["selectors"]), 2)
                kind = "k8s_psat" if entry["parent_id"].endswith("/spire/server") else "k8s"
                values = [item["value"] for item in entry["selectors"] if item["type"] == kind]
                for prefix in required[kind]:
                    self.assertTrue(any(value.startswith(prefix) for value in values), prefix)

    def test_cluster_roles_are_exactly_the_reviewed_rules(self):
        config = configuration()
        roles = {item["metadata"]["name"]: item["rules"] for item in self.resources if item["kind"] == "ClusterRole"}
        self.assertEqual(roles, {
            "identity-control-spire-server": [
                {"apiGroups": [""], "resources": ["pods", "nodes"], "verbs": ["get"]},
                {"apiGroups": ["authentication.k8s.io"], "resources": ["tokenreviews"], "verbs": ["create"]},
            ],
            "identity-agents-spire-agent": [
                {"apiGroups": [""], "resources": ["nodes/proxy"], "verbs": ["get"], "resourceNames": config["node_names"]},
            ],
        })


class UpstreamAuthorityTests(unittest.TestCase):
    def render_with(self, mutate):
        config = configuration()
        mutate(config)
        return render(config, lambda image: [])

    def test_regulated_profile_chains_the_server_ca_to_the_managed_private_ca(self):
        files = self.render_with(lambda c: None)
        authority = configuration()["upstream_authority"]
        plugins = json.loads(files["server.conf"])["plugins"]
        self.assertEqual(plugins["UpstreamAuthority"], [{"aws_pca": {"plugin_data": {
            "region": "eu-west-1", "certificate_authority_arn": authority["certificate_authority_arn"],
            "ca_signing_template_arn": authority["ca_signing_template_arn"], "signing_algorithm": "SHA256WITHECDSA",
        }}}])
        self.assertEqual([set(item) for item in plugins["KeyManager"]], [{"aws_kms"}])
        statements = {item["Sid"]: item for item in json.loads(files["server-role-policy.json"])["Statement"]}
        signing = statements["SignIntermediateWithUpstreamAuthority"]
        self.assertEqual(signing["Resource"], authority["certificate_authority_arn"])
        self.assertEqual(set(signing["Action"]), {
            "acm-pca:DescribeCertificateAuthority", "acm-pca:IssueCertificate", "acm-pca:GetCertificate",
        })

    def test_standard_profile_may_stay_self_rooted_and_then_renders_no_upstream_plugin_or_policy(self):
        files = self.render_with(lambda c: c.update(assurance_level="standard", upstream_authority=None))
        self.assertNotIn("UpstreamAuthority", json.loads(files["server.conf"])["plugins"])
        sids = {item["Sid"] for item in json.loads(files["server-role-policy.json"])["Statement"]}
        self.assertNotIn("SignIntermediateWithUpstreamAuthority", sids)

    def test_disk_upstream_authority_is_never_rendered(self):
        for mutate in [lambda c: c["upstream_authority"].update(type="disk"), lambda c: c.update(upstream_authority={"type": "disk"})]:
            with self.assertRaises(ValueError):
                self.render_with(mutate)


class OperatorAccessPolicyTests(unittest.TestCase):
    def policy_and_binding(self, config=None):
        files = render(config or configuration(), lambda image: [])
        items = json.loads(files["manifests.json"])["items"]
        name = "identity-control-identity-operator-access"
        return (next(i for i in items if i["kind"] == "ValidatingAdmissionPolicy" and i["metadata"]["name"] == name),
                next(i for i in items if i["kind"] == "ValidatingAdmissionPolicyBinding" and i["metadata"]["name"] == name))

    def test_connect_to_exec_attach_and_portforward_and_ephemeral_containers_are_refused_outside_the_groups(self):
        policy, binding = self.policy_and_binding()
        self.assertEqual(policy["spec"]["failurePolicy"], "Fail")
        self.assertEqual(policy["spec"]["matchConstraints"]["resourceRules"], [
            {"apiGroups": [""], "apiVersions": ["v1"], "operations": ["CONNECT"], "resources": ["pods/exec", "pods/attach", "pods/portforward"]},
            {"apiGroups": [""], "apiVersions": ["v1"], "operations": ["UPDATE"], "resources": ["pods/ephemeralcontainers"]},
        ])
        self.assertEqual(policy["spec"]["matchConstraints"]["namespaceSelector"]["matchExpressions"],
                         [{"key": "kubernetes.io/metadata.name", "operator": "In", "values": ["identity-agents", "identity-control"]}])
        self.assertEqual(binding["spec"], {"policyName": policy["metadata"]["name"], "validationActions": ["Deny"]})
        self.assertEqual(len(policy["spec"]["validations"]), 1)
        self.assertIn("request.userInfo.groups", policy["spec"]["validations"][0]["expression"])
        self.assertIn("server operator group", policy["spec"]["validations"][0]["message"])

    def test_the_declared_groups_are_the_only_groups_the_policy_names(self):
        config = configuration()
        config["server_operator_groups"] = ["sre-oncall", "identity-operators"]
        policy, _ = self.policy_and_binding(config)
        self.assertEqual(json.loads(policy["spec"]["variables"][0]["expression"]), ["sre-oncall", "identity-operators"])
        self.assertEqual(policy["spec"]["variables"][0]["name"], "operatorGroups")

    def test_the_example_deployment_declares_a_group(self):
        self.assertEqual(configuration()["server_operator_groups"], ["identity-operators"])


class TelemetryAndAlertTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.files = render(configuration(), lambda image: [])
        cls.server = json.loads(cls.files["server.conf"])
        cls.agent = json.loads(cls.files["agent.conf"])
        cls.resources = json.loads(cls.files["manifests.json"])["items"]
        cls.rules = json.loads(cls.files["alert-rules.json"])

    def find(self, kind, name):
        return next(item for item in self.resources if item["kind"] == kind and item["metadata"]["name"] == name)

    def test_server_and_agent_expose_prometheus_metrics_only_to_the_scraper(self):
        self.assertEqual(self.server["telemetry"], {"Prometheus": {"host": "0.0.0.0", "port": 9988}})
        self.assertEqual(self.agent["telemetry"], {"Prometheus": {"host": "0.0.0.0", "port": 9989}})
        self.assertTrue(self.server["server"]["audit_log_enabled"])
        for kind, name, port in [("StatefulSet", "spire-server", 9988), ("DaemonSet", "spire-agent", 9989)]:
            container = self.find(kind, name)["spec"]["template"]["spec"]["containers"][0]
            self.assertIn({"name": "metrics", "containerPort": port}, container["ports"])
        for namespace, port in [("identity-control", 9988), ("identity-agents", 9989)]:
            policy = next(item for item in self.resources if item["kind"] == "NetworkPolicy"
                          and item["metadata"]["namespace"] == namespace and item["metadata"]["name"].startswith("spire-"))
            rules = [rule for rule in policy["spec"]["ingress"] if rule["ports"] == [{"protocol": "TCP", "port": port}]]
            self.assertEqual(len(rules), 1)
            self.assertEqual(rules[0]["from"], [{
                "namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "monitoring"}},
                "podSelector": {"matchLabels": {"app.kubernetes.io/name": "prometheus"}},
            }])

    def test_the_rendered_server_logs_at_info_in_json_so_bundle_fetches_and_audit_lines_are_kept(self):
        self.assertEqual((self.server["server"]["log_level"], self.server["server"]["log_format"]), ("INFO", "json"))
        self.assertEqual((self.agent["agent"]["log_level"], self.agent["agent"]["log_format"]), ("INFO", "json"))
        self.assertTrue(self.server["server"]["audit_log_enabled"])
        self.assertNotIn("audit_log_enabled", self.agent["agent"])

    def test_every_event_alert_counts_the_first_event_of_a_new_series_one_name_at_a_time(self):
        for group in self.rules["groups"]:
            for rule in group["rules"]:
                with self.subTest(rule=rule.get("alert") or rule["record"]):
                    names = set(re.findall(r"spire_\w+", rule["expr"]))
                    self.assertNotIn("__name__", rule["expr"], "a regex name selector collides when two names share labels")
                    self.assertEqual(rule["expr"].count(" unless "), len(names))
                    self.assertEqual(rule["expr"].count("or vector(0)"), 2 * len(names))
                    self.assertEqual(rule["expr"].count("increase("), len(names))

    def test_the_join_token_alert_is_critical_and_fires_on_the_first_token(self):
        rule = next(rule for group in self.rules["groups"] for rule in group["rules"] if rule.get("alert") == "SpireJoinTokenCreated")
        self.assertEqual(rule["labels"], {"severity": "critical"})
        self.assertEqual(rule["for"], "0m")
        self.assertIn("spire_server_rpc_agent_v1_agent_create_join_token", rule["expr"])

    def test_the_promtool_cases_cover_new_flat_incrementing_reset_and_colliding_series_for_the_rendered_rules(self):
        document = alert_tests.test_document()
        names = [test["name"] for test in document["tests"]]
        self.assertEqual(len(names), len(set(names)))
        for wanted in ["appears at 1", "flat series", "increment", "counter reset", "same labels", "join token", "failed attestation appearing"]:
            self.assertTrue(any(wanted in name for name in names), wanted)
        for test in document["tests"]:
            alerts = {item["alertname"] for item in test["alert_rule_test"]}
            self.assertTrue(alerts <= {rule["alert"] for group in self.rules["groups"] for rule in group["rules"] if "alert" in rule})
            self.assertTrue(all(item["exp_alerts"][0]["exp_annotations"] for item in test["alert_rule_test"] if item["exp_alerts"]))
        self.assertEqual(alert_tests.rules_document(), self.rules)

    def test_admission_policy_pins_the_agent_container_ports(self):
        policy = self.find("ValidatingAdmissionPolicy", "identity-control-identity-sockets")
        process = next(item["expression"] for item in policy["spec"]["variables"] if item["name"] == "agentProcess")
        self.assertIn("object.spec.containers[0].ports[0].containerPort == 9989", process)
        self.assertNotIn("ports.size() == 0", process)

    def test_alert_rules_ship_in_a_configmap_in_the_monitoring_namespace(self):
        configmap = self.find("ConfigMap", "spire-alert-rules")
        self.assertEqual(configmap["metadata"]["namespace"], "monitoring")
        self.assertEqual(json.loads(configmap["data"]["alert-rules.json"]), self.rules)

    def test_alerts_cover_the_documented_catalogue_with_documented_metric_names(self):
        alerts = {rule["alert"]: rule["expr"] for group in self.rules["groups"] for rule in group["rules"] if "alert" in rule}
        self.assertEqual(set(alerts), {
            "SpireRegistrationEntryChanged", "SpireRegistrationEntryDatastoreWrite", "SpireAgentAttestationFailed",
            "SpireAgentBanned", "SpireJoinTokenCreated", "SpireFederationRelationshipChanged", "SpireWorkloadAttestationFailing",
            "SpireWorkloadIdentityDenied", "SpireSvidRotationFailed",
        })
        expected = {
            "SpireRegistrationEntryChanged": ["spire_server_rpc_entry_v1_entry_batch_create_entry",
                "spire_server_rpc_entry_v1_entry_batch_update_entry", "spire_server_rpc_entry_v1_entry_batch_delete_entry"],
            "SpireRegistrationEntryDatastoreWrite": ["spire_server_datastore_registration_entry_create",
                "spire_server_datastore_registration_entry_update", "spire_server_datastore_registration_entry_delete"],
            "SpireAgentAttestationFailed": ["spire_server_rpc_agent_v1_agent_attest_agent"],
            "SpireAgentBanned": ["spire_server_rpc_agent_v1_agent_ban_agent"],
            "SpireJoinTokenCreated": ["spire_server_rpc_agent_v1_agent_create_join_token"],
            "SpireFederationRelationshipChanged": ["spire_server_rpc_trustdomain_v1_trust_domain_batch_create_federation_relationship",
                "spire_server_rpc_trustdomain_v1_trust_domain_batch_update_federation_relationship",
                "spire_server_rpc_trustdomain_v1_trust_domain_batch_delete_federation_relationship"],
            "SpireWorkloadAttestationFailing": ["spire_agent_workload_api_workload_attestation"],
            "SpireWorkloadIdentityDenied": ["spire_agent_rpc_workload_api_fetch_x509svid", "spire_agent_rpc_workload_api_fetch_jwtsvid"],
            "SpireSvidRotationFailed": ["spire_agent_node_attestor_new_svid", "spire_server_svid_rotate"],
        }
        for name, metrics in expected.items():
            with self.subTest(alert=name):
                for metric in metrics:
                    self.assertIn(metric, alerts[name])
        for name in ["SpireAgentAttestationFailed", "SpireWorkloadAttestationFailing", "SpireSvidRotationFailed"]:
            self.assertIn('status!="OK"', alerts[name])
        self.assertIn('status="PermissionDenied"', alerts["SpireWorkloadIdentityDenied"])
        self.assertNotIn("spire_agent_agent_svid_rotate", json.dumps(self.rules), "k8s_psat agents re-attest, so this series is never emitted")
        pinned = {"spire_agent_node_attestor_new_svid", "spire_server_svid_rotate"}
        self.assertEqual({name for name in re.findall(r"spire_\w+", alerts["SpireSvidRotationFailed"])}, pinned)
        self.assertEqual({name for name in re.findall(r"spire_\w+", json.dumps(self.rules)) if "svid_rotate" in name or "new_svid" in name} - {"spire_svid_rotation_failures"}, pinned)


class ImageSelectorTests(unittest.TestCase):
    def test_every_workload_entry_requires_a_digest_pinned_image_selector(self):
        files = render(configuration(), lambda image: [])
        for entry in workloads(json.loads(files["registrations.json"])["entries"]):
            with self.subTest(entry=entry["spiffe_id"]):
                images = [item["value"] for item in entry["selectors"] if item["type"] == "k8s" and item["value"].startswith("container-image:")]
                self.assertEqual(len(images), 1)
                self.assertRegex(images[0], r"@sha256:[a-f0-9]{64}$")

    def test_tag_only_or_missing_image_is_refused(self):
        for image in ["registry.example.org/egress:latest", "registry.example.org/egress@sha256:abc", None]:
            with self.subTest(image=image):
                config = configuration()
                config["registrations"][0]["container_image"] = image
                with self.assertRaises(ValueError):
                    render(config, lambda i: [])


class NodeScopeAndMigrationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.config = configuration()
        cls.files = render(cls.config, lambda image: [])
        cls.items = json.loads(cls.files["manifests.json"])["items"]

    def kind(self, kind):
        return [item for item in self.items if item["kind"] == kind]

    def test_node_affinity_is_one_single_value_term_per_node_with_the_pool_expression(self):
        for kind in ["StatefulSet", "DaemonSet"]:
            with self.subTest(kind=kind):
                workload = self.kind(kind)[0]
                terms = workload["spec"]["template"]["spec"]["affinity"]["nodeAffinity"]["requiredDuringSchedulingIgnoredDuringExecution"]["nodeSelectorTerms"]
                nodes = self.config["server_node_names"] if kind == "StatefulSet" else self.config["node_names"]
                self.assertEqual([term["matchFields"][0]["values"] for term in terms], [[node] for node in nodes])
                for term in terms:
                    self.assertEqual(len(term["matchFields"]), 1)
                    self.assertEqual(term["matchFields"][0]["key"], "metadata.name")
                    self.assertEqual(len(term["matchExpressions"]), 1)

    def test_admission_policy_requires_single_value_node_terms(self):
        rendered = json.dumps(self.kind("ValidatingAdmissionPolicy"))
        self.assertIn("f.values.size() == 1", rendered)

    def test_api_server_port_is_validated_and_rendered_in_the_server_network_policy(self):
        policy = next(item for item in self.kind("NetworkPolicy") if item["metadata"]["name"] == "spire-server")
        ports = [port["port"] for rule in policy["spec"]["egress"] for port in rule.get("ports", [])]
        self.assertIn(self.config["api_server_port"], ports)
        config = configuration()
        config["api_server_port"] = 6443
        rendered = json.loads(render(config, lambda image: [])["manifests.json"])["items"]
        policy = next(item for item in rendered if item["kind"] == "NetworkPolicy" and item["metadata"]["name"] == "spire-server")
        rule = next(rule for rule in policy["spec"]["egress"] if any(block["ipBlock"]["cidr"] in config["api_server_cidrs"] for block in rule["to"] if "ipBlock" in block))
        self.assertEqual([port["port"] for port in rule["ports"]], [6443])

    def test_migration_job_reuses_the_pinned_image_and_datastore_and_is_least_privilege(self):
        job = self.kind("Job")[0]
        server = self.kind("StatefulSet")[0]["spec"]["template"]["spec"]
        pod = job["spec"]["template"]["spec"]
        containers = pod["initContainers"] + pod["containers"]
        self.assertEqual({container["image"] for container in containers}, {IMAGES["server"]})
        self.assertFalse(pod["automountServiceAccountToken"])
        self.assertEqual(pod["restartPolicy"], "OnFailure")
        for container in containers:
            self.assertFalse(container["securityContext"]["allowPrivilegeEscalation"])
            self.assertEqual(container["securityContext"]["capabilities"]["drop"], ["ALL"])
        migrate = json.loads(self.files["migrate.conf"])
        database = migrate["plugins"]["DataStore"][0]["sql"]["plugin_data"]
        live = json.loads(self.files["server.conf"])["plugins"]["DataStore"][0]["sql"]["plugin_data"]
        self.assertFalse(database["disable_migration"])
        self.assertEqual({key: value for key, value in database.items() if key != "disable_migration"}, {key: value for key, value in live.items() if key != "disable_migration"})
        self.assertEqual([volume["name"] for volume in pod["volumes"] if "secret" in volume], [volume["name"] for volume in server["volumes"] if "secret" in volume and volume["name"].startswith("database")])
        self.assertTrue(any(item["kind"] == "ConfigMap" and item["metadata"]["name"] == "spire-server-migrate" for item in self.items))

    def test_migration_job_name_changes_with_anything_its_immutable_template_or_config_carries(self):
        name = self.kind("Job")[0]["metadata"]["name"]
        self.assertRegex(name, r"^spire-server-migrate-[0-9a-f]{10}$")
        self.assertEqual(json.loads(render(configuration(), lambda image: [])["manifests.json"])["items"], self.items)
        changes = {
            "server node": lambda c: c["server_node_names"].append("control-extra"),
            "database secret": lambda c: c["database"].update(client_secret=c["database"]["client_secret"] + "-rotated"),
            "database CA": lambda c: c["database"].update(root_ca_configmap=c["database"]["root_ca_configmap"] + "-rotated"),
        }
        for label, change in changes.items():
            with self.subTest(change=label):
                config = configuration()
                change(config)
                items = json.loads(render(config, lambda image: [])["manifests.json"])["items"]
                self.assertNotEqual([item for item in items if item["kind"] == "Job"][0]["metadata"]["name"], name)

    def test_migration_job_is_covered_by_a_network_policy_and_the_default_deny(self):
        job = self.kind("Job")[0]
        labels = job["spec"]["template"]["metadata"]["labels"]
        policies = self.kind("NetworkPolicy")
        self.assertEqual(len(policies), 5)
        covering = [policy for policy in policies if all(labels.get(key) == value for key, value in policy["spec"]["podSelector"].get("matchLabels", {}).items()) and policy["spec"]["podSelector"].get("matchLabels")]
        self.assertTrue(covering)
        self.assertEqual(covering[0]["spec"]["ingress"], [])

    def test_agent_projected_token_audience_equals_the_server_node_attestor_audience(self):
        agent = self.kind("DaemonSet")[0]["spec"]["template"]["spec"]
        projected = next(volume for volume in agent["volumes"] if volume["name"] == "psat")["projected"]["sources"]
        audiences = [source["serviceAccountToken"]["audience"] for source in projected]
        clusters = json.loads(self.files["server.conf"])["plugins"]["NodeAttestor"][0]["k8s_psat"]["plugin_data"]["clusters"]
        self.assertEqual([[audience] for audience in audiences], [cluster["audience"] for cluster in clusters.values()])

    def test_every_workload_entry_has_exactly_the_five_selector_kinds(self):
        entries = workloads(json.loads(self.files["registrations.json"])["entries"])
        self.assertTrue(entries)
        for entry in entries:
            with self.subTest(entry=entry["spiffe_id"], parent=entry["parent_id"]):
                kinds = sorted(item["value"].split(":", 1)[0] for item in entry["selectors"])
                self.assertEqual(kinds, ["container-image", "container-name", "ns", "sa", "uid"])
                self.assertEqual(len(entry["selectors"]), 5)

    def test_each_workload_entry_is_rendered_once_per_allowed_node_under_that_nodes_alias(self):
        entries = json.loads(self.files["registrations.json"])["entries"]
        registration = self.config["registrations"][0]
        by_parent = {entry["spiffe_id"]: entry for entry in aliases(entries)}
        self.assertEqual(len(by_parent), len(self.config["node_names"]))
        children = [entry for entry in workloads(entries) if entry["spiffe_id"] == registration["spiffe_id"]]
        self.assertEqual(len(children), len(registration["node_names"]))
        for child in children:
            node = [item["value"] for item in by_parent[child["parent_id"]]["selectors"] if item["value"].startswith("agent_node_name:")]
            self.assertEqual(len(node), 1)
        self.assertEqual(sorted(value.split(":", 1)[1] for child in children for value in [next(item["value"] for item in by_parent[child["parent_id"]]["selectors"] if item["value"].startswith("agent_node_name:"))]), sorted(registration["node_names"]))
        for alias in by_parent.values():
            types = {item["type"] for item in alias["selectors"]}
            self.assertEqual(types, {"k8s_psat"})

    def test_a_registration_missing_a_selector_field_is_refused(self):
        for field in ["namespace", "service_account", "container", "container_image", "uid"]:
            with self.subTest(field=field):
                config = configuration()
                config["registrations"][0].pop(field, None)
                with self.assertRaises(ValueError):
                    render(config, lambda image: [])


class SpiffeIdConventionTests(unittest.TestCase):
    def render_with(self, mutate):
        config = configuration()
        mutate(config)
        return render(config, lambda image: [])

    def test_accepts_convention_paths(self):
        for path in ["/platform/egress", "/platform/egress/blue", "/team-a/svc_1/a/b", "/ab/c0"]:
            with self.subTest(path=path):
                self.render_with(lambda c: c["registrations"][0].update(spiffe_id="spiffe://production.identity.example.org" + path))

    def test_rejects_paths_outside_the_convention(self):
        generated = "-" + "bcdfghjk" + "-" + "xz246"
        rejected = {
            "single segment": "/egress",
            "too many segments": "/a/b/c/d/e",
            "uppercase": "/Platform/egress",
            "leading hyphen": "/platform/-egress",
            "trailing underscore": "/platform/egress_",
            "segment over 63": "/platform/" + "a" * 64,
            "uuid": "/platform/123e4567-e89b-12d3-a456-426614174000",
            "long hex": "/platform/" + "ab12" * 8,
            "pod name": "/platform/egress" + generated,
            "pod name with longest hash": "/platform/egress-bcdfghjklm-xz246",
            "trailing slash": "/platform/egress/",
            "empty segment": "/platform//egress",
            "query": "/platform/egress?x=1",
            "fragment": "/platform/egress#x",
            "trailing newline": "/platform/egress\n",
            "tab": "/platform/egress\t",
            "control character": "/platform/eg\x00ress",
            "userinfo": "@evil.example.org/platform/egress",
            "port": ":8443/platform/egress",
            "percent encoding": "/platform/egre%73s",
            "dot segment": "/platform/./egress",
            "dot dot segment": "/platform/../egress",
            "non ascii": "/platform/\u00e9gress",
            "over 2048 bytes": "/platform/" + "/".join(["a" * 60] * 40),
        }
        for name, path in rejected.items():
            with self.subTest(case=name):
                with self.assertRaises(ValueError):
                    self.render_with(lambda c: c["registrations"][0].update(spiffe_id="spiffe://production.identity.example.org" + path))

    def test_rejects_malformed_identities_outside_the_path(self):
        base = "spiffe://production.identity.example.org/platform/egress"
        rejected = {
            "uppercase scheme": "SPIFFE://production.identity.example.org/platform/egress",
            "mixed scheme": "Spiffe://production.identity.example.org/platform/egress",
            "other trust domain": "spiffe://other.example.org/platform/egress",
            "domain suffix": "spiffe://production.identity.example.org.evil.example/platform/egress",
            "userinfo": "spiffe://user@production.identity.example.org/platform/egress",
            "port": "spiffe://production.identity.example.org:8443/platform/egress",
            "query": base + "?a=b",
            "fragment": base + "#a",
            "trailing newline": base + "\n",
            "leading space": " " + base,
            "trailing slash": base + "/",
            "no scheme": "production.identity.example.org/platform/egress",
            "not a string": 7,
        }
        for name, identity in rejected.items():
            with self.subTest(case=name):
                with self.assertRaises(ValueError):
                    self.render_with(lambda c: c["registrations"][0].update(spiffe_id=identity))

    def test_refuses_ephemeral_values_in_workload_paths_but_keeps_version_segments(self):
        rejected = {
            "statefulset ordinal": "/platform/web-0", "ip shaped": "/platform/ip-10-0-0-1", "ip dotted": "/platform/10.0.0.1",
            "long digit run": "/platform/svc-1732406400", "date": "/platform/20261124", "dashed date": "/platform/run-2026-11-24",
            "timestamp": "/platform/1732406400", "replicaset hash": "/platform/egress-6d5f7c9b4d",
            "node name": "/platform/identity-node-a", "node name inside": "/platform/on-identity-node-a-x",
            "server node name": "/platform/identity-control-a",
        }
        for name, path in rejected.items():
            with self.subTest(case=name):
                with self.assertRaises(ValueError):
                    self.render_with(lambda c: c["registrations"][0].update(spiffe_id="spiffe://production.identity.example.org" + path))
        for path in ["/platform/egress-v2", "/platform/v2", "/platform/v10/api", "/team-a/svc_1/v2"]:
            with self.subTest(path=path):
                self.render_with(lambda c: c["registrations"][0].update(spiffe_id="spiffe://production.identity.example.org" + path))

    def test_an_identity_of_exactly_2048_bytes_is_accepted_and_one_byte_more_is_refused(self):
        domain = "production.identity.example.org"
        prefix = "spiffe://" + domain + "/"
        longest = prefix + "a" * (2048 - len(prefix))
        self.assertEqual(len(longest.encode()), 2048)
        self.assertEqual(profile.spiffe_path(longest, domain), longest[len("spiffe://" + domain):])
        with self.assertRaises(ValueError):
            profile.spiffe_path(longest + "a", domain)

    def test_rejects_unusable_trust_domains(self):
        for name, domain in {
            "uppercase": "Production.identity.example.org",
            "label over 63": "a" * 64 + ".example.org",
            "over 100": ".".join(["a" * 30] * 4) + ".org",
            "over 253": ".".join(["a" * 60] * 5) + ".org",
        }.items():
            with self.subTest(case=name):
                def mutate(config):
                    config["trust_domain"] = domain
                    config["registrations"][0]["spiffe_id"] = "spiffe://" + domain + "/platform/egress"
                with self.assertRaises(ValueError):
                    self.render_with(mutate)


COMMIT_ID = "a" * 40


class FakeGit:
    """A runner for the three git questions about the input file; `tracked` and `dirty` pick the answers."""

    def __init__(self, tracked=True, dirty=False, head=COMMIT_ID):
        self.tracked, self.dirty, self.head, self.calls = tracked, dirty, head, []

    def __call__(self, argv):
        self.calls.append(argv)
        command = argv[3]
        if command == "ls-files":
            return (0 if self.tracked else 1), ""
        if command == "status":
            return 0, (" M deployment.json\n" if self.dirty else "")
        return 0, self.head + "\n"


class VersionedInputTests(unittest.TestCase):
    def render_command(self, *options, runner):
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / "deployment.json"
            config.write_bytes(Path(__file__).with_name("deployment.example.json").read_bytes())
            output = Path(directory) / "rendered"
            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr):
                try:
                    status = main(["--config", str(config), "--output", str(output), *options], runner=runner, verify=lambda image: [])
                except SystemExit as raised:
                    status = raised.code
            plan = json.loads((output / "registration-plan.json").read_text()) if (output / "registration-plan.json").exists() else None
            return status, stderr.getvalue(), plan, hashlib.sha256(config.read_bytes()).hexdigest(), output.exists()

    def test_a_committed_input_renders_and_the_plan_carries_its_digest_and_commit(self):
        status, _, plan, digest, _ = self.render_command(runner=FakeGit())
        self.assertEqual(status, None)
        self.assertEqual(plan["input"], {"sha256": digest, "commit": COMMIT_ID})

    def test_an_input_not_tracked_by_git_is_refused_before_anything_is_written(self):
        status, message, plan, _, written = self.render_command(runner=FakeGit(tracked=False))
        self.assertEqual(status, 1)
        self.assertIn("not tracked by git", message)
        self.assertFalse(written)

    def test_an_input_with_uncommitted_changes_is_refused_before_anything_is_written(self):
        status, message, _, _, written = self.render_command(runner=FakeGit(dirty=True))
        self.assertEqual(status, 1)
        self.assertIn("uncommitted changes", message)
        self.assertFalse(written)

    def test_an_input_outside_any_repository_or_without_git_is_refused(self):
        def outside(argv):
            return 128, ""
        def missing(argv):
            raise FileNotFoundError("git")
        for runner in [outside, missing]:
            with self.subTest(runner=runner.__name__):
                status, _, _, _, written = self.render_command(runner=runner)
                self.assertEqual(status, 1)
                self.assertFalse(written)

    def test_the_escape_flag_renders_an_unversioned_input_without_asking_git_and_records_no_commit(self):
        git = FakeGit(tracked=False, dirty=True)
        status, _, plan, digest, _ = self.render_command("--allow-unversioned-input", runner=git)
        self.assertEqual(status, None)
        self.assertEqual(git.calls, [])
        self.assertEqual(plan["input"], {"sha256": digest, "commit": None})

    def test_git_is_asked_in_the_inputs_own_directory_about_that_file(self):
        git = FakeGit()
        self.assertEqual(input_commit(Path(__file__).with_name("deployment.example.json"), git), COMMIT_ID)
        for argv in git.calls:
            self.assertEqual(argv[:3], ["git", "-C", str(Path(__file__).resolve().parent)])
        self.assertIn("deployment.example.json", git.calls[0])

    def test_an_unreadable_head_is_refused(self):
        with self.assertRaises(ValueError):
            input_commit(Path(__file__), FakeGit(head="not-a-commit"))


class ReviewDigestTests(unittest.TestCase):
    def test_the_plan_carries_the_sha256_of_the_exact_registrations_bytes(self):
        files = render(configuration(), lambda image: [], {"sha256": "b" * 64, "commit": None})
        plan = json.loads(files["registration-plan.json"])
        self.assertEqual(plan["registrations_sha256"], hashlib.sha256(files["registrations.json"].encode()).hexdigest())
        self.assertEqual(plan["input"], {"sha256": "b" * 64, "commit": None})

    def test_a_changed_registration_changes_the_digest(self):
        config = configuration()
        before = json.loads(render(config, lambda image: [])["registration-plan.json"])["registrations_sha256"]
        config["registrations"][0]["node_names"] = ["identity-node-a"]
        self.assertNotEqual(json.loads(render(config, lambda image: [])["registration-plan.json"])["registrations_sha256"], before)


if __name__ == "__main__":
    unittest.main()
