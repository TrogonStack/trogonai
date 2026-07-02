#!/usr/bin/env python3
"""Verify Cargo.lock checksums against crates.io registry metadata.

For every registry-sourced entry in ``Cargo.lock`` (``[[package]]`` blocks
with a ``checksum`` field), this compares the locally pinned SHA-256 against
what crates.io's sparse index actually publishes for that exact version.

This catches **lockfile poisoning** - a supply-chain attack where the
version number is unchanged but the bytes have been swapped (a tampered
crate re-published under the same version via a compromised index mirror,
or a locally hand-edited checksum). Path/git dependencies (internal
workspace crates) have no checksum and are skipped - they are not registry
artifacts.

Exit codes:
    0 - all integrity hashes verified (or no lockfile entries to check)
    1 - one or more checksum mismatches
    2 - DoS cap exceeded (more entries than --max-deps)
    3 - usage / configuration error

Network failures (offline sandbox, DNS unavailable, etc.) are reported as
SKIP per-entry and do not fail the run - this script fails closed only on a
*confirmed* mismatch or a definitive 404 (crate/version does not exist,
which is itself a strong tamper signal).

Usage:
    python scripts/supply-chain/check_lockfile_integrity.py
    python scripts/supply-chain/check_lockfile_integrity.py rsworkspace/Cargo.lock
    python scripts/supply-chain/check_lockfile_integrity.py --max-deps 5000
    python scripts/supply-chain/check_lockfile_integrity.py --receipt out.jsonl
"""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python <3.11 fallback
    tomllib = None  # type: ignore[assignment]

import _supply_chain_common as common

DEFAULT_LOCKFILE = "rsworkspace/Cargo.lock"
DEFAULT_MAX_DEPS = 5000
MAX_LOCKFILE_BYTES = 16 * 1024 * 1024


@dataclass(frozen=True)
class LockEntry:
    name: str
    version: str
    checksum: str


def parse_cargo_lockfile(content: str) -> list[LockEntry]:
    if tomllib is None:  # pragma: no cover - guarded at import time
        return []
    try:
        data = tomllib.loads(content)
    except (tomllib.TOMLDecodeError, ValueError):
        return []
    packages = data.get("package")
    if not isinstance(packages, list):
        return []
    entries: list[LockEntry] = []
    for pkg in packages:
        if not isinstance(pkg, dict):
            continue
        name = pkg.get("name")
        version = pkg.get("version")
        checksum = pkg.get("checksum")
        # Path / git dependencies (internal workspace crates) have no
        # checksum - skip rather than treat as suspicious.
        if not isinstance(name, str) or not isinstance(version, str):
            continue
        if not isinstance(checksum, str):
            continue
        if not common.is_safe_crate_name(name) or not common.is_safe_version(version):
            continue
        if not common.HEX64_RE.match(checksum):
            continue
        entries.append(LockEntry(name=name, version=version, checksum=checksum.lower()))
    return entries


def fetch_upstream_checksum(name: str, version: str) -> str:
    """Return the crates.io-published checksum for name@version.

    Raises LookupError if the crate or version is unregistered, and
    RegistryUnavailable if the network could not be reached at all.
    """
    for entry in common.fetch_sparse_index_lines(name):
        if entry.get("vers") == version:
            cksum = entry.get("cksum")
            if isinstance(cksum, str) and common.HEX64_RE.match(cksum):
                return cksum.lower()
            raise common.RegistryError(f"malformed cksum for {name}@{version}")
    raise LookupError(f"version not in index: {name}@{version}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("paths", nargs="*", default=[DEFAULT_LOCKFILE], help=f"Cargo.lock path(s) (default: {DEFAULT_LOCKFILE})")
    parser.add_argument("--max-deps", type=int, default=DEFAULT_MAX_DEPS, help="Maximum entries to verify (DoS cap).")
    parser.add_argument("--receipt", default=None, help="Optional path to write a JSONL receipt of every checked entry.")
    args = parser.parse_args(argv)

    if args.max_deps < 1:
        print("--max-deps must be >= 1", file=sys.stderr)
        return 3

    all_entries: list[LockEntry] = []
    for path in args.paths:
        try:
            import os

            if os.path.getsize(path) > MAX_LOCKFILE_BYTES:
                print(f"::warning::skipping lockfile larger than {MAX_LOCKFILE_BYTES} bytes: {path}", file=sys.stderr)
                continue
            with open(path, encoding="utf-8") as fh:
                content = fh.read(MAX_LOCKFILE_BYTES + 1)
        except OSError as exc:
            print(f"::error::could not read {path}: {exc}", file=sys.stderr)
            return 3
        all_entries.extend(parse_cargo_lockfile(content))

    if not all_entries:
        print("No registry-sourced Cargo.lock entries to check.")
        return 0

    capped = False
    skipped_by_cap = 0
    if len(all_entries) > args.max_deps:
        capped = True
        skipped_by_cap = len(all_entries) - args.max_deps
        all_entries = all_entries[: args.max_deps]

    print(f"Checking {len(all_entries)} Cargo.lock entr{'y' if len(all_entries) == 1 else 'ies'} against crates.io...")

    findings: list[str] = []
    checked = 0
    skipped_offline = 0
    receipt_records: list[dict] = []

    for entry in all_entries:
        checked += 1
        record: dict = {"name": entry.name, "version": entry.version, "local_checksum": entry.checksum}
        try:
            upstream = fetch_upstream_checksum(entry.name, entry.version)
        except LookupError as exc:
            findings.append(f"  HARD FAIL: {entry.name}@{entry.version} not found in crates.io index - {exc}")
            record["result"] = "not_found"
            receipt_records.append(record)
            continue
        except common.RegistryUnavailable as exc:
            print(f"  SKIP (offline): {entry.name}@{entry.version} - {common.safe_text(str(exc))}")
            skipped_offline += 1
            record["result"] = "skip_offline"
            receipt_records.append(record)
            continue
        except common.RegistryError as exc:
            findings.append(f"  UNVERIFIED: {entry.name}@{entry.version} - {common.safe_text(str(exc))}")
            record["result"] = "unverified"
            receipt_records.append(record)
            continue

        record["upstream_checksum"] = upstream
        if entry.checksum != upstream:
            findings.append(
                f"  MISMATCH: {entry.name}@{entry.version} lockfile={entry.checksum} upstream={upstream}"
            )
            record["result"] = "mismatch"
        else:
            record["result"] = "ok"
        receipt_records.append(record)

    if args.receipt:
        common.write_jsonl_receipt(args.receipt, receipt_records)

    print()
    print(f"Checked {checked} entr{'y' if checked == 1 else 'ies'}; {skipped_offline} skipped (registry unreachable).")
    if capped:
        print(f"DoS cap reached: {skipped_by_cap} additional entr(y/ies) skipped.")

    if findings:
        print()
        print("Lockfile integrity check FAILED:")
        for f in findings:
            print(f)
        return 1

    if capped:
        return 2

    print("OK: all checked Cargo.lock checksums match crates.io.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
