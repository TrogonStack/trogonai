from dataclasses import dataclass

from gate_errors import format_boundary
from rust_references import require


@dataclass(frozen=True)
class CompiledInventory:
    selectors: frozenset[str]
    ignored: frozenset[str]
    filtered: frozenset[str]
    evidence = "compiled-and-listed"


@format_boundary("test-list")
def compiled_tests(document):
    selectors, ignored, filtered, seen = set(), set(), set(), set()
    suites = document["rust-suites"]
    require(isinstance(suites, dict), "test-list")
    for suite in suites.values():
        if suite["package-name"] != "trogon-claude-agent-sdk":
            continue
        require(suite["status"] == "listed", "test-list")
        binary = suite["binary-name"]
        require(isinstance(binary, str) and binary and "::" not in binary, "test-binary")
        require(isinstance(suite["testcases"], dict), "test-cases")
        for name, case in suite["testcases"].items():
            require(isinstance(name, str) and name and type(case["ignored"]) is bool
                    and case["kind"] == "test", "test-case")
            status = case["filter-match"]["status"]
            require(status in ("matches", "mismatch"), "test-filter")
            selector = binary + "::" + name
            require(selector not in seen, "duplicate-test-selector")
            seen.add(selector)
            if case["ignored"]:
                ignored.add(selector)
            elif status == "mismatch":
                filtered.add(selector)
            else:
                selectors.add(selector)
    return CompiledInventory(frozenset(selectors), frozenset(ignored), frozenset(filtered))
