#!/usr/bin/env python3
"""Guard the trogon-*/trogonai-* crate namespace against crates.io squatting.

This repo's internal workspace crates (path dependencies, never published)
live under the ``trogon-*`` and ``trogonai-*`` name prefixes. A dependency-
confusion / namespace-squatting attack works like this: an attacker
publishes a crate to crates.io under the exact name of one of our internal
crates. If our build ever resolves that name from the registry instead of
the workspace path (a manifest typo, a vendoring bug, a future package
rename that forgets to keep the ``path =`` pin), the attacker's code runs
in our build.

This script cross-checks every internal ``trogon-*`` / ``trogonai-*``
workspace crate name against crates.io:

  - REGISTERED, owned by us: fine, no finding (only relevant once/if we
    publish; currently none are).
  - REGISTERED, owned by someone else: HIGH-RISK finding - a namespace
    squat already exists and any accidental registry resolution would
    silently pull attacker code.
  - UNREGISTERED: WARNING finding - the name is squattable today. Not
    fatal (path deps are safe as long as they stay path deps), but flagged
    so the reservation can be tracked deliberately.

Exit codes:
    0 - no squats detected (unregistered names are printed as warnings only
        unless --strict)
    1 - a namespace squat (registered under a different owner) was found,
        or --strict was passed and unregistered names exist
    3 - usage / configuration error

Usage:
    python scripts/supply-chain/check_dependency_confusion.py
    python scripts/supply-chain/check_dependency_confusion.py --workspace rsworkspace
    python scripts/supply-chain/check_dependency_confusion.py --strict
    python scripts/supply-chain/check_dependency_confusion.py --owner-allowlist trogonai
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

import _supply_chain_common as common

DEFAULT_WORKSPACE = "rsworkspace"
NAMESPACE_PREFIXES = ("trogon-", "trogonai-")
NAME_RE = re.compile(r'^name\s*=\s*"([^"]+)"')


def discover_internal_crate_names(workspace_dir: str) -> list[str]:
    """Return internal (path-dependency) crate names under the guarded namespace.

    Scans every ``Cargo.toml`` under the workspace for a top-level
    ``[package]`` ``name = "..."`` whose value starts with one of
    ``NAMESPACE_PREFIXES``. These are, by construction, path-referenced
    workspace members - never registry dependencies - so any of them
    showing up as REGISTERED on crates.io under a foreign owner is a
    pre-positioned squat.
    """
    names: list[str] = []
    root = Path(workspace_dir)
    if not root.is_dir():
        return names
    for manifest in sorted(root.rglob("Cargo.toml")):
        if "target" in manifest.parts:
            continue
        try:
            content = manifest.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        in_package = False
        for line in content.splitlines():
            stripped = line.strip()
            if stripped.startswith("["):
                in_package = stripped == "[package]"
                continue
            if not in_package:
                continue
            m = NAME_RE.match(stripped)
            if m:
                name = m.group(1)
                if name.startswith(NAMESPACE_PREFIXES):
                    names.append(name)
                break
    return sorted(set(names))


def lookup_crate_owner_logins(name: str) -> list[str] | None:
    """Return the list of owner logins for a registered crate, or None if unregistered.

    Raises RegistryUnavailable if the network could not be reached.
    """
    url = f"https://crates.io/api/v1/crates/{name}/owners"
    try:
        data = common.fetch_crates_io_json(url)
    except LookupError:
        return None
    users = data.get("users")
    if not isinstance(users, list):
        return []
    logins = []
    for u in users:
        if isinstance(u, dict) and isinstance(u.get("login"), str):
            logins.append(u["login"])
    return logins


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--workspace", default=DEFAULT_WORKSPACE, help=f"Cargo workspace directory (default: {DEFAULT_WORKSPACE})")
    parser.add_argument("--owner-allowlist", action="append", default=[], metavar="LOGIN", help="crates.io login(s) that are considered 'us' (repeatable).")
    parser.add_argument("--strict", action="store_true", help="Also fail (exit 1) on unregistered/squattable names, not just confirmed squats.")
    parser.add_argument("--receipt", default=None, help="Optional path to write a JSONL receipt of every checked crate name.")
    args = parser.parse_args(argv)

    names = discover_internal_crate_names(args.workspace)
    if not names:
        print(f"No trogon-*/trogonai-* crates found under {args.workspace}.")
        return 0

    allowlist = {login.lower() for login in args.owner_allowlist}
    print(f"Checking {len(names)} internal crate name(s) against crates.io for namespace squatting...")

    squats: list[str] = []
    squattable: list[str] = []
    offline = 0
    receipt_records: list[dict] = []

    for name in names:
        record: dict = {"name": name}
        try:
            owners = lookup_crate_owner_logins(name)
        except common.RegistryUnavailable as exc:
            print(f"  SKIP (offline): {name} - {common.safe_text(str(exc))}")
            offline += 1
            record["result"] = "skip_offline"
            receipt_records.append(record)
            continue
        except common.RegistryError as exc:
            print(f"  SKIP (registry error): {name} - {common.safe_text(str(exc))}")
            record["result"] = "skip_error"
            receipt_records.append(record)
            continue

        if owners is None:
            print(f"  OK (unregistered, squattable): {name}")
            squattable.append(name)
            record["result"] = "unregistered"
            receipt_records.append(record)
            continue

        owner_logins_lower = {o.lower() for o in owners}
        if owner_logins_lower & allowlist:
            print(f"  OK (registered, owned by us): {name} (owners: {', '.join(owners) or 'unknown'})")
            record["result"] = "owned_by_us"
            record["owners"] = owners
            receipt_records.append(record)
            continue

        squats.append(
            f"  SQUAT DETECTED: '{name}' is registered on crates.io under owner(s) "
            f"{', '.join(owners) or 'unknown'} - this is NOT our workspace crate. "
            f"Verify the workspace Cargo.toml pins this as a path dependency, not a "
            f"registry version."
        )
        record["result"] = "squat"
        record["owners"] = owners
        receipt_records.append(record)

    if args.receipt:
        common.write_jsonl_receipt(args.receipt, receipt_records)

    print()
    print(f"Checked {len(names)} name(s); {offline} skipped (registry unreachable).")

    if squattable:
        print()
        print("Unregistered, squattable names (consider reserving on crates.io):")
        for name in squattable:
            print(f"  - {name}")

    if squats:
        print()
        print("Dependency confusion check FAILED - namespace squat(s) detected:")
        for s in squats:
            print(s)
        return 1

    if args.strict and squattable:
        print()
        print("--strict: unregistered/squattable names are treated as failures.")
        return 1

    print()
    print("OK: no crates.io namespace squats detected in the trogon-*/trogonai-* prefix.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
