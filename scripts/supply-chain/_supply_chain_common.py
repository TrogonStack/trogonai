#!/usr/bin/env python3
"""Shared helpers for the Cargo/crates.io supply-chain scanning scripts.

This module exists so ``check_lockfile_integrity.py``,
``check_dependency_confusion.py``, and ``check_release_age.py`` share
*exactly one* implementation of the security-sensitive plumbing: bounded
HTTP, strict name/version validation, wall-clock budgets, and JSONL receipt
emission.

Defensive contracts upheld here:

* **Bounded reads.** ``fetch_json()`` rejects responses larger than
  ``MAX_RESPONSE_BYTES`` before JSON parsing, so a hostile registry mirror
  cannot OOM the runner.
* **Strict name/version syntax.** ``is_safe_crate_name()`` /
  ``is_safe_version()`` only accept the conservative Cargo identifier forms.
  Anything else is rejected before being interpolated into a registry URL.
* **Fail-closed deadline.** ``Deadline`` lets the caller abort a candidate
  loop with a non-zero exit once the wall clock is exhausted, so a hostile
  PR cannot stretch registry-bound work to a multi-hour CI bill.
* **Offline is a SKIP, not a silent pass.** Every network helper raises
  ``RegistryUnavailable`` on network-level failure so callers can print a
  clear SKIP line instead of reporting a false "OK".

Python 3.11+, stdlib only.
"""

from __future__ import annotations

import json
import re
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from typing import Any

USER_AGENT = (
    "trogonai-supply-chain-check/1.0 "
    "(+https://github.com/trogonai/tehran)"
)

# Per-request HTTP timeout. Hostile or slow registries that hang the socket
# are killed at this boundary; the wall-clock deadline below caps the
# *total* loop budget across many requests.
REGISTRY_TIMEOUT = 10  # seconds

# Cap on a single registry response. crates.io sparse-index lines and
# api/v1 crate/version documents are both well under 1 MB in practice.
MAX_RESPONSE_BYTES = 5 * 1024 * 1024

# Strict accepted crate-name syntax (crates.io allows ASCII alphanumerics,
# `-`, and `_`).
CARGO_NAME_RE = re.compile(r"\A[A-Za-z0-9_-]+\Z")

# Strict accepted version syntax. Disallows `/`, `..`, `:`, whitespace,
# quotes, and control chars. Anchored with \A...\Z (not ^...$) so a
# trailing newline cannot smuggle past the check.
SAFE_VERSION_RE = re.compile(r"\A[0-9A-Za-z][0-9A-Za-z.\-+_]*\Z")

HEX64_RE = re.compile(r"\A[0-9a-fA-F]{64}\Z")


class RegistryUnavailable(Exception):
    """Raised when crates.io cannot be reached at all (offline / DNS / timeout).

    Distinct from a definitive 404: callers should treat this as "could not
    verify" and print a SKIP, not a failure, so the checks degrade
    gracefully in offline/sandboxed environments.
    """


class RegistryError(Exception):
    """Raised when a registry lookup fails for a reason other than being offline."""


@dataclass
class Deadline:
    """Wall-clock budget tracker for a candidate loop.

    A ``budget_seconds`` of zero disables the budget; a positive value
    causes ``expired()`` to flip true once exhausted, at which point the
    caller should break out of its candidate loop and fail closed (non-zero
    exit) rather than silently truncate the scan.
    """

    budget_seconds: float
    _start: float = field(default=0.0, init=False, repr=False)

    def __post_init__(self) -> None:
        self._start = time.monotonic()

    def expired(self) -> bool:
        if self.budget_seconds <= 0:
            return False
        return (time.monotonic() - self._start) >= self.budget_seconds

    def elapsed_seconds(self) -> float:
        return time.monotonic() - self._start


def is_safe_crate_name(name: str) -> bool:
    return bool(name) and bool(CARGO_NAME_RE.match(name))


def is_safe_version(version: str) -> bool:
    return bool(version) and bool(SAFE_VERSION_RE.match(version))


def safe_text(token: Any, *, max_len: int = 200) -> str:
    """Strip control chars and cap length before printing attacker-controlled text."""
    if not isinstance(token, str):
        token = repr(token)
    cleaned = "".join(c for c in token if 32 <= ord(c) < 127)
    if len(cleaned) > max_len:
        cleaned = cleaned[:max_len] + "..."
    return cleaned or "<empty>"


def crates_index_path(name: str) -> str:
    """Return the sparse-index path component for *name*.

    Mirrors the documented crates.io sparse layout:
      1-char  -> "1/<name>"
      2-char  -> "2/<name>"
      3-char  -> "3/<first>/<name>"
      4+      -> "<first2>/<chars3-4>/<name>"
    Names are lowercased per index convention.
    """
    if not is_safe_crate_name(name):
        raise RegistryError(f"invalid crate name: {safe_text(name)}")
    lower = name.lower()
    n = len(lower)
    if n == 1:
        return f"1/{lower}"
    if n == 2:
        return f"2/{lower}"
    if n == 3:
        return f"3/{lower[0]}/{lower}"
    return f"{lower[0:2]}/{lower[2:4]}/{lower}"


def _fetch_url(url: str, *, timeout: int = REGISTRY_TIMEOUT) -> bytes:
    req = urllib.request.Request(
        url,
        headers={"User-Agent": USER_AGENT, "Accept": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:  # noqa: S310 - https only, host is a constant
            buf = resp.read(MAX_RESPONSE_BYTES + 1)
    except urllib.error.HTTPError as exc:
        if exc.code == 404:
            raise LookupError(f"404 from registry: {safe_text(url)}") from exc
        raise RegistryError(f"http {exc.code} for {safe_text(url)}") from exc
    except (urllib.error.URLError, TimeoutError, OSError) as exc:
        raise RegistryUnavailable(f"network error for {safe_text(url)}: {safe_text(str(exc))}") from exc

    if len(buf) > MAX_RESPONSE_BYTES:
        raise RegistryError(f"response too large for {safe_text(url)}")
    return buf


def fetch_crates_io_json(url: str) -> dict:
    """GET a crates.io ``api/v1`` JSON document. Raises LookupError on 404."""
    raw = _fetch_url(url)
    try:
        return json.loads(raw)
    except (json.JSONDecodeError, UnicodeDecodeError) as exc:
        raise RegistryError(f"invalid json from {safe_text(url)}: {safe_text(str(exc))}") from exc


def fetch_sparse_index_lines(name: str) -> list[dict]:
    """Fetch and parse the crates.io sparse-index document for *name*.

    Returns one dict per published version (newline-delimited JSON). Raises
    ``LookupError`` if the crate name is not registered at all (404).
    """
    url = f"https://index.crates.io/{crates_index_path(name)}"
    raw = _fetch_url(url)
    text = raw.decode("utf-8", errors="strict")
    entries: list[dict] = []
    for line in text.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            entries.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return entries


def emit_deadline_warning(label: str, deadline: Deadline) -> None:
    print(
        f"::error::{label}: wall-clock deadline of {deadline.budget_seconds}s "
        f"exhausted (elapsed {deadline.elapsed_seconds():.1f}s). Aborting scan - "
        f"this is a fail-closed exit so a hostile PR cannot stretch the scan "
        f"past the budget to hide a malicious dependency behind it.",
        file=sys.stderr,
    )


def write_jsonl_receipt(path: str, records: list[dict]) -> None:
    """Append-write a JSONL receipt: one JSON object per line.

    Used for CI artifact upload / audit trail. Errors writing the receipt
    are surfaced but never change the script's exit code.
    """
    try:
        with open(path, "w", encoding="utf-8") as fh:
            for record in records:
                fh.write(json.dumps(record, sort_keys=True))
                fh.write("\n")
    except OSError as exc:
        print(f"::warning::could not write JSONL receipt to {path}: {exc}", file=sys.stderr)
