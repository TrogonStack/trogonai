import unittest
import copy
import json
from pathlib import Path
from unittest.mock import patch

from render import render, verify_image, IMAGES, COMMIT, IDENTITY, ISSUER


def configuration():
    return json.loads(Path(__file__).with_name("deployment.example.json").read_text())


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
        self.assertEqual(server_config["server"]["default_x509_svid_ttl"], "1h")
        self.assertEqual(server_config["server"]["tls_config"]["min_tls_version"], "VersionTLS13")
        agent_config = json.loads(files["agent.conf"])
        attestor = agent_config["plugins"]["WorkloadAttestor"][0]["k8s"]["plugin_data"]
        self.assertFalse(attestor["skip_kubelet_verification"])
        self.assertFalse(attestor["disable_container_selectors"])
        self.assertFalse(attestor["use_anonymous_authentication"])
        self.assertTrue(agent_config["agent"]["disable_sds_api"])

    def test_registration_requires_the_actual_supervisor_container(self):
        files = self.render()
        entries = json.loads(files["registrations.json"])["entries"]
        workload = entries[1]
        self.assertFalse(workload["admin"])
        self.assertEqual(workload["x509_svid_ttl"], 3600)
        selectors = {(item["type"], item["value"]) for item in workload["selectors"]}
        registration = configuration()["registrations"][0]
        self.assertIn(("k8s", "container-name:supervisor"), selectors)
        self.assertIn(("k8s", "container-image:" + registration["container_image"]), selectors)
        self.assertIn(("unix", "uid:10001"), selectors)
        self.assertEqual(workload["parent_id"], entries[0]["spiffe_id"])
        self.assertEqual(entries[0]["parent_id"], "spiffe://production.identity.example.org/spire/server")
        self.assertIn("epoch-1", entries[0]["spiffe_id"])
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
        policy = next(item for item in resources if item["kind"] == "ValidatingAdmissionPolicy")
        binding = next(item for item in resources if item["kind"] == "ValidatingAdmissionPolicyBinding")
        self.assertEqual(policy["spec"]["failurePolicy"], "Fail")
        self.assertEqual(binding["spec"]["validationActions"], ["Deny"])
        expressions = " ".join(item["expression"] for item in policy["spec"]["validations"])
        self.assertIn("ephemeralContainers", expressions)
        self.assertIn("initContainers", expressions)
        self.assertIn("readOnly", expressions)
        self.assertNotIn("namespaceObject", expressions)

    def test_refuses_unsafe_or_ambiguous_deployment_inputs(self):
        mutations = [
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
        ]
        for mutate in mutations:
            with self.subTest(mutation=mutate.__code__.co_firstlineno):
                config = configuration()
                mutate(config)
                with self.assertRaises(ValueError):
                    self.render(config)


if __name__ == "__main__":
    unittest.main()
