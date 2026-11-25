"""Prometheus alert rules for the SPIRE deployment, built from SPIRE call counter names.

Metric names follow doc/telemetry/telemetry.md and telemetry_config.md of the pinned SPIRE release, and
every series named here was observed on the `/metrics` endpoint of a running SPIRE 1.16.0 server and
agent. SPIRE reports each RPC as a call counter keyed `rpc.<service>.<method>`, and the Prometheus
collector joins the key parts with underscores after the `spire_server` or `spire_agent` prefix, so
`rpc.agent.v1.agent.attest_agent` is scraped as `spire_server_rpc_agent_v1_agent_attest_agent`. The
trust domain service is keyed `trustdomain.v1.trust_domain`. Every call counter carries a `status`
label holding the gRPC status code.
"""

JOIN_TOKEN = "create_join_token"
ENTRY_CHANGES = ["batch_create_entry", "batch_update_entry", "batch_delete_entry"]
WORKLOAD_FETCHES = ["fetch_x509svid", "fetch_jwtsvid"]
FEDERATION_CHANGES = [
    "batch_create_federation_relationship", "batch_update_federation_relationship",
    "batch_delete_federation_relationship",
]


def metric(process, *key):
    """Return the Prometheus series name of a documented SPIRE call counter key."""
    return "_".join(["spire_" + process, *(part.replace(".", "_") for part in key)])


def increase(names, window, selector=""):
    """Events in the window, summed over the named series one name at a time.

    `increase()` alone misses the first event: SPIRE creates a call counter on its first increment, so the
    series starts at 1 and `increase()` returns nothing for one sample and 0 for a flat run. Each name adds
    `increase()` (counter resets included) to the value of every series that did not exist one window ago
    (`unless ... offset`). Names are never merged into one regex selector: functions drop `__name__`, so two
    names with the same labels collide ("vector cannot contain metrics with the same labelset"). `or vector(0)`
    keeps an absent series from emptying the sum.
    """
    terms = []
    for name in names:
        series = f"{name}{{{selector}}}" if selector else name
        terms += [f"(sum(increase({series}[{window}])) or vector(0))",
                  f"(sum({series} unless {series} offset {window}) or vector(0))"]
    return " + ".join(terms)


def alert(name, expr, summary, severity, window="0m"):
    return {"alert": name, "expr": expr, "for": window, "labels": {"severity": severity},
            "annotations": {"summary": summary, "runbook": "docs/how-to/deploy-workload-identity.md#alert-on-issuer-events"}}


def alert_rules():
    failed = 'status!="OK"'
    entry_rpc = [metric("server", "rpc", "entry.v1.entry", method) for method in ENTRY_CHANGES]
    federation_rpc = [metric("server", "rpc", "trustdomain.v1.trust_domain", method) for method in FEDERATION_CHANGES]
    datastore = [metric("server", "datastore", "registration_entry", action) for action in ["create", "update", "delete"]]
    attest = metric("server", "rpc", "agent.v1.agent", "attest_agent")
    ban = metric("server", "rpc", "agent.v1.agent", "ban_agent")
    join_token = metric("server", "rpc", "agent.v1.agent", JOIN_TOKEN)
    workload = metric("agent", "workload_api", "workload_attestation")
    denied = [metric("agent", "rpc", "workload_api", method) for method in WORKLOAD_FETCHES]
    rotations = [metric("agent", "node_attestor", "new_svid"), metric("server", "svid", "rotate")]
    return {"groups": [
        {"name": "spire-server-events", "rules": [
            alert("SpireRegistrationEntryChanged", increase(entry_rpc, "5m") + " > 0",
                  "A registration entry was created, updated or deleted through the SPIRE server API; match it to an approved change", "critical"),
            alert("SpireRegistrationEntryDatastoreWrite", increase(datastore, "5m") + " > 0",
                  "The SPIRE datastore wrote a registration entry; match it to an approved change", "warning"),
            alert("SpireAgentAttestationFailed", increase([attest], "5m", failed) + " > 0",
                  "A SPIRE agent failed node attestation", "critical"),
            alert("SpireAgentBanned", increase([ban], "5m") + " > 0", "A SPIRE agent was banned", "critical"),
            alert("SpireJoinTokenCreated", increase([join_token], "5m") + " > 0",
                  "A SPIRE join token was minted; this profile attests agents with k8s_psat only and a token lets any holder attest an agent", "critical"),
            alert("SpireFederationRelationshipChanged", increase(federation_rpc, "5m") + " > 0",
                  "A SPIRE federation relationship was created, updated or deleted; this profile configures none", "critical"),
        ]},
        {"name": "spire-agent-events", "rules": [
            alert("SpireWorkloadAttestationFailing", increase([workload], "10m", failed) + " > 5",
                  "Repeated workload attestation failures: the agent cannot derive selectors for calling processes",
                  "warning", "10m"),
            alert("SpireWorkloadIdentityDenied", increase(denied, "10m", 'status="PermissionDenied"') + " > 5",
                  "A process keeps asking the Workload API for an identity that no registration entry grants it", "warning", "10m"),
            alert("SpireSvidRotationFailed", increase(rotations, "5m", failed) + " > 0",
                  "An agent or server SVID rotation failed", "critical"),
        ]},
        {"name": "spire-progress", "rules": [
            {"record": "spire:svid_rotation_failures:increase1w", "expr": increase(rotations, "1w", failed)},
        ]},
    ]}
