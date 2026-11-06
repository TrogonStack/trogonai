import json

from profile import POOL_LABEL, SOCKET_DIRECTORY, SOCKET_PATH, SERVER_SOCKET


def resource(api, kind, name, namespace=None, **values):
    metadata = {"name": name}
    if namespace is not None:
        metadata["namespace"] = namespace
    return {"apiVersion": api, "kind": kind, "metadata": metadata, **values}


def token_volume(name, audience=None):
    token = {"path": "token", "expirationSeconds": 600}
    if audience is not None:
        token["audience"] = audience
    sources = [{"serviceAccountToken": token}]
    if audience is None:
        sources += [
            {"configMap": {"name": "kube-root-ca.crt", "items": [{"key": "ca.crt", "path": "ca.crt"}]}},
            {"downwardAPI": {"items": [{"path": "namespace", "fieldRef": {"fieldPath": "metadata.namespace"}}]}},
        ]
    return {"name": name, "projected": {"defaultMode": 0o440, "sources": sources}}


def mount(name, path, readonly=True):
    return {"name": name, "mountPath": path, "readOnly": readonly}


def config_volume(name, configmap):
    return {"name": name, "configMap": {"name": configmap, "defaultMode": 0o440}}


def node_affinity(names, pool):
    return {"nodeAffinity": {"requiredDuringSchedulingIgnoredDuringExecution": {"nodeSelectorTerms": [{
        "matchFields": [{"key": "metadata.name", "operator": "In", "values": names}],
        "matchExpressions": [{"key": POOL_LABEL, "operator": "In", "values": [pool]}],
    }]}}}


def probe(binary, socket, shallow=False):
    command = [binary, "healthcheck", "-socketPath", socket]
    if shallow:
        command.append("-shallow")
    return {"exec": {"command": command}, "timeoutSeconds": 5, "periodSeconds": 10, "failureThreshold": 3}


def rbac(config):
    result = []
    for component, namespace in [("server", config["namespace"]), ("agent", config["agent_namespace"])]:
        name = namespace + "-spire-" + component
        result.append(resource("v1", "ServiceAccount", "spire-" + component, namespace, automountServiceAccountToken=False))
        rules = ([
            {"apiGroups": [""], "resources": ["pods", "nodes"], "verbs": ["get"]},
            {"apiGroups": ["authentication.k8s.io"], "resources": ["tokenreviews"], "verbs": ["create"]},
        ] if component == "server" else [
            {"apiGroups": [""], "resources": ["nodes/proxy"], "verbs": ["get"], "resourceNames": config["node_names"]},
        ])
        result += [
            resource("rbac.authorization.k8s.io/v1", "ClusterRole", name, rules=rules),
            resource("rbac.authorization.k8s.io/v1", "ClusterRoleBinding", name,
                roleRef={"apiGroup": "rbac.authorization.k8s.io", "kind": "ClusterRole", "name": name},
                subjects=[{"kind": "ServiceAccount", "name": "spire-" + component, "namespace": namespace}]),
        ]
    return result


def server(config, images):
    namespace = config["namespace"]
    labels = {"app.kubernetes.io/name": "spire-server"}
    binary = "/opt/spire/bin/spire-server"
    epoch = config["recovery_epoch"]
    pod = {
        "serviceAccountName": "spire-server", "automountServiceAccountToken": False,
        "terminationGracePeriodSeconds": 60,
        "securityContext": {"runAsUser": 1000, "runAsGroup": 1000, "runAsNonRoot": True,
                            "fsGroup": 1000, "fsGroupChangePolicy": "OnRootMismatch", "seccompProfile": {"type": "RuntimeDefault"}},
        "affinity": node_affinity(config["server_node_names"], config["cluster"] + "-control"),
        "topologySpreadConstraints": [{"maxSkew": 1, "minDomains": 3, "topologyKey": "topology.kubernetes.io/zone",
                                       "whenUnsatisfiable": "DoNotSchedule", "labelSelector": {"matchLabels": labels}}],
        "containers": [{
            "name": "server", "image": images["server"], "imagePullPolicy": "IfNotPresent",
            "command": [binary], "args": ["run", "-config", "/run/spire/config/server.conf", "-expandEnv"],
            "ports": [{"name": "grpc", "containerPort": 8081}],
            "securityContext": {"allowPrivilegeEscalation": False, "readOnlyRootFilesystem": True, "capabilities": {"drop": ["ALL"]}},
            "resources": {"requests": {"cpu": "500m", "memory": "512Mi"}, "limits": {"cpu": "2", "memory": "2Gi"}},
            "env": [
                {"name": "POD_NAME", "valueFrom": {"fieldRef": {"fieldPath": "metadata.name"}}},
                {"name": "SPIRE_SERVER_ID", "value": f"epoch-{epoch}-$(POD_NAME)"},
                {"name": "AWS_REGION", "value": config["aws_region"]},
                {"name": "AWS_ROLE_ARN", "value": config["server_role_arn"]},
                {"name": "AWS_WEB_IDENTITY_TOKEN_FILE", "value": "/run/spire/aws/token"},
                {"name": "AWS_EC2_METADATA_DISABLED", "value": "true"},
                {"name": "AWS_STS_REGIONAL_ENDPOINTS", "value": "regional"},
            ],
            "volumeMounts": [mount("config", "/run/spire/config"), mount("data", "/var/lib/spire", False),
                mount("admin", "/run/spire/admin", False), mount("tmp", "/tmp", False),
                mount("api-token", "/var/run/secrets/kubernetes.io/serviceaccount"), mount("aws-token", "/run/spire/aws"),
                mount("database-client", "/run/spire/database"), mount("database-ca", "/run/spire/database-ca")],
            "livenessProbe": probe(binary, SERVER_SOCKET, True), "readinessProbe": probe(binary, SERVER_SOCKET),
            "startupProbe": {**probe(binary, SERVER_SOCKET), "failureThreshold": 30},
        }],
        "volumes": [config_volume("config", "spire-server"), {"name": "admin", "emptyDir": {"medium": "Memory", "sizeLimit": "16Mi"}},
            {"name": "tmp", "emptyDir": {"sizeLimit": "64Mi"}}, token_volume("api-token"), token_volume("aws-token", "sts.amazonaws.com"),
            {"name": "database-client", "secret": {"secretName": config["database"]["client_secret"], "defaultMode": 0o440}},
            config_volume("database-ca", config["database"]["root_ca_configmap"])],
    }
    return [
        resource("v1", "Service", "spire-server", namespace, spec={"selector": labels, "ports": [{"name": "grpc", "port": 8081, "targetPort": "grpc"}]}),
        resource("v1", "Service", "spire-server-headless", namespace, spec={"clusterIP": "None", "selector": labels,
                 "ports": [{"name": "grpc", "port": 8081, "targetPort": "grpc"}]}),
        resource("policy/v1", "PodDisruptionBudget", "spire-server", namespace, spec={"maxUnavailable": 1, "selector": {"matchLabels": labels}}),
        resource("apps/v1", "StatefulSet", "spire-server", namespace, spec={
            "serviceName": "spire-server-headless", "replicas": config["server_replicas"],
            "podManagementPolicy": "Parallel", "selector": {"matchLabels": labels},
            "persistentVolumeClaimRetentionPolicy": {"whenDeleted": "Retain", "whenScaled": "Retain"},
            "template": {"metadata": {"labels": labels, "annotations": {"workload-identity.platform.example/recovery-epoch": str(epoch)}}, "spec": pod},
            "volumeClaimTemplates": [{"metadata": {"name": "data"}, "spec": {"accessModes": ["ReadWriteOnce"],
                "storageClassName": config["storage_class"], "resources": {"requests": {"storage": "5Gi"}}}}],
        }),
    ]


def agent(config, images):
    namespace = config["agent_namespace"]
    labels = {"app.kubernetes.io/name": "spire-agent"}
    binary = "/opt/spire/bin/spire-agent"
    return resource("apps/v1", "DaemonSet", "spire-agent", namespace, spec={
        "selector": {"matchLabels": labels}, "updateStrategy": {"type": "RollingUpdate", "rollingUpdate": {"maxUnavailable": 1}},
        "template": {"metadata": {"labels": labels}, "spec": {
            "serviceAccountName": "spire-agent", "automountServiceAccountToken": False, "hostPID": True,
            "securityContext": {"runAsUser": 0, "runAsGroup": 0, "seccompProfile": {"type": "RuntimeDefault"}},
            "affinity": node_affinity(config["node_names"], config["cluster"]),
            "containers": [{"name": "agent", "image": images["agent"], "imagePullPolicy": "IfNotPresent",
                "command": [binary], "args": ["run", "-config", "/run/spire/config/agent.conf"],
                "securityContext": {"allowPrivilegeEscalation": False, "readOnlyRootFilesystem": True,
                    "capabilities": {"drop": ["ALL"], "add": ["SYS_PTRACE"]}},
                "resources": {"requests": {"cpu": "250m", "memory": "256Mi"}, "limits": {"cpu": "1", "memory": "1Gi"}},
                "env": [{"name": "MY_NODE_NAME", "valueFrom": {"fieldRef": {"fieldPath": "spec.nodeName"}}}],
                "volumeMounts": [mount("config", "/run/spire/config"), mount("bootstrap", "/run/spire/bootstrap"),
                    mount("data", "/var/lib/spire", False), mount("socket", SOCKET_DIRECTORY, False), mount("tmp", "/tmp", False),
                    mount("psat", "/run/spire/psat"), mount("api-token", "/var/run/secrets/kubernetes.io/serviceaccount"),
                    mount("kubelet-ca", "/run/spire/kubelet-ca")],
                "livenessProbe": probe(binary, SOCKET_PATH, True), "readinessProbe": probe(binary, SOCKET_PATH),
                "startupProbe": {**probe(binary, SOCKET_PATH), "failureThreshold": 30},
            }],
            "volumes": [config_volume("config", "spire-agent"), config_volume("bootstrap", config["bootstrap_bundle_configmap"]),
                config_volume("kubelet-ca", config["kubelet_ca_configmap"]), {"name": "tmp", "emptyDir": {"sizeLimit": "64Mi"}},
                {"name": "data", "hostPath": {"path": f"/var/lib/platform-spire/epoch-{config['recovery_epoch']}", "type": "DirectoryOrCreate"}},
                {"name": "socket", "hostPath": {"path": SOCKET_DIRECTORY, "type": "DirectoryOrCreate"}},
                token_volume("psat", "spire-server"), token_volume("api-token")],
        }},
    })


def network_policies(config):
    resources = []
    dns = {"to": [{"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "kube-system"}},
                   "podSelector": {"matchLabels": {"k8s-app": "kube-dns"}}}],
           "ports": [{"protocol": "UDP", "port": 53}, {"protocol": "TCP", "port": 53}]}
    for namespace in [config["namespace"], config["agent_namespace"]]:
        resources.append(resource("networking.k8s.io/v1", "NetworkPolicy", "default-deny", namespace,
            spec={"podSelector": {}, "policyTypes": ["Ingress", "Egress"]}))
    def destinations(cidrs, port):
        return {"to": [{"ipBlock": {"cidr": cidr}} for cidr in cidrs], "ports": [{"protocol": "TCP", "port": port}]}
    server_peer = {"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": config["namespace"]}},
                   "podSelector": {"matchLabels": {"app.kubernetes.io/name": "spire-server"}}}
    agent_peer = {"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": config["agent_namespace"]}},
                  "podSelector": {"matchLabels": {"app.kubernetes.io/name": "spire-agent"}}}
    resources.append(resource("networking.k8s.io/v1", "NetworkPolicy", "spire-server", config["namespace"], spec={
        "podSelector": {"matchLabels": {"app.kubernetes.io/name": "spire-server"}}, "policyTypes": ["Ingress", "Egress"],
        "ingress": [{"from": [agent_peer], "ports": [{"protocol": "TCP", "port": 8081}]}],
        "egress": [dns, destinations(config["api_server_cidrs"], 443), destinations(config["aws_endpoint_cidrs"], 443),
                   destinations(config["database"]["cidrs"], 5432)],
    }))
    resources.append(resource("networking.k8s.io/v1", "NetworkPolicy", "spire-agent", config["agent_namespace"], spec={
        "podSelector": {"matchLabels": {"app.kubernetes.io/name": "spire-agent"}}, "policyTypes": ["Ingress", "Egress"],
        "egress": [dns, {"to": [server_peer], "ports": [{"protocol": "TCP", "port": 8081}]}, destinations(config["node_cidrs"], 10250)],
    }))
    return resources


def matching_fields(path, expected):
    if isinstance(expected, dict):
        clauses = []
        for key, value in expected.items():
            child = path + "." + key
            match = matching_fields(child, value)
            if key == "readOnly" and value is False:
                clauses.append(f"(!has({child}) || ({match}))")
            else:
                clauses.append(f"(has({child}) && ({match}))")
        return " && ".join(clauses) or "true"
    if isinstance(expected, list):
        clauses = [f"{path}.size() == {len(expected)}"]
        clauses += [matching_fields(f"{path}[{index}]", value) for index, value in enumerate(expected)]
        return " && ".join("(" + clause + ")" for clause in clauses)
    return f"{path} == {json.dumps(expected)}"


def protected_namespaces(config):
    return {config["namespace"]: config["server_node_names"], config["agent_namespace"]: config["node_names"],
            **{registration["namespace"]: config["node_names"] for registration in config["registrations"]}}


def socket_policy(config, images):
    quote = json.dumps
    container = "object.spec.containers[0]"
    expected_agent = agent(config, images)["spec"]["template"]["spec"]
    expected_container = expected_agent["containers"][0]
    no_host_namespaces = " && ".join(
        f"(!has(object.spec.{field}) || !object.spec.{field})"
        for field in ["hostPID", "hostIPC", "hostNetwork", "shareProcessNamespace"]
    )
    no_mount_overlays = " && ".join([
        "(!has(m.subPath) || m.subPath == '')", "(!has(m.subPathExpr) || m.subPathExpr == '')",
        "(!has(m.mountPropagation) || m.mountPropagation == 'None')",
    ])
    agent_identity = " && ".join([
        f"object.metadata.namespace == {quote(config['agent_namespace'])}",
        "object.spec.serviceAccountName == 'spire-agent'", "object.spec.containers.size() == 1",
        f"{container}.name == 'agent'", f"{container}.image == {quote(images['agent'])}",
    ])
    privileges = " && ".join([
        matching_fields("object.spec.securityContext", expected_agent["securityContext"]),
        f"has({container}.securityContext)", matching_fields(container + ".securityContext", expected_container["securityContext"]),
        f"(!has({container}.securityContext.privileged) || !{container}.securityContext.privileged)",
        f"(!has({container}.securityContext.runAsUser) || {container}.securityContext.runAsUser == 0)",
        f"(!has({container}.securityContext.runAsGroup) || {container}.securityContext.runAsGroup == 0)",
        f"(!has({container}.securityContext.runAsNonRoot) || !{container}.securityContext.runAsNonRoot)",
        f"(!has({container}.securityContext.procMount) || {container}.securityContext.procMount == 'Default')",
        f"(!has({container}.securityContext.seccompProfile) || {container}.securityContext.seccompProfile.type == 'RuntimeDefault')",
        "(!has(object.spec.securityContext.sysctls) || object.spec.securityContext.sysctls.size() == 0)",
        "has(object.spec.hostPID) && object.spec.hostPID",
        "(!has(object.spec.hostIPC) || !object.spec.hostIPC)",
        "(!has(object.spec.hostNetwork) || !object.spec.hostNetwork)",
        "(!has(object.spec.shareProcessNamespace) || !object.spec.shareProcessNamespace)",
        "has(object.spec.automountServiceAccountToken) && !object.spec.automountServiceAccountToken",
    ])
    process = matching_fields(container, {key: expected_container[key] for key in
        ["command", "args", "env", "livenessProbe", "readinessProbe", "startupProbe"]})
    process += " && " + " && ".join([
        f"!has({container}.lifecycle)",
        f"(!has({container}.envFrom) || {container}.envFrom.size() == 0)",
        f"(!has({container}.ports) || {container}.ports.size() == 0)",
        f"(!has({container}.volumeDevices) || {container}.volumeDevices.size() == 0)",
        "(!has(object.spec.initContainers) || object.spec.initContainers.size() == 0)",
        "(!has(object.spec.ephemeralContainers) || object.spec.ephemeralContainers.size() == 0)",
    ])
    placement = " && ".join([
        "(!has(object.spec.nodeName) || object.spec.nodeName == '' || variables.agentNodes.exists(n, n == object.spec.nodeName))",
        "has(object.spec.affinity) && has(object.spec.affinity.nodeAffinity)",
        "has(object.spec.affinity.nodeAffinity.requiredDuringSchedulingIgnoredDuringExecution)",
        "object.spec.affinity.nodeAffinity.requiredDuringSchedulingIgnoredDuringExecution.nodeSelectorTerms.size() > 0",
        "object.spec.affinity.nodeAffinity.requiredDuringSchedulingIgnoredDuringExecution.nodeSelectorTerms.all(t, "
        "has(t.matchFields) && t.matchFields.exists(f, f.key == 'metadata.name' && f.operator == 'In' && "
        "f.values.size() > 0 && f.values.all(n, variables.agentNodes.exists(approved, approved == n))))",
    ])
    variables = [
        {"name": "agentNodes", "expression": quote(config["node_names"])},
        {"name": "agentIdentity", "expression": agent_identity},
        {"name": "agentPrivileges", "expression": "variables.agentIdentity && has(object.spec.securityContext) && " + privileges},
        {"name": "agentProcess", "expression": "variables.agentIdentity && " + process},
        {"name": "agentPlacement", "expression": "variables.agentIdentity && " + placement},
        {"name": "agentVolumeShape", "expression": "variables.agentIdentity && has(object.spec.volumes) && "
            f"object.spec.volumes.size() == {len(expected_agent['volumes'])}"},
        {"name": "agentMounts", "expression": "variables.agentIdentity && " +
            matching_fields(container, {"volumeMounts": expected_container["volumeMounts"]}) +
            f" && {container}.volumeMounts.all(m, {no_mount_overlays})"},
    ]
    volume_checks = []
    for index, volume in enumerate(expected_agent["volumes"]):
        name = "agentVolume" + str(index)
        variables.append({"name": name, "expression": "variables.agentVolumeShape && " + matching_fields(f"object.spec.volumes[{index}]", volume)})
        volume_checks.append("variables." + name)
    variables.append({"name": "isAgent", "expression": " && ".join([
        "variables.agentIdentity", "variables.agentPrivileges", "variables.agentProcess", "variables.agentPlacement",
        "variables.agentMounts", *volume_checks,
    ])})
    supervisors = []
    for registration in config["registrations"]:
        supervisors.append("(" + " && ".join([
            f"object.metadata.namespace == {quote(registration['namespace'])}",
            f"object.spec.serviceAccountName == {quote(registration['service_account'])}",
            "object.spec.containers.exists(c, " + " && ".join([
                f"c.name == {quote(registration['container'])}", f"c.image == {quote(registration['container_image'])}",
                "has(c.securityContext)", f"has(c.securityContext.runAsUser) && c.securityContext.runAsUser == {registration['uid']}",
            ]) + ")",
        ]) + ")")
    variables.extend([
        {"name": "isSupervisor", "expression": " || ".join(supervisors)},
        {"name": "hostVolumes", "expression": "has(object.spec.volumes) ? object.spec.volumes.filter(v, has(v.hostPath)).map(v, v.name) : []"},
    ])
    permitted_mounts = " || ".join(
        f"(object.metadata.namespace == {quote(r['namespace'])} && object.spec.serviceAccountName == {quote(r['service_account'])} && c.name == {quote(r['container'])})"
        for r in config["registrations"]
    )
    def inherited(field, condition):
        return f"(has(c.securityContext.{field}) ? c.securityContext.{field}{condition} : " \
            f"(has(object.spec.securityContext) && has(object.spec.securityContext.{field}) && object.spec.securityContext.{field}{condition}))"
    seccomp = "(has(c.securityContext.seccompProfile) ? c.securityContext.seccompProfile.type == 'RuntimeDefault' : " \
        "(has(object.spec.securityContext) && has(object.spec.securityContext.seccompProfile) && object.spec.securityContext.seccompProfile.type == 'RuntimeDefault'))"
    safe_container = " && ".join([
        "has(c.securityContext)", inherited("runAsNonRoot", " == true"), inherited("runAsUser", " > 0"),
        "has(c.securityContext.allowPrivilegeEscalation) && !c.securityContext.allowPrivilegeEscalation",
        "has(c.securityContext.readOnlyRootFilesystem) && c.securityContext.readOnlyRootFilesystem",
        "(!has(c.securityContext.privileged) || !c.securityContext.privileged)",
        "has(c.securityContext.capabilities) && has(c.securityContext.capabilities.drop) && c.securityContext.capabilities.drop.exists(cap, cap == 'ALL')",
        "(!has(c.securityContext.capabilities.add) || c.securityContext.capabilities.add.size() == 0)",
        "(!has(c.securityContext.procMount) || c.securityContext.procMount == 'Default')", seccomp,
        "(!has(c.securityContext.windowsOptions) || !has(c.securityContext.windowsOptions.hostProcess) || !c.securityContext.windowsOptions.hostProcess)",
    ])
    safe_pod = " && ".join([
        no_host_namespaces, "(!has(object.spec.os) || object.spec.os.name == 'linux')",
        "(!has(object.spec.securityContext) || !has(object.spec.securityContext.sysctls) || object.spec.securityContext.sysctls.size() == 0)",
        "object.spec.containers.all(c, " + safe_container + ")",
        "(!has(object.spec.initContainers) || object.spec.initContainers.all(c, " + safe_container + "))",
        "(!has(object.spec.ephemeralContainers) || object.spec.ephemeralContainers.all(c, " + safe_container + "))",
    ])
    safe_volumes = " || ".join("has(v." + kind + ")" for kind in
        ["configMap", "csi", "downwardAPI", "emptyDir", "ephemeral", "persistentVolumeClaim", "projected", "secret"])
    direct_nodes = " && ".join(
        f"(object.metadata.namespace != {quote(namespace)} || !has(object.spec.nodeName) || object.spec.nodeName == '' || "
        f"{quote(nodes)}.exists(n, n == object.spec.nodeName))" for namespace, nodes in protected_namespaces(config).items()
    )
    validations = [
        {"expression": "variables.isAgent || (" + safe_pod + ")",
         "message": "Pods require nonroot isolated processes; only the exact SPIRE agent profile has host privileges."},
        {"expression": "variables.isAgent || !has(object.spec.volumes) || object.spec.volumes.all(v, "
            f"(!has(v.hostPath) && ({safe_volumes})) || (variables.isSupervisor && has(v.hostPath) && "
            f"v.hostPath.path == {quote(SOCKET_DIRECTORY)} && has(v.hostPath.type) && v.hostPath.type == 'Directory'))",
         "message": "Host volumes are denied globally except the exact agent profile and admitted supervisor socket."},
        {"expression": "variables.isAgent || object.spec.containers.all(c, !has(c.volumeMounts) || c.volumeMounts.all(m, "
            "!variables.hostVolumes.exists(n, n == m.name) || "
            f"(has(m.readOnly) && m.readOnly && m.mountPath == {quote(SOCKET_DIRECTORY)} && {no_mount_overlays} && ({permitted_mounts}))))",
         "message": "Only the admitted supervisor container receives its canonical read-only socket mount."},
        {"expression": "!has(object.spec.initContainers) || object.spec.initContainers.all(c, !has(c.volumeMounts) || "
            "c.volumeMounts.all(m, !variables.hostVolumes.exists(n, n == m.name)))",
         "message": "Init containers cannot mount host identity endpoints."},
        {"expression": "!has(object.spec.ephemeralContainers) || object.spec.ephemeralContainers.all(c, !has(c.volumeMounts) || "
            "c.volumeMounts.all(m, !variables.hostVolumes.exists(n, n == m.name)))",
         "message": "Ephemeral containers cannot mount host identity endpoints."},
        {"expression": direct_nodes, "message": "Identity namespaces cannot select nodes outside their approved pool."},
    ]
    name = config["namespace"] + "-identity-sockets"
    policy = resource("admissionregistration.k8s.io/v1", "ValidatingAdmissionPolicy", name, spec={
        "failurePolicy": "Fail", "matchConstraints": {"resourceRules": [{"apiGroups": [""], "apiVersions": ["v1"],
            "operations": ["CREATE", "UPDATE"], "resources": ["pods", "pods/ephemeralcontainers"]}]},
        "variables": variables, "validations": validations,
    })
    binding = resource("admissionregistration.k8s.io/v1", "ValidatingAdmissionPolicyBinding", name,
        spec={"policyName": name, "validationActions": ["Deny"]})
    node_name = config["namespace"] + "-identity-node-bindings"
    expression = " && ".join(
        f"(object.metadata.namespace != {quote(namespace)} || {quote(nodes)}.exists(n, n == object.target.name))"
        for namespace, nodes in protected_namespaces(config).items()
    )
    node_policy = resource("admissionregistration.k8s.io/v1", "ValidatingAdmissionPolicy", node_name, spec={
        "failurePolicy": "Fail", "matchConstraints": {"resourceRules": [{"apiGroups": [""], "apiVersions": ["v1"],
            "operations": ["CREATE"], "resources": ["pods/binding", "bindings"]}]},
        "variables": [], "validations": [{"expression": expression, "message": "Identity bindings require an approved node."}],
    })
    node_binding = resource("admissionregistration.k8s.io/v1", "ValidatingAdmissionPolicyBinding", node_name,
        spec={"policyName": node_name, "validationActions": ["Deny"]})
    return [policy, binding, node_policy, node_binding]


def supervisor_patches(config):
    return [{"namespace": r["namespace"], "service_account": r["service_account"], "pod_spec_patch": {
        "serviceAccountName": r["service_account"], "automountServiceAccountToken": False,
        "affinity": node_affinity(config["node_names"], config["cluster"]),
        "containers": [{"name": r["container"], "image": r["container_image"],
            "securityContext": {"runAsUser": r["uid"], "runAsNonRoot": True, "allowPrivilegeEscalation": False,
                "readOnlyRootFilesystem": True, "capabilities": {"drop": ["ALL"]}, "seccompProfile": {"type": "RuntimeDefault"}},
            "env": [{"name": "SPIFFE_ENDPOINT_SOCKET", "value": "unix://" + SOCKET_PATH}] + ([
                {"name": "TROGON_EGRESS_WORKLOAD_API_ENDPOINT", "value": "unix://" + SOCKET_PATH},
                {"name": "TROGON_EGRESS_WORKLOAD_SPIFFE_ID", "value": r["spiffe_id"]},
            ] if r["role"] == "egress" else []),
            "volumeMounts": [mount("workload-api", SOCKET_DIRECTORY)]}],
        "volumes": [{"name": "workload-api", "hostPath": {"path": SOCKET_DIRECTORY, "type": "Directory"}}],
    }} for r in config["registrations"]]


def manifests(config, images, files):
    namespaces = []
    for namespace, security in [(config["namespace"], "restricted"), (config["agent_namespace"], "privileged")]:
        item = resource("v1", "Namespace", namespace)
        item["metadata"]["labels"] = {"pod-security.kubernetes.io/enforce": security,
            "pod-security.kubernetes.io/enforce-version": "v1.37", "pod-security.kubernetes.io/audit": "restricted",
            "pod-security.kubernetes.io/warn": "restricted"}
        namespaces.append(item)
    supervisor_namespaces = sorted({r["namespace"] for r in config["registrations"]})
    for namespace in supervisor_namespaces:
        item = resource("v1", "Namespace", namespace)
        item["metadata"]["labels"] = {"pod-security.kubernetes.io/enforce": "privileged",
            "pod-security.kubernetes.io/audit": "restricted", "pod-security.kubernetes.io/warn": "restricted"}
        namespaces.append(item)
    accounts = [resource("v1", "ServiceAccount", name, namespace, automountServiceAccountToken=False)
                for namespace, name in sorted({(r["namespace"], r["service_account"]) for r in config["registrations"]})]
    configurations = [
        resource("v1", "ConfigMap", "spire-server", config["namespace"], data={"server.conf": files["server.conf"], "kms-key-policy.json": files["kms-key-policy.json"]}),
        resource("v1", "ConfigMap", "spire-agent", config["agent_namespace"], data={"agent.conf": files["agent.conf"]}),
    ]
    return {"apiVersion": "v1", "kind": "List", "items": namespaces + accounts + rbac(config) + configurations +
        network_policies(config) + socket_policy(config, images) + server(config, images) + [agent(config, images)]}
