"""Deploy the rendered SPIRE manifests on a throwaway kind cluster and exercise them.

Opt-in: skipped unless SPIRE_CLUSTER_LIVE=1 and `kind`, `kubectl` and `docker` are available, so default unit
runs and Bazel stay hermetic. The test creates a seven-node kind cluster with its own kubeconfig in a temporary
directory (the default kubeconfig is never read or written), deletes the cluster when it finishes, and never
prints key material. It needs network access to pull the pinned node image and the rendered SPIRE images.

    SPIRE_CLUSTER_LIVE=1 mise exec -- python3 -m unittest -v test_cluster_live

The rendered deployment targets AWS. The rendered aws_kms KeyManager and aws_pca UpstreamAuthority plugin blocks run
unchanged against a moto AWS emulator pod inside the cluster. OVERLAY lists the only test-only substitutions:
everything else, including every NetworkPolicy, admission policy and security setting, is applied exactly as
rendered. Profile fields carry the real kind values (node names, node and API server addresses, cluster name, kubelet
CA, bootstrap bundle) and the emulator's values (endpoint address, private CA ARN). What each substitution leaves
unproven is in the README of this directory and in the `aws_unproven` result.
"""

import base64
import concurrent.futures
import contextlib
import copy
import hashlib
import io
import json
import os
import re
import shlex
import shutil
import ssl
import subprocess
import tempfile
import threading
import time
import unittest
import uuid
from datetime import datetime, timedelta, timezone
from pathlib import Path

from profile import AGENT_METRICS_PORT, MIGRATION_TRUST_DOMAIN, POOL_LABEL, SERVER_METRICS_PORT, SOCKET_DIRECTORY, SOCKET_PATH
import alert_tests
from reconcile import main as reconcile_main, run_command
from render import IMAGES, render

NODE_IMAGE = "kindest/node:v1.37.0@sha256:a1ed56cfb0e7b93589bdf97c8cd566405a265939e3620fc4f5de89adff580ae5"
POSTGRES_IMAGE = "postgres:16-alpine"
CURL_IMAGE = "curlimages/curl:8.11.1"
TRUST_DOMAIN = "kind.spire-test.example.org"
SERVER_BINARY = "/opt/spire/bin/spire-server"
AGENT_BINARY = "/opt/spire/bin/spire-agent"
DATABASE_HOST = "identity-db.database.svc.cluster.local"
SHORT_TTL = 90
SERVER_SOCKET = "/run/spire/admin/api.sock"
PROMETHEUS_IMAGE = "prom/prometheus:v3.7.3"
OUTAGE_PROBE_SECONDS = 180
EMULATOR_IMAGE = "motoserver/moto:5.2.5@sha256:59a1965d845249b04def37a0e2565f9cc6c44543cb89b68e90f28f344b3b0d1a"
EMULATOR_NAMESPACE = "aws-emulator"
EMULATOR_PORT = 443
ACCOUNT = "111122223333"
ROOT_TEMPLATE = "arn:aws:acm-pca:::template/RootCACertificate/V1"
MAX_NODES = 256
EMULATOR_LAUNCHER = """\
import threading
from cryptography import x509
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import Prehashed
from moto.acmpca import models
from moto.core.responses import BaseResponse
from moto.kms import responses as kms_responses, utils as kms_utils
from moto.server import main

SUBORDINATE = "arn:aws:acm-pca:::template/SubordinateCACertificate_PathLen0/V1"
original_extensions = models.CertificateAuthority._x509_extensions


def extensions(self, csr, template_arn):
    if template_arn != SUBORDINATE:
        return original_extensions(self, csr, template_arn)
    usage = x509.KeyUsage(digital_signature=True, content_commitment=False, key_encipherment=False, data_encipherment=False,
                          key_agreement=False, key_cert_sign=True, crl_sign=True, encipher_only=False, decipher_only=False)
    return [(x509.BasicConstraints(ca=True, path_length=0), True), (usage, True),
            (x509.SubjectKeyIdentifier.from_public_key(csr.public_key()), False),
            (x509.AuthorityKeyIdentifier.from_issuer_public_key(self.key.public_key()), False)]


models.CertificateAuthority._x509_extensions = extensions
digest = threading.local()
original_ecdsa_sign = kms_utils.ECDSAPrivateKey.sign
original_sign = kms_responses.KmsResponse.sign


def ecdsa_sign(self, message, signing_algorithm):
    if not getattr(digest, "prehashed", False):
        return original_ecdsa_sign(self, message, signing_algorithm)
    return self.private_key.sign(message, ec.ECDSA(Prehashed(self._ECDSAPrivateKey__hash_algorithm(signing_algorithm))))


def sign(self):
    digest.prehashed = self._get_param("MessageType") == "DIGEST"
    try:
        return original_sign(self)
    finally:
        digest.prehashed = False


kms_utils.ECDSAPrivateKey.sign = ecdsa_sign
kms_responses.KmsResponse.sign = sign
call_action = BaseResponse.call_action
log_lock = threading.Lock()


def logged(self):
    result = call_action(self)
    with log_lock:
        print("API", self.__class__.__module__.split(".")[1], self._get_action(), result[0], flush=True)
    return result


BaseResponse.call_action = logged
main(["-H", "0.0.0.0", "-p", "443"])
"""
EMULATOR_PRELUDE = """\
import json, boto3
options = dict(endpoint_url="http://localhost:443", region_name="eu-west-1", aws_access_key_id="emulator-admin", aws_secret_access_key="emulator-admin")
kms, pca = (boto3.client(name, **options) for name in ("kms", "acm-pca"))
"""
OVERLAY = [
    "AWS endpoint: the rendered aws_kms and aws_pca plugin blocks, the web-identity credential variables and every NetworkPolicy run as rendered; the only server change is two added env vars, AWS_ENDPOINT_URL and AWS_ENDPOINT_URL_STS = http://<emulator pod IP>:443 (AWS SDK for Go v2 endpoint overrides, plain HTTP; the STS-specific one is required because the web identity credential provider builds its STS client before the global override is applied), so KMS, ACM PCA and the STS AssumeRoleWithWebIdentity call go to a moto emulator pod instead of AWS",
    "deployment input values (not rendered-output edits): aws_endpoint_cidrs = the emulator pod /32 (the rendered egress rule on TCP 443 is what lets the server reach it) and upstream_authority.certificate_authority_arn = the ARN of the private root CA created in the emulator before the servers start; region, role ARN and the account 111122223333 are the example input values",
    f"emulator: {EMULATOR_IMAGE} runs unprivileged on port 443 (sysctl net.ipv4.ip_unprivileged_port_start=0) through a launcher that adds three things moto lacks: KMS Sign with MessageType DIGEST (moto hashes the digest again, so every ECDSA signature the server gets back is invalid), the SubordinateCACertificate_PathLen0 template (moto issues a subordinate without basicConstraints CA:TRUE) and one log line per API call; moto ignores requested validity (365 days), signs every issued certificate RSA SHA-512 and keeps an RSA CA key whatever the CA configuration says, and does not validate IAM, signatures or the web identity token",
    "the private root CA is created, given a self-issued root certificate (RootCACertificate/V1) and activated through the emulator API; its certificate is the bootstrap bundle the agents trust (it replaces the throwaway disk upstream root)",
    "server.agent_ttl and server.default_x509_svid_ttl 3590s -> 300s so agent and server SVID rotation, and their alert series, occur within one run",
    "deployment input server_operator_groups = the groups of the kind admin as `kubectl auth whoami` reports them (system:authenticated removed), so the test's own kubectl exec calls pass the rendered operator access policy",
    "registration of the short-lived pilot: x509_svid_ttl 3590 -> 90 on the entry created through reconcile",
    "release signature verification stubbed (cosign and network trust are not exercised)",
    "admission ceiling test only: the two generated ValidatingAdmissionPolicies and bindings are temporarily replaced by the ones rendered for 256 nodes and restored afterwards",
]
AWS_UNPROVEN = [
    "real AWS IAM authorisation: the emulator accepts any caller, so the rendered server-role policy, the KMS key policy principals and conditions and the IRSA trust policy were not evaluated",
    "real KMS: the emulator holds keys in process memory, so the HSM boundary, key non-exportability, request quotas, throttling and key deletion scheduling were not exercised",
    "real ACM PCA: the emulator ignores validity and signing algorithm, issues synchronously, signs RSA SHA-512 and implements the subordinate template only through the launcher patch, so real issuance latency, CA activation state, quotas and template semantics were not exercised",
    "real STS and the EKS OIDC provider: the projected token is sent but never validated, so the audience, issuer and subject conditions of the trust policy were not evaluated",
    "TLS to the AWS endpoints: the emulator is reached over plain HTTP on 443",
]
ENABLED = os.environ.get("SPIRE_CLUSTER_LIVE") == "1" and all(shutil.which(tool) for tool in ["kind", "kubectl", "docker"])
STARTED = time.monotonic()


def log(message):
    print(f"[{time.monotonic() - STARTED:7.1f}s] {message}", flush=True)


def run(argv, stdin=None, check=True, timeout=600):
    done = subprocess.run(argv, input=stdin, capture_output=True, text=True, timeout=timeout)
    if check and done.returncode:
        raise AssertionError(f"{' '.join(argv[:2])} {' '.join(a for a in argv[2:6] if not a.startswith('--kubeconfig'))} failed: {(done.stderr or done.stdout).strip()[-1500:]}")
    return done


def held_open(argv, seconds=8):
    """Start a long-running command, let it run, and return (still running, stdout, stderr) after killing it."""
    process = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        process.wait(timeout=seconds)
        running = False
    except subprocess.TimeoutExpired:
        running = True
        process.kill()
    out, err = process.communicate()
    return running, out, err


def wait_for(check, timeout, message, interval=3):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            result = check()
            if result:
                return result
        except (AssertionError, subprocess.SubprocessError, ValueError) as error:
            last = error
        time.sleep(interval)
    raise AssertionError(f"timed out waiting for {message}" + (f": {last}" if last else ""))


def openssl(*arguments):
    run(["openssl", *arguments])


def authority(directory, name, subject):
    openssl("req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes", "-days", "2",
            "-keyout", f"{directory}/{name}.key", "-out", f"{directory}/{name}.crt", "-subj", subject,
            "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign,cRLSign")


def leaf(directory, name, subject, signer, extension):
    openssl("req", "-new", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes",
            "-keyout", f"{directory}/{name}.key", "-out", f"{directory}/{name}.csr", "-subj", subject)
    Path(f"{directory}/{name}.ext").write_text(extension)
    openssl("x509", "-req", "-in", f"{directory}/{name}.csr", "-CA", f"{directory}/{signer}.crt", "-CAkey", f"{directory}/{signer}.key",
            "-CAcreateserial", "-days", "2", "-out", f"{directory}/{name}.crt", "-extfile", f"{directory}/{name}.ext")


def parse_time(text):
    match = re.search(r"(\d{4}-\d\d-\d\d \d\d:\d\d:\d\d) ([+-]\d{4})", text)
    return datetime.strptime(" ".join(match.groups()), "%Y-%m-%d %H:%M:%S %z").astimezone(timezone.utc)


def series(text):
    return {line.split("{")[0].split(" ")[0] for line in text.splitlines() if line.startswith("spire_")}


def alert_series(rules):
    """Every spire_ series name the rendered rules query."""
    return {name for group in rules["groups"] for rule in group["rules"] for name in re.findall(r"spire_\w+", rule["expr"])}


def counter_total(text, name):
    """Sum of one call counter over every scraped series line."""
    return sum(float(match.group(1)) for match in re.finditer(rf"^{name}(?:{{[^}}]*}})? ([0-9.e+]+)$", text, re.M))


def audit_lines(text):
    """The parsed `type: audit` JSON log lines of one server pod."""
    found = []
    for line in text.splitlines():
        if '"type":"audit"' in line:
            with contextlib.suppress(ValueError):
                found.append(json.loads(line))
    return found


def labelled_series(text):
    """Map each spire_ series name to the sorted label sets it was scraped with."""
    found = {}
    for line in text.splitlines():
        match = re.match(r"(spire_\w+)(?:\{(.*)\})? ", line)
        if match:
            labels = tuple(sorted(re.findall(r'(\w+)="([^"]*)"', match.group(2) or "")))
            found.setdefault(match.group(1), set()).add(labels)
    return found


def pod_document(name, namespace, spec, labels=None):
    return {"apiVersion": "v1", "kind": "Pod", "metadata": {"name": name, "namespace": namespace, **({"labels": labels} if labels else {})}, "spec": spec}


def curl_pod(name, namespace, node):
    return pod_document(name, namespace, {
        "nodeName": node, "automountServiceAccountToken": False, "terminationGracePeriodSeconds": 0,
        "securityContext": {"runAsNonRoot": True, "runAsUser": 100, "seccompProfile": {"type": "RuntimeDefault"}},
        "containers": [{"name": "curl", "image": CURL_IMAGE, "imagePullPolicy": "IfNotPresent", "command": ["sleep", "7200"],
                        "securityContext": {"allowPrivilegeEscalation": False, "readOnlyRootFilesystem": True, "capabilities": {"drop": ["ALL"]}}}],
    }, {"app.kubernetes.io/name": "prometheus"})


@unittest.skipUnless(ENABLED, "set SPIRE_CLUSTER_LIVE=1 with kind, kubectl and docker available to run against a kind cluster")
class ClusterLiveTests(unittest.TestCase):
    results = {}

    @classmethod
    def setUpClass(cls):
        cls.name = "spire-cluster-test-" + uuid.uuid4().hex[:8]
        cls.work = tempfile.mkdtemp(prefix=cls.name + "-")
        cls.kubeconfig = f"{cls.work}/kubeconfig"
        cls.addClassCleanup(shutil.rmtree, cls.work, True)
        cls.addClassCleanup(lambda: run(["kind", "delete", "cluster", "--name", cls.name, "--kubeconfig", cls.kubeconfig], check=False))
        cls.addClassCleanup(lambda: log("results " + json.dumps(cls.results, sort_keys=True)))
        cls.create_cluster()
        cls.prepare_cluster()
        cls.start_database()
        cls.start_emulator()
        cls.render_deployment()
        cls.apply_deployment()

    @classmethod
    def kubectl(cls, *arguments, stdin=None, check=True, timeout=300):
        return run(["kubectl", "--kubeconfig", cls.kubeconfig, *arguments], stdin, check, timeout)

    @classmethod
    def apply(cls, documents):
        cls.kubectl("apply", "-f", "-", stdin=json.dumps({"apiVersion": "v1", "kind": "List", "items": documents}))

    @classmethod
    def create_cluster(cls):
        config = Path(cls.work, "kind.yaml")
        config.write_text("kind: Cluster\napiVersion: kind.x-k8s.io/v1alpha4\nnodes:\n- role: control-plane\n" + "- role: worker\n" * 6)
        log("creating the kind cluster")
        run(["kind", "create", "cluster", "--name", cls.name, "--kubeconfig", cls.kubeconfig, "--image", NODE_IMAGE, "--config", str(config), "--wait", "300s"])
        workers = sorted(cls.kubectl("get", "nodes", "-l", "!node-role.kubernetes.io/control-plane", "-o", "name").stdout.split())
        cls.nodes = [item.removeprefix("node/") for item in workers]
        assert len(cls.nodes) == 6
        cls.servers, cls.agents = cls.nodes[:3], cls.nodes[3:]
        slice_path = "jsonpath={.items[0].endpoints[0].addresses[0]} {.items[0].ports[0].port}"
        cls.api_endpoint, port = cls.kubectl("get", "endpointslices", "-l", "kubernetes.io/service-name=kubernetes", "-o", slice_path).stdout.split()
        cls.api_port = int(port)
        cls.node_ips = {}
        for node in cls.nodes:
            cls.node_ips[node] = cls.kubectl("get", "node", node, "-o", r"jsonpath={.status.addresses[?(@.type=='InternalIP')].address}").stdout.strip()

    @classmethod
    def prepare_cluster(cls):
        for index, node in enumerate(cls.servers):
            cls.kubectl("label", "node", node, "--overwrite", f"{POOL_LABEL}={cls.name}-control", f"topology.kubernetes.io/zone=zone-{index}")
        for node in cls.agents:
            cls.kubectl("label", "node", node, "--overwrite", f"{POOL_LABEL}={cls.name}")
        documents = [{"apiVersion": "storage.k8s.io/v1", "kind": "StorageClass", "metadata": {"name": "encrypted-retained"},
                      "provisioner": "kubernetes.io/no-provisioner", "volumeBindingMode": "WaitForFirstConsumer", "reclaimPolicy": "Retain"}]
        for index, node in enumerate(cls.servers):
            path = f"/var/lib/spire-volume-{index}"
            run(["docker", "exec", node, "sh", "-c", f"mkdir -p {path} && chmod 0777 {path}"])
            documents.append({"apiVersion": "v1", "kind": "PersistentVolume", "metadata": {"name": f"spire-server-data-{index}"}, "spec": {
                "capacity": {"storage": "5Gi"}, "accessModes": ["ReadWriteOnce"], "persistentVolumeReclaimPolicy": "Retain",
                "storageClassName": "encrypted-retained", "local": {"path": path},
                "claimRef": {"namespace": "identity-control", "name": f"data-spire-server-{index}"},
                "nodeAffinity": {"required": {"nodeSelectorTerms": [{"matchExpressions": [{"key": "kubernetes.io/hostname", "operator": "In", "values": [node]}]}]}},
            }})
        cls.apply(documents)
        for image, nodes in [(POSTGRES_IMAGE, cls.servers[:1]), (CURL_IMAGE, cls.servers[:2])]:
            run(["kind", "load", "docker-image", image, "--name", cls.name, "--nodes", ",".join(nodes)], check=False)

    @classmethod
    def start_database(cls):
        directory = cls.work
        authority(directory, "database-ca", "/CN=test database CA")
        leaf(directory, "database-server", f"/CN={DATABASE_HOST}", "database-ca", f"subjectAltName=DNS:{DATABASE_HOST}\nextendedKeyUsage=serverAuth\n")
        leaf(directory, "database-client", "/CN=identity_server", "database-ca", "extendedKeyUsage=clientAuth\n")
        hba = "local all all trust\nhostssl all all 0.0.0.0/0 cert\n"
        cls.kubectl("create", "namespace", "database")
        cls.kubectl("create", "secret", "generic", "database-tls", "-n", "database", f"--from-file=server.crt={directory}/database-server.crt",
                    f"--from-file=server.key={directory}/database-server.key", f"--from-file=ca.crt={directory}/database-ca.crt")
        cls.kubectl("create", "configmap", "database-hba", "-n", "database", f"--from-literal=pg_hba.conf={hba}")
        postgres = pod_document("identity-db", "database", {
            "nodeName": cls.servers[0], "terminationGracePeriodSeconds": 5, "automountServiceAccountToken": False,
            "securityContext": {"runAsNonRoot": True, "runAsUser": 70, "runAsGroup": 70, "fsGroup": 70, "seccompProfile": {"type": "RuntimeDefault"}},
            "containers": [{
                "name": "postgres", "image": POSTGRES_IMAGE, "imagePullPolicy": "IfNotPresent",
                "args": ["-c", "ssl=on", "-c", "ssl_cert_file=/run/tls/server.crt", "-c", "ssl_key_file=/run/tls/server.key",
                         "-c", "ssl_ca_file=/run/tls/ca.crt", "-c", "hba_file=/run/conf/pg_hba.conf"],
                "env": [{"name": "POSTGRES_USER", "value": "identity_server"}, {"name": "POSTGRES_DB", "value": "identity"},
                        {"name": "POSTGRES_HOST_AUTH_METHOD", "value": "trust"}, {"name": "PGDATA", "value": "/var/lib/data/pg"}],
                "securityContext": {"allowPrivilegeEscalation": False, "readOnlyRootFilesystem": True, "capabilities": {"drop": ["ALL"]}},
                "readinessProbe": {"exec": {"command": ["pg_isready", "-U", "identity_server", "-d", "identity"]}, "periodSeconds": 2},
                "volumeMounts": [{"name": "data", "mountPath": "/var/lib/data"}, {"name": "run", "mountPath": "/var/run/postgresql"},
                                 {"name": "tmp", "mountPath": "/tmp"}, {"name": "tls", "mountPath": "/run/tls", "readOnly": True},
                                 {"name": "conf", "mountPath": "/run/conf", "readOnly": True}],
            }],
            "volumes": [{"name": "data", "emptyDir": {}}, {"name": "run", "emptyDir": {}}, {"name": "tmp", "emptyDir": {}},
                        {"name": "tls", "secret": {"secretName": "database-tls", "defaultMode": 0o640}}, {"name": "conf", "configMap": {"name": "database-hba"}}],
        }, {"app": "identity-db"})
        service = {"apiVersion": "v1", "kind": "Service", "metadata": {"name": "identity-db", "namespace": "database"},
                   "spec": {"clusterIP": "None", "selector": {"app": "identity-db"}, "ports": [{"port": 5432}]}}
        cls.apply([postgres, service])
        log("waiting for PostgreSQL")
        cls.kubectl("wait", "--for=condition=Ready", "pod/identity-db", "-n", "database", "--timeout=240s")
        cls.database_ip = cls.kubectl("get", "pod", "identity-db", "-n", "database", "-o", "jsonpath={.status.podIP}").stdout.strip()

    @classmethod
    def emulator_call(cls, script):
        """Run a boto3 script inside the emulator pod against the emulator itself; the script prints one JSON document last."""
        done = cls.kubectl("exec", "-i", "-n", EMULATOR_NAMESPACE, "moto", "--", "python", "-", stdin=EMULATOR_PRELUDE + script)
        return json.loads(done.stdout.strip().splitlines()[-1])

    @classmethod
    def emulator_calls(cls):
        """Count the API calls the emulator answered, from the launcher's one-line-per-call log."""
        counts = {}
        for line in cls.kubectl("logs", "-n", EMULATOR_NAMESPACE, "moto").stdout.splitlines():
            parts = line.split()
            if len(parts) == 4 and parts[0] == "API" and parts[3] == "200":
                counts[parts[1] + "." + parts[2]] = counts.get(parts[1] + "." + parts[2], 0) + 1
        return counts

    @classmethod
    def start_emulator(cls):
        security = {"allowPrivilegeEscalation": False, "readOnlyRootFilesystem": True, "capabilities": {"drop": ["ALL"]}}
        cls.kubectl("create", "namespace", EMULATOR_NAMESPACE)
        cls.kubectl("create", "configmap", "emulator-launcher", "-n", EMULATOR_NAMESPACE, f"--from-literal=launch.py={EMULATOR_LAUNCHER}")
        cls.apply([pod_document("moto", EMULATOR_NAMESPACE, {
            "nodeName": cls.servers[2], "automountServiceAccountToken": False, "terminationGracePeriodSeconds": 0,
            "securityContext": {"runAsNonRoot": True, "runAsUser": 1000, "runAsGroup": 1000, "seccompProfile": {"type": "RuntimeDefault"},
                                "sysctls": [{"name": "net.ipv4.ip_unprivileged_port_start", "value": "0"}]},
            "containers": [{
                "name": "moto", "image": EMULATOR_IMAGE, "imagePullPolicy": "IfNotPresent", "command": ["python", "/launcher/launch.py"],
                "env": [{"name": "MOTO_ACCOUNT_ID", "value": ACCOUNT}, {"name": "HOME", "value": "/tmp"}, {"name": "PYTHONDONTWRITEBYTECODE", "value": "1"}],
                "ports": [{"containerPort": EMULATOR_PORT}], "securityContext": security,
                "readinessProbe": {"tcpSocket": {"port": EMULATOR_PORT}, "periodSeconds": 2},
                "volumeMounts": [{"name": "launcher", "mountPath": "/launcher", "readOnly": True}, {"name": "tmp", "mountPath": "/tmp"}],
            }],
            "volumes": [{"name": "launcher", "configMap": {"name": "emulator-launcher"}}, {"name": "tmp", "emptyDir": {}}],
        }, {"app": "aws-emulator"})])
        log("waiting for the AWS emulator")
        cls.kubectl("wait", "--for=condition=Ready", "pod/moto", "-n", EMULATOR_NAMESPACE, "--timeout=300s", timeout=330)
        cls.emulator_ip = cls.kubectl("get", "pod", "moto", "-n", EMULATOR_NAMESPACE, "-o", "jsonpath={.status.podIP}").stdout.strip()
        root = cls.emulator_call(f"""
subject = {{"CommonName": "kind test private root CA"}}
arn = pca.create_certificate_authority(CertificateAuthorityConfiguration={{"KeyAlgorithm": "EC_prime256v1", "SigningAlgorithm": "SHA256WITHECDSA", "Subject": subject}}, CertificateAuthorityType="ROOT")["CertificateAuthorityArn"]
csr = pca.get_certificate_authority_csr(CertificateAuthorityArn=arn)["Csr"]
issued = pca.issue_certificate(CertificateAuthorityArn=arn, Csr=csr.encode(), SigningAlgorithm="SHA256WITHECDSA", TemplateArn="{ROOT_TEMPLATE}", Validity={{"Type": "DAYS", "Value": 3}})["CertificateArn"]
certificate = pca.get_certificate(CertificateAuthorityArn=arn, CertificateArn=issued)["Certificate"]
pca.import_certificate_authority_certificate(CertificateAuthorityArn=arn, Certificate=certificate.encode())
status = pca.describe_certificate_authority(CertificateAuthorityArn=arn)["CertificateAuthority"]["Status"]
print(json.dumps({{"arn": arn, "certificate": certificate, "status": status}}))
""")
        assert root["status"] == "ACTIVE", root["status"]
        cls.ca_arn, cls.ca_certificate = root["arn"], root["certificate"]
        Path(cls.work, "aws-ca.crt").write_text(cls.ca_certificate)
        cls.results["emulator"] = {"image": EMULATOR_IMAGE, "pod_ip": cls.emulator_ip, "private_ca": cls.ca_arn, "private_ca_status": root["status"]}

    @classmethod
    def deployment_config(cls):
        config = json.loads(Path(__file__).with_name("deployment.example.json").read_text())
        who = json.loads(cls.kubectl("auth", "whoami", "-o", "json").stdout)["status"]["userInfo"]
        cls.admin_groups = [group for group in who.get("groups", []) if group != "system:authenticated"]
        config.update(server_operator_groups=cls.admin_groups,

            trust_domain=TRUST_DOMAIN, other_environment_trust_domain="other." + TRUST_DOMAIN, cluster=cls.name,
            node_names=cls.agents, server_node_names=cls.servers, node_cidrs=[cls.node_ips[node] + "/32" for node in cls.agents],
            api_server_cidrs=[cls.api_endpoint + "/32"], api_server_port=cls.api_port, aws_endpoint_cidrs=[cls.emulator_ip + "/32"],
        )
        config["upstream_authority"]["certificate_authority_arn"] = cls.ca_arn
        config["database"].update(host=DATABASE_HOST, cidrs=[cls.database_ip + "/32"])
        image = IMAGES["agent"]
        first, second, third = cls.agents
        pilots = [("egress", "egress", "egress-supervisor", 10001, [first, second]), ("audit", "supervisor", "audit-supervisor", 10001, [first]),
                  ("uid-two", "supervisor", "egress-supervisor", 10002, [second]), ("short-lived", "supervisor", "ttl-supervisor", 10003, [third])]
        config["registrations"] = [{
            "spiffe_id": f"spiffe://{TRUST_DOMAIN}/platform/{path}", "tenant": "test-tenant", "project": "test-project", "role": role,
            "application_id": None, "namespace": "trusted-supervisors", "service_account": account, "container": "supervisor",
            "container_image": image, "uid": uid, "node_names": nodes,
        } for path, role, account, uid, nodes in pilots]
        return config

    @classmethod
    def render_deployment(cls):
        cls.config = cls.deployment_config()
        cls.files = render(cls.config, lambda image: [])
        server = json.loads(cls.files["server.conf"])
        cls.rendered_plugins = copy.deepcopy(server["plugins"])
        server["server"].update(agent_ttl="300s", default_x509_svid_ttl="300s")
        cls.files["server.conf"] = json.dumps(server, indent=2, sort_keys=True) + "\n"
        cls.manifests = json.loads(cls.files["manifests.json"])["items"]
        for item in cls.manifests:
            if item["kind"] == "ConfigMap" and item["metadata"]["name"] == "spire-server":
                item["data"]["server.conf"] = cls.files["server.conf"]
            if item["kind"] == "StatefulSet" and item["metadata"]["name"] == "spire-server":
                item["spec"]["template"]["spec"]["containers"][0]["env"] += [{"name": name, "value": f"http://{cls.emulator_ip}:{EMULATOR_PORT}"} for name in ["AWS_ENDPOINT_URL", "AWS_ENDPOINT_URL_STS"]]
        cls.registrations = json.loads(cls.files["registrations.json"])["entries"]
        for entry in cls.registrations:
            if entry["spiffe_id"].endswith("/platform/short-lived"):
                entry["x509_svid_ttl"] = SHORT_TTL
        Path(cls.work, "registrations.json").write_text(json.dumps({"entries": cls.registrations}))
        cls.patches = {}
        for item in json.loads(cls.files["supervisor-patches.json"]):
            cls.patches[item["service_account"] + "/" + str(item["pod_spec_patch"]["containers"][0]["securityContext"]["runAsUser"])] = item

    @classmethod
    def apply_deployment(cls):
        namespaces = [item for item in cls.manifests if item["kind"] == "Namespace"]
        workloads = {"StatefulSet", "DaemonSet"}
        rest = [item for item in cls.manifests if item["kind"] != "Namespace" and item["kind"] not in workloads]
        cls.apply(namespaces + [{"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": cls.config["monitoring"]["namespace"]}}])
        server_namespace, agent_namespace = cls.config["namespace"], cls.config["agent_namespace"]
        kubelet_ca = "".join(run(["docker", "exec", node, "cat", "/var/lib/kubelet/pki/kubelet.crt"]).stdout for node in cls.agents)
        Path(cls.work, "kubelet-ca.crt").write_text(kubelet_ca)
        cls.kubectl("create", "configmap", cls.config["database"]["root_ca_configmap"], "-n", server_namespace, f"--from-file=ca.crt={cls.work}/database-ca.crt")
        cls.kubectl("create", "secret", "generic", cls.config["database"]["client_secret"], "-n", server_namespace,
                    f"--from-file=tls.crt={cls.work}/database-client.crt", f"--from-file=tls.key={cls.work}/database-client.key")
        cls.kubectl("create", "configmap", cls.config["bootstrap_bundle_configmap"], "-n", agent_namespace, f"--from-file=bundle.pem={cls.work}/aws-ca.crt")
        cls.kubectl("create", "configmap", cls.config["kubelet_ca_configmap"], "-n", agent_namespace, f"--from-file=ca.crt={cls.work}/kubelet-ca.crt")
        log("applying the rendered manifests and the migration job")
        cls.apply(rest)
        job = next(item["metadata"]["name"] for item in rest if item["kind"] == "Job")
        cls.kubectl("wait", "--for=condition=complete", f"job/{job}", "-n", server_namespace, "--timeout=420s", timeout=450)
        cls.results["migration_job"] = job + " completed"
        cls.apply([item for item in cls.manifests if item["kind"] in workloads])
        log("waiting for the SPIRE servers")
        cls.kubectl("rollout", "status", "statefulset/spire-server", "-n", server_namespace, "--timeout=420s", timeout=450)
        log("waiting for the SPIRE agents")
        cls.kubectl("rollout", "status", "daemonset/spire-agent", "-n", agent_namespace, "--timeout=300s", timeout=330)

    @classmethod
    def server_command(cls):
        return shlex.join(["kubectl", "--kubeconfig", cls.kubeconfig, "exec", "-i", "-n", cls.config["namespace"], "spire-server-0", "-c", "server", "--", SERVER_BINARY])

    def server_call(self, *arguments, pod="spire-server-0", stdin=None, check=True):
        return self.kubectl("exec", "-i", "-n", self.config["namespace"], pod, "-c", "server", "--", SERVER_BINARY, *arguments,
                            "-socketPath", SERVER_SOCKET, stdin=stdin, check=check)

    def listing(self):
        return json.loads(self.server_call("entry", "show", "-output", "json").stdout).get("entries", [])

    def bundles(self):
        return [item["trust_domain"] for item in json.loads(self.server_call("bundle", "list", "-output", "json").stdout).get("bundles", [])]

    def reconcile(self, entries, *extra, expect=0):
        path = Path(self.work, "desired.json")
        path.write_text(json.dumps({"entries": entries}))
        approval = [] if {"--dry-run", "--check"} & set(extra) else ["--approved-digest", hashlib.sha256(path.read_bytes()).hexdigest()]
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            try:
                status = reconcile_main(["--registrations", str(path), "--server-command", self.server_command(), "--socket-path", SERVER_SOCKET, *approval, *extra])
            except SystemExit as raised:
                status = raised.code
        self.assertEqual(status, expect)
        return json.JSONDecoder().raw_decode(out.getvalue())[0]

    def reconcile_refused(self, entries, *extra):
        """Run reconcile.py and return (exit status, stderr), for runs that refuse and print no plan."""
        path = Path(self.work, "desired.json")
        path.write_text(json.dumps({"entries": entries}))
        err = io.StringIO()
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(err):
            try:
                status = reconcile_main(["--registrations", str(path), "--server-command", self.server_command(), "--socket-path", SERVER_SOCKET, *extra])
            except SystemExit as raised:
                status = raised.code
        return status, err.getvalue()

    def server_logs(self, pod="spire-server-0", since="30m"):
        return self.kubectl("logs", "-n", self.config["namespace"], pod, "-c", "server", f"--since={since}").stdout

    def pilot_pod(self, key, name, node=None):
        spec = copy.deepcopy(self.patches[key]["pod_spec_patch"])
        if node:
            del spec["affinity"]
            spec["nodeName"] = node
        spec["containers"][0].update(command=[AGENT_BINARY], args=["api", "watch", "-socketPath", SOCKET_PATH])
        return pod_document(name, self.patches[key]["namespace"], spec)

    def fetch(self, pod, kind="x509"):
        started = datetime.now(timezone.utc)
        extra = ["-audience", "kind-test"] if kind == "jwt" else []
        done = run(["kubectl", "--kubeconfig", self.kubeconfig, "exec", "-n", "trusted-supervisors", pod, "-c", "supervisor", "--", AGENT_BINARY,
                    "api", "fetch", kind, "-socketPath", SOCKET_PATH, *extra], check=False, timeout=60)
        identity = re.search(r"SPIFFE ID:\s+(\S+)", done.stdout)
        until = re.search(r"SVID Valid Until:\s+(.+)", done.stdout)
        return {"ok": done.returncode == 0 and (kind == "jwt" or identity is not None), "id": identity and identity.group(1), "started": started,
                "until": parse_time(until.group(1)) if until else None, "error": done.stderr.strip()[-300:]}

    def scrape(self, selector, namespace, port):
        pods = self.kubectl("get", "pods", "-n", namespace, "-l", selector, "-o", r"jsonpath={range .items[*]}{.status.podIP}{'\n'}{end}").stdout.split()
        return "".join(self.kubectl("exec", "-n", "monitoring", "scraper", "--", "curl", "-s", "--max-time", "5", f"http://{ip}:{port}/metrics").stdout for ip in pods)

    def spiffe(self, path):
        return f"spiffe://{TRUST_DOMAIN}/platform/{path}"

    def test_1_rendered_manifests_are_admitted_and_agents_attest(self):
        self.assertEqual(self.kubectl("get", "statefulset", "spire-server", "-n", self.config["namespace"], "-o", "jsonpath={.status.readyReplicas}").stdout, "3")
        ready = self.kubectl("get", "daemonset", "spire-agent", "-n", self.config["agent_namespace"], "-o", "jsonpath={.status.numberReady}/{.status.desiredNumberScheduled}").stdout
        self.assertEqual(ready, "3/3")
        agents = json.loads(self.server_call("agent", "list", "-output", "json").stdout)["agents"]
        self.assertEqual(len(agents), 3)
        self.assertTrue(all(agent["id"]["path"].startswith(f"/spire/agent/k8s_psat/{self.name}/") for agent in agents))
        for kind in ["validatingadmissionpolicy", "validatingadmissionpolicybinding"]:
            self.assertEqual(len(self.kubectl("get", kind, "-o", "name").stdout.split()), 3)
        typing = self.kubectl("get", "validatingadmissionpolicy", "-o", r"jsonpath={.items[*].status.typeChecking}").stdout.strip()
        self.results["admission_type_checking"] = typing[:300] or "no warnings"
        self.assertEqual(len(self.kubectl("get", "networkpolicy", "-A", "-o", "name").stdout.split()), 5)
        self.assertEqual(self.kubectl("get", "job", "-n", self.config["namespace"], "-o", "jsonpath={.items[*].status.succeeded}").stdout, "1")
        migrated = self.kubectl("get", "pods", "-n", self.config["namespace"], "-l", "app.kubernetes.io/name=spire-server-migrate", "-o", "jsonpath={.items[0].spec.nodeName}").stdout
        self.assertIn(migrated, self.servers)
        self.kubectl("get", "configmap", "spire-alert-rules", "-n", self.config["monitoring"]["namespace"])
        self.results["agents_attested_k8s_psat"] = len(agents)

    def pem(self, encoded):
        return ssl.DER_cert_to_PEM_cert(base64.b64decode(encoded))

    def test_1b_aws_plugins_ran_as_rendered_against_the_emulator(self):
        namespace = self.config["namespace"]
        mounted = json.loads(self.kubectl("get", "configmap", "spire-server", "-n", namespace, "-o", "json").stdout)["data"]["server.conf"]
        running = json.loads(mounted)
        spec = json.loads(self.kubectl("get", "statefulset", "spire-server", "-n", namespace, "-o", "json").stdout)["spec"]["template"]["spec"]
        self.assertIn("/run/spire/config/server.conf", spec["containers"][0]["args"], "the server reads the mounted ConfigMap, which carries the rendered plugin blocks")
        self.assertEqual(running["plugins"], self.rendered_plugins, "the running server config must carry the rendered plugin blocks")
        self.assertIn("aws_kms", running["plugins"]["KeyManager"][0])
        self.assertIn("aws_pca", running["plugins"]["UpstreamAuthority"][0])
        self.assertEqual(running["plugins"]["UpstreamAuthority"][0]["aws_pca"]["plugin_data"]["certificate_authority_arn"], self.ca_arn)
        env = self.kubectl("get", "statefulset", "spire-server", "-n", namespace, "-o", "json").stdout
        rendered_env = next(item for item in json.loads(self.files["manifests.json"])["items"] if item["kind"] == "StatefulSet")["spec"]["template"]["spec"]["containers"][0]["env"]
        added = {entry["name"] for entry in json.loads(env)["spec"]["template"]["spec"]["containers"][0]["env"]} - {entry["name"] for entry in rendered_env}
        self.assertEqual(added, {"AWS_ENDPOINT_URL", "AWS_ENDPOINT_URL_STS"})
        for pod in [f"spire-server-{index}" for index in range(3)]:
            log_text = self.kubectl("logs", "-n", namespace, pod, "-c", "server").stdout
            for plugin in ["aws_kms", "aws_pca"]:
                self.assertRegex(log_text, rf'"msg":"Plugin loaded","plugin_name":"{plugin}"', f"{pod} did not load {plugin}")
            self.assertIn('"msg":"X509 CA activated"', log_text)

        state = self.emulator_call("""
aliases = kms.list_aliases()["Aliases"]
keys = {}
for entry in aliases:
    try:
        meta = kms.describe_key(KeyId=entry["TargetKeyId"])["KeyMetadata"]
    except Exception:
        continue
    tags = {tag["TagKey"]: tag["TagValue"] for tag in kms.list_resource_tags(KeyId=entry["TargetKeyId"])["Tags"]}
    keys[entry["AliasName"]] = {"state": meta["KeyState"], "spec": meta.get("KeySpec"), "usage": meta["KeyUsage"], "tags": sorted(tags)}
print(json.dumps({"keys": keys}))
""")
        keys = state["keys"]
        for index in range(3):
            for key_id in ["x509-CA-A", "JWT-Signer-A"]:
                alias = f"alias/SPIRE_SERVER/kind_spire-test_example_org/epoch-1-spire-server-{index}/{key_id}"
                self.assertIn(alias, keys)
                self.assertEqual((keys[alias]["state"], keys[alias]["spec"], keys[alias]["usage"]), ("Enabled", "ECC_NIST_P256", "SIGN_VERIFY"))
                self.assertIn("spire-server-id", keys[alias]["tags"])
                self.assertIn("Environment", keys[alias]["tags"])
        calls = self.emulator_calls()
        for call in ["kms.CreateKey", "kms.CreateAlias", "kms.Sign", "kms.GetPublicKey", "acmpca.IssueCertificate", "acmpca.GetCertificate", "sts.AssumeRoleWithWebIdentity"]:
            self.assertGreater(calls.get(call, 0), 0, call)
        self.assertGreaterEqual(calls["acmpca.IssueCertificate"], 4, "the root plus one server CA per server")
        self.assertGreaterEqual(calls["sts.AssumeRoleWithWebIdentity"], 3)

        issued = set()
        for pod in [f"spire-server-{index}" for index in range(3)]:
            issued.update(re.findall(r'"certificate_arn":"([^"]+)"[^\n]*"msg":"Certificate issued"', self.kubectl("logs", "-n", namespace, pod, "-c", "server").stdout))
        self.assertGreaterEqual(len(issued), 3)
        fetched = self.emulator_call("""
import sys
arns = json.loads(%r)
ca = %r
print(json.dumps({arn: pca.get_certificate(CertificateAuthorityArn=ca, CertificateArn=arn)["Certificate"] for arn in arns}))
""" % (json.dumps(sorted(issued)), self.ca_arn))
        for arn, certificate in fetched.items():
            path = Path(self.work, "issued.crt")
            path.write_text(certificate)
            run(["openssl", "verify", "-CAfile", f"{self.work}/aws-ca.crt", str(path)])

        minted = json.loads(self.server_call("x509", "mint", "-spiffeID", self.spiffe("aws-chain-probe"), "-output", "json").stdout)
        leaf_pem, server_ca_pem = (self.pem(item) for item in minted["x509_svid"])
        Path(self.work, "probe-leaf.pem").write_text(leaf_pem)
        Path(self.work, "probe-ca.pem").write_text(server_ca_pem)
        run(["openssl", "verify", "-CAfile", f"{self.work}/aws-ca.crt", "-untrusted", f"{self.work}/probe-ca.pem", f"{self.work}/probe-leaf.pem"])
        self.assertEqual([self.pem(item).strip() for item in minted["root_cas"]], [self.ca_certificate.strip()])
        bundle = self.server_call("bundle", "show").stdout
        self.assertEqual(bundle.strip(), self.ca_certificate.strip(), "the bundle workloads receive must be the emulator CA certificate only")
        self.assertEqual(Path(self.work, "aws-ca.crt").read_text().strip(), bundle.strip())
        self.assertIn(run(["openssl", "x509", "-noout", "-issuer"], stdin=server_ca_pem).stdout.split("=", 1)[1].strip().replace(" ", ""),
                      run(["openssl", "x509", "-noout", "-subject"], stdin=self.ca_certificate).stdout.split("=", 1)[1].strip().replace(" ", ""))
        self.results["aws_plugins"] = {"running_config_plugins_equal_rendered": True, "kms_keys": len(keys), "server_certificates_issued_by_the_emulator_ca": len(issued),
                                       "emulator_calls_after_startup": {call: calls[call] for call in sorted(calls)},
                                       "authority_chain": "probe leaf -> server X.509 CA (issued by the emulator CA through aws_pca) -> emulator CA; bundle show and x509 mint root_cas equal the emulator CA certificate"}

    def test_2_reconcile_dry_run_check_upgrade_apply_and_rerun(self):
        self.assertEqual(self.bundles(), [MIGRATION_TRUST_DOMAIN])
        plan = self.reconcile(self.registrations, "--dry-run")
        self.assertEqual(len(plan["create"]["entries"]), 8)
        self.assertEqual((plan["bundles_inspected"], plan["undeclared_bundles"]), (True, [f"spiffe://{MIGRATION_TRUST_DOMAIN}"]))
        self.assertEqual(self.listing(), [])
        self.assertEqual(self.bundles(), [MIGRATION_TRUST_DOMAIN])
        self.reconcile(self.registrations, "--check", expect=2)
        aliases = [entry for entry in self.registrations if entry["parent_id"].endswith("/spire/server")]
        self.assertEqual(len(aliases), 3)
        legacy_id = aliases[0]["spiffe_id"].rsplit("/node/", 1)[0]
        legacy_alias = {**aliases[0], "spiffe_id": legacy_id, "selectors": [item for item in aliases[0]["selectors"] if not item["value"].startswith("agent_node_name:")]}
        egress = next(entry for entry in self.registrations if entry["spiffe_id"].endswith("/platform/egress"))
        legacy = [legacy_alias, {**egress, "parent_id": legacy_id}]
        self.server_call("entry", "create", "-data", "-", stdin=json.dumps({"entries": legacy}))
        self.assertEqual(len(self.listing()), 2)
        upgrade = self.reconcile(self.registrations, "--dry-run")
        self.assertEqual((len(upgrade["delete"]), len(upgrade["create"]["entries"])), (2, 8))
        self.reconcile(self.registrations)
        self.assertEqual(len(self.listing()), 8)
        self.assertEqual(self.bundles(), [])
        self.assertEqual(self.reconcile(self.registrations)["steps"], [])
        self.reconcile(self.registrations, "--check")
        stand_in = self.server_call("bundle", "show").stdout
        self.server_call("bundle", "set", "-id", f"spiffe://{MIGRATION_TRUST_DOMAIN}", stdin=stand_in)
        self.assertEqual(self.reconcile(self.registrations, "--check", expect=2)["steps"], [f"delete bundle spiffe://{MIGRATION_TRUST_DOMAIN}"])
        self.reconcile(self.registrations)
        self.assertEqual(self.bundles(), [])
        self.reconcile(self.registrations, "--check")
        entries = {(entry["spiffe_id"]["path"], entry["parent_id"]["path"]): entry for entry in self.listing()}
        self.assertEqual(len({path for path, _ in entries if path == "/platform/egress"}), 1)
        self.assertEqual(len([key for key in entries if key[0] == "/platform/egress"]), 2)
        short = [entry for key, entry in entries.items() if key[0] == "/platform/short-lived"]
        self.assertEqual([entry["x509_svid_ttl"] for entry in short], [SHORT_TTL])
        parent = f"spiffe://{TRUST_DOMAIN}/spire/server"
        self.server_call("entry", "create", "-parentID", parent, "-spiffeID", self.spiffe("broad"), "-selector", "unix:uid:1")
        broad = self.reconcile(self.registrations, "--check", expect=2)
        self.assertEqual([item["spiffe_id"] for item in broad["broad_selectors"]], [self.spiffe("broad")])
        self.server_call("entry", "delete", "-entryID", next(item["id"] for item in self.listing() if item["spiffe_id"]["path"] == "/platform/broad"))
        self.results["reconcile"] = "dry-run planned 8 creates; --check exited 2 on drift and on a single-selector entry, 0 once converged; upgrade from the single alias planned 2 deletes and 8 creates and converged to 8 entries; rerun planned 0 steps; the migration Job left a bundle for migration.invalid on the real server, --check exited 2 on it alone, the first apply deleted it, and bundle list and --check were clean afterwards"

    def test_3_pilot_pods_receive_only_their_own_identity_on_their_allowed_nodes(self):
        first, second, third = self.agents
        pilots = {"pilot-egress": ("egress-supervisor/10001", "egress", first), "pilot-egress-b": ("egress-supervisor/10001", "egress", second),
                  "pilot-audit": ("audit-supervisor/10001", "audit", None), "pilot-uid-two": ("egress-supervisor/10002", "uid-two", None),
                  "pilot-short": ("ttl-supervisor/10003", "short-lived", None)}
        outsiders = {"pilot-egress-denied": ("egress-supervisor/10001", third), "pilot-audit-denied": ("audit-supervisor/10001", second)}
        documents = [self.pilot_pod(key, name, node) for name, (key, _, node) in pilots.items()]
        documents += [self.pilot_pod(key, name, node) for name, (key, node) in outsiders.items()]
        self.apply(documents)
        for name in [*pilots, *outsiders]:
            self.kubectl("wait", "--for=condition=Ready", f"pod/{name}", "-n", "trusted-supervisors", "--timeout=240s")
        placed = {name: self.kubectl("get", "pod", name, "-n", "trusted-supervisors", "-o", "jsonpath={.spec.nodeName}").stdout for name in [*pilots, *outsiders]}
        self.assertEqual(placed["pilot-audit"], first)
        self.assertEqual(placed["pilot-uid-two"], second)
        self.assertEqual(placed["pilot-short"], third)
        for name, (_, path, _) in pilots.items():
            result = wait_for(lambda: self.fetch(name)["ok"] and self.fetch(name), 90, f"{name} to receive an identity")
            self.assertEqual(result["id"], self.spiffe(path))
            self.assertGreater(result["until"], result["started"])
        denied = []
        for name in outsiders:
            for _ in range(5):
                result = self.fetch(name)
                self.assertFalse(result["ok"], f"{name} received an identity on a node its registration does not allow")
                denied.append(result["error"])
                time.sleep(3)
        self.assertTrue(all(re.search("PermissionDenied|no identity", error) for error in denied), denied)
        self.assertTrue(self.fetch("pilot-egress", "jwt")["ok"])
        self.assertFalse(self.fetch("pilot-egress-denied", "jwt")["ok"])
        without_audit = [entry for entry in self.registrations if not entry["spiffe_id"].endswith("/platform/audit")]
        self.reconcile(without_audit)
        denied_audit = wait_for(lambda: not (result := self.fetch("pilot-audit"))["ok"] and result, 60, "the pod without a registration to lose its identity")
        self.assertRegex(denied_audit["error"], "PermissionDenied|no identity")
        self.assertEqual(self.fetch("pilot-egress")["id"], self.spiffe("egress"))
        self.reconcile(self.registrations)
        wait_for(lambda: self.fetch("pilot-audit")["ok"], 60, "the restored registration to serve the pod again")
        self.results["node_scope"] = {
            "allowed": {"pilot-egress": first, "pilot-egress-b": second, "pilot-audit": first, "pilot-uid-two": second, "pilot-short": third},
            "denied_same_pod_spec": {"pilot-egress-denied": third, "pilot-audit-denied": second}, "denied_attempts": len(denied), "denied_error": denied[0][:160],
        }
        self.results["pilots"] = "5 pods on allowed nodes received exactly their own SPIFFE ID; the same specs on a disallowed node received PermissionDenied; the pod whose registration was retired received PermissionDenied"

    def test_4_every_alert_series_exists_after_its_event(self):
        namespace = self.config["monitoring"]["namespace"]
        self.apply([curl_pod("scraper", namespace, self.servers[1]), curl_pod("outsider", "default", self.servers[1])])
        for name, where in [("scraper", namespace), ("outsider", "default")]:
            self.kubectl("wait", "--for=condition=Ready", f"pod/{name}", "-n", where, "--timeout=240s")
        parent = f"spiffe://{TRUST_DOMAIN}/spire/server"
        scratch = f"spiffe://{TRUST_DOMAIN}/scratch/series"
        self.server_call("entry", "create", "-parentID", parent, "-spiffeID", scratch, "-selector", "unix:uid:1")
        entry = next(item for item in self.listing() if item["spiffe_id"]["path"] == "/scratch/series")["id"]
        self.server_call("entry", "update", "-entryID", entry, "-parentID", parent, "-spiffeID", scratch, "-selector", "unix:uid:1", "-x509SVIDTTL", "600")
        self.server_call("entry", "delete", "-entryID", entry)
        servers = lambda: self.scrape("app.kubernetes.io/name=spire-server", self.config["namespace"], SERVER_METRICS_PORT)
        entry_counter = "spire_server_rpc_entry_v1_entry_batch_create_entry"
        before = counter_total(servers(), entry_counter)
        out_of_band = {"admin": self.spiffe("scratch/oob-admin"), "downstream": self.spiffe("scratch/oob-downstream")}
        for flag, spiffe_id in out_of_band.items():
            self.server_call("entry", "create", "-parentID", parent, "-spiffeID", spiffe_id, "-selector", "unix:uid:2", "-" + flag)
        privileged = {item["spiffe_id"]["path"]: item for item in self.listing() if item["spiffe_id"]["path"].startswith("/platform/scratch/oob-")}
        self.assertEqual({path: (item["admin"], item["downstream"]) for path, item in privileged.items()},
                         {"/platform/scratch/oob-admin": (True, False), "/platform/scratch/oob-downstream": (False, True)})
        wait_for(lambda: counter_total(servers(), entry_counter) >= before + 2, 60, "the entry RPC counter to move for the privileged entries", interval=5)
        audited = {item["spiffe_id"]: item for item in audit_lines(self.server_logs()) if item.get("method") == "BatchCreateEntry" and item.get("spiffe_id", "").startswith(self.spiffe("scratch/oob-"))}
        for flag, spiffe_id in out_of_band.items():
            line = audited[spiffe_id]
            self.assertEqual((line["type"], line["msg"], line["status"], line["service"], line["authorized_as"], line[flag]), ("audit", "API accessed", "success", "entry.v1.Entry", "local", True))
            self.assertEqual(line["admin"] if flag == "downstream" else line["downstream"], False)
        status, message = self.reconcile_refused(self.registrations, "--check")
        self.assertEqual(status, 3)
        self.assertIn("privileged grant present", message)
        for path, item in privileged.items():
            self.assertIn(f"{item['id']} spiffe://{TRUST_DOMAIN}{path} ({'admin' if item['admin'] else 'downstream'})", message)
        self.results["privileged_check"] = {"exit": status, "stderr": message.strip()[:300]}
        self.results["privileged_audit_line"] = json.dumps(audited[out_of_band["admin"]], sort_keys=True)[:300]
        for item in privileged.values():
            self.server_call("entry", "delete", "-entryID", item["id"])
        self.reconcile(self.registrations, "--check")
        token = self.server_call("token", "generate", "-ttl", "1").stdout
        self.assertTrue(re.search(r"^Token: \S{16,}\s*$", token, re.M), "token generate printed no token line (shape: %s)" % re.sub(r"[0-9a-zA-Z]", "x", token)[:80])
        del token
        federation = ["-trustDomain", "scratch.example.org", "-bundleEndpointURL", "https://scratch.example.org/bundle", "-bundleEndpointProfile", "https_web"]
        for verb in ["create", "update"]:
            self.server_call("federation", verb, *federation, check=False)
        scratch_lines = lambda: [json.loads(line) for pod in ["spire-server-0", "spire-server-1", "spire-server-2"] for line in self.server_logs(pod).splitlines()
                                 if '"subsystem_name":"bundle_client"' in line and '"trust_domain":"scratch.example.org"' in line]
        wait_for(lambda: any(item["msg"] == "Error updating bundle" for item in scratch_lines()), 150, "the server to log the bundle fetch attempt for the scratch trust domain", interval=10)
        fetch = [item for item in scratch_lines() if item["msg"] in {"Trust domain is now managed", "Error updating bundle"}]
        self.assertEqual({item["msg"] for item in fetch}, {"Trust domain is now managed", "Error updating bundle"})
        self.assertEqual({item["level"] for item in fetch}, {"info", "error"})
        self.assertIn("https://scratch.example.org/bundle", json.dumps(fetch))
        self.results["bundle_fetch_log"] = [json.dumps(item, sort_keys=True)[:300] for item in fetch[:2]]
        self.server_call("federation", "delete", "-id", "scratch.example.org", check=False)
        self.server_call("agent", "ban", "-spiffeID", f"{parent.removesuffix('/server')}/agent/k8s_psat/{self.name}/absent", check=False)
        self.assertTrue(self.fetch("pilot-egress", "jwt")["ok"])
        self.assertTrue(self.fetch("pilot-egress")["ok"])
        expected = alert_series(json.loads(self.files["alert-rules.json"]))
        observed = {}

        def collect():
            observed["server"] = series(self.scrape("app.kubernetes.io/name=spire-server", self.config["namespace"], SERVER_METRICS_PORT))
            observed["agent"] = series(self.scrape("app.kubernetes.io/name=spire-agent", self.config["agent_namespace"], AGENT_METRICS_PORT))
            return not (expected - observed["server"] - observed["agent"])

        try:
            wait_for(collect, 360, "every alert series", interval=15)
        except AssertionError:
            pass
        found = observed["server"] | observed["agent"]
        labels = labelled_series(self.scrape("app.kubernetes.io/name=spire-server", self.config["namespace"], SERVER_METRICS_PORT)
                                 + self.scrape("app.kubernetes.io/name=spire-agent", self.config["agent_namespace"], AGENT_METRICS_PORT))
        self.results["alert_series_labels"] = {name: sorted(sorted(label) for label in labels.get(name, [])) for name in sorted(expected)}
        missing = sorted(expected - found)
        self.results["alert_series_found"] = sorted(expected & found)
        self.results["alert_series_missing"] = missing
        self.results["agent_series_related_to_alerts"] = sorted(name for name in observed["agent"] if re.search(r"fetch|rotate|svid|attestation", name) and "elapsed" not in name)
        self.results["server_series_related_to_alerts"] = sorted(name for name in observed["server"] if re.search(r"entry|rotate|svid|ban|federation|attest_agent", name) and "elapsed" not in name)
        self.results["agent_series_all"] = sorted(observed["agent"])
        self.assertEqual(missing, [])
        outsider = self.kubectl("exec", "-n", "default", "outsider", "--", "curl", "-s", "--max-time", "5", "http://" + self.kubectl(
            "get", "pods", "-n", self.config["namespace"], "-l", "app.kubernetes.io/name=spire-server", "-o", "jsonpath={.items[0].status.podIP}").stdout + f":{SERVER_METRICS_PORT}/metrics", check=False)
        self.assertNotEqual(outsider.returncode, 0)
        self.results["scrape_outside_the_monitoring_namespace"] = f"blocked (curl exit {outsider.returncode})"

    def test_5_rotation_under_load_and_during_a_server_outage(self):
        window, workers = 200, 4
        until_values, failures, expired, total = set(), [], [], [0]
        lock = threading.Lock()
        end = time.monotonic() + window

        def loop():
            while time.monotonic() < end:
                result = self.fetch("pilot-short")
                with lock:
                    total[0] += 1
                    if not result["ok"]:
                        failures.append(result["error"])
                    else:
                        until_values.add(result["until"])
                        if result["until"] <= result["started"]:
                            expired.append(result["until"])

        with concurrent.futures.ThreadPoolExecutor(workers) as pool:
            for future in [pool.submit(loop) for _ in range(workers)]:
                future.result()
        self.results["rotation_under_load"] = {"seconds": window, "concurrent_loops": workers, "fetches": total[0], "failures": len(failures), "distinct_svids": len(until_values)}
        self.assertEqual(failures, [])
        self.assertEqual(expired, [])
        self.assertGreaterEqual(len(until_values), 3)
        agent_pods = lambda: self.kubectl("get", "pods", "-n", self.config["agent_namespace"], "-o", r"jsonpath={range .items[*]}{.metadata.uid}{'='}{.status.containerStatuses[0].restartCount}{' '}{end}").stdout
        before = agent_pods()
        previous = max(until_values)
        current = wait_for(lambda: (result := self.fetch("pilot-short"))["ok"] and result["until"] > previous and result, 120, "a fresh rotation", interval=2)
        self.kubectl("scale", "statefulset/spire-server", "-n", self.config["namespace"], "--replicas=0")
        self.kubectl("wait", "--for=delete", "pod", "-l", "app.kubernetes.io/name=spire-server", "-n", self.config["namespace"], "--timeout=180s")
        down = time.monotonic()
        expiry = current["until"]
        served, refused, accepted_expired = [], [], []
        while datetime.now(timezone.utc) < expiry.replace(microsecond=0) + timedelta(seconds=20):
            result = self.fetch("pilot-short")
            if result["ok"]:
                served.append(result["until"])
                if result["until"] <= result["started"]:
                    accepted_expired.append(result["until"])
            else:
                refused.append(result["started"])
        last_served = max(served, default=None)
        self.results["outage"] = {"seconds_down_before_expiry_loop_ended": round(time.monotonic() - down), "svid_valid_until": expiry.isoformat(),
                                  "fetches_served_from_cache": len(served), "fetches_refused": len(refused),
                                  "all_served_had_the_same_svid": set(served) == {expiry}, "refused_all_after_expiry": all(started >= expiry - timedelta(seconds=2) for started in refused)}
        self.assertTrue(served)
        self.assertEqual(set(served), {expiry})
        self.assertEqual(accepted_expired, [])
        # A 1.16.0 agent keeps sending the cached SVID after it expires; the refusal is the fetch client verifying the chain.
        self.assertTrue(refused, "the fetch client still accepted the SVID after it expired")
        self.assertTrue(all(started >= expiry - timedelta(seconds=2) for started in refused), "a fetch failed before the SVID expired")
        self.assertEqual(agent_pods(), before)
        probe = {}
        deadline = time.monotonic() + OUTAGE_PROBE_SECONDS
        while time.monotonic() < deadline and not any(dict(label).get("status", "OK") != "OK" for label in probe.get("spire_agent_node_attestor_new_svid", [])):
            try:
                probe = labelled_series(self.scrape("app.kubernetes.io/name=spire-agent", self.config["agent_namespace"], AGENT_METRICS_PORT))
            except AssertionError:
                pass  # an agent restarting during the outage refuses the scrape; try again
            time.sleep(10)
        self.results["outage_agent_attestor_labels"] = {name: sorted(sorted(label) for label in probe.get(name, [])) for name in ["spire_agent_node_attestor_new_svid", "spire_agent_agent_svid_rotate"]}
        self.kubectl("scale", "statefulset/spire-server", "-n", self.config["namespace"], "--replicas=3")
        self.kubectl("rollout", "status", "statefulset/spire-server", "-n", self.config["namespace"], "--timeout=300s", timeout=330)
        restored = time.monotonic()
        resumed = wait_for(lambda: (result := self.fetch("pilot-short"))["ok"] and result["until"] > expiry and result, 180, "rotation to resume", interval=2)
        self.assertGreater(resumed["until"], datetime.now(timezone.utc))
        last_state = self.kubectl("get", "pods", "-n", self.config["agent_namespace"], "-o", r"jsonpath={range .items[*]}{.metadata.name}{'='}{.status.containerStatuses[0].restartCount}{':'}{.status.containerStatuses[0].lastState.terminated.reason}{' '}{end}").stdout
        self.results["outage"].update(seconds_from_server_ready_to_new_svid=round(time.monotonic() - restored), agent_restarts_before_the_probe_extension="none",
                                      agent_restarts_after_the_probe_extension=last_state.strip(), last_served=last_served.isoformat())

    def test_6_admission_policies_refuse_violating_pods(self):
        agent = next(item for item in self.manifests if item["kind"] == "DaemonSet")["spec"]["template"]
        probe = pod_document("agent-probe", self.config["agent_namespace"], copy.deepcopy(agent["spec"]), agent["metadata"]["labels"])
        dry_run = lambda document: self.kubectl("create", "--dry-run=server", "-f", "-", stdin=json.dumps(document), check=False)
        self.assertEqual(dry_run(probe).returncode, 0, "the exact agent profile must be admitted")
        terms = probe["spec"]["affinity"]["nodeAffinity"]["requiredDuringSchedulingIgnoredDuringExecution"]["nodeSelectorTerms"]
        self.assertEqual([term["matchFields"][0]["values"] for term in terms], [[node] for node in self.agents])
        terms.append({"matchFields": [{"key": "metadata.name", "operator": "In", "values": self.servers[:1]}]})
        widened = dry_run(probe)
        self.assertNotEqual(widened.returncode, 0)
        self.assertIn("only the exact SPIRE agent profile has host privileges", widened.stderr)
        terms.pop()
        probe["spec"]["containers"][0]["ports"][0]["hostPort"] = AGENT_METRICS_PORT
        refused = dry_run(probe)
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("only the exact SPIRE agent profile has host privileges", refused.stderr)
        self.kubectl("create", "serviceaccount", "intruder", "-n", "trusted-supervisors")
        intruder = self.pilot_pod("egress-supervisor/10001", "intruder")
        intruder["spec"]["serviceAccountName"] = "intruder"
        socket = dry_run(intruder)
        self.assertNotEqual(socket.returncode, 0)
        self.assertIn("Host volumes are denied globally", socket.stderr)
        host_network = pod_document("host-network", "default", {"hostNetwork": True, "containers": [{"name": "c", "image": CURL_IMAGE}]})
        escaped = dry_run(host_network)
        self.assertNotEqual(escaped.returncode, 0)
        self.assertIn("Pods require nonroot isolated processes", escaped.stderr)
        self.results["admission"] = "an extra affinity term for a server node refused; " + "exact agent pod admitted; hostPort on the agent, socket mount for another service account and hostNetwork refused by the generated policies"

    def test_6b_admission_ceiling_of_256_nodes(self):
        original = [item for item in self.manifests if item["kind"].startswith("ValidatingAdmissionPolicy")]
        template = copy.deepcopy(self.config)

        def render_for(count):
            config = copy.deepcopy(template)
            extra = [f"ceiling-agent-{index:03d}" for index in range(count - len(self.agents))]
            config.update(node_names=self.agents + extra, node_cidrs=template["node_cidrs"] + ["10.200.0.0/16"])
            return json.loads(render(config, lambda image: [])["manifests.json"])["items"]

        def attempt(count):
            items = render_for(count)
            policies = [item for item in items if item["kind"].startswith("ValidatingAdmissionPolicy")]
            workloads = [item for item in items if item["kind"] in {"StatefulSet", "DaemonSet", "Job"}]
            self.assertEqual({item["kind"] for item in workloads}, {"StatefulSet", "DaemonSet", "Job"})
            applied = self.kubectl("apply", "-f", "-", stdin=json.dumps({"apiVersion": "v1", "kind": "List", "items": policies}), check=False)
            if applied.returncode:
                return {"stage": "policy apply", "error": applied.stderr.strip()[-600:]}
            errors = {}
            documents = []
            for item in workloads:
                documents.append((item["kind"], item))
                template_spec = item["spec"]["template"]
                pod_spec = copy.deepcopy(template_spec["spec"])
                pod_spec["volumes"] = pod_spec.get("volumes", []) + [{"name": claim["metadata"]["name"], "persistentVolumeClaim": {"claimName": "ceiling-" + claim["metadata"]["name"]}} for claim in item["spec"].get("volumeClaimTemplates", [])]
                documents.append((item["kind"] + " pod", pod_document(item["metadata"]["name"] + "-ceiling", item["metadata"]["namespace"], pod_spec, template_spec["metadata"].get("labels"))))
            for kind, document in documents:
                affinity = document.get("spec", {}).get("template", {}).get("spec", document.get("spec", {})).get("affinity")
                terms = affinity["nodeAffinity"]["requiredDuringSchedulingIgnoredDuringExecution"]["nodeSelectorTerms"] if affinity else []
                verb = ["apply", "--dry-run=server"] if kind in {"StatefulSet", "DaemonSet", "Job"} else ["create", "--dry-run=server"]
                done = None
                for _ in range(8):
                    done = self.kubectl(*verb, "-f", "-", stdin=json.dumps(document), check=False)
                    if done.returncode == 0:
                        break
                    time.sleep(4)
                errors[kind] = {"terms": len(terms), "admitted": done.returncode == 0, "error": done.stderr.strip()[-600:] if done.returncode else ""}
            return {"stage": "dry-run", "objects": errors}

        def passed(outcome):
            return outcome["stage"] == "dry-run" and all(value["admitted"] for value in outcome["objects"].values())

        try:
            outcome = attempt(MAX_NODES)
            summary = {"nodes": MAX_NODES, "result": outcome}
            if not passed(outcome):
                low, high = len(self.agents), MAX_NODES
                while high - low > 1:
                    middle = (low + high) // 2
                    if passed(attempt(middle)):
                        low = middle
                    else:
                        high = middle
                summary["largest_admitted_node_count"] = low
        finally:
            self.cleanup_policies(original)
        self.results["admission_ceiling"] = summary
        self.assertTrue(passed(outcome), json.dumps(summary)[:1500])
        self.assertEqual(outcome["objects"]["DaemonSet"]["terms"], MAX_NODES)
        self.assertEqual(outcome["objects"]["DaemonSet pod"]["terms"], MAX_NODES)

    @classmethod
    def cleanup_policies(cls, original):
        cls.kubectl("apply", "-f", "-", stdin=json.dumps({"apiVersion": "v1", "kind": "List", "items": original}))
        wait_for(lambda: cls.kubectl("create", "--dry-run=server", "-f", "-", stdin=json.dumps(pod_document("restore-probe", cls.config["agent_namespace"], copy.deepcopy(next(item for item in cls.manifests if item["kind"] == "DaemonSet")["spec"]["template"]["spec"]))), check=False).returncode == 0, 90, "the original admission policies to apply again", 4)

    def test_6c_interactive_access_to_the_spire_pods_needs_a_declared_operator_group(self):
        namespace, agents = self.config["namespace"], self.config["agent_namespace"]
        message = "requires membership in a declared server operator group"
        self.apply([
            {"apiVersion": "rbac.authorization.k8s.io/v1", "kind": "ClusterRole", "metadata": {"name": "spire-access-probe"}, "rules": [
                {"apiGroups": [""], "resources": ["pods"], "verbs": ["get"]},
                {"apiGroups": [""], "resources": ["pods/exec", "pods/attach", "pods/portforward"], "verbs": ["create", "get"]},
                {"apiGroups": [""], "resources": ["pods/ephemeralcontainers"], "verbs": ["get", "patch", "update"]}]},
            {"apiVersion": "rbac.authorization.k8s.io/v1", "kind": "ClusterRoleBinding", "metadata": {"name": "spire-access-probe"},
             "roleRef": {"apiGroup": "rbac.authorization.k8s.io", "kind": "ClusterRole", "name": "spire-access-probe"},
             "subjects": [{"apiGroup": "rbac.authorization.k8s.io", "kind": "User", "name": name} for name in ["outsider", "insider"]]},
        ])
        self.addCleanup(lambda: self.kubectl("delete", "clusterrolebinding,clusterrole", "spire-access-probe", check=False))
        outsider, insider = ["--as=outsider"], ["--as=insider", *[f"--as-group={group}" for group in self.config["server_operator_groups"]]]
        container = {"name": "access-probe", "image": CURL_IMAGE, "command": ["sleep", "1"], "securityContext": {
            "runAsNonRoot": True, "runAsUser": 10005, "allowPrivilegeEscalation": False, "readOnlyRootFilesystem": True,
            "capabilities": {"drop": ["ALL"]}, "seccompProfile": {"type": "RuntimeDefault"}}}
        patch = json.dumps({"spec": {"ephemeralContainers": [container]}})
        commands = {
            "exec": lambda who, pod, where: ["exec", *who, "-n", where, pod, "-c", "server" if where == namespace else "agent", "--", SERVER_BINARY, "entry", "count", "-socketPath", SERVER_SOCKET],
            "attach": lambda who, pod, where: ["attach", *who, "-n", where, pod, "--pod-running-timeout=5s"],
            "port-forward": lambda who, pod, where: ["port-forward", *who, "-n", where, pod, ":9988" if where == namespace else ":9989"],
            "ephemeral container": lambda who, pod, where: ["patch", *who, "-n", where, f"pod/{pod}", "--subresource=ephemeralcontainers", "--type=strategic", "--dry-run=server", "-p", patch],
        }
        agent_pod = self.kubectl("get", "pods", "-n", agents, "-l", "app.kubernetes.io/name=spire-agent", "-o", "jsonpath={.items[0].metadata.name}").stdout
        refused = {}
        for target, pod, where in [("server", "spire-server-0", namespace), ("agent", agent_pod, agents)]:
            for verb, build in commands.items():
                with self.subTest(refused=verb, pod=target):
                    argv = ["kubectl", "--kubeconfig", self.kubeconfig, *build(outsider, pod, where)]
                    done = run(argv, check=False, timeout=60) if verb != "port-forward" else None
                    if done is None:
                        running, out, err = held_open(argv)
                        self.assertFalse(running, "the port-forward must be refused, not held open")
                        self.assertNotIn("Forwarding from", out)
                        text = err
                    else:
                        self.assertNotEqual(done.returncode, 0)
                        text = done.stderr
                    if (verb, target) == ("ephemeral container", "agent"):
                        # the agent pod violates the sockets policy once an ephemeral container joins it, and the API server reports one denial
                        self.assertTrue(message in text or "denied request" in text, text)
                    else:
                        self.assertIn(message, text)
                    refused[f"{verb} {target}"] = text.strip()[-300:]
        # The sockets policy also refuses an ephemeral container on an agent pod, so its binding is removed to show the operator access policy refuses it alone.
        policies = [item for item in self.manifests if item["kind"].startswith("ValidatingAdmissionPolicy")]
        access_name = namespace + "-identity-operator-access"
        self.kubectl("delete", "validatingadmissionpolicybinding", namespace + "-identity-sockets")
        alone = {}

        def refused_by_the_access_policy_alone():
            done = run(["kubectl", "--kubeconfig", self.kubeconfig, *commands["ephemeral container"](outsider, agent_pod, agents)], check=False, timeout=60)
            alone["text"] = done.stderr.strip()
            return done.returncode != 0 and access_name in done.stderr and message in done.stderr

        try:
            wait_for(refused_by_the_access_policy_alone, 90, "the operator access policy alone to refuse an ephemeral container on an agent pod", 4)
        finally:
            self.cleanup_policies(policies)
        refused["ephemeral container agent, sockets policy binding removed"] = alone["text"][-300:]
        self.results["operator_access_refused"] = refused
        for verb in ["exec", "ephemeral container"]:
            with self.subTest(admitted=verb):
                done = run(["kubectl", "--kubeconfig", self.kubeconfig, *commands[verb](insider, "spire-server-0", namespace)], check=False, timeout=60)
                self.assertEqual(done.returncode, 0, done.stderr)
        for verb in ["attach", "port-forward"]:
            with self.subTest(admitted=verb):
                running, out, err = held_open(["kubectl", "--kubeconfig", self.kubeconfig, *commands[verb](insider, "spire-server-0", namespace)])
                self.assertNotIn(message, err)
                self.assertTrue(running or verb == "attach", err)
                if verb == "port-forward":
                    self.assertIn("Forwarding from", out)
        self.assertEqual(self.server_call("entry", "count").returncode, 0, "the declared group (the kind admin) can exec the SPIRE server")
        self.results["operator_access_groups"] = self.config["server_operator_groups"]

    def promtool(self, *arguments, files):
        """Run promtool from the pinned Prometheus image on the given files, copied in rather than mounted."""
        name = "spire-promtool-" + uuid.uuid4().hex[:8]
        directory = Path(self.work, name)
        directory.mkdir()
        for file, text in files.items():
            (directory / file).write_text(text)
        run(["docker", "create", "--name", name, "--entrypoint", "promtool", PROMETHEUS_IMAGE, *arguments], timeout=120)
        try:
            run(["docker", "cp", str(directory) + "/.", f"{name}:/t"], timeout=60)
            return run(["docker", "start", "-a", name], check=False, timeout=180)
        finally:
            run(["docker", "rm", "-f", name], check=False, timeout=60)

    def test_7_rendered_alert_rules_pass_promtool(self):
        pulled = run(["docker", "image", "inspect", PROMETHEUS_IMAGE], check=False).returncode == 0 or run(["docker", "pull", PROMETHEUS_IMAGE], check=False, timeout=300).returncode == 0
        if not pulled:
            self.results["promtool"] = f"skipped: {PROMETHEUS_IMAGE} could not be pulled"
            self.skipTest(self.results["promtool"])
        configmap = json.loads(self.kubectl("get", "configmap", "spire-alert-rules", "-n", self.config["monitoring"]["namespace"], "-o", "json").stdout)
        rules = configmap["data"]["alert-rules.json"]
        done = run(["docker", "run", "-i", "--rm", "--entrypoint", "promtool", PROMETHEUS_IMAGE, "check", "rules"], stdin=rules, check=False)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.results["promtool"] = done.stdout.strip().replace("\n", "; ")[:200]
        self.assertEqual(json.loads(rules), alert_tests.rules_document(), "the cases must run against the rules the cluster serves")
        tested = self.promtool("test", "rules", "/t/tests.yml", files={"rules.yml": rules, "tests.yml": json.dumps(alert_tests.test_document())})
        self.assertEqual(tested.returncode, 0, tested.stdout + tested.stderr)
        self.results["promtool_tests"] = (tested.stdout + tested.stderr).strip().replace("\n", "; ")[:200]
        legacy = json.loads(rules)
        for group in legacy["groups"]:
            for rule in group["rules"]:
                if rule.get("alert") == "SpireRegistrationEntryChanged":
                    rule["expr"] = 'sum(increase({__name__=~"spire_server_rpc_entry_v1_entry_batch_create_entry|spire_server_rpc_entry_v1_entry_batch_update_entry"}[5m])) > 0'
        control = self.promtool("test", "rules", "/t/tests.yml", files={"rules.yml": json.dumps(legacy), "tests.yml": json.dumps(alert_tests.test_document())})
        self.assertNotEqual(control.returncode, 0, "the first-event cases must fail against a plain increase() rule")
        self.assertIn("a series that appears at 1 fires", control.stdout + control.stderr)
        self.results["promtool_legacy_expression"] = "fails the new-series case, as intended"

    def test_8_emulator_served_every_signing_and_issuance_after_rotation(self):
        before = self.results["aws_plugins"]["emulator_calls_after_startup"]
        after = self.emulator_calls()
        self.assertGreater(after["kms.Sign"], before["kms.Sign"], "SVID issuance after startup must keep signing through KMS")
        self.assertGreater(after["acmpca.IssueCertificate"], 0)
        self.assertGreaterEqual(after.get("kms.GetPublicKey", 0), before["kms.GetPublicKey"])
        self.results["aws_plugins"]["emulator_calls_at_end"] = {call: after[call] for call in sorted(after)}
        self.results["aws_unproven"] = AWS_UNPROVEN
        self.results["aws_substitution"] = "the aws_kms and aws_pca plugin blocks ran as rendered; only the endpoint environment and the input values (emulator /32, CA ARN) differ; see OVERLAY"


if __name__ == "__main__":
    unittest.main()
