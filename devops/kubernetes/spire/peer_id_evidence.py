"""Refuse a rollout to enforcement unless the proxy's saved log shows the expected peer SPIFFE IDs.

The pilot runs the egress proxy in permissive mode (`dry_run`, which forwards
a caller with no permit and audits it) while the old secret still works. The
operator saves the proxy's JSON log lines, then this tool checks, for each
named call path or every path in state `parallel`:

- at least one `peer_connection_accepted` event names the path's
  `caller_spiffe_id` in `peer.spiffe_id`;
- no `egress_would_deny` event names it.

Peer IDs seen in either event that no inventory path expects are printed, and
refused under `--strict`. The tool prints the SHA-256 of the log file it
accepted; the operator records it as `peer_id_evidence_sha256` on the call
path, and `credential_inventory.py` accepts state `enforced` only with it.

Log shape: the proxy writes one JSON object per line through
`tracing_subscriber`'s JSON formatter (trogon-telemetry `init_logger`), the
event's own fields nested under `fields`:
{"level", "fields": {"event": "peer_connection_accepted", "peer", "peer.spiffe_id",
"credential_source", "message"}, "target", ...} and
{"level": "WARN", "fields": {"event": "egress_would_deny", "peer.spiffe_id",
"agent", "destination", "mode", "message"}, ...}. A line that is not such an
object is refused with its line number, never skipped, because a skipped line
could be a would-deny event. The log is the proxy's side only: it proves what
the proxy saw, not what the callee saw.
"""

import argparse
import hashlib
import json
import sys
from collections import Counter
from pathlib import Path

from credential_inventory import InventoryRefused, summarize, unique_fields, utc_now, workload_identities

ACCEPTED = "peer_connection_accepted"
WOULD_DENY = "egress_would_deny"
PEER_ID_FIELD = "peer.spiffe_id"


class EvidenceRefused(ValueError):
    pass


def digest_of(data):
    return hashlib.sha256(data).hexdigest()


def parse_log(data):
    """Return (accepted, would_deny), each a list of (line number, peer SPIFFE ID, credential source or None).

    Every non-blank line must be a JSON object with an object `fields`; any
    other line, and any of the two events without a text `peer.spiffe_id`,
    raises EvidenceRefused naming the line.
    """
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as error:
        raise EvidenceRefused(f"the log is not UTF-8 text: {error}") from error
    accepted, would_deny = [], []
    for number, line in enumerate(text.split("\n"), 1):
        if not line.strip():
            continue
        try:
            record = json.loads(line, object_pairs_hook=unique_fields)
        except ValueError as error:
            raise EvidenceRefused(f"line {number} is not a JSON log record ({error}); refusing rather than risk skipping a would-deny event") from error
        if not isinstance(record, dict) or not isinstance(record.get("fields"), dict):
            raise EvidenceRefused(f"line {number} is not a log record with a `fields` object")
        fields = record["fields"]
        event = fields.get("event")
        if event not in (ACCEPTED, WOULD_DENY):
            continue
        peer = fields.get(PEER_ID_FIELD)
        if not isinstance(peer, str) or not peer:
            raise EvidenceRefused(f"line {number} is a {event} event without a text {PEER_ID_FIELD}")
        if event == ACCEPTED:
            source = fields.get("credential_source")
            accepted.append((number, peer, source if isinstance(source, str) else None))
        else:
            would_deny.append((number, peer, None))
    return accepted, would_deny


def check_path(record, events):
    """Refuse one call path that lacks accepted-connection evidence or is named by a would-deny event; return its facts."""
    accepted, would_deny = events
    name, expected = record["name"], record["caller_spiffe_id"]
    if not expected:
        raise EvidenceRefused(f"{name}: the call path names no caller_spiffe_id, so there is no peer ID to expect")
    denied = [number for number, peer, _ in would_deny if peer == expected]
    if denied:
        raise EvidenceRefused(f"{name}: {len(denied)} egress_would_deny event(s) name {expected} (first at line {denied[0]})")
    seen = Counter(source or "unknown" for _, peer, source in accepted if peer == expected)
    if not seen:
        raise EvidenceRefused(f"{name}: no peer_connection_accepted event names {expected}")
    return {"name": name, "caller_spiffe_id": expected, "accepted_connections": sum(seen.values()),
            "accepted_by_credential_source": dict(sorted(seen.items())), "would_deny_events": 0}


def selected(inventory, names):
    records = inventory["call_paths"]
    if not names:
        chosen = [record for record in records if record["state"] == "parallel"]
        if not chosen:
            raise EvidenceRefused("no call path is in state parallel; name one with --path")
        return chosen
    chosen = []
    for name in names:
        found = [record for record in records if record["name"] == name]
        if not found:
            raise EvidenceRefused(f"{name}: no such call path in the inventory")
        chosen.append(found[0])
    return chosen


def unexpected_peers(inventory, events):
    expected = {record["caller_spiffe_id"] for record in inventory["call_paths"] if record["caller_spiffe_id"]}
    return sorted({peer for part in events for _, peer, _ in part} - expected)


def verify(inventory, data, names, strict):
    """Return the accepted facts for the selected paths; the inventory must already be validated."""
    events = parse_log(data)
    paths = [check_path(record, events) for record in selected(inventory, names)]
    unexpected = unexpected_peers(inventory, events)
    if strict and unexpected:
        raise EvidenceRefused("peer IDs observed that no inventory path expects: " + ", ".join(unexpected))
    return {"verified": True, "evidence_sha256": digest_of(data), "paths": paths, "unexpected_peer_ids": unexpected}


def main(argv=None, clock=None):
    parser = argparse.ArgumentParser(description="Refuse unless the saved proxy log shows each call path's expected peer SPIFFE ID accepted and never would-denied.")
    parser.add_argument("--inventory", type=Path, required=True, help="operator-owned inventory JSON")
    parser.add_argument("--registrations", type=Path, required=True, help="rendered registrations.json")
    parser.add_argument("--proxy-log", type=Path, required=True, help="saved JSON log lines of the egress proxy")
    parser.add_argument("--path", action="append", default=[], help="call path name; repeatable; default every path in state parallel")
    parser.add_argument("--strict", action="store_true", help="also refuse when a peer ID is observed that no inventory path expects")
    args = parser.parse_args(argv)
    try:
        inventory = json.loads(args.inventory.read_text(), object_pairs_hook=unique_fields)
        registered = workload_identities(json.loads(args.registrations.read_text(), object_pairs_hook=unique_fields))
        summarize(inventory, registered, (clock or utc_now)())
        facts = verify(inventory, args.proxy_log.read_bytes(), args.path, args.strict)
    except EvidenceRefused as error:
        parser.exit(1, f"peer ID evidence refused: {error}\n")
    except InventoryRefused as error:
        parser.exit(1, f"peer ID evidence refused: inventory refused: {error}\n")
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        parser.exit(1, f"peer ID evidence refused: input is not usable: {error}\n")
    sys.stdout.write(json.dumps(facts, indent=2, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
