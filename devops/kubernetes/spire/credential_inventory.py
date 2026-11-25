"""Validate the operator-owned inventory of static service credentials and count the ones retired.

The inventory is one record per call path that authenticates with a static
credential today. It never holds the credential: a record carries its kind and
a locator saying where it lives, and any other field is refused. A path moves
through static, parallel (SVID issued, old secret still works), enforced
(authorisation allows only the caller's SPIFFE ID, old path disabled but
restorable) and retired (static secret revoked). A path in state enforced or
retired carries `peer_id_evidence_sha256`, the SHA-256 that `peer_id_evidence.py`
printed for the saved proxy log it accepted; with `--evidence-dir` this tool
finds that file by digest and runs the same check on it again. Retirement is only accepted
after one full CA rotation under enforcement, the CA lifetime rendered in
`profile.CA_TTL_SECONDS`, and a path past static must name a caller SPIFFE ID
that the rendered `registrations.json` registers.

Inventory shape: {"call_paths": [{"name", "owner", "caller", "callee",
"credential": {"kind", "locator"}, "state", "caller_spiffe_id",
"enforcement_started_at", "retired_at", "peer_id_evidence_sha256"}]}. Timestamps are RFC 3339 UTC
(`2026-03-02T09:00:00Z`) or null.
"""

import argparse
import hashlib
import json
import re
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

from profile import CA_TTL_SECONDS

STATES = ["static", "parallel", "enforced", "retired"]
KINDS = {"api_key", "shared_password", "long_lived_certificate", "other"}
RECORD_FIELDS = "name owner caller callee credential state caller_spiffe_id enforcement_started_at retired_at peer_id_evidence_sha256"
NAME = re.compile(r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?")
# `scheme:reference`, for example `k8s-secret:identity-control/egress-upstream` or `file:/etc/egress/client.pem`.
LOCATOR = re.compile(r"[a-z][a-z0-9-]*:[A-Za-z0-9_./@-]{1,200}")
VALUE_SHAPED = re.compile(r"[A-Za-z0-9_-]{40,}")
DIGEST = re.compile(r"[0-9a-f]{64}")
TIMESTAMP = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z")


class InventoryRefused(ValueError):
    def __init__(self, path, rule):
        super().__init__(f"{path}: {rule}")


def unique_fields(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("duplicate field in the inventory")
        value[key] = item
    return value


def utc_now():
    return datetime.now(timezone.utc)


def timestamp(path, field, value):
    if value is None:
        return None
    if not isinstance(value, str) or not TIMESTAMP.fullmatch(value):
        raise InventoryRefused(path, f"{field} must be an RFC 3339 UTC timestamp such as 2026-03-02T09:00:00Z")
    try:
        return datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
    except ValueError as error:
        raise InventoryRefused(path, f"{field} is not a real date and time") from error


def check_record(record, index, registered, now, evidence=None):
    """Refuse one call path that breaks a rule; returns it unchanged. `registered` is the set of workload SPIFFE IDs.

    `evidence` maps SHA-256 hex to the bytes of a saved proxy log; when given, a path past parallel must cite a digest in it
    and `peer_id_evidence.check_path` must accept that log for the path.
    """
    path = record.get("name") if isinstance(record, dict) and isinstance(record.get("name"), str) else f"call_paths[{index}]"
    if not isinstance(record, dict):
        raise InventoryRefused(path, "a call path must be an object")
    extra, missing = sorted(set(record) - set(RECORD_FIELDS.split())), sorted(set(RECORD_FIELDS.split()) - set(record))
    if extra:
        raise InventoryRefused(path, "unsupported field " + ", ".join(extra) + "; the inventory never stores a credential value")
    if missing:
        raise InventoryRefused(path, "missing field " + ", ".join(missing))
    if not NAME.fullmatch(record["name"]):
        raise InventoryRefused(path, "name must be lowercase letters, digits and hyphens")
    for field in ["owner", "caller", "callee"]:
        if not isinstance(record[field], str) or not record[field].strip() or len(record[field]) > 128:
            raise InventoryRefused(path, f"{field} must be a non-empty name of at most 128 characters")
    credential = record["credential"]
    if not isinstance(credential, dict) or set(credential) != {"kind", "locator"}:
        raise InventoryRefused(path, "credential must hold exactly kind and locator; the inventory never stores a credential value")
    if credential["kind"] not in KINDS:
        raise InventoryRefused(path, "credential kind must be one of " + ", ".join(sorted(KINDS)))
    locator = credential["locator"]
    if not isinstance(locator, str) or not LOCATOR.fullmatch(locator) or VALUE_SHAPED.search(locator):
        raise InventoryRefused(path, "credential locator must be a scheme:reference to where the credential lives, not its value")
    state = record["state"]
    if state not in STATES:
        raise InventoryRefused(path, "state must be one of " + ", ".join(STATES))
    started = timestamp(path, "enforcement_started_at", record["enforcement_started_at"])
    retired = timestamp(path, "retired_at", record["retired_at"])
    spiffe_id, rank = record["caller_spiffe_id"], STATES.index(state)
    if rank >= 1 and not spiffe_id:
        raise InventoryRefused(path, f"state {state} requires caller_spiffe_id")
    if rank < 1 and spiffe_id is not None:
        raise InventoryRefused(path, "state static must not name a caller_spiffe_id")
    if rank >= 2 and started is None:
        raise InventoryRefused(path, f"state {state} requires enforcement_started_at")
    if rank < 2 and started is not None:
        raise InventoryRefused(path, f"state {state} must not carry enforcement_started_at")
    digest = record["peer_id_evidence_sha256"]
    if rank >= 2 and (not isinstance(digest, str) or not DIGEST.fullmatch(digest)):
        raise InventoryRefused(path, f"state {state} requires peer_id_evidence_sha256, the lowercase SHA-256 printed by peer_id_evidence.py")
    if rank < 2 and digest is not None:
        raise InventoryRefused(path, f"state {state} must not carry peer_id_evidence_sha256")
    if rank >= 3 and retired is None:
        raise InventoryRefused(path, "state retired requires retired_at")
    if rank < 3 and retired is not None:
        raise InventoryRefused(path, f"state {state} must not carry retired_at")
    if rank >= 1 and (not isinstance(spiffe_id, str) or spiffe_id not in registered):
        raise InventoryRefused(path, "caller_spiffe_id is not a workload registered in registrations.json")
    for field, value in [("enforcement_started_at", started), ("retired_at", retired)]:
        if value is not None and value > now:
            raise InventoryRefused(path, f"{field} is in the future")
    if retired is not None and retired - started < timedelta(seconds=CA_TTL_SECONDS):
        raise InventoryRefused(path, f"retired_at is less than one full CA rotation ({CA_TTL_SECONDS} seconds) after enforcement_started_at")
    if rank >= 2 and evidence is not None:
        confirm_evidence(record, digest, evidence)
    return record


def confirm_evidence(record, digest, evidence):
    from peer_id_evidence import EvidenceRefused, check_path, parse_log  # peer_id_evidence imports this module
    if digest not in evidence:
        raise InventoryRefused(record["name"], "peer_id_evidence_sha256 matches no file in the evidence directory")
    try:
        check_path(record, parse_log(evidence[digest]))
    except EvidenceRefused as error:
        raise InventoryRefused(record["name"], f"the evidence file no longer passes peer_id_evidence: {error}") from error


def summarize(inventory, registered, now, evidence=None):
    """Validate the whole inventory and return the counts; raises InventoryRefused naming the call path and the rule."""
    if not isinstance(inventory, dict) or set(inventory) != {"call_paths"} or not isinstance(inventory["call_paths"], list) or not inventory["call_paths"]:
        raise InventoryRefused("inventory", "expected an object with a non-empty call_paths list")
    names = set()
    for index, record in enumerate(inventory["call_paths"]):
        check_record(record, index, registered, now, evidence)
        if record["name"] in names:
            raise InventoryRefused(record["name"], "call path name is not unique")
        names.add(record["name"])
    records = inventory["call_paths"]
    counts = {state: sum(record["state"] == state for record in records) for state in STATES}
    return {
        "inventory_size": len(records), "by_state": counts,
        "retired": {"count": counts["retired"], "of": len(records)},
        "static_paths": sorted(record["name"] for record in records if record["state"] == "static"),
        "static_credential_still_live": sorted(record["name"] for record in records if record["state"] in ("static", "parallel")),
    }


def workload_identities(registrations):
    """SPIFFE IDs of the workload entries in a rendered registrations.json: every entry not parented by the server itself."""
    return {entry["spiffe_id"] for entry in registrations["entries"] if not entry["parent_id"].endswith("/spire/server")}


def evidence_files(directory):
    """SHA-256 hex to bytes for every regular file directly inside `directory`."""
    return {hashlib.sha256(data).hexdigest(): data for data in (path.read_bytes() for path in sorted(directory.iterdir()) if path.is_file())}


def main(argv=None, clock=utc_now):
    parser = argparse.ArgumentParser(description="Validate the static credential inventory against the rendered registrations and print the retirement counts.")
    parser.add_argument("--inventory", type=Path, required=True, help="operator-owned inventory JSON")
    parser.add_argument("--registrations", type=Path, required=True, help="rendered registrations.json")
    parser.add_argument("--evidence-dir", type=Path, help="directory of saved proxy logs; each enforced or retired path's digest must match one and pass peer_id_evidence again")
    args = parser.parse_args(argv)
    try:
        inventory = json.loads(args.inventory.read_text(), object_pairs_hook=unique_fields)
        registered = workload_identities(json.loads(args.registrations.read_text(), object_pairs_hook=unique_fields))
        summary = summarize(inventory, registered, clock(), evidence_files(args.evidence_dir) if args.evidence_dir else None)
    except InventoryRefused as error:
        parser.exit(1, f"inventory refused: {error}\n")
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        parser.exit(1, f"inventory refused: input is not usable: {error}\n")
    sys.stdout.write(json.dumps(summary, indent=2, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
