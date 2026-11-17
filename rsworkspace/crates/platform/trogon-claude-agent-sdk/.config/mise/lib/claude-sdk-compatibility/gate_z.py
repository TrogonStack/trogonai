"""Gate Z: the parity bar the SDK must clear before it counts as done.

Every API mapping feature is `supported`, every behavior row follows upstream and names the tests that prove it, and
no OpenSSL crate is in the SDK's dependency tree. Each check returns the selectors that miss the bar, so a failing gate
lists every gap at once instead of only the first.
"""
import re
import tomllib

from gate_errors import Code, format_boundary, GateError

ACCEPTED_DECISIONS = ("upstream", "follow-typescript", "follow-python")
RUST_ADAPTATION = "rust-adaptation"
TLS_ROWS = re.compile(r"m(?:19|2[0-5])-[a-z0-9]+(?:-[a-z0-9]+)*|m26-python-tls-trust-directory-lookup")
TLS_REASON = "The owner chose rustls under ADR 0015 on 2026-11-14 over OpenSSL, so the difference is accepted."
DEFECT_ROWS = frozenset(("m18-python-system-proxy-undefined-reads",))
DEFECT_REASON = (
    "Upstream here is a defect, not a contract: CPython _scproxy aborts the interpreter with an NSException on macOS and "
    "winreg Reg2Py reads uninitialized memory on Windows, so Rust reads the values safely instead."
)
OPENSSL_CRATES = frozenset(("openssl", "openssl-sys", "openssl-src", "native-tls"))
SDK_PACKAGE = "trogon-claude-agent-sdk"


@format_boundary("mapping")
def unsupported_features(mapping):
    """`<id>.status` for each mapping feature that is not `supported`."""
    return [f"{feature['id']}.status" for feature in mapping["features"] if feature["status"] != "supported"]


def _accepted_adaptation(row_id, reason):
    # Only the TLS rows the owner accepted under ADR 0015 may differ, plus the rows whose upstream behavior is a crash or
    # an undefined read, which Rust does not copy. Each must give the owner's reason for it.
    if TLS_ROWS.fullmatch(row_id) is not None:
        return reason.endswith(TLS_REASON)
    if row_id in DEFECT_ROWS:
        return reason.endswith(DEFECT_REASON)
    return False


def _decision_gap(row):
    decision = row["decision"]
    kind = decision["kind"]
    if kind == RUST_ADAPTATION:
        return not _accepted_adaptation(row["id"], decision["reason"])
    if kind == "follow-python":
        # Python is followed only where TypeScript has no counterpart; following it over a TypeScript form is a choice
        # between the SDKs, which parity does not allow.
        return row["typescript"].get("status") != "absent"
    return kind not in ACCEPTED_DECISIONS


@format_boundary("behaviors")
def behavior_gaps(behaviors):
    """`<id>.decision` for each row that does not follow upstream, and `<id>.contracts` for each row with no test."""
    gaps = []
    for row in behaviors["rows"]:
        if _decision_gap(row):
            gaps.append(f"{row['id']}.decision")
        if not row["contracts"]:
            gaps.append(f"{row['id']}.contracts")
    return gaps


def _locked_package(packages, dependency):
    # Cargo.lock writes a dependency as `name`, `name version` or `name version (source)`.
    name, _, rest = dependency.partition(" ")
    version = rest.split(" ", 1)[0] if rest else None
    matches = [package for package in packages.get(name, ()) if version is None or package["version"] == version]
    if not matches:
        raise GateError(Code.REFERENCE, f"Cargo.lock.{dependency}")
    return matches[0]


@format_boundary("Cargo.lock")
def openssl_dependencies(lock_text, package=SDK_PACKAGE):
    """Each OpenSSL crate reachable from `package` in a Cargo.lock, as `name version`.

    The lockfile does not record targets, features or dependency kinds, so the walk counts every locked edge, dev and
    platform-specific ones included. That only over-reports, which is the safe side for this check.
    """
    packages = {}
    for locked in tomllib.loads(lock_text)["package"]:
        packages.setdefault(locked["name"], []).append(locked)
    found, seen, pending = set(), set(), [_locked_package(packages, package)]
    while pending:
        locked = pending.pop()
        key = (locked["name"], locked["version"])
        if key in seen:
            continue
        seen.add(key)
        if locked["name"] in OPENSSL_CRATES:
            found.add(f"{locked['name']} {locked['version']}")
        pending.extend(_locked_package(packages, dependency) for dependency in locked.get("dependencies", ()))
    return sorted(found)
