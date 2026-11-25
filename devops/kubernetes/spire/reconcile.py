"""Plan and apply the registration entry changes on a SPIRE server.

`registrations.json` only ever creates entries. This module compares the
rendered entries with the live `spire-server entry show -output json` listing,
plans the difference, and can apply it so that a registration removed from the
deployment input is also removed from the server. The server is reached through
a configurable command (for example `kubectl exec -i ... -- /opt/spire/bin/spire-server`)
run by an injectable runner, so the planner and the apply order are testable
without a server.

Listing shape (SPIRE 1.16.0, verified against a live server and kept in
testdata/): {"entries": [{"id", "spiffe_id": {"trust_domain", "path"},
"parent_id": {...}, "selectors": [{"type", "value"}], "x509_svid_ttl",
"jwt_svid_ttl", "wit_svid_ttl", "federates_with", "admin", "downstream",
"expires_at", "dns_names", "revision_number", "store_svid", "hint",
"created_at"}], "next_page_token": ""}. 64-bit integers are JSON strings, empty
lists and false flags are emitted, and the CLI merges every page itself, so a
non-empty token is refused as an incomplete listing. Selectors are returned in
sorted order, not the order they were created in.

Bundle listing shape (`spire-server bundle list -output json`, same server):
{"bundles": [{"trust_domain", "x509_authorities", "jwt_authorities",
"wit_authorities", "refresh_hint", "sequence_number"}], "next_page_token": ""}.
Only federated bundles are listed, never the server's own. The compiler renders
no federation, so every listed bundle is undeclared. The migration Job leaves
the bundle of its disposable trust domain behind; apply deletes that one with
`bundle delete -mode restrict` and never deletes any other.
"""

import argparse
import hashlib
import json
import re
import shlex
import subprocess
import sys
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path

from profile import MIGRATION_TRUST_DOMAIN, REQUESTED_X509_SVID_TTL_SECONDS

MIGRATION_BUNDLE = "spiffe://" + MIGRATION_TRUST_DOMAIN
ALIAS_PATH = re.compile(r"/platform/agent-group/([a-z0-9-]+)/epoch-[0-9]+(?:/node/[a-z0-9.-]+)?")


PRIVILEGED_GRANT_EXIT = 3


class ReconcileRefused(ValueError):
    pass


class PrivilegedGrant(ReconcileRefused):
    """The server holds entries with `admin` or `downstream` set that this deployment never renders."""


@dataclass(frozen=True)
class Plan:
    """What to create and delete; `steps` is the order in which to do it.

    A changed entry is a delete plus a create, never an in-place update: every
    attribute that grants authority (selectors, parent, TTLs, privilege flags)
    is then matched against exactly one reviewed definition, and the
    superseded entry ID stops being valid, so no partially updated entry can
    exist. The server also rejects an entry that is identical in SPIFFE ID,
    parent and selectors, so the old entry must go before its replacement
    exists. The cost is a short window without that SVID; `replaced` lists the
    affected SPIFFE IDs so an operator can schedule it.

    `swaps` pairs each replacement entry with the IDs it supersedes; `retired`
    holds (ID, is node alias) for entries removed with no replacement. `delete_ttls` holds (ID,
    listed `x509_svid_ttl`) for every deleted entry, 0 when the entry carries none. `create` and `delete` are the
    full sets. `broad` lists every live entry, owned or not, with fewer than two selectors; the
    target is none, and `drifted` is what `--check` turns into an exit status.
    """

    create: list
    delete: list
    replaced: list
    swaps: tuple = ()
    retired: tuple = ()
    delete_ttls: tuple = ()
    broad: tuple = ()
    undeclared_bundles: tuple = ()
    bundles_inspected: bool = False

    def foreign_bundles(self):
        """Stored federated bundles that apply never deletes."""
        return [bundle for bundle in self.undeclared_bundles if bundle != MIGRATION_BUNDLE]

    def drifted(self):
        return bool(self.steps() or self.broad or self.undeclared_bundles)

    def steps(self):
        """Ordered operations, ("create", entry) or ("delete", entry ID).

        New entries first, node aliases ahead of the workloads beneath them, so
        nothing new waits on anything being removed. Each replaced entry is then
        deleted and re-created back to back, so a workload is without an entry
        for one call only. Retirements come last, workloads before the node
        alias that parents them. A rerun after any failure plans from a fresh
        listing, so it resumes where this stopped.
        """
        replacing = {id(entry) for entry, _ in self.swaps}
        steps = [("create", entry) for entry in sorted(
            (entry for entry in self.create if id(entry) not in replacing), key=alias_first)]
        for entry, ids in sorted(self.swaps, key=lambda swap: alias_first(swap[0])):
            steps += [("delete", entry_id) for entry_id in ids] + [("create", entry)]
        steps += [("delete", entry_id) for entry_id, _ in sorted(self.retired, key=lambda item: (item[1], item[0]))]
        steps += [("delete-bundle", bundle) for bundle in self.undeclared_bundles if bundle == MIGRATION_BUNDLE]
        return steps

    def to_json(self):
        return json.dumps({
            "delete": self.delete, "replaced": self.replaced, "create": {"entries": self.create}, "broad_selectors": list(self.broad),
            "bundles_inspected": self.bundles_inspected, "undeclared_bundles": list(self.undeclared_bundles),
            "steps": [f"create {item['spiffe_id']} under {item['parent_id']}" if kind == "create" else f"{kind.replace('-', ' ')} {item}"
                      for kind, item in self.steps()],
        }, indent=2, sort_keys=True) + "\n"


def alias_first(entry):
    return (0 if spiffe_id(entry["parent_id"]).endswith("/spire/server") else 1, entry["spiffe_id"])


def spiffe_id(value):
    if isinstance(value, str):
        return value
    if isinstance(value, dict) and isinstance(value.get("trust_domain"), str) and isinstance(value.get("path", ""), str):
        return "spiffe://" + value["trust_domain"] + value.get("path", "")
    raise ReconcileRefused("entry has an unreadable SPIFFE ID")


def identity(entry):
    """The full reviewed identity of an entry; the server-assigned ID is not part of it."""
    try:
        selectors = frozenset((item["type"], item["value"]) for item in entry["selectors"])
        return (
            spiffe_id(entry["spiffe_id"]), spiffe_id(entry["parent_id"]), selectors,
            int(entry.get("x509_svid_ttl", 0)), int(entry.get("jwt_svid_ttl", 0)), int(entry.get("wit_svid_ttl", 0)),
            bool(entry.get("admin", False)), bool(entry.get("downstream", False)),
            frozenset(entry.get("federates_with", [])), frozenset(entry.get("dns_names", [])),
            bool(entry.get("store_svid", False)), int(entry.get("expires_at", 0)), entry.get("hint", ""),
        )
    except (KeyError, TypeError, ValueError) as error:
        raise ReconcileRefused("entry is not in the expected shape") from error


def slot(entry):
    return spiffe_id(entry["spiffe_id"]), spiffe_id(entry["parent_id"])


def ownership(desired):
    """The namespace this deployment owns: the node alias entries it renders and everything under them.

    An entry is owned when it is a node alias directly under the server whose
    SPIFFE ID is inside `<trust domain>/platform/agent-group/<cluster>/`, or
    when its parent is inside that prefix. This covers the per node aliases of
    every recovery epoch, the single cluster alias of earlier releases, and the
    workload entries registered beneath them, so a retired epoch or alias shape
    is cleaned up, and nothing else on the server is touched.
    """
    found = set()
    for entry in desired:
        parent = spiffe_id(entry["parent_id"])
        if parent.endswith("/spire/server"):
            base = parent.removesuffix("/spire/server")
            match = ALIAS_PATH.fullmatch(spiffe_id(entry["spiffe_id"]).removeprefix(base))
            if not match:
                raise ReconcileRefused("the rendered node alias is outside the platform agent-group namespace")
            found.add((base, match.group(1)))
    if not found:
        raise ReconcileRefused("the rendered set must contain node alias entries")
    if len(found) != 1:
        raise ReconcileRefused("the rendered node aliases must all belong to one cluster")
    base, cluster = found.pop()
    prefix = f"{base}/platform/agent-group/{cluster}/"
    server = base + "/spire/server"

    def owned(entry):
        parent = spiffe_id(entry["parent_id"])
        return parent.startswith(prefix) or (parent == server and spiffe_id(entry["spiffe_id"]).startswith(prefix))

    return base, owned


def bundle_trust_domains(listing):
    """The SPIFFE IDs of the stored federated bundles in `bundle list -output json` output."""
    if not isinstance(listing, dict) or not isinstance(listing.get("bundles"), list):
        raise ReconcileRefused("the bundle listing is not `bundle list -output json` output")
    if listing.get("next_page_token"):
        raise ReconcileRefused("the bundle listing is incomplete")
    found = []
    for bundle in listing["bundles"]:
        if not isinstance(bundle, dict) or not isinstance(bundle.get("trust_domain"), str) or not bundle["trust_domain"]:
            raise ReconcileRefused("bundle without a trust domain")
        found.append("spiffe://" + bundle["trust_domain"].removeprefix("spiffe://"))
    return tuple(sorted(found))


def plan_reconciliation(desired, listing, bundles=None):
    """`bundles` is the parsed `bundle list -output json`; None means bundles were not inspected."""
    base, owned = ownership(desired)
    for entry in desired:
        if entry.get("admin") is not False or entry.get("downstream") is not False:
            raise ReconcileRefused("a rendered entry must set admin and downstream to false")
    if not isinstance(listing, dict) or not isinstance(listing.get("entries"), list):
        raise ReconcileRefused("the listing is not `entry show -output json` output")
    if listing.get("next_page_token"):
        raise ReconcileRefused("the listing is incomplete")
    wanted = {slot(entry): entry for entry in desired}
    if len(wanted) != len(desired):
        raise ReconcileRefused("the rendered set contains duplicate entries")
    current, broad, privileged = {}, [], []
    for entry in listing["entries"]:
        if not isinstance(entry, dict) or not isinstance(entry.get("id"), str) or not entry["id"]:
            raise ReconcileRefused("entry without an ID")
        key = identity(entry)
        if not key[0].startswith(base + "/") or key[1] != base and not key[1].startswith(base + "/"):
            raise ReconcileRefused("the listing belongs to another trust domain")
        if entry.get("admin") or entry.get("downstream"):
            privileged.append(f"{entry['id']} {key[0]} ({'admin' if entry.get('admin') else ''}{' and ' if entry.get('admin') and entry.get('downstream') else ''}{'downstream' if entry.get('downstream') else ''})")
            continue
        if len(key[2]) < 2:
            broad.append({"id": entry["id"], "spiffe_id": key[0], "parent_id": key[1], "selectors": len(key[2])})
        if owned(entry):
            current.setdefault(slot(entry), []).append((entry["id"], key))
    if privileged:
        raise PrivilegedGrant("the server holds admin or downstream entries this deployment did not render: " + "; ".join(privileged))
    create, delete, replaced, swaps, retired = [], [], [], [], []
    listed_ttls = {}
    for entry in listing["entries"]:
        listed_ttls[entry["id"]] = int(entry.get("x509_svid_ttl", 0))
        if listed_ttls[entry["id"]] < 0:
            raise ReconcileRefused("entry with a negative x509_svid_ttl")

    def retire(entry_id, key):
        retired.append((entry_id, key[1].endswith("/spire/server")))
        delete.append(entry_id)

    for key, entry in wanted.items():
        existing = sorted(current.pop(key, []))
        kept = [entry_id for entry_id, found in existing if found == identity(entry)]
        if kept:
            for entry_id, found in existing:
                if entry_id != kept[0]:
                    retire(entry_id, found)
            continue
        create.append(entry)
        if existing:
            replaced.append(key[0])
            ids = [entry_id for entry_id, _ in existing]
            swaps.append((entry, tuple(ids)))
            delete += ids
    for existing in current.values():
        for entry_id, found in existing:
            retire(entry_id, found)
    return Plan(create=create, delete=sorted(delete), replaced=sorted(replaced), swaps=tuple(swaps), retired=tuple(retired),
                delete_ttls=tuple((entry_id, listed_ttls[entry_id]) for entry_id in sorted(delete)), broad=tuple(sorted(broad, key=lambda item: item["id"])),
                undeclared_bundles=() if bundles is None else bundle_trust_domains(bundles), bundles_inspected=bundles is not None)


class ApplyFailed(RuntimeError):
    pass


def run_command(argv, stdin=None):
    """Run one server command; returns (exit status, stdout, stderr)."""
    try:
        done = subprocess.run(argv, input=stdin, capture_output=True, text=True, timeout=120, check=False)
    except (OSError, subprocess.SubprocessError) as error:
        return 127, "", str(error)
    return done.returncode, done.stdout, done.stderr


def server_call(server, runner, arguments, stdin=None, socket_path=None):
    """Run `spire-server <command> <subcommand> [-socketPath path] ...`: the flag belongs after the subcommand, not after the program."""
    argv = [*server, *arguments[:2], *(["-socketPath", socket_path] if socket_path else []), *arguments[2:]]
    status, out, err = runner(argv, stdin)
    if status != 0:
        detail = (err or out).strip().replace("\n", " ")
        raise ApplyFailed(f"`{' '.join(arguments[:2])}` failed with exit status {status}: {detail}")
    return out


def live_listing(server, runner, socket_path=None):
    out = server_call(server, runner, ["entry", "show", "-output", "json"], socket_path=socket_path)
    try:
        return json.loads(out)
    except ValueError as error:
        raise ReconcileRefused("the server listing is not JSON") from error


def live_bundles(server, runner, socket_path=None):
    out = server_call(server, runner, ["bundle", "list", "-output", "json"], socket_path=socket_path)
    try:
        return json.loads(out)
    except ValueError as error:
        raise ReconcileRefused("the bundle listing is not JSON") from error


TIME_FORMAT = "%Y-%m-%dT%H:%M:%SZ"


def utc_now():
    return datetime.now(timezone.utc).strftime(TIME_FORMAT)


def apply_plan(plan, server, runner=run_command, log=lambda line: None, socket_path=None, clock=utc_now):
    """Execute `plan.steps()` against the server, stopping at the first failure.

    Entries are created one at a time from `entry create -data -` and deleted
    one at a time by ID, without `-output json`: with it the CLI exits 0 even
    when the server rejected the entry. Nothing here can set admin or
    downstream; the planner refuses a rendered set that does. The one bundle step
    removes the migration Job's disposable trust domain with `-mode restrict`, so
    the server refuses while any entry federates with it. Each deleted entry is
    logged on one line with the UTC time the server accepted the deletion, the
    entry's X.509-SVID TTL, the UTC time by which the last valid SVID of the
    entry has expired and the time to revoke in seconds, then one summary line
    carries the maximum. The TTL is the one the server listed for the entry,
    or the rendered server default `default_x509_svid_ttl` when the entry
    carries none. The value is an exact upper bound, not an estimate: an SVID
    issued the instant before the deletion is valid for at most its TTL, and
    none can be issued after. Line formats:
    `deleted entry <id> at <UTC> x509_svid_ttl_seconds=<n> last_svid_expires_at=<UTC> time_to_revoke_seconds=<n>`
    and `time to revoke maximum_seconds=<n> entries=<n>`.
    """
    ttls = dict(plan.delete_ttls)
    revoked = []
    steps = plan.steps()
    for number, (kind, item) in enumerate(steps, 1):
        label = f"step {number} of {len(steps)}: " + (f"create {item['spiffe_id']}" if kind == "create" else f"{kind.replace('-', ' ')} {item}")
        log(label)
        try:
            if kind == "create":
                server_call(server, runner, ["entry", "create", "-data", "-"], json.dumps({"entries": [item]}), socket_path)
            elif kind == "delete-bundle":
                server_call(server, runner, ["bundle", "delete", "-id", item, "-mode", "restrict"], socket_path=socket_path)
            else:
                server_call(server, runner, ["entry", "delete", "-entryID", item], socket_path=socket_path)
                deleted_at = clock()
                ttl = ttls.get(item) or REQUESTED_X509_SVID_TTL_SECONDS
                expires = datetime.strptime(deleted_at, TIME_FORMAT).replace(tzinfo=timezone.utc) + timedelta(seconds=ttl)
                revoked.append(ttl)
                log(f"deleted entry {item} at {deleted_at} x509_svid_ttl_seconds={ttl} "
                    f"last_svid_expires_at={expires.strftime(TIME_FORMAT)} time_to_revoke_seconds={ttl}")
        except ApplyFailed as error:
            raise ApplyFailed(f"{label} failed: {error}; rerun to converge from the current state") from error
    log(f"time to revoke maximum_seconds={max(revoked, default=0)} entries={len(revoked)}")


def reconcile(desired, server, runner=run_command, dry_run=False, log=lambda line: None, socket_path=None):
    """Plan against the live listing, apply, and confirm the server converged."""
    plan = plan_reconciliation(desired, live_listing(server, runner, socket_path), live_bundles(server, runner, socket_path))
    log(plan.to_json().rstrip())
    if dry_run:
        return plan
    apply_plan(plan, server, runner, log, socket_path)
    remaining = plan_reconciliation(desired, live_listing(server, runner, socket_path), live_bundles(server, runner, socket_path))
    if remaining.steps():
        raise ApplyFailed("the server did not converge after the plan was applied")
    if remaining.foreign_bundles():
        raise ReconcileRefused("the server stores federated bundles this deployment does not declare and the tool never deletes: "
                               + ", ".join(remaining.foreign_bundles()) + "; review them and remove each with `spire-server bundle delete`")
    return plan


def approve(source, approved):
    """Refuse an apply unless the registrations bytes are the ones a reviewer approved by digest."""
    if not approved:
        raise ReconcileRefused("apply requires --approved-digest, the SHA-256 of the reviewed registrations.json recorded in registration-plan.json")
    if not re.fullmatch(r"[0-9a-f]{64}", approved) or approved != hashlib.sha256(source).hexdigest():
        raise ReconcileRefused("the registrations file does not match --approved-digest; review the rendered file again")


def main(argv=None, runner=run_command):
    parser = argparse.ArgumentParser(description="Plan, or apply, the entry changes between a rendered registrations.json and a live server.")
    parser.add_argument("--registrations", type=Path, required=True, help="rendered registrations.json")
    parser.add_argument("--current", type=Path, help="plan offline from a saved `spire-server entry show -output json`; never changes a server")
    parser.add_argument("--server-command", default="spire-server",
                        help="command that runs spire-server, for example '/opt/spire/bin/spire-server' or "
                             "'kubectl exec -i -n identity-control spire-server-0 -c spire-server -- /opt/spire/bin/spire-server'")
    parser.add_argument("--socket-path", help="admin API socket of the server, passed to every spire-server call after its subcommand "
                        "(the rendered server uses /run/spire/admin/api.sock, not the CLI default)")
    parser.add_argument("--dry-run", action="store_true", help="print the plan for the live server and change nothing; exits 0 whatever the plan")
    parser.add_argument("--check", action="store_true",
                        help="print the plan, change nothing, and exit 2 when the plan is not empty, a live entry has fewer than two "
                             "selectors or any federated bundle is stored; exit 3 when the server holds an admin or downstream entry (the message names each "
                             "entry ID and SPIFFE ID); exit 1 when the listing is refused for any other reason")
    parser.add_argument("--approved-digest", help="SHA-256 (hex) of the reviewed registrations.json, as recorded in registration-plan.json; "
                        "required to apply, and checked against the file bytes before the server is contacted")
    args = parser.parse_args(argv)
    try:
        source = args.registrations.read_bytes()
        applying = not (args.current or args.dry_run or args.check)
        if applying:
            approve(source, args.approved_digest)
        desired = json.loads(source)["entries"]
        if args.current:
            plan = plan_reconciliation(desired, json.loads(args.current.read_text()))
            sys.stdout.write(plan.to_json())
        else:
            server = shlex.split(args.server_command)
            if not server:
                raise ReconcileRefused("the server command is empty")
            if args.socket_path is not None and not args.socket_path.startswith("/"):
                raise ReconcileRefused("the socket path must be absolute")
            plan = reconcile(desired, server, runner, args.dry_run or args.check, lambda line: print(line, flush=True), args.socket_path)
        if args.check and plan.drifted():
            parser.exit(2, f"drift: {len(plan.steps())} planned steps, {len(plan.broad)} entries with fewer than two selectors, "
                      f"{len(plan.undeclared_bundles)} undeclared bundles\n")
    except PrivilegedGrant as error:
        parser.exit(PRIVILEGED_GRANT_EXIT, f"privileged grant present: {error}\n")
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.exit(1, f"reconciliation refused: {error}\n")
    except ApplyFailed as error:
        parser.exit(1, f"reconciliation failed: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
