#!/usr/bin/env python3
"""Flag Cargo.lock dependencies published to crates.io less than N days ago.

Defends against the *new-release* class of supply-chain attacks where a
compromised maintainer or hijacked publishing token ships a malicious
version that ecosystem responders typically catch and yank within a week.
By refusing to adopt a crate version that is less than N days old (default
7), we let the broader community absorb the first-discovery shock before it
lands in this workspace.

Only registry-sourced entries (real ``checksum``) are checked; internal
``trogon-*``/``trogonai-*`` path dependencies have no checksum and are
skipped, as are git dependencies.

A wall-clock budget (``--total-deadline-sec``, default 120) caps the whole
candidate loop; when the budget is exhausted, unscanned candidates are
reported as findings (fail-closed) rather than silently dropped.

Exit codes:
    0 - all resolved versions satisfy the cooling-off rule
    1 - one or more dependencies are too fresh, or the wall-clock deadline
        tripped with unscanned candidates remaining
    2 - DoS cap exceeded (more candidates than --max-deps)
    3 - usage / configuration error

Network failures for a given crate are reported as SKIP and do not fail the
run (fail-open only for "could not reach the registry at all"); a
definitive 404 (crate/version not on crates.io) is a hard failure.

Usage:
    python scripts/supply-chain/check_release_age.py
    python scripts/supply-chain/check_release_age.py rsworkspace/Cargo.lock
    python scripts/supply-chain/check_release_age.py --min-age-days 14
    python scripts/supply-chain/check_release_age.py --allow serde@1.0.228
"""

from __future__ import annotations

import argparse
import os
import sys
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python <3.11 fallback
    tomllib = None  # type: ignore[assignment]

import _supply_chain_common as common

DEFAULT_LOCKFILE = "rsworkspace/Cargo.lock"
DEFAULT_MAX_DEPS = 200
DEFAULT_DEADLINE_SEC = 120
MAX_LOCKFILE_BYTES = 16 * 1024 * 1024


@dataclass(frozen=True)
class LockEntry:
    name: str
    version: str


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
        # Only registry-sourced entries carry a checksum; path/git deps
        # (including our own trogon-*/trogonai-* workspace crates) do not
        # and are intentionally excluded from release-age scanning.
        if not isinstance(name, str) or not isinstance(version, str):
            continue
        if not isinstance(checksum, str):
            continue
        if not common.is_safe_crate_name(name) or not common.is_safe_version(version):
            continue
        entries.append(LockEntry(name=name, version=version))
    return entries


def fetch_release_time(name: str, version: str) -> datetime | None:
    """Return the crates.io publish timestamp, or None on transient failure.

    Raises LookupError on a definitive 404 and RegistryUnavailable if the
    network could not be reached at all.
    """
    url = f"https://crates.io/api/v1/crates/{name}/{version}"
    data = common.fetch_crates_io_json(url)
    version_obj = data.get("version") or {}
    stamp = version_obj.get("created_at") if isinstance(version_obj, dict) else None
    if not stamp:
        raise LookupError(f"crates.io has no release timestamp for {name}@{version}")
    return _parse_iso(stamp)


def _parse_iso(stamp: str | None) -> datetime | None:
    if not isinstance(stamp, str) or not stamp:
        return None
    s = stamp.replace("Z", "+00:00")
    try:
        dt = datetime.fromisoformat(s)
    except ValueError:
        return None
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt.astimezone(timezone.utc)


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("paths", nargs="*", default=[DEFAULT_LOCKFILE], help=f"Cargo.lock path(s) (default: {DEFAULT_LOCKFILE})")
    p.add_argument("--min-age-days", type=int, default=7, help="Minimum release age in days (default: 7)")
    p.add_argument("--max-deps", type=int, default=DEFAULT_MAX_DEPS, help=f"Refuse to inspect more than this many candidates (default: {DEFAULT_MAX_DEPS}). 0 disables the cap.")
    p.add_argument("--total-deadline-sec", type=int, default=DEFAULT_DEADLINE_SEC, help=f"Wall-clock budget in seconds for the candidate loop (default: {DEFAULT_DEADLINE_SEC}). 0 disables the budget.")
    p.add_argument("--allow", action="append", default=[], metavar="CRATE@VERSION", help="Allow a specific crate@version to bypass the age check")
    p.add_argument("--receipt", default=None, help="Optional path to write a JSONL receipt of every checked entry.")
    return p


def main(argv: list[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    allow_set = {a.strip() for a in args.allow}
    threshold = timedelta(days=args.min_age_days)
    now = datetime.now(timezone.utc)

    all_entries: list[LockEntry] = []
    for path in args.paths:
        try:
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

    if args.max_deps > 0 and len(all_entries) > args.max_deps:
        print(
            f"::error::Lockfile has {len(all_entries)} registry-sourced entries, "
            f"exceeding --max-deps={args.max_deps}. Refusing to scan every "
            "resolved dependency on every run; pass --max-deps=0 to override or "
            "target the diff explicitly.",
            file=sys.stderr,
        )
        return 2

    deadline = common.Deadline(budget_seconds=args.total_deadline_sec)
    findings: list[str] = []
    receipt_records: list[dict] = []
    offline = 0
    print(
        f"Checking {len(all_entries)} resolved crate version(s) against "
        f"{args.min_age_days}-day cooling-off rule (wall-clock budget {args.total_deadline_sec}s)..."
    )

    for i, entry in enumerate(all_entries):
        if deadline.expired():
            common.emit_deadline_warning("check_release_age", deadline)
            for u in all_entries[i:]:
                findings.append(
                    f"  DEADLINE-UNSCANNED: {u.name}@{u.version} could not be verified "
                    f"before the wall-clock budget expired"
                )
            break

        spec = f"{entry.name}@{entry.version}"
        record: dict = {"name": entry.name, "version": entry.version}
        if spec in allow_set:
            print(f"  SKIP (allow-list): {spec}")
            record["result"] = "allowlisted"
            receipt_records.append(record)
            continue

        try:
            released = fetch_release_time(entry.name, entry.version)
        except LookupError as exc:
            findings.append(f"  HARD FAIL: {spec} not found in crates.io - {exc}")
            record["result"] = "not_found"
            receipt_records.append(record)
            continue
        except common.RegistryUnavailable as exc:
            print(f"  SKIP (offline): {spec} - {common.safe_text(str(exc))}")
            offline += 1
            record["result"] = "skip_offline"
            receipt_records.append(record)
            continue
        except common.RegistryError as exc:
            print(f"  SKIP (registry error): {spec} - {common.safe_text(str(exc))}")
            record["result"] = "skip_error"
            receipt_records.append(record)
            continue

        if released is None:
            findings.append(f"  UNVERIFIED: {spec} - registry response had no parseable timestamp")
            record["result"] = "unverified"
            receipt_records.append(record)
            continue

        age = now - released
        record["released_at"] = released.isoformat()
        record["age_days"] = age.days
        if age < threshold:
            findings.append(
                f"  TOO FRESH: {spec} released {released.isoformat()} "
                f"({age.days}d {age.seconds // 3600}h ago, threshold {args.min_age_days}d)"
            )
            record["result"] = "too_fresh"
        else:
            print(f"  OK: {spec} released {released.date()} ({age.days}d ago)")
            record["result"] = "ok"
        receipt_records.append(record)

    if args.receipt:
        common.write_jsonl_receipt(args.receipt, receipt_records)

    print()
    print(f"Checked {len(all_entries) - offline} entr(y/ies); {offline} skipped (registry unreachable).")

    if findings:
        print()
        print("Release-age check FAILED - newly-published or unverifiable versions detected:")
        for f in findings:
            print(f)
        print()
        print("Options:")
        print(f"  - Wait until the version is at least {args.min_age_days} days old.")
        print("  - Pin to the previous stable release.")
        print("  - If urgent and the release is verified safe, bypass with:")
        print("      --allow CRATE@VERSION")
        return 1

    print()
    print("OK: all newly-resolved versions satisfy the cooling-off rule.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
