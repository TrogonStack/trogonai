"""Validate manifest structure without asserting artifact trust."""

import json
import subprocess
import sys

from validation_fixture import deployment_fixture

KUBERNETES_VERSION = "1.37.2"
SCHEMA_COMMIT = "102361970337d65537d40141dc29b991b8e95786"
SCHEMA_LOCATION = (
    "https://raw.githubusercontent.com/yannh/kubernetes-json-schema/"
    + SCHEMA_COMMIT
    + "/{{.NormalizedKubernetesVersion}}-standalone{{.StrictSuffix}}/{{.ResourceKind}}{{.KindSuffix}}.json"
)


def require_complete_validation(report, expected):
    if type(expected) is not int or expected <= 0 or not isinstance(report, dict):
        raise ValueError("schema validation did not establish complete coverage")
    summary = report.get("summary")
    required = {"valid": expected, "invalid": 0, "errors": 0, "skipped": 0}
    if not isinstance(summary, dict) or summary != required or any(type(value) is not int for value in summary.values()):
        raise ValueError("schema validation did not establish complete coverage")


def main():
    fixture = deployment_fixture()["resources"]
    result = subprocess.run([
        "kubeconform", "-strict", "-summary", "-output", "json",
        "-kubernetes-version", KUBERNETES_VERSION, "-schema-location", SCHEMA_LOCATION, "-",
    ], input=json.dumps(fixture), capture_output=True, text=True, timeout=120)
    print(result.stdout, end="")
    print(result.stderr, end="", file=sys.stderr)
    if result.returncode:
        raise SystemExit(result.returncode)
    require_complete_validation(json.loads(result.stdout), len(fixture["items"]))


if __name__ == "__main__":
    try:
        main()
    except ValueError as error:
        raise SystemExit(str(error)) from error
