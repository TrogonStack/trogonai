"""`promtool test rules` document for the rendered alert rules: the first event of a new series must fire.

SPIRE creates a call counter on its first increment, so the first event of a kind is a series that appears
at 1 rather than an increase. `increase()` alone sees one sample or a flat run and stays silent; the
rendered expressions add the value of series that did not exist one window ago. The cases below pin that
with a new series at 1, a flat pre-existing series, a normal increment and a counter reset.
"""
from alerts import alert_rules

ENTRY = "spire_server_rpc_entry_v1_entry_batch_create_entry"
TOKEN = "spire_server_rpc_agent_v1_agent_create_join_token"
OK = '{status="OK"}'


def case(name, series, firing_at, silent_at, alert="SpireRegistrationEntryChanged"):
    """One test: `series` is the input sample string; the alert fires at `firing_at` and not at `silent_at`."""
    rule = next(rule for group in alert_rules()["groups"] for rule in group["rules"] if rule.get("alert") == alert)
    labels = {"alertname": alert, **rule["labels"]}
    expected = {"exp_labels": labels, "exp_annotations": rule["annotations"]}
    return {"name": name, "interval": "1m", "input_series": series,
            "alert_rule_test": [{"eval_time": at, "alertname": alert, "exp_alerts": [expected] if fires else []}
                                for at, fires in [*((t, True) for t in firing_at), *((t, False) for t in silent_at)]]}


def test_document(rules_file="rules.yml"):
    entry = lambda values, status="OK": {"series": f'{ENTRY}{{status="{status}"}}', "values": values}
    return {"rule_files": [rules_file], "evaluation_interval": "1m", "tests": [
        case("a series that appears at 1 fires", [entry("_ _ _ 1 1 1 1 1 1 1 1 1 1 1")], ["4m", "7m"], ["9m", "12m"]),
        case("a flat series that already existed stays silent", [entry("1 1 1 1 1 1 1 1 1 1 1 1 1 1")], [], ["5m", "8m", "12m"]),
        case("an increment of an existing series fires", [entry("1 1 1 1 1 1 2 2 2 2 2 2 2 2 2")], ["7m"], ["5m", "13m"]),
        case("a counter reset fires", [entry("5 5 5 5 5 5 1 1 1 1 1 1 1 1 1")], ["7m"], ["5m", "13m"]),
        case("two names sharing the same labels are summed without a labelset collision",
             [entry("_ _ _ 1 1 1 1 1 1 1 1 1 1 1"), {"series": f'spire_server_rpc_entry_v1_entry_batch_delete_entry{OK}', "values": "_ _ _ 1 1 1 1 1 1 1 1 1 1 1"}],
             ["4m"], ["12m"]),
        case("a minted join token fires", [{"series": f"{TOKEN}{OK}", "values": "_ _ _ 1 1 1 1 1 1 1 1 1 1 1"}], ["4m"], ["12m"], "SpireJoinTokenCreated"),
        case("a failed attestation fires but a successful one does not",
             [{"series": 'spire_server_rpc_agent_v1_agent_attest_agent{status="OK"}', "values": "_ _ _ 1 1 1 1 1 1 1 1 1 1 1"}], [], ["4m"], "SpireAgentAttestationFailed"),
        case("a failed attestation appearing at 1 fires",
             [{"series": 'spire_server_rpc_agent_v1_agent_attest_agent{status="Internal"}', "values": "_ _ _ 1 1 1 1 1 1 1 1 1 1 1"}], ["4m"], ["12m"], "SpireAgentAttestationFailed"),
    ]}


def rules_document():
    return alert_rules()
