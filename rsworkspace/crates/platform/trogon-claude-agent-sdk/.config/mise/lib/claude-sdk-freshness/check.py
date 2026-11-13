#!/usr/bin/env python3
"""Inventory upstream SDK releases and report review work without changing Rust."""

from __future__ import annotations

import argparse
import base64
from contextlib import ExitStack
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tarfile
import tempfile
import time
from typing import Any
from urllib.parse import quote, urlparse
from urllib.request import HTTPRedirectHandler, Request, build_opener


ROOT = Path(__file__).resolve().parents[4]
HERE = Path(__file__).resolve().parent
LANGUAGES = ("python", "typescript")
STATUSES = {"supported", "partial", "deferred", "out_of_scope", "rust_difference"}
ISSUE_MARKER = "<!-- claude-sdk-freshness:v1 -->"
ISSUE_AUTHOR = "github-actions[bot]"
ISSUE_TITLE = "Claude SDK review: upstream changes and Rust coverage"
MAX_METADATA_BYTES = 8 * 1024 * 1024
MAX_ARCHIVE_BYTES = 128 * 1024 * 1024
MAX_EXPANDED_BYTES = 256 * 1024 * 1024
MAX_ARCHIVE_MEMBERS = 20000
MAX_MEMBER_BYTES = 64 * 1024 * 1024
MAX_ISSUE_CHARS = 55000
REGISTRY_HOSTS = {"pypi.org", "files.pythonhosted.org", "registry.npmjs.org"}
VERSION = re.compile(r"[A-Za-z0-9][A-Za-z0-9.+_-]{0,99}\Z")
FINGERPRINT_FILES = ("check.py", "extract_python.py", "extract_typescript.mjs", "package.json", "pnpm-lock.yaml")


class CheckError(Exception):
    """A failed check that must not be reported as upstream drift."""


class FetchError(CheckError):
    pass


class SnapshotError(CheckError):
    pass


class MappingError(CheckError):
    pass


class CommandError(CheckError):
    pass


def canonical(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True, allow_nan=False)


def read_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise SnapshotError("Could not read a required JSON snapshot or mapping") from error


def write_json(path: Path, value: Any) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True, allow_nan=False) + "\n", encoding="utf-8")


def relative_path(value: Any) -> PurePosixPath:
    if not isinstance(value, str) or not value or "\\" in value or "\x00" in value:
        raise SnapshotError("Source/evidence paths must be relative POSIX paths")
    path = PurePosixPath(value)
    if path.is_absolute() or any(part in {"..", "."} for part in value.split("/")) or ":" in value:
        raise SnapshotError("Source/evidence paths must stay inside their source root")
    return path


def checked_version(value: str) -> str:
    if not isinstance(value, str) or not VERSION.fullmatch(value):
        raise FetchError("Registry returned an invalid release version")
    return value


def validate_registry_url(url: str) -> None:
    parsed = urlparse(url)
    if parsed.scheme != "https" or parsed.hostname not in REGISTRY_HOSTS or parsed.username or parsed.password:
        raise FetchError("Refusing a package URL outside the official HTTPS registries")


class RegistryRedirect(HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, newurl):
        validate_registry_url(newurl)
        return super().redirect_request(request, response, code, message, headers, newurl)


def download(url: str, destination: Path, limit: int) -> None:
    validate_registry_url(url)
    request = Request(url, headers={"User-Agent": "claude-sdk-freshness/1", "Accept": "application/json, */*"})
    try:
        deadline = time.monotonic() + 180
        with build_opener(RegistryRedirect()).open(request, timeout=45) as response, destination.open("xb") as output:
            validate_registry_url(response.url)
            declared = response.headers.get("Content-Length")
            if declared is not None and (not declared.isdigit() or int(declared) > limit):
                raise FetchError("Registry response exceeds its download limit")
            total = 0
            while block := response.read(min(1024 * 1024, limit - total + 1)):
                if time.monotonic() > deadline:
                    raise FetchError("Registry download exceeded its total time limit")
                total += len(block)
                if total > limit:
                    raise FetchError("Registry response exceeds its download limit")
                output.write(block)
    except (OSError, ValueError) as error:
        raise FetchError("Could not download an upstream registry artifact") from error


def fetch_json(url: str, scratch: Path, name: str) -> Any:
    destination = scratch / name
    download(url, destination, MAX_METADATA_BYTES)
    try:
        return json.loads(destination.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise FetchError("Registry returned malformed metadata") from error


def verify_archive(path: Path, algorithm: str, expected: str) -> None:
    try:
        with path.open("rb") as source:
            digest = hashlib.file_digest(source, algorithm).digest()
        actual = digest.hex() if algorithm == "sha256" else base64.b64encode(digest).decode("ascii")
    except (OSError, ValueError) as error:
        raise FetchError("Could not verify the upstream archive digest") from error
    if actual != expected:
        raise FetchError("Upstream archive failed its registry integrity check")


def extract_archive(archive: Path, destination: Path, language: str) -> Path:
    destination.mkdir()
    selected = 0
    expanded = 0
    prefix: str | None = None
    try:
        with tarfile.open(archive, "r:gz") as package:
            for index, member in enumerate(package):
                if index >= MAX_ARCHIVE_MEMBERS:
                    raise FetchError("Archive contains too many members")
                name = relative_path(member.name)
                if member.issym() or member.islnk() or not (member.isfile() or member.isdir()):
                    raise FetchError("Archive contains a link or nonregular member")
                if member.size < 0 or member.size > MAX_MEMBER_BYTES:
                    raise FetchError("Archive member exceeds its extraction limit")
                expanded += member.size
                if expanded > MAX_EXPANDED_BYTES:
                    raise FetchError("Archive exceeds its expanded-size limit")
                if len(name.parts) < 2:
                    if member.isdir():
                        continue
                    raise FetchError("Archive has no single package root")
                if prefix is None:
                    prefix = name.parts[0]
                elif name.parts[0] != prefix:
                    raise FetchError("Archive has conflicting package roots")
                local = PurePosixPath(*name.parts[1:])
                if not member.isfile():
                    continue
                if language == "python":
                    wanted = str(local) == "pyproject.toml" or (str(local).startswith("src/claude_agent_sdk/") and local.suffix == ".py")
                else:
                    wanted = str(local) == "package.json" or ("node_modules" not in local.parts and local.suffix in {".ts", ".js", ".mjs"})
                if not wanted:
                    continue
                target = destination.joinpath(*local.parts)
                target.parent.mkdir(parents=True, exist_ok=True)
                source = package.extractfile(member)
                if source is None:
                    raise FetchError("Archive regular file could not be read")
                with source, target.open("xb") as output:
                    remaining = member.size
                    while remaining:
                        block = source.read(min(1024 * 1024, remaining))
                        if not block:
                            raise FetchError("Archive member was truncated")
                        output.write(block)
                        remaining -= len(block)
                selected += 1
    except (OSError, tarfile.TarError, SnapshotError) as error:
        raise FetchError("Could not safely extract the verified upstream source archive") from error
    if selected == 0:
        raise FetchError("Archive contains no inspectable SDK sources")
    return destination


def fetch_python(scratch: Path, version: str | None) -> tuple[Path, str, dict[str, str]]:
    suffix = f"/{quote(checked_version(version), safe='')}" if version else ""
    metadata = fetch_json(f"https://pypi.org/pypi/claude-agent-sdk{suffix}/json", scratch, "pypi.json")
    try:
        resolved = checked_version(metadata["info"]["version"])
        if metadata["info"]["name"].lower().replace("_", "-") != "claude-agent-sdk":
            raise FetchError("PyPI returned the wrong package identity")
        sdists = [entry for entry in metadata["urls"] if entry["packagetype"] == "sdist" and not entry.get("yanked", False)]
        if len(sdists) != 1 or (version is not None and resolved != version):
            raise FetchError("PyPI did not return exactly the requested source release")
        entry = sdists[0]
        digest = entry["digests"]["sha256"]
        if not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise FetchError("PyPI source digest is malformed")
        url = entry["url"]
    except (KeyError, TypeError) as error:
        raise FetchError("PyPI source metadata is incomplete") from error
    archive = scratch / "python.tgz"
    download(url, archive, MAX_ARCHIVE_BYTES)
    verify_archive(archive, "sha256", digest)
    return extract_archive(archive, scratch / "python", "python"), resolved, {"url": url, "integrity": f"sha256-{digest}"}


def fetch_typescript(scratch: Path, version: str | None) -> tuple[Path, str, dict[str, str]]:
    target = checked_version(version) if version else "latest"
    metadata = fetch_json(f"https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk/{quote(target, safe='')}", scratch, "npm.json")
    try:
        resolved = checked_version(metadata["version"])
        if metadata["name"] != "@anthropic-ai/claude-agent-sdk" or (version is not None and resolved != version):
            raise FetchError("npm returned the wrong source package identity/version")
        integrity = metadata["dist"]["integrity"]
        if not re.fullmatch(r"sha512-[A-Za-z0-9+/]{86}==", integrity):
            raise FetchError("npm source package must provide SHA-512 integrity")
        url = metadata["dist"]["tarball"]
    except (KeyError, TypeError) as error:
        raise FetchError("npm source metadata is incomplete") from error
    archive = scratch / "typescript.tgz"
    download(url, archive, MAX_ARCHIVE_BYTES)
    verify_archive(archive, "sha512", integrity.removeprefix("sha512-"))
    return extract_archive(archive, scratch / "typescript", "typescript"), resolved, {"url": url, "integrity": integrity}


def validate_inventory(inventory: Any, language: str) -> None:
    if not isinstance(inventory, dict) or inventory.get("schema_version") != 1 or inventory.get("language") != language:
        raise SnapshotError("Unsupported inventory schema or source language")
    symbols = inventory.get("symbols")
    if not isinstance(symbols, dict) or not symbols:
        raise SnapshotError("An upstream public API inventory is empty or unreadable")
    for name, symbol in symbols.items():
        if not isinstance(name, str) or not name or not isinstance(symbol, dict):
            raise SnapshotError("Malformed inventory symbol")
        if not isinstance(symbol.get("kind"), str) or not symbol["kind"] or "shape" not in symbol:
            raise SnapshotError("Inventory symbol lacks a structural representation")
        source = symbol.get("source", {})
        relative_path(source.get("path"))
        if not isinstance(source.get("line"), int) or source["line"] < 1:
            raise SnapshotError("Inventory symbol lacks a source location")
        canonical(symbol["shape"])


def extract_inventory(root: Path, language: str) -> dict[str, Any]:
    try:
        if language == "python":
            spec = importlib.util.spec_from_file_location("claude_sdk_inventory_python", HERE / "extract_python.py")
            if spec is None or spec.loader is None:
                raise SnapshotError("Python inventory extractor is unavailable")
            module = importlib.util.module_from_spec(spec)
            sys.modules[spec.name] = module
            spec.loader.exec_module(module)
            inventory = module.extract(root)
        else:
            command = subprocess.run(["node", str(HERE / "extract_typescript.mjs"), str(root)], check=True, capture_output=True, text=True, timeout=120)
            inventory = json.loads(command.stdout)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        raise SnapshotError(f"The {language} inventory extractor failed; check parser/source compatibility") from error
    validate_inventory(inventory, language)
    return inventory


def runtime_hashes(root: Path, language: str) -> dict[str, str]:
    search = root / "src/claude_agent_sdk" if language == "python" else root
    if not search.is_dir():
        raise SnapshotError("Upstream runtime source directory is missing")
    result = {}
    for path in sorted(search.rglob("*")):
        if path.is_symlink():
            raise SnapshotError("Local source inventory refuses symlinked source files")
        if not path.is_file() or "node_modules" in path.relative_to(root).parts:
            continue
        if language == "python" and (path.suffix != ".py" or path.name == "_version.py"):
            continue
        if language == "typescript" and path.suffix not in {".mjs", ".js"}:
            continue
        if path.stat().st_size > MAX_MEMBER_BYTES:
            raise SnapshotError("Runtime source file exceeds its inventory limit")
        result[path.relative_to(root).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    if not result:
        raise SnapshotError("No upstream runtime sources were inventoried")
    return result


def local_version(root: Path, language: str) -> str:
    try:
        if language == "python":
            import tomllib
            value = tomllib.loads((root / "pyproject.toml").read_text(encoding="utf-8"))["project"]["version"]
        else:
            value = json.loads((root / "package.json").read_text(encoding="utf-8"))["version"]
        return checked_version(value)
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise SnapshotError("Local source package does not declare an inspectable version") from error


def source_snapshot(root: Path, language: str, version: str, artifact: dict[str, str]) -> dict[str, Any]:
    hashes = runtime_hashes(root, language)
    return {"version": version, "artifact": artifact, "inventory": extract_inventory(root, language), "runtime_hashes": hashes}


def extractor_fingerprint() -> dict[str, Any]:
    try:
        files = {name: hashlib.sha256((HERE / name).read_bytes()).hexdigest() for name in FINGERPRINT_FILES}
        compiler = json.loads((HERE / "package.json").read_text(encoding="utf-8"))["dependencies"]["typescript"]
    except (OSError, ValueError, KeyError) as error:
        raise SnapshotError("Could not fingerprint the pinned inventory extractors") from error
    return {"schema_version": 1, "files": files, "typescript_version": compiler, "python_ast_version": f"{sys.version_info.major}.{sys.version_info.minor}"}


def validate_snapshot(snapshot: Any) -> None:
    if not isinstance(snapshot, dict) or snapshot.get("schema_version") != 1 or set(snapshot.get("sources", {})) != set(LANGUAGES):
        raise SnapshotError("A review baseline must contain both source languages and schema_version 1")
    fingerprint = snapshot.get("extractor_fingerprint")
    if not isinstance(fingerprint, dict) or fingerprint.get("schema_version") != 1 or set(fingerprint.get("files", {})) != set(FINGERPRINT_FILES):
        raise SnapshotError("Snapshot lacks a complete extractor fingerprint; regenerate/review the same pinned-source baseline")
    if any(not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest) for digest in fingerprint["files"].values()):
        raise SnapshotError("Snapshot extractor fingerprint contains an invalid digest")
    if not isinstance(fingerprint.get("typescript_version"), str) or not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", fingerprint["typescript_version"]):
        raise SnapshotError("Snapshot must pin the TypeScript compiler version")
    if not isinstance(fingerprint.get("python_ast_version"), str) or not re.fullmatch(r"[0-9]+\.[0-9]+", fingerprint["python_ast_version"]):
        raise SnapshotError("Snapshot must record the Python AST version")
    for language, source in snapshot["sources"].items():
        if not isinstance(source, dict):
            raise SnapshotError("Malformed source snapshot")
        checked_version(source.get("version", ""))
        artifact = source.get("artifact")
        if not isinstance(artifact, dict) or not (artifact.get("kind") == "local" or {"url", "integrity"} <= artifact.keys()):
            raise SnapshotError("A source snapshot lacks artifact provenance")
        if "url" in artifact:
            validate_registry_url(artifact["url"])
        validate_inventory(source.get("inventory"), language)
        hashes = source.get("runtime_hashes")
        if not isinstance(hashes, dict) or not hashes:
            raise SnapshotError("A review baseline lacks runtime hashes")
        for name, digest in hashes.items():
            relative_path(name)
            if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
                raise SnapshotError("A review baseline contains an invalid runtime hash")


def string_list(value: Any, name: str) -> list[str]:
    if not isinstance(value, list) or any(not isinstance(item, str) or not item for item in value) or len(set(value)) != len(value):
        raise MappingError(f"Mapping {name} must be a list of unique nonempty strings")
    return value


def selector_match(name: str, prefix: str) -> bool:
    return name == prefix or name.startswith(prefix + ".")


def resolve_mapping(features: list[dict[str, Any]], language: str, symbol: str) -> dict[str, Any] | None:
    matches = []
    for feature in features:
        selector = feature["upstream"].get(language, {"symbols": [], "prefixes": []})
        if symbol in selector["symbols"]:
            matches.append((sys.maxsize, feature))
        for prefix in selector["prefixes"]:
            if selector_match(symbol, prefix):
                matches.append((len(prefix), feature))
    if not matches:
        return None
    priority = max(priority for priority, _ in matches)
    chosen = {feature["id"]: feature for rank, feature in matches if rank == priority}
    if len(chosen) != 1:
        raise MappingError("Two compatibility features claim the same source symbol at equal priority")
    return next(iter(chosen.values()))


def rust_code(text: str) -> str:
    """Mask comments/literals before locating function evidence, without parsing Rust."""
    masked = list(text)
    position = 0
    while position < len(text):
        end = position
        if text.startswith("//", position):
            end = text.find("\n", position)
            if end < 0:
                end = len(text)
        elif text.startswith("/*", position):
            depth = 1
            end = position + 2
            while depth and end < len(text):
                if text.startswith("/*", end):
                    depth += 1
                    end += 2
                elif text.startswith("*/", end):
                    depth -= 1
                    end += 2
                else:
                    end += 1
            if depth:
                raise MappingError("Rust evidence contains an unterminated block comment")
        elif raw := re.match(r'(?:b|c)?r(#{0,255})"', text[position:]):
            closing = '"' + raw[1]
            tail = text.find(closing, position + raw.end())
            if tail < 0:
                raise MappingError("Rust evidence contains an unterminated raw string")
            end = tail + len(closing)
        elif text[position] == '"':
            end = position + 1
            while end < len(text):
                if text[end] == "\\":
                    end += 2
                elif text[end] == '"':
                    end += 1
                    break
                else:
                    end += 1
            else:
                raise MappingError("Rust evidence contains an unterminated string")
        elif character := re.match(r"'(?:\\.|[^'\\\n])'", text[position:]):
            end = position + character.end()
        if end > position:
            masked[position:end] = ["\n" if char == "\n" else " " for char in text[position:end]]
            position = end
        else:
            position += 1
    return "".join(masked)


def validate_mapping(mapping: Any, baseline: dict[str, Any], repository: Path = ROOT) -> list[dict[str, Any]]:
    if not isinstance(mapping, dict) or mapping.get("schema_version") != 1 or not isinstance(mapping.get("features"), list):
        raise MappingError("Unsupported compatibility mapping schema")
    features = mapping["features"]
    ids = set()
    for feature in features:
        if not isinstance(feature, dict) or not isinstance(feature.get("id"), str) or not feature["id"] or feature["id"] in ids:
            raise MappingError("Mapping feature IDs must be unique nonempty strings")
        ids.add(feature["id"])
        if feature.get("status") not in STATUSES or not isinstance(feature.get("rationale"), str) or not feature["rationale"].strip():
            raise MappingError("Every mapping feature requires an explicit status and rationale")
        string_list(feature.get("rust_api"), "rust_api")
        upstream = feature.get("upstream")
        if not isinstance(upstream, dict) or not upstream or not set(upstream) <= set(LANGUAGES):
            raise MappingError("Every mapping feature requires known upstream languages")
        claims = 0
        for language, selector in upstream.items():
            if not isinstance(selector, dict) or set(selector) != {"symbols", "prefixes"}:
                raise MappingError("Each source selector requires symbols and prefixes")
            symbols = string_list(selector["symbols"], "symbols")
            prefixes = string_list(selector["prefixes"], "prefixes")
            baseline_symbols = baseline["sources"][language]["inventory"]["symbols"]
            for symbol in symbols:
                if symbol not in baseline_symbols:
                    raise MappingError("Mapping refers to a symbol absent from its reviewed baseline")
            for prefix in prefixes:
                if prefix.endswith(".") or not any(selector_match(symbol, prefix) for symbol in baseline_symbols):
                    raise MappingError("Mapping prefix is absent from the reviewed baseline or has a trailing dot")
            claims += len(symbols) + len(prefixes)
        if claims == 0:
            raise MappingError("Mapping feature has no upstream selectors")
        evidence = feature.get("tests")
        if not isinstance(evidence, list):
            raise MappingError("Mapping tests must be a list of evidence references")
        if feature["status"] == "supported" and (not feature["rust_api"] or not evidence):
            raise MappingError("Supported mappings require Rust API and named test evidence")
        for test in evidence:
            if not isinstance(test, dict) or set(test) != {"path", "name"} or not isinstance(test["name"], str) or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", test["name"]):
                raise MappingError("Each test reference requires a relative file and Rust function name")
            try:
                relative = relative_path(test["path"])
                path = repository.joinpath(*relative.parts).resolve(strict=True)
                if not path.is_relative_to(repository.resolve()) or path.suffix != ".rs" or not path.is_file():
                    raise MappingError("Test evidence must resolve to a Rust file within the repository")
                text = path.read_text(encoding="utf-8")
            except (OSError, SnapshotError) as error:
                raise MappingError("A mapped Rust test evidence file is missing or outside the repository") from error
            if not re.search(r"\bfn\s+" + re.escape(test["name"]) + r"\s*\(", rust_code(text)):
                raise MappingError("A mapped Rust test function is absent from its evidence file")
    for language in LANGUAGES:
        for symbol in baseline["sources"][language]["inventory"]["symbols"]:
            resolve_mapping(features, language, symbol)
    return features


def comparison(baseline: dict[str, Any], current: dict[str, Any], features: list[dict[str, Any]]) -> dict[str, Any]:
    validate_snapshot(baseline)
    validate_snapshot(current)
    if canonical(baseline["extractor_fingerprint"]) != canonical(current["extractor_fingerprint"]):
        raise SnapshotError("Inventory extractor changed; regenerate/review the same pinned-source baseline before attributing differences to upstream")
    changes = {}
    unreviewed = {}
    for language in LANGUAGES:
        before = baseline["sources"][language]
        after = current["sources"][language]
        old_symbols = before["inventory"]["symbols"]
        new_symbols = after["inventory"]["symbols"]
        api = []
        for symbol in sorted(set(old_symbols) | set(new_symbols)):
            old = old_symbols.get(symbol)
            new = new_symbols.get(symbol)
            if old is None:
                kind = "added"
            elif new is None:
                kind = "removed"
            elif canonical({key: old[key] for key in ("kind", "shape")}) != canonical({key: new[key] for key in ("kind", "shape")}):
                kind = "changed"
            else:
                continue
            mapped = resolve_mapping(features, language, symbol)
            evidence = {key: mapped[key] for key in ("rust_api", "tests", "rationale")} if mapped else None
            api.append({"symbol": symbol, "change": kind, "before": old, "after": new, "mapping": mapped["id"] if mapped else None, "mapping_status": mapped["status"] if mapped else "unreviewed", "rust_evidence": evidence, "needs_review": True})
        old_hashes, new_hashes = before["runtime_hashes"], after["runtime_hashes"]
        runtime = [{"path": path, "before": old_hashes.get(path), "after": new_hashes.get(path)} for path in sorted(set(old_hashes) | set(new_hashes)) if old_hashes.get(path) != new_hashes.get(path)]
        integrity_changed = bool(before["artifact"].get("integrity") and after["artifact"].get("integrity") and before["artifact"]["integrity"] != after["artifact"]["integrity"])
        changes[language] = {"reviewed_version": before["version"], "observed_version": after["version"], "version_changed": before["version"] != after["version"], "registry_integrity_changed": integrity_changed, "api_changes": api, "runtime_content_changes": runtime, "reviewed_artifact": before["artifact"], "observed_artifact": after["artifact"], "artifact": after["artifact"]}
        unreviewed[language] = {"reviewed_baseline": sorted(symbol for symbol in old_symbols if resolve_mapping(features, language, symbol) is None), "observed": sorted(symbol for symbol in new_symbols if resolve_mapping(features, language, symbol) is None)}
    gaps = sorted((feature for feature in features if feature["status"] in {"partial", "deferred"}), key=lambda feature: feature["id"])
    exceptions = sorted((feature for feature in features if feature["status"] in {"out_of_scope", "rust_difference"}), key=lambda feature: feature["id"])
    drift = any(source["version_changed"] or source["registry_integrity_changed"] or source["api_changes"] or source["runtime_content_changes"] for source in changes.values())
    attention = drift or bool(gaps) or any(value["observed"] for value in unreviewed.values())
    return {"schema_version": 1, "upstream_drift": drift, "needs_attention": attention, "reviewed_extractor_fingerprint": baseline["extractor_fingerprint"], "observed_extractor_fingerprint": current["extractor_fingerprint"], "sources": changes, "unreviewed_rust_mappings": unreviewed, "declared_known_gaps": gaps, "intentional_differences": exceptions, "limits": ["Static API shapes and source hashes do not prove runtime or behavioral parity.", "Runtime content changes trigger behavior review; they are not proven semantic changes.", "Unreviewed Rust mappings are inventory review work, not proof that functionality is missing.", "Rust mappings are reviewed design/evidence references, not generated type equivalence or an instruction to copy upstream APIs.", "A named test reference proves the test exists, not that it passed in this workflow. Run the Rust SDK suite when changing functionality."]}


def inline(value: str) -> str:
    return "`" + value.replace("`", "'").replace("|", "\\|").replace("\n", " ") + "`"


def markdown(report: dict[str, Any], run_url: str | None = None, bounded: bool = False) -> str:
    lines = [ISSUE_MARKER, "# Claude SDK compatibility review", "", "This monitor reports upstream changes and reviewed Rust coverage. It does not edit the SDK or accept a new baseline.", "", "| Source | Reviewed | Observed | API additions / changes / removals | Runtime files changed |", "| --- | --- | --- | --- | --- |"]
    for language, source in report["sources"].items():
        counts = {kind: sum(change["change"] == kind for change in source["api_changes"]) for kind in ("added", "changed", "removed")}
        lines.append(f"| {language} | {inline(source['reviewed_version'])} | {inline(source['observed_version'])} | {counts['added']} / {counts['changed']} / {counts['removed']} | {len(source['runtime_content_changes'])} |")
    lines += ["", "A version change with no structural diff still needs release review. Runtime content hashes are behavior-review triggers, including implementation-only changes; they do not establish semantic change.", "", "## Artifact provenance", ""]
    for language, source in report["sources"].items():
        for label in ("reviewed", "observed"):
            artifact = source[f"{label}_artifact"]
            description = f"{artifact['url']} ({artifact['integrity']})" if "url" in artifact else "local source snapshot"
            lines.append(f"- {language} {label}: {description}")
    fingerprint = report["observed_extractor_fingerprint"]
    lines += ["", f"Inventory tooling: Python AST {inline(fingerprint['python_ast_version'])}, TypeScript {inline(fingerprint['typescript_version'])}. Extractor/lock digests are recorded in the full JSON report; a tooling change requires same-release baseline review before upstream comparison.", "", "## API changes needing review", ""]
    limit = 100 if bounded else sys.maxsize
    for language, source in report["sources"].items():
        lines.append(f"### {language}")
        lines.append("")
        for change in source["api_changes"][:limit]:
            mapping = change["mapping"] or "unreviewed Rust mapping"
            lines.append(f"- {change['change']}: {inline(change['symbol'])}; mapping {inline(mapping)} ({change['mapping_status']}). Review is required even when the mapping was supported.")
            locations = []
            for label, entry in (("reviewed", change["before"]), ("observed", change["after"])):
                if entry:
                    locations.append(f"{label} {entry['source']['path']}:{entry['source']['line']}")
            lines.append("  - Source: " + "; ".join(inline(location) for location in locations))
            evidence = change["rust_evidence"]
            if evidence:
                lines.append("  - Rust API: " + (", ".join(inline(api) for api in evidence["rust_api"]) or "no implementation claimed"))
                lines.append("  - Named test evidence: " + (", ".join(inline(test['path'] + '::' + test['name']) for test in evidence["tests"]) or "none declared"))
                lines.append("  - Mapping rationale: " + evidence["rationale"])
        if not source["api_changes"]:
            lines.append("No structural API differences detected.")
        if len(source["api_changes"]) > limit:
            lines.append("Additional API changes are listed in the full report artifact.")
        lines += ["", "Runtime source content changes:", ""]
        lines += [f"- {inline(change['path'])}" for change in source["runtime_content_changes"][:limit]] or ["None detected."]
        if len(source["runtime_content_changes"]) > limit:
            lines.append("Additional changed files are listed in the full report artifact.")
        lines.append("")
    lines += ["## Declared known gaps", ""]
    for feature in report["declared_known_gaps"][:limit]:
        lines.append(f"- {inline(feature['id'])} ({feature['status']}): {feature['rationale']}")
    if not report["declared_known_gaps"]:
        lines.append("No partial/deferred features are declared in the reviewed mapping.")
    lines += ["", "## Unreviewed Rust mappings", "", "These symbols need a reviewed mapping. Their presence here does not establish missing functionality.", ""]
    for language, review in report["unreviewed_rust_mappings"].items():
        lines.append(f"- {language}: {len(review['reviewed_baseline'])} in the reviewed baseline; {len(review['observed'])} in the observed release.")
        lines += [f"  - {inline(symbol)}" for symbol in review["observed"][:limit]]
        if len(review["observed"]) > limit:
            lines.append("  - Remaining symbols are listed in the full report artifact.")
    lines += ["", "## Intentional Rust differences and scope", ""]
    lines += [f"- {inline(feature['id'])} ({feature['status']}): {feature['rationale']}" for feature in report["intentional_differences"][:limit]] or ["No explicit exceptions are recorded."]
    lines += ["", "## Review checklist", "", "- [ ] Inspect API changes, release notes and runtime content changes against the pinned sources.", "- [ ] Review unreviewed symbols and partial/deferred features without equating language-specific API shape to functionality.", "- [ ] Add idiomatic Rust types/behavior and public regression tests where needed, or record an explicit rationale and limitations.", "- [ ] Run the Rust SDK suite and any required native conformance checks; static extraction does not prove behavioral parity.", "- [ ] Update mapping and baseline together only after review, using the explicit baseline-write command.", "", "## Evidence limits", ""]
    lines += [f"- {limit_text}" for limit_text in report["limits"]]
    if run_url:
        lines += ["", f"Full JSON, source inventories and Markdown report: [workflow run artifacts]({run_url})."]
    else:
        lines += ["", "Full JSON, source inventories and Markdown report accompany this check's output directory."]
    lines += ["", "Generated by `.github/workflows/claude-sdk-freshness.yml`. Existing review issues stay open until a maintainer resolves them.", ""]
    body = "\n".join(lines)
    if bounded and len(body) > MAX_ISSUE_CHARS:
        suffix = f"\n\nIssue summary truncated. Full report and inventories: {run_url or 'see the workflow run artifacts'}.\n"
        body = body[:MAX_ISSUE_CHARS - len(suffix)] + suffix
    return body


def run_gh(args: list[str]) -> str:
    try:
        return subprocess.run(["gh", *args], check=True, text=True, capture_output=True, timeout=90).stdout
    except (OSError, subprocess.SubprocessError) as error:
        raise CommandError("GitHub issue synchronization failed; the report remains available locally") from error


def sync_issue(repository: str, report: dict[str, Any], body_path: Path) -> str:
    if not report["needs_attention"]:
        return "not-needed"
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise CommandError("Issue publication requires an explicit owner/repository")
    try:
        pages = json.loads(run_gh(["api", "--paginate", "--slurp", f"repos/{repository}/issues?state=open&per_page=100"]))
        issues = [
            issue for page in pages for issue in page
            if "pull_request" not in issue
            and ISSUE_MARKER in (issue.get("body") or "")
            and (issue.get("user") or {}).get("login") == ISSUE_AUTHOR
        ]
    except (ValueError, TypeError, KeyError) as error:
        raise CommandError("Could not parse existing GitHub review issues") from error
    if len(issues) > 1:
        raise CommandError("Multiple open SDK review issues have the same marker; refusing an ambiguous update")
    if issues:
        run_gh(["issue", "edit", str(issues[0]["number"]), "--repo", repository, "--title", ISSUE_TITLE, "--body-file", str(body_path)])
        return "updated"
    run_gh(["issue", "create", "--repo", repository, "--title", ISSUE_TITLE, "--body-file", str(body_path)])
    return "created"


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument("--baseline", type=Path, default=HERE / "baseline.json")
    command.add_argument("--mapping", type=Path, default=HERE / "mapping.json")
    command.add_argument("--python-source", type=Path)
    command.add_argument("--typescript-package", type=Path)
    command.add_argument("--python-version")
    command.add_argument("--typescript-version")
    command.add_argument("--write-baseline", type=Path, help="Explicitly write a candidate baseline; never used by the periodic check")
    command.add_argument("--out", type=Path, required=True)
    command.add_argument("--file-issue", action="store_true")
    command.add_argument("--repository", default=os.environ.get("GITHUB_REPOSITORY", ""))
    command.add_argument("--run-url")
    return command


def main(argv: list[str] | None = None) -> int:
    arguments = parser().parse_args(argv)
    try:
        if bool(arguments.python_source) != bool(arguments.typescript_package):
            raise SnapshotError("Local inspection requires both Python and TypeScript source directories")
        if arguments.write_baseline and arguments.file_issue:
            raise SnapshotError("Writing a candidate baseline cannot also publish an issue")
        if arguments.run_url and not re.fullmatch(r"https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+/actions/runs/[0-9]+", arguments.run_url):
            raise SnapshotError("Workflow artifact link must name a GitHub Actions run")
        arguments.out.mkdir(parents=True, exist_ok=True)
        with ExitStack() as stack:
            scratch = Path(stack.enter_context(tempfile.TemporaryDirectory(prefix="claude-sdk-freshness-")))
            sources = {}
            for language, root, requested in (("python", arguments.python_source, arguments.python_version), ("typescript", arguments.typescript_package, arguments.typescript_version)):
                if root is None:
                    root, version, artifact = (fetch_python if language == "python" else fetch_typescript)(scratch, requested)
                else:
                    version = local_version(root, language)
                    if requested is not None and version != checked_version(requested):
                        raise SnapshotError("Local source version differs from the requested release")
                    artifact = {"kind": "local"}
                sources[language] = source_snapshot(root, language, version, artifact)
            current = {"schema_version": 1, "extractor_fingerprint": extractor_fingerprint(), "sources": sources}
            validate_snapshot(current)
            write_json(arguments.out / "observed.json", current)
            if arguments.write_baseline:
                arguments.write_baseline.parent.mkdir(parents=True, exist_ok=True)
                write_json(arguments.write_baseline, current)
                print("Candidate baseline written explicitly. Review mappings, behavior changes and tests before accepting it.")
                return 0
            baseline = read_json(arguments.baseline)
            validate_snapshot(baseline)
            features = validate_mapping(read_json(arguments.mapping), baseline)
            report = comparison(baseline, current, features)
            write_json(arguments.out / "report.json", report)
            (arguments.out / "report.md").write_text(markdown(report, arguments.run_url), encoding="utf-8")
            body_path = arguments.out / "issue.md"
            body_path.write_text(markdown(report, arguments.run_url, bounded=True), encoding="utf-8")
            print(f"Claude SDK review: upstream_drift={str(report['upstream_drift']).lower()} needs_attention={str(report['needs_attention']).lower()}")
            if arguments.file_issue:
                print(f"GitHub issue: {sync_issue(arguments.repository, report, body_path)}")
            return 0
    except (CheckError, OSError, ValueError, TypeError, KeyError) as error:
        message = str(error) if isinstance(error, CheckError) else "Could not process monitor inputs or write report outputs"
        failure = {"schema_version": 1, "status": "failed", "error_kind": type(error).__name__, "message": message, "upstream_drift": None}
        try:
            arguments.out.mkdir(parents=True, exist_ok=True)
            write_json(arguments.out / "failure.json", failure)
        except OSError:
            pass
        print(f"Claude SDK freshness check failed: {message}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
