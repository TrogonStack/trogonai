import ipaddress
import re
from urllib.parse import urlsplit


POOL_LABEL = "workload-identity.platform.example.node-restriction.kubernetes.io/pool"
SOCKET_DIRECTORY = "/run/platform-spire/sockets"
SOCKET_PATH = SOCKET_DIRECTORY + "/api.sock"
SERVER_SOCKET = "/run/spire/admin/api.sock"
DATABASE_DIRECTORY = "/run/spire/database"


def fields(value, expected):
    if not isinstance(value, dict) or set(value) != set(expected.split()):
        raise ValueError("deployment fields do not match the supported profile")


def token(value, limit=63):
    if not isinstance(value, str) or len(value) > limit or not re.fullmatch(r"[a-z0-9](?:[a-z0-9-]*[a-z0-9])?", value):
        raise ValueError("invalid deployment name")


def domain(value):
    if not isinstance(value, str) or len(value) > 253 or "." not in value:
        raise ValueError("invalid trust domain or hostname")
    for label in value.split("."):
        token(label)


def cidrs(values):
    if not isinstance(values, list) or not 1 <= len(values) <= 64:
        raise ValueError("a bounded explicit network allowlist is required")
    for value in values:
        if not isinstance(value, str):
            raise ValueError("network allowlists require explicit CIDR strings")
        network = ipaddress.ip_network(value, strict=True)
        if network.prefixlen == 0 or network.is_multicast or network.is_loopback or network.is_unspecified:
            raise ValueError("unrestricted or unusable network allowlist")


def application_identity(value):
    prefix = "did:key:z"
    alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
    if not isinstance(value, str) or not value.startswith(prefix) or len(value) > 128:
        raise ValueError("an Ed25519 did:key application identity is required")
    encoded = value.removeprefix(prefix)
    number = 0
    for character in encoded:
        if character not in alphabet:
            raise ValueError("application identity is not canonical base58btc")
        number = number * 58 + alphabet.index(character)
    decoded = b"\x00" * (len(encoded) - len(encoded.lstrip("1")))
    decoded += number.to_bytes((number.bit_length() + 7) // 8, "big")
    if len(decoded) != 34 or decoded[:2] != b"\xed\x01":
        raise ValueError("application identity is not an Ed25519 did:key")


def validate(config):
    fields(config, "environment trust_domain other_environment_trust_domain namespace agent_namespace cluster recovery_epoch server_replicas storage_class aws_region server_role_arn key_admin_role_arn oidc_provider_arn api_server_cidrs aws_endpoint_cidrs node_names server_node_names node_cidrs bootstrap_bundle_configmap kubelet_ca_configmap database registrations")
    if not isinstance(config["environment"], str) or config["environment"] not in {"production", "nonproduction"}:
        raise ValueError("select a production or nonproduction environment")
    domain(config["trust_domain"])
    domain(config["other_environment_trust_domain"])
    if len(config["trust_domain"]) > 100:
        raise ValueError("trust domain exceeds the KMS alias profile bound")
    if config["trust_domain"] == config["other_environment_trust_domain"]:
        raise ValueError("production and nonproduction must have distinct trust domains")
    for name in ["namespace", "agent_namespace", "cluster", "storage_class", "bootstrap_bundle_configmap", "kubelet_ca_configmap"]:
        token(config[name])
    if len(config["cluster"]) > 55:
        raise ValueError("cluster name exceeds the protected node-pool label bound")
    if len(config["namespace"]) > 40 or len(config["agent_namespace"]) > 40:
        raise ValueError("identity namespace exceeds the RBAC naming bound")
    if config["namespace"] == config["agent_namespace"]:
        raise ValueError("server and privileged node agents require separate namespaces")
    if type(config["recovery_epoch"]) is not int or not 1 <= config["recovery_epoch"] <= 2**63 - 1:
        raise ValueError("an independently provisioned nonzero recovery epoch is required")
    if type(config["server_replicas"]) is not int or not 3 <= config["server_replicas"] <= 9:
        raise ValueError("the HA profile requires 3 through 9 server replicas")
    if not isinstance(config["aws_region"], str) or not re.fullmatch(r"[a-z]{2}(?:-[a-z]+)+-[0-9]", config["aws_region"]):
        raise ValueError("invalid AWS region")
    roles = []
    for key in ["server_role_arn", "key_admin_role_arn"]:
        value = config[key]
        if not isinstance(value, str) or not re.fullmatch(r"arn:aws:iam::[0-9]{12}:role/[A-Za-z0-9_+=,.@/-]+", value):
            raise ValueError("invalid AWS IAM role ARN")
        roles.append(value)
    account = roles[0].split(":")[4]
    if roles[0] == roles[1] or roles[1].split(":")[4] != account:
        raise ValueError("a distinct key administrator in the deployment account is required")
    provider = config["oidc_provider_arn"]
    prefix = f"arn:aws:iam::{account}:oidc-provider/oidc.eks.{config['aws_region']}.amazonaws.com/id/"
    if not isinstance(provider, str) or not provider.startswith(prefix) or not re.fullmatch(r"[A-Za-z0-9]+", provider.removeprefix(prefix)):
        raise ValueError("the exact cluster OIDC provider is required")
    for key in ["api_server_cidrs", "aws_endpoint_cidrs", "node_cidrs"]:
        cidrs(config[key])
    for key in ["node_names", "server_node_names"]:
        nodes = config[key]
        if not isinstance(nodes, list) or not 3 <= len(nodes) <= 256 or not all(isinstance(node, str) for node in nodes) or len(set(nodes)) != len(nodes):
            raise ValueError("the HA profile requires a bounded list of distinct identity nodes")
        for node in nodes:
            if len(node) > 253:
                raise ValueError("node name exceeds the Kubernetes hostname bound")
            for label in node.split("."):
                token(label)
    if set(config["node_names"]) & set(config["server_node_names"]):
        raise ValueError("node agents cannot share the issuer's control-plane nodes")
    database = config["database"]
    fields(database, "host name user client_secret root_ca_configmap cidrs")
    domain(database["host"])
    for key in ["name", "user"]:
        if not isinstance(database[key], str) or not re.fullmatch(r"[a-z][a-z0-9_]{0,62}", database[key]):
            raise ValueError("invalid database identity")
    token(database["client_secret"])
    token(database["root_ca_configmap"])
    cidrs(database["cidrs"])
    registrations = config["registrations"]
    if not isinstance(registrations, list) or not 1 <= len(registrations) <= 256:
        raise ValueError("a bounded explicit workload registration set is required")
    identities, subjects = set(), set()
    for registration in registrations:
        fields(registration, "spiffe_id tenant project role application_id namespace service_account container container_image uid")
        for key in ["tenant", "project", "role", "namespace", "service_account", "container"]:
            token(registration[key])
        if registration["namespace"] in {config["namespace"], config["agent_namespace"], "kube-system", "default"}:
            raise ValueError("supervisors require a dedicated namespace")
        identity = registration["spiffe_id"]
        if not isinstance(identity, str) or len(identity) > 2048:
            raise ValueError("invalid workload SPIFFE ID")
        parsed = urlsplit(identity)
        if parsed.scheme != "spiffe" or parsed.netloc != config["trust_domain"] or parsed.query or parsed.fragment or not parsed.path.startswith("/") or parsed.path == "/":
            raise ValueError("workload identity must exactly match the deployment trust domain")
        if not re.fullmatch(r"/[A-Za-z0-9/_-]+", parsed.path) or "//" in parsed.path:
            raise ValueError("workload SPIFFE path is not canonical")
        if parsed.path.startswith("/spire/"):
            raise ValueError("workloads cannot select reserved SPIRE identities")
        if registration["role"] not in {"supervisor", "egress", "agent"}:
            raise ValueError("only trusted supervisor roles may receive the socket")
        if registration["role"] == "agent":
            application_identity(registration["application_id"])
        elif registration["application_id"] is not None:
            raise ValueError("service roles cannot acquire an application agent identity")
        if type(registration["uid"]) is not int or not 1000 <= registration["uid"] <= 2**31 - 1:
            raise ValueError("trusted supervisors require an explicit nonroot UID")
        image = registration["container_image"]
        if not isinstance(image, str) or not re.fullmatch(r"[a-z0-9][a-z0-9./:_-]+@sha256:[a-f0-9]{64}", image):
            raise ValueError("supervisor registration requires an immutable container image")
        subject = tuple(registration[key] for key in ["namespace", "service_account", "container", "uid"])
        if identity in identities or subject in subjects:
            raise ValueError("duplicate or ambiguous workload registration")
        identities.add(identity)
        subjects.add(subject)


def server_config(config):
    database = config["database"]
    connection = " ".join([
        f"host={database['host']}", "port=5432", f"dbname={database['name']}", f"user={database['user']}",
        "sslmode=verify-full", "sslrootcert=/run/spire/database-ca/ca.crt",
        f"sslcert={DATABASE_DIRECTORY}/tls.crt", f"sslkey={DATABASE_DIRECTORY}/tls.key", "connect_timeout=5",
    ])
    result = {
        "server": {
            "trust_domain": config["trust_domain"], "bind_address": "0.0.0.0", "bind_port": 8081,
            "socket_path": SERVER_SOCKET, "data_dir": "/var/lib/spire", "log_level": "INFO", "log_format": "json",
            "audit_log_enabled": True, "default_x509_svid_ttl": "1h", "agent_ttl": "1h", "ca_ttl": "24h",
            "ca_key_type": "ec-p256", "tls_config": {"min_tls_version": "VersionTLS13"},
        },
        "plugins": {
            "DataStore": {"sql": {"plugin_data": {"database_type": "postgres", "connection_string": connection,
                "disable_migration": True, "max_open_conns": 50, "max_idle_conns": 10}}},
            "KeyManager": {"aws_kms": {"plugin_data": {
                "region": config["aws_region"], "key_identifier_value": "$SPIRE_SERVER_ID",
                "key_policy_file": "/run/spire/config/kms-key-policy.json", "enable_tag_based_key_discovery": True,
                "key_tags": {"Environment": config["environment"]},
            }}},
            "NodeAttestor": {"k8s_psat": {"plugin_data": {"clusters": {config["cluster"]: {
                "service_account_allow_list": [config["agent_namespace"] + ":spire-agent"],
                "audience": ["spire-server"], "allowed_node_label_keys": [POOL_LABEL],
            }}}}},
        },
    }
    result["plugins"] = {kind: [value] for kind, value in result["plugins"].items()}
    return result


def agent_config(config):
    result = {
        "agent": {
            "trust_domain": config["trust_domain"], "data_dir": "/var/lib/spire", "socket_path": SOCKET_PATH,
            "server_address": f"spire-server.{config['namespace']}.svc", "server_port": 8081,
            "trust_bundle_path": "/run/spire/bootstrap/bundle.pem", "log_level": "INFO", "log_format": "json",
            "disable_sds_api": True,
        },
        "plugins": {
            "NodeAttestor": {"k8s_psat": {"plugin_data": {"cluster": config["cluster"], "token_path": "/run/spire/psat/token"}}},
            "KeyManager": {"disk": {"plugin_data": {"directory": "/var/lib/spire/keys"}}},
            "WorkloadAttestor": {
                "k8s": {"plugin_data": {
                    "kubelet_secure_port": 10250, "kubelet_ca_path": "/run/spire/kubelet-ca/ca.crt",
                    "skip_kubelet_verification": False, "disable_container_selectors": False,
                    "use_anonymous_authentication": False, "node_name_env": "MY_NODE_NAME",
                    "token_path": "/var/run/secrets/kubernetes.io/serviceaccount/token",
                }},
                "unix": {"plugin_data": {"discover_workload_path": False, "workload_size_limit": -1}},
            },
        },
    }
    result["plugins"] = {kind: [value] for kind, value in result["plugins"].items()}
    return result


def registrations(config):
    base = "spiffe://" + config["trust_domain"]
    parent = f"{base}/platform/agent-group/{config['cluster']}/epoch-{config['recovery_epoch']}"
    def selector(kind, value):
        return {"type": kind, "value": value}
    entries = [{
        "parent_id": base + "/spire/server", "spiffe_id": parent,
        "selectors": [selector("k8s_psat", value) for value in [
            "cluster:" + config["cluster"], "agent_ns:" + config["agent_namespace"], "agent_sa:spire-agent",
            f"agent_node_label:{POOL_LABEL}:{config['cluster']}",
        ]], "admin": False, "downstream": False, "x509_svid_ttl": 3600,
    }]
    for registration in config["registrations"]:
        entries.append({
            "parent_id": parent, "spiffe_id": registration["spiffe_id"], "admin": False,
            "downstream": False, "x509_svid_ttl": 3600,
            "selectors": [selector("k8s", prefix + ":" + registration[key]) for prefix, key in [
                ("ns", "namespace"), ("sa", "service_account"), ("container-name", "container"), ("container-image", "container_image"),
            ]] + [selector("unix", "uid:" + str(registration["uid"]))],
        })
    return {"entries": entries}
