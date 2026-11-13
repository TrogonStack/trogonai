import base64
import hashlib
import json
import re

from gate_errors import Code, GateError, format_boundary

VERSION = 1
SIDES = ("python", "typescript")
VERDICTS = ("agree", "disagree")
FOLLOW = ("follow-python", "follow-typescript")
KINDS = ("upstream", "rust-adaptation") + FOLLOW
ROW_KEYS = frozenset(
    ("id", "concept", "python", "typescript", "upstream", "decision", "contracts", "rust"))
ABSENT_KEYS = frozenset(("status", "reason"))
PYTHON_KEYS = frozenset(("file", "line_start", "line_end", "sha256"))
TYPESCRIPT_KEYS = frozenset(
    ("file", "anchor", "occurrence", "occurrences", "span_before", "span_after", "sha256"))
ROW_ID = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)*")
SHA256 = re.compile(r"[0-9a-f]{64}")
REVISION = re.compile(r"[0-9a-f]{40}")
RUST_PATH = re.compile(r"src/(?:[a-z0-9_]+/)*[a-z0-9_]+\.rs")


def require(condition, code, selector):
    if not condition:
        raise GateError(code, selector)


def load_json(data, selector):
    """Decode UTF-8 JSON bytes, rejecting duplicate object keys."""

    def pairs(items):
        keys = [key for key, _ in items]
        require(len(keys) == len(set(keys)), Code.FORMAT, selector)
        return dict(items)

    try:
        return json.loads(data.decode("utf-8"), object_pairs_hook=pairs)
    except (UnicodeDecodeError, ValueError, RecursionError) as error:
        if isinstance(error, GateError):
            raise
        raise GateError(Code.FORMAT, selector) from error


def _keys(value, expected, selector):
    require(isinstance(value, dict) and set(value) == set(expected), Code.FORMAT, selector)


def _text(value, selector):
    require(isinstance(value, str) and value.strip() == value and value, Code.FORMAT, selector)
    return value


def _count(value, selector, minimum=0):
    require(type(value) is int and value >= minimum, Code.FORMAT, selector)
    return value


def _sha256(value, selector):
    require(isinstance(value, str) and SHA256.fullmatch(value), Code.FORMAT, selector)
    return value


def _digest(data):
    return hashlib.sha256(data).hexdigest()


def _file_pin(value, selector, with_url):
    _keys(value, ("url", "sha256", "bytes") if with_url else ("sha256", "bytes"), selector)
    return _sha256(value["sha256"], selector + ".sha256"), _count(value["bytes"], selector + ".bytes")


def pinned_files(pins):
    """Return {(side, path): (sha256, byte_count)} after checking the pin document."""
    _keys(pins, ("version", "python", "typescript"), "pins")
    require(pins["version"] == VERSION, Code.FORMAT, "pins.version")
    files = {}

    typescript = pins["typescript"]
    _keys(typescript, ("package", "version", "url", "integrity", "members"), "pins.typescript")
    package = _text(typescript["package"], "pins.typescript.package")
    version = _text(typescript["version"], "pins.typescript.version")
    tarball = f"{package.rsplit('/', 1)[-1]}-{version}.tgz"
    require(typescript["url"] == f"https://registry.npmjs.org/{package}/-/{tarball}",
            Code.PIN, "pins.typescript.url")
    _integrity(typescript["integrity"])
    members = typescript["members"]
    require(isinstance(members, dict) and members, Code.FORMAT, "pins.typescript.members")
    for path, pin in members.items():
        require(isinstance(path, str) and path.startswith("package/"), Code.FORMAT,
                "pins.typescript.members")
        files["typescript", path] = _file_pin(pin, f"pins.typescript.{path}", False)

    python = pins["python"]
    _keys(python, ("repository", "revision", "files"), "pins.python")
    repository = _text(python["repository"], "pins.python.repository")
    revision = python["revision"]
    require(isinstance(revision, str) and REVISION.fullmatch(revision), Code.FORMAT,
            "pins.python.revision")
    sources = python["files"]
    require(isinstance(sources, dict) and sources, Code.FORMAT, "pins.python.files")
    for path, pin in sources.items():
        require(isinstance(path, str) and path.startswith("src/") and ".." not in path.split("/"),
                Code.FORMAT, "pins.python.files")
        files["python", path] = _file_pin(pin, f"pins.python.{path}", True)
        require(pin["url"] == f"https://raw.githubusercontent.com/{repository}/{revision}/{path}",
                Code.PIN, f"pins.python.{path}.url")
    return files


def _integrity(value):
    selector = "pins.typescript.integrity"
    require(isinstance(value, str) and value.startswith("sha512-"), Code.FORMAT, selector)
    try:
        digest = base64.b64decode(value[len("sha512-"):], validate=True)
    except ValueError as error:
        raise GateError(Code.FORMAT, selector) from error
    require(len(digest) == 64, Code.FORMAT, selector)
    return digest


def verify_tarball(pins, data):
    """Check reviewed npm tarball bytes against the pinned SRI integrity."""
    require(isinstance(data, bytes), Code.FORMAT, "typescript.tarball")
    pinned_files(pins)
    expected = _integrity(pins["typescript"]["integrity"])
    require(hashlib.sha512(data).digest() == expected, Code.PIN, "typescript.tarball")


def verify_sources(pins, sources):
    """Check that sources holds exactly the pinned files with pinned bytes."""
    files = pinned_files(pins)
    _keys(sources, SIDES, "sources")
    supplied = set()
    for side in SIDES:
        require(isinstance(sources[side], dict), Code.FORMAT, f"sources.{side}")
        supplied.update((side, path) for path in sources[side])
    for side, path in sorted(supplied - set(files)):
        raise GateError(Code.PIN, f"{side}:{path}")
    for (side, path), (sha256, size) in sorted(files.items()):
        selector = f"{side}:{path}"
        require((side, path) in supplied, Code.PIN, selector)
        data = sources[side][path]
        require(isinstance(data, bytes), Code.FORMAT, selector)
        require(len(data) == size and _digest(data) == sha256, Code.PIN, selector)
    return files


def python_span(data, line_start, line_end):
    """Return the exact bytes of 1-based inclusive lines, newlines included."""
    lines = data.split(b"\n")
    terminated = data.endswith(b"\n")
    if terminated:
        lines.pop()
    if not 1 <= line_start <= line_end <= len(lines):
        return None
    span = b"\n".join(lines[line_start - 1:line_end])
    return span + b"\n" if line_end < len(lines) or terminated else span


def anchor_offsets(data, anchor):
    """Return every start offset of anchor, overlapping matches included."""
    offsets, start = [], 0
    while (found := data.find(anchor, start)) >= 0:
        offsets.append(found)
        start = found + 1
    return offsets


def typescript_span(data, anchor, occurrence, span_before, span_after):
    offsets = anchor_offsets(data, anchor)
    if occurrence >= len(offsets):
        return None
    start = offsets[occurrence] - span_before
    end = offsets[occurrence] + len(anchor) + span_after
    return data[start:end] if start >= 0 and end <= len(data) else None


def _absent(value, selector):
    if not (isinstance(value, dict) and value.get("status") == "absent"):
        return False
    _keys(value, ABSENT_KEYS, selector)
    _text(value["reason"], selector + ".reason")
    return True


def _python_evidence(value, files, sources, selector):
    if _absent(value, selector):
        return False
    _keys(value, PYTHON_KEYS, selector)
    path = value["file"]
    require(isinstance(path, str) and ("python", path) in files, Code.REFERENCE, selector + ".file")
    start = _count(value["line_start"], selector + ".line_start", 1)
    end = _count(value["line_end"], selector + ".line_end", 1)
    expected = _sha256(value["sha256"], selector + ".sha256")
    span = python_span(sources["python"][path], start, end)
    require(span is not None, Code.REFERENCE, selector + ".lines")
    require(_digest(span) == expected, Code.SHAPE, selector + ".sha256")
    return True


def _typescript_evidence(value, files, sources, selector):
    if _absent(value, selector):
        return False
    _keys(value, TYPESCRIPT_KEYS, selector)
    path = value["file"]
    require(isinstance(path, str) and ("typescript", path) in files, Code.REFERENCE,
            selector + ".file")
    anchor = _text(value["anchor"], selector + ".anchor").encode("utf-8")
    occurrences = _count(value["occurrences"], selector + ".occurrences", 1)
    occurrence = _count(value["occurrence"], selector + ".occurrence")
    require(occurrence < occurrences, Code.FORMAT, selector + ".occurrence")
    before = _count(value["span_before"], selector + ".span_before")
    after = _count(value["span_after"], selector + ".span_after")
    expected = _sha256(value["sha256"], selector + ".sha256")
    data = sources["typescript"][path]
    found = len(anchor_offsets(data, anchor))
    require(found > 0, Code.REFERENCE, selector + ".anchor")
    require(found == occurrences, Code.REFERENCE, selector + ".occurrences")
    span = typescript_span(data, anchor, occurrence, before, after)
    require(span is not None, Code.REFERENCE, selector + ".span")
    require(_digest(span) == expected, Code.SHAPE, selector + ".sha256")
    return True


def _decision(row, selector):
    verdict, decision = row["upstream"]["verdict"], row["decision"]
    require(isinstance(decision, dict) and decision.get("kind") in KINDS, Code.FORMAT, selector)
    kind = decision["kind"]
    if kind == "upstream":
        _keys(decision, ("kind",), selector)
    else:
        require(set(decision) <= {"kind", "reason"}, Code.FORMAT, selector)
        reason = decision.get("reason")
        require(isinstance(reason, str) and reason.strip(), Code.MAPPING, selector + ".reason")
    if verdict == "agree":
        require(kind not in FOLLOW, Code.MAPPING, selector)
    else:
        require(kind in FOLLOW, Code.MAPPING, selector)


def _strings(value, selector, pattern=None, nonempty=False):
    require(isinstance(value, list) and (value or not nonempty), Code.FORMAT, selector)
    for item in value:
        _text(item, selector)
        require(pattern is None or pattern.fullmatch(item), Code.FORMAT, selector)
    require(len(value) == len(set(value)), Code.FORMAT, selector)
    return value


@format_boundary("behaviors")
def validate(behaviors, pins, sources, require_contracts=False):
    """Validate the mapping against pinned upstream bytes; return row ids in order."""
    files = verify_sources(pins, sources)
    _keys(behaviors, ("version", "rows"), "behaviors")
    require(behaviors["version"] == VERSION, Code.FORMAT, "behaviors.version")
    rows = behaviors["rows"]
    require(isinstance(rows, list) and rows, Code.FORMAT, "behaviors.rows")
    ids, cited = [], set()
    for index, row in enumerate(rows):
        _keys(row, ROW_KEYS, f"behaviors.rows[{index}]")
        row_id = row["id"]
        require(isinstance(row_id, str) and ROW_ID.fullmatch(row_id), Code.FORMAT,
                f"behaviors.rows[{index}].id")
        require(row_id not in ids, Code.MAPPING, row_id + ".id")
        ids.append(row_id)
        _text(row["concept"], row_id + ".concept")
        if _python_evidence(row["python"], files, sources, row_id + ".python"):
            cited.add(("python", row["python"]["file"]))
        if _typescript_evidence(row["typescript"], files, sources, row_id + ".typescript"):
            cited.add(("typescript", row["typescript"]["file"]))
        _keys(row["upstream"], ("verdict", "behavior"), row_id + ".upstream")
        require(row["upstream"]["verdict"] in VERDICTS, Code.FORMAT, row_id + ".upstream.verdict")
        _text(row["upstream"]["behavior"], row_id + ".upstream.behavior")
        _decision(row, row_id + ".decision")
        contracts = _strings(row["contracts"], row_id + ".contracts")
        require(contracts or not require_contracts, Code.MAPPING, row_id + ".contracts")
        _strings(row["rust"], row_id + ".rust", RUST_PATH, nonempty=True)
    for side, path in sorted(set(files) - cited):
        raise GateError(Code.MAPPING, f"{side}:{path}")
    return tuple(ids)


@format_boundary("behaviors")
def contracts_check(behaviors, compiled_selectors):
    """Require every mapped contract to name a compiled, listed Rust test."""
    require(isinstance(compiled_selectors, (set, frozenset)), Code.FORMAT, "compiled-selectors")
    for row in behaviors["rows"]:
        for selector in row["contracts"]:
            require(selector in compiled_selectors, Code.REFERENCE,
                    f"{row['id']}.contracts.{selector}")
