#!/usr/bin/env python3
"""Verify public connector fixtures, not a production HTTP parser or relay.

Requires Python 3.11+ and the installed cryptography package. The deliberately
narrow signature grammar accepts this fixture profile only. Recovery is a
reference transition model, not distributed-store or network acceptance.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import re
import sys
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey


class Failure(StrEnum):
    HEADER_FORMAT = "HEADER_FORMAT"
    SIGNATURE_INPUT_FORMAT = "SIGNATURE_INPUT_FORMAT"
    SIGNATURE_FORMAT = "SIGNATURE_FORMAT"
    UNSIGNED_COMPONENT = "UNSIGNED_COMPONENT"
    ALGORITHM_REJECTED = "ALGORITHM_REJECTED"
    KEY_ID_MISMATCH = "KEY_ID_MISMATCH"
    PROOF_FUTURE = "PROOF_FUTURE"
    PROOF_EXPIRED = "PROOF_EXPIRED"
    PROOF_LIFETIME = "PROOF_LIFETIME"
    GRANT_EXPIRY = "GRANT_EXPIRY"
    SIGNATURE_INVALID = "SIGNATURE_INVALID"
    CONTENT_DIGEST_FORMAT = "CONTENT_DIGEST_FORMAT"
    BODY_DIGEST_MISMATCH = "BODY_DIGEST_MISMATCH"
    CONTEXT_FORMAT = "CONTEXT_FORMAT"
    CONTEXT_TOO_LARGE = "CONTEXT_TOO_LARGE"
    CONTEXT_UNKNOWN_FIELD = "CONTEXT_UNKNOWN_FIELD"
    CONTEXT_DUPLICATE_FIELD = "CONTEXT_DUPLICATE_FIELD"
    CONTEXT_WIRE_TYPE = "CONTEXT_WIRE_TYPE"
    CONTEXT_TRUNCATED = "CONTEXT_TRUNCATED"
    CONTEXT_MISSING_FIELD = "CONTEXT_MISSING_FIELD"
    CONTEXT_STRING = "CONTEXT_STRING"
    GRANT_BINDING_MISMATCH = "GRANT_BINDING_MISMATCH"
    NONCE_MISMATCH = "NONCE_MISMATCH"
    TRANSPORT_PROFILE_REJECTED = "TRANSPORT_PROFILE_REJECTED"


class Rejected(Exception):
    def __init__(self, reason: Failure):
        self.reason = reason


def b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode("ascii")


def decode_b64url(value: str) -> bytes:
    if not re.fullmatch(r"[A-Za-z0-9_-]*", value):
        raise Rejected(Failure.CONTEXT_FORMAT)
    try:
        result = base64.b64decode(value + "=" * (-len(value) % 4), altchars=b"-_", validate=True)
    except ValueError as exc:
        raise Rejected(Failure.CONTEXT_FORMAT) from exc
    if b64url(result) != value:
        raise Rejected(Failure.CONTEXT_FORMAT)
    return result


def public_jwk_thumbprint(public_jwk: dict) -> str:
    if set(public_jwk) != {"crv", "kty", "x"}:
        raise ValueError("fixture public JWK must contain only crv, kty and x")
    if public_jwk["crv"] != "Ed25519" or public_jwk["kty"] != "OKP":
        raise ValueError("fixture key must be public Ed25519 OKP")
    canonical = json.dumps(public_jwk, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    return b64url(hashlib.sha256(canonical).digest())


def normalized_headers(request: dict) -> dict[str, str]:
    result = {}
    for name, value in request["headers"]:
        key = name.lower()
        if not re.fullmatch(r"[a-z0-9-]+", key) or key in result or "\r" in value or "\n" in value:
            raise Rejected(Failure.HEADER_FORMAT)
        result[key] = value.strip(" \t")
    return result


def signature_input(value: str) -> tuple[list[str], dict[str, str | int], str]:
    match = re.fullmatch(r'connection=(\((?:"[@a-z0-9-]+" ?)+\))((?:;[a-z]+=(?:"[^"\\\r\n]*"|[0-9]+))+)', value)
    if match is None:
        raise Rejected(Failure.SIGNATURE_INPUT_FORMAT)
    names = re.findall(r'"([@a-z0-9-]+)"', match[1])
    if len(set(names)) != len(names):
        raise Rejected(Failure.SIGNATURE_INPUT_FORMAT)
    parameters = {}
    for key, encoded in re.findall(r';([a-z]+)=("[^"\\\r\n]*"|[0-9]+)', match[2]):
        if key in parameters:
            raise Rejected(Failure.SIGNATURE_INPUT_FORMAT)
        parameters[key] = encoded[1:-1] if encoded.startswith('"') else int(encoded)
    if set(parameters) != {"created", "expires", "nonce", "keyid", "alg"}:
        raise Rejected(Failure.SIGNATURE_INPUT_FORMAT)
    if not isinstance(parameters["created"], int) or not isinstance(parameters["expires"], int):
        raise Rejected(Failure.SIGNATURE_INPUT_FORMAT)
    return names, parameters, value.removeprefix("connection=")


def signature_base(request: dict, headers: dict, components: list[str], parameters: str) -> bytes:
    derived = {"@method": request["method"], "@target-uri": request["target_uri"]}
    lines = []
    for name in components:
        value = derived.get(name) if name.startswith("@") else headers.get(name)
        if value is None:
            raise Rejected(Failure.UNSIGNED_COMPONENT)
        lines.append(f'"{name}": {value}')
    lines.append(f'"@signature-params": {parameters}')
    return "\n".join(lines).encode("utf-8")


SHAPES = {
    "context": {1: ("binding", "binding"), 2: ("invocation", "string")},
    "binding": {1: ("grant", "grant"), 2: ("id", "string")},
    "grant": {2: ("id", "string"), 3: ("parent", "parent")},
    "parent": {1: ("project", "string")},
    "string": {1: ("value", None)},
}


def wire_varint(data: bytes, offset: int) -> tuple[int, int]:
    value = 0
    for shift in range(0, 70, 7):
        if offset >= len(data):
            raise Rejected(Failure.CONTEXT_TRUNCATED)
        part = data[offset]
        offset += 1
        if shift == 63 and part > 1:
            raise Rejected(Failure.CONTEXT_FORMAT)
        value |= (part & 127) << shift
        if not part & 128:
            return value, offset
    raise Rejected(Failure.CONTEXT_FORMAT)


def wire_message(data: bytes, shape: str) -> dict:
    expected = SHAPES[shape]
    fields = {}
    offset = 0
    while offset < len(data):
        tag, offset = wire_varint(data, offset)
        number, wire_type = tag >> 3, tag & 7
        if number not in expected:
            raise Rejected(Failure.CONTEXT_UNKNOWN_FIELD)
        name, child = expected[number]
        if name in fields:
            raise Rejected(Failure.CONTEXT_DUPLICATE_FIELD)
        if wire_type != 2:
            raise Rejected(Failure.CONTEXT_WIRE_TYPE)
        size, offset = wire_varint(data, offset)
        end = offset + size
        if end > len(data):
            raise Rejected(Failure.CONTEXT_TRUNCATED)
        raw = data[offset:end]
        offset = end
        if child is None:
            try:
                text = raw.decode("utf-8")
            except UnicodeDecodeError as exc:
                raise Rejected(Failure.CONTEXT_STRING) from exc
            if not text:
                raise Rejected(Failure.CONTEXT_STRING)
            fields[name] = text
        else:
            fields[name] = wire_message(raw, child)
    if len(fields) != len(expected):
        raise Rejected(Failure.CONTEXT_MISSING_FIELD)
    return fields


def verify_signature_vector(artifact: dict, vector: dict) -> dict:
    signature_valid = None
    try:
        request = vector["request"]
        headers = normalized_headers(request)
        components, parameters, serialized = signature_input(headers["signature-input"])
        base = signature_base(request, headers, components, serialized)
        if base.decode("utf-8") != vector["verification_base_utf8"]:
            raise ValueError("reconstructed signature base disagrees with the artifact")
        encoded_signature = re.fullmatch(r"connection=:([A-Za-z0-9+/]*={0,2}):", headers["signature"])
        if encoded_signature is None:
            raise Rejected(Failure.SIGNATURE_FORMAT)
        try:
            signature = base64.b64decode(encoded_signature[1], validate=True)
        except ValueError as exc:
            raise Rejected(Failure.SIGNATURE_FORMAT) from exc
        public_key = Ed25519PublicKey.from_public_bytes(decode_b64url(artifact["public_jwk"]["x"]))
        try:
            public_key.verify(signature, base)
            signature_valid = True
        except InvalidSignature:
            signature_valid = False
        required = {"@method", "@target-uri", "content-digest", "trogon-connection-context", "accept-encoding", "content-type"}
        forwarded = set(headers) - {"signature-input", "signature"}
        if not required.issubset(components) or not forwarded.issubset(components):
            raise Rejected(Failure.UNSIGNED_COMPONENT)
        if parameters["alg"] != "ed25519":
            raise Rejected(Failure.ALGORITHM_REJECTED)
        if parameters["keyid"] != public_jwk_thumbprint(artifact["public_jwk"]):
            raise Rejected(Failure.KEY_ID_MISMATCH)
        created, expires, now = parameters["created"], parameters["expires"], vector["verify_at"]
        if created > now:
            raise Rejected(Failure.PROOF_FUTURE)
        if expires <= now:
            raise Rejected(Failure.PROOF_EXPIRED)
        if not 0 < expires - created <= 60:
            raise Rejected(Failure.PROOF_LIFETIME)
        if expires > vector["grant_expires_at"]:
            raise Rejected(Failure.GRANT_EXPIRY)
        if not signature_valid:
            raise Rejected(Failure.SIGNATURE_INVALID)
        body = base64.b64decode(request["body_base64"], validate=True)
        digest = re.fullmatch(r"sha-256=:([A-Za-z0-9+/]*={0,2}):", headers["content-digest"])
        if digest is None:
            raise Rejected(Failure.CONTENT_DIGEST_FORMAT)
        try:
            supplied_digest = base64.b64decode(digest[1], validate=True)
        except ValueError as exc:
            raise Rejected(Failure.CONTENT_DIGEST_FORMAT) from exc
        if supplied_digest != hashlib.sha256(body).digest():
            raise Rejected(Failure.BODY_DIGEST_MISMATCH)
        context_bytes = decode_b64url(headers["trogon-connection-context"])
        if len(context_bytes) > 4096:
            raise Rejected(Failure.CONTEXT_TOO_LARGE)
        context = wire_message(context_bytes, "context")
        grant = context["binding"]["grant"]
        attested = artifact["attested_binding"]
        if (grant["id"]["value"], grant["parent"]["project"]["value"], context["binding"]["id"]["value"]) != (attested["grant_id"], attested["grant_project"], attested["binding_id"]):
            raise Rejected(Failure.GRANT_BINDING_MISMATCH)
        if parameters["nonce"] != context["invocation"]["value"]:
            raise Rejected(Failure.NONCE_MISMATCH)
        if headers["accept-encoding"] != "identity":
            raise Rejected(Failure.TRANSPORT_PROFILE_REJECTED)
        return {"outcome": "accept", "reason": None, "signature_valid": signature_valid}
    except Rejected as exc:
        return {"outcome": "reject", "reason": exc.reason.value, "signature_valid": signature_valid}


class Phase(StrEnum):
    CLAIMED = "CLAIMED"
    AUDITED = "AUDITED"
    SEND_INTENT = "SEND_INTENT"
    COMPLETED = "COMPLETED"
    NOT_SENT = "NOT_SENT"
    OUTCOME_UNKNOWN = "OUTCOME_UNKNOWN"


TERMINAL = {Phase.COMPLETED, Phase.NOT_SENT, Phase.OUTCOME_UNKNOWN}


@dataclass
class Receipt:
    phase: Phase
    owner: str
    fence: int = 1
    owner_alive: bool = True
    send_permit: bool = False
    sends: int = 0
    outbox_pending: bool = False
    reserved_bytes: int = 256
    terminal_at: int | None = None


class RecoveryModel:
    """One grant, deterministic event order, no network or durable adapter."""

    def __init__(self, initial: dict):
        self.epoch = initial.get("epoch", 1)
        self.expires_at = initial.get("expires_at", 100)
        self.grant_id = initial.get("grant_id", "fixture-grant")
        self.key_id = initial.get("key_id", "fixture-key")
        self.closed = False
        self.available = True
        self.receipts: dict[str, Receipt] = {}
        self.acceptance_events: set[str] = set()
        self.completion_events: set[str] = set()
        self.retired_keys: set[str] = set()
        self.retired_receipts: dict[str, dict[str, Receipt]] = {}
        self.known_grants = {self.grant_id}
        self.total_sends = 0
        self.completion_slot_limit = initial.get("completion_slot_limit", 1024)
        self.completion_metadata_limit = initial.get("completion_metadata_limit", 8 * 1024 * 1024)
        self.completion_age_limit = initial.get("completion_age_limit", 60)
        if not (0 < self.completion_slot_limit <= 1024 and 0 < self.completion_metadata_limit <= 8 * 1024 * 1024 and 0 < self.completion_age_limit <= 60):
            raise ValueError("fixture operator limits may only lower tenant defaults")

    def outstanding(self) -> list[Receipt]:
        receipts = list(self.receipts.values())
        for retired in self.retired_receipts.values():
            receipts.extend(retired.values())
        return [receipt for receipt in receipts if receipt.phase not in TERMINAL or receipt.outbox_pending]

    def apply(self, event: dict) -> str:
        op = event["op"]
        invocation = event.get("invocation", "request-a")
        now = event.get("now", 1)
        if op == "availability":
            self.available = event["available"]
            return "AVAILABLE" if self.available else "UNAVAILABLE"
        if op == "restore":
            if event["epoch"] <= self.epoch:
                return "EPOCH_NOT_ADVANCING"
            self.retired_keys.add(self.key_id)
            self.epoch = event["epoch"]
            self.closed = True
            for receipt in self.receipts.values():
                receipt.fence += 1
                receipt.owner_alive = False
                receipt.send_permit = False
            return "RESTORE_FENCED"
        if op == "fresh_grant":
            if event["epoch"] != self.epoch or event["grant_id"] in self.known_grants or event["key_id"] in self.retired_keys:
                return "FRESH_AUTHORITY_REQUIRED"
            if not self.closed or any(receipt.phase not in TERMINAL or receipt.outbox_pending for receipt in self.receipts.values()):
                return "RECOVERY_REQUIRED"
            self.retired_receipts[self.grant_id] = self.receipts
            self.grant_id, self.key_id = event["grant_id"], event["key_id"]
            self.known_grants.add(self.grant_id)
            self.expires_at = event["expires_at"]
            self.receipts = {}
            self.closed = False
            return "FRESH_GRANT_INSTALLED"
        if event.get("grant_id", self.grant_id) != self.grant_id:
            return "GRANT_MISMATCH"
        if event.get("key_id", self.key_id) != self.key_id:
            return "KEY_MISMATCH"
        if op == "revoke":
            self.closed = True
            return "GRANT_CLOSED"
        if op == "expire":
            if now < self.expires_at:
                return "NOT_EXPIRED"
            self.closed = True
            return "GRANT_CLOSED"
        if op == "renew":
            if not self.available:
                return "STORE_UNAVAILABLE"
            if self.closed or now >= self.expires_at or event.get("epoch", self.epoch) != self.epoch:
                return "GRANT_CLOSED"
            if event["expires_at"] <= self.expires_at:
                return "RENEWAL_NOT_EXTENDING"
            self.expires_at = event["expires_at"]
            return "RENEWED"
        if op == "claim":
            if not self.available:
                return "STORE_UNAVAILABLE"
            if event.get("epoch", self.epoch) != self.epoch:
                return "EPOCH_MISMATCH"
            if self.closed or now >= self.expires_at:
                return "GRANT_CLOSED"
            if invocation in self.receipts:
                return "REPLAY"
            outstanding = self.outstanding()
            if any(receipt.outbox_pending and receipt.terminal_at is not None and now - receipt.terminal_at >= self.completion_age_limit for receipt in outstanding):
                return "TENANT_BACKLOG_STALE"
            if len(outstanding) >= self.completion_slot_limit:
                return "TENANT_BACKLOG_FULL"
            charge = event.get("metadata_bytes", 256)
            if not isinstance(charge, int) or charge <= 0:
                raise ValueError("fixture metadata reservation must be positive")
            if sum(receipt.reserved_bytes for receipt in outstanding) + charge > self.completion_metadata_limit:
                return "TENANT_METADATA_FULL"
            self.receipts[invocation] = Receipt(Phase.CLAIMED, event["worker"], reserved_bytes=charge)
            return "CLAIMED"
        receipt = self.receipts.get(invocation)
        if receipt is None:
            return "NO_RECEIPT"
        if op == "crash":
            receipt.owner_alive = False
            receipt.send_permit = False
            return "OWNER_CRASHED"
        if op == "recover":
            if not self.available:
                return "STORE_UNAVAILABLE"
            if receipt.phase in TERMINAL:
                return "TERMINAL_PRESERVED"
            receipt.fence += 1
            receipt.owner_alive = False
            receipt.send_permit = False
            receipt.phase = Phase.OUTCOME_UNKNOWN if receipt.phase == Phase.SEND_INTENT else Phase.NOT_SENT
            receipt.outbox_pending = True
            receipt.terminal_at = now
            return receipt.phase.value
        if op == "publish_completion":
            if receipt.phase not in TERMINAL or not receipt.outbox_pending:
                return "NO_PENDING_COMPLETION"
            if not event.get("audit_available", True):
                return "AUDIT_UNAVAILABLE"
            self.completion_events.add(self.grant_id + ":" + invocation)
            if not event.get("acknowledged", True):
                return "AUDIT_ACK_LOST"
            receipt.outbox_pending = False
            return "COMPLETION_RECORDED"
        if op == "prune":
            if not self.available:
                return "STORE_UNAVAILABLE"
            if not self.closed or now < self.expires_at + 60 or receipt.phase not in TERMINAL or receipt.outbox_pending:
                return "RETENTION_REQUIRED"
            del self.receipts[invocation]
            return "PRUNED"
        if event.get("worker") != receipt.owner or event.get("fence", 1) != receipt.fence or not receipt.owner_alive:
            return "FENCED"
        if not self.available:
            return "STORE_UNAVAILABLE"
        if op in {"accept_audit", "reconcile_audit"}:
            if receipt.phase != Phase.CLAIMED:
                return "PHASE_REJECTED"
            identity = self.grant_id + ":" + invocation
            if op == "accept_audit":
                if not event.get("audit_available", True):
                    return "AUDIT_UNAVAILABLE"
                self.acceptance_events.add(identity)
                if not event.get("acknowledged", True):
                    return "AUDIT_ACK_LOST"
            elif identity not in self.acceptance_events:
                return "ACCEPTANCE_NOT_CONFIRMED"
            receipt.phase = Phase.AUDITED
            return "AUDITED"
        if op == "send_intent":
            if self.closed or now >= self.expires_at or event.get("epoch", self.epoch) != self.epoch:
                return "AUTHORITY_CLOSED"
            if receipt.phase != Phase.AUDITED:
                return "PHASE_REJECTED"
            receipt.phase = Phase.SEND_INTENT
            receipt.send_permit = event.get("acknowledged", True)
            return "SEND_INTENT" if receipt.send_permit else "INTENT_ACK_LOST"
        if op == "send":
            if self.closed or now >= self.expires_at or event.get("epoch", self.epoch) != self.epoch:
                return "AUTHORITY_CLOSED"
            if receipt.phase != Phase.SEND_INTENT or not receipt.send_permit:
                return "NO_SEND_PERMIT"
            receipt.send_permit = False
            receipt.sends += 1
            self.total_sends += 1
            return "PROVIDER_SENT"
        if op == "complete":
            if receipt.phase != Phase.SEND_INTENT:
                return "PHASE_REJECTED"
            if not event.get("committed", True):
                return "TERMINAL_ACK_LOST"
            receipt.phase = Phase.COMPLETED
            receipt.send_permit = False
            receipt.outbox_pending = True
            receipt.terminal_at = now
            return "COMPLETED" if event.get("acknowledged", True) else "TERMINAL_ACK_LOST"
        raise ValueError("unknown recovery fixture operation")

    def snapshot(self) -> dict:
        return {
            "epoch": self.epoch,
            "expires_at": self.expires_at,
            "closed": self.closed,
            "provider_sends": self.total_sends,
            "acceptance_events": len(self.acceptance_events),
            "completion_events": len(self.completion_events),
            "retired_receipts": sum(len(receipts) for receipts in self.retired_receipts.values()),
            "receipts": {key: {"phase": value.phase.value, "fence": value.fence, "sends": value.sends, "outbox_pending": value.outbox_pending} for key, value in self.receipts.items()},
        }


def check_standard_control(control: dict) -> None:
    public_key = Ed25519PublicKey.from_public_bytes(decode_b64url(control["public_key_base64url"]))
    signature = base64.b64decode(control["signature_base64"], validate=True)
    message = control["signature_base_utf8"].encode("utf-8")
    public_key.verify(signature, message)
    try:
        public_key.verify(signature, message + b"\n")
    except InvalidSignature:
        return
    raise ValueError("standard control incorrectly accepted a changed signature base")


def verify(fixtures: Path) -> dict:
    signatures = json.loads((fixtures / "connector-relay-signatures-v1.json").read_text())
    recovery = json.loads((fixtures / "connector-relay-recovery-v1.json").read_text())
    if signatures["profile"] != "connector-relay-signatures-v1" or recovery["profile"] != "connector-relay-recovery-v1":
        raise ValueError("unsupported fixture profile")
    if recovery["constants"] != {
        "replay_safety_margin_seconds": 60,
        "maximum_completion_slots_per_tenant": 1024,
        "maximum_completion_metadata_bytes_per_tenant": 8 * 1024 * 1024,
        "maximum_undelivered_completion_age_seconds": 60,
    }:
        raise ValueError("fixture constants disagree with the tenant-wide recovery profile")
    public_jwk_thumbprint(signatures["public_jwk"])
    check_standard_control(signatures["standards_control"])
    signature_results = []
    for vector in signatures["vectors"]:
        actual = verify_signature_vector(signatures, vector)
        signature_results.append({"name": vector["name"], "passed": actual == vector["expected"], "actual": actual, "expected": vector["expected"]})
    recovery_results = []
    for trace in recovery["traces"]:
        model = RecoveryModel(trace.get("initial", {}))
        steps = []
        for event in trace["events"]:
            actual = model.apply(event)
            steps.append({"op": event["op"], "passed": actual == event["expected"], "actual": actual, "expected": event["expected"]})
        snapshot = model.snapshot()
        recovery_results.append({"name": trace["name"], "passed": all(step["passed"] for step in steps) and snapshot == trace["expected_final"], "steps": steps, "actual_final": snapshot, "expected_final": trace["expected_final"]})
    passed = all(item["passed"] for item in signature_results + recovery_results)
    return {
        "passed": passed,
        "notice": "Conformance fixture/reference-model checks only; no production relay, storage adapter, distributed race or deployment acceptance.",
        "standards_control": "RFC9421 B.2.6 valid base accepted and changed base rejected",
        "signature_vectors": len(signature_results),
        "positive_signature_vectors": sum(vector["expected"]["outcome"] == "accept" for vector in signatures["vectors"]),
        "negative_signature_vectors": sum(vector["expected"]["outcome"] == "reject" for vector in signatures["vectors"]),
        "recovery_traces": len(recovery_results),
        "recovery_steps": sum(len(trace["events"]) for trace in recovery["traces"]),
        "tenant_wide_recovery_defaults": recovery["constants"],
        "signature_results": signature_results,
        "recovery_results": recovery_results,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixtures-dir", type=Path, default=Path(__file__).resolve().parents[1] / "docs/public/connector-fixtures")
    parser.add_argument("--json", action="store_true", help="print complete per-vector results")
    args = parser.parse_args()
    try:
        report = verify(args.fixtures_dir)
    except (KeyError, ValueError, InvalidSignature, OSError) as exc:
        print(f"fixture verification failed: {type(exc).__name__}: {exc}", file=sys.stderr)
        return 1
    if args.json:
        print(json.dumps(report, indent=2))
    else:
        print(json.dumps({key: value for key, value in report.items() if not key.endswith("_results")}, indent=2))
        if not report["passed"]:
            for category in ("signature_results", "recovery_results"):
                for result in report[category]:
                    if not result["passed"]:
                        print(json.dumps(result, indent=2), file=sys.stderr)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
