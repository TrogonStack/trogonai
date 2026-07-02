#!/usr/bin/env python3
"""Report .rs source files that are missing a per-file SPDX license header.

This complements `.github/actions/enforce-cargo-license`, which enforces
`license = "Apache-2.0"` at the *manifest* level (once per crate). This
script checks the *file* level: whether individual `.rs` files carry an
SPDX header comment identifying the license.

As of this script's introduction, no `.rs` file in rsworkspace/ carries a
header - see MS_AGENT_GOV_TOOLKIT_WORKITEMS.md WI-15. Retrofitting ~1500
files is a separate, deliberate effort. Until that happens, this check is
REPORT-ONLY: it always exits 0 and prints a summary, so it can be wired
into CI as visibility without blocking merges. Pass --enforce to make
missing headers a failure (exit 1) once the repo has adopted headers.

Usage:
    python scripts/supply-chain/check_license_headers.py
    python scripts/supply-chain/check_license_headers.py --root rsworkspace
    python scripts/supply-chain/check_license_headers.py --enforce
    python scripts/supply-chain/check_license_headers.py --receipt out.jsonl
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

import _supply_chain_common as common

DEFAULT_ROOT = "rsworkspace"
EXPECTED_HEADER = "SPDX-License-Identifier: Apache-2.0"

SKIP_DIRS = {"target", ".git", "node_modules"}


def should_skip(path: Path) -> bool:
    if any(part in SKIP_DIRS for part in path.parts):
        return True
    try:
        if path.stat().st_size == 0:
            return True
    except OSError:
        return True
    return False


def has_header(path: Path) -> bool:
    try:
        content = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return True  # unreadable files are not reportable findings
    head = "\n".join(content.split("\n", 5)[:5])
    return EXPECTED_HEADER in head


def discover_rs_files(root: str) -> list[Path]:
    base = Path(root)
    if not base.is_dir():
        return []
    return [p for p in sorted(base.rglob("*.rs")) if not should_skip(p)]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--root", default=DEFAULT_ROOT, help=f"Directory to scan for .rs files (default: {DEFAULT_ROOT})")
    parser.add_argument("--enforce", action="store_true", help="Exit 1 if any file is missing the header (default: report-only, always exit 0).")
    parser.add_argument("--receipt", default=None, help="Optional path to write a JSONL receipt of every checked file.")
    args = parser.parse_args(argv)

    files = discover_rs_files(args.root)
    if not files:
        print(f"No .rs files found under {args.root}.")
        return 0

    missing: list[Path] = []
    receipt_records: list[dict] = []
    for path in files:
        ok = has_header(path)
        receipt_records.append({"path": str(path), "result": "ok" if ok else "missing_header"})
        if not ok:
            missing.append(path)

    if args.receipt:
        common.write_jsonl_receipt(args.receipt, receipt_records)

    print(f"Checked {len(files)} .rs file(s) under {args.root} for '{EXPECTED_HEADER}'.")

    if not missing:
        print("OK: all .rs files carry the expected license header.")
        return 0

    print()
    print(f"Missing license header in {len(missing)} of {len(files)} file(s).")
    if not args.enforce:
        print(
            "This check is REPORT-ONLY (no .rs file in this repo carries a header "
            "yet; see MS_AGENT_GOV_TOOLKIT_WORKITEMS.md WI-15). Re-run with "
            "--enforce once headers have been adopted repo-wide."
        )
        return 0

    print("Files missing the header:")
    for path in missing:
        print(f"  {path}")
    return 1


if __name__ == "__main__":
    sys.exit(main())
