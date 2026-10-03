#!/usr/bin/env python3

import argparse
import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib


def run(command, payload=None):
    return subprocess.run(
        command,
        input=payload,
        capture_output=True,
        text=True,
        check=False,
    )


def fail(message):
    print(message, file=sys.stderr)
    raise SystemExit(1)


def patched_payload(payload, patches):
    result = copy.deepcopy(payload)
    for patch in patches:
        segments = [
            part.replace("~1", "/").replace("~0", "~")
            for part in patch["path"].split("/")[1:]
        ]
        target = result
        for segment in segments[:-1]:
            target = target[int(segment)] if isinstance(target, list) else target[segment]
        key = segments[-1]
        operation = patch["op"]
        if operation not in ("add", "replace", "remove"):
            fail(f"Unsupported fixture patch operation: {operation}")
        if isinstance(target, list):
            index = len(target) if key == "-" else int(key)
            if operation == "add":
                target.insert(index, copy.deepcopy(patch["value"]))
            elif operation == "replace":
                target[index] = copy.deepcopy(patch["value"])
            else:
                del target[index]
        elif operation == "remove":
            del target[key]
        else:
            if operation == "replace" and key not in target:
                fail(f"Fixture patch cannot replace absent field: {patch['path']}")
            target[key] = copy.deepcopy(patch["value"])
    return result


def descriptor_messages(descriptor):
    messages = {}

    def collect(parent, message):
        name = f"{parent}.{message['name']}" if parent else message["name"]
        messages[name] = message
        for nested in message.get("nestedType", []):
            collect(name, nested)

    for file in descriptor["file"]:
        for message in file.get("messageType", []):
            collect(file.get("package", ""), message)
    return messages


def unknown_fields(payload, message_type, messages, path=""):
    if not isinstance(payload, dict):
        return []
    if message_type in (
        "google.protobuf.Struct",
        "google.protobuf.Value",
        "google.protobuf.ListValue",
    ):
        return []
    if message_type == "google.protobuf.Any":
        type_url = payload.get("@type", "")
        embedded_type = type_url.rsplit("/", 1)[-1]
        embedded = {key: value for key, value in payload.items() if key != "@type"}
        return unknown_fields(embedded, embedded_type, messages, path)
    if message_type not in messages:
        return [f"{path}: unknown message type {message_type}"]
    fields = {}
    for field in messages[message_type].get("field", []):
        fields[field["name"]] = field
        fields[field.get("jsonName", field["name"])] = field
    unknown = []
    for key, value in payload.items():
        location = f"{path}/{key}"
        if key not in fields:
            unknown.append(location)
            continue
        field = fields[key]
        if field.get("type") != "TYPE_MESSAGE":
            continue
        child_type = field["typeName"].lstrip(".")
        child = messages[child_type]
        if child.get("options", {}).get("mapEntry"):
            value_field = next(item for item in child["field"] if item["name"] == "value")
            if value_field.get("type") == "TYPE_MESSAGE" and isinstance(value, dict):
                for map_key, map_value in value.items():
                    unknown.extend(
                        unknown_fields(
                            map_value,
                            value_field["typeName"].lstrip("."),
                            messages,
                            f"{location}/{map_key}",
                        )
                    )
        elif field.get("label") == "LABEL_REPEATED" and isinstance(value, list):
            for index, item in enumerate(value):
                unknown.extend(unknown_fields(item, child_type, messages, f"{location}/{index}"))
        else:
            unknown.extend(unknown_fields(value, child_type, messages, location))
    return unknown


def main():
    parser = argparse.ArgumentParser(description="Validate workflow protobuf fixtures")
    parser.add_argument("--case", help="Run only case names containing this text")
    parser.add_argument("--buf", help="Override the Buf executable selected by mise")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[3]
    pinned_buf = tomllib.loads((root / ".mise.toml").read_text())["tools"]["buf"]
    buf = args.buf
    if not buf:
        selected = run(["mise", "which", "buf", "--tool", f"buf@{pinned_buf}"])
        if selected.returncode:
            fail(selected.stderr)
        buf = selected.stdout.strip()
    version = run([buf, "--version"])
    if version.returncode or version.stdout.strip() != pinned_buf:
        fail(f"Run with the repository's pinned Buf {pinned_buf} through mise.")

    fixtures = []
    for path in sorted((Path(__file__).parent / "fixtures").glob("*.json")):
        group = json.loads(path.read_text())
        templates = group["templates"]
        for case in group["cases"]:
            fixture = copy.deepcopy(case)
            template = templates[fixture["template"]]
            fixture["type"] = template["type"]
            fixture["payload"] = patched_payload(
                template["payload"], fixture.get("patch", [])
            )
            fixtures.append(fixture)
    names = [fixture["name"] for fixture in fixtures]
    if len(set(names)) != len(names):
        fail("Fixture names must be unique.")
    if args.case:
        fixtures = [fixture for fixture in fixtures if args.case in fixture["name"]]
    if not fixtures:
        fail("No workflow fixtures selected.")

    failures = []
    valid = 0
    invalid = 0
    with tempfile.TemporaryDirectory(prefix="workflow-proto-") as directory:
        image = Path(directory) / "workflows.json"
        build = run(
            [
                buf,
                "build",
                str(root),
                "--path",
                str(root / "proto/trogonai/workflows"),
                "--as-file-descriptor-set",
                "--exclude-source-info",
                "-o",
                str(image),
            ]
        )
        if build.returncode:
            fail(build.stderr)
        messages = descriptor_messages(json.loads(image.read_text()))
        for fixture in fixtures:
            unknown = unknown_fields(fixture["payload"], fixture["type"], messages)
            if unknown:
                failures.append(
                    f"{fixture['name']}: unknown fixture fields: {', '.join(unknown)}"
                )
                continue
            command = [
                buf,
                "convert",
                str(image),
                "--type",
                fixture["type"],
                "--from=-#format=json",
                "--to=-#format=json",
            ]
            payload = json.dumps(fixture["payload"])
            parsed = run(command, payload)
            if parsed.returncode:
                failures.append(f"{fixture['name']}: fixture cannot parse: {parsed.stderr}")
                continue
            validated = run([*command, "--validate"], payload)
            expected = fixture.get("violations", [])
            if not expected:
                valid += 1
                if validated.returncode:
                    failures.append(
                        f"{fixture['name']}: valid payload rejected: {validated.stderr}"
                    )
            else:
                invalid += 1
                if not validated.returncode:
                    failures.append(f"{fixture['name']}: invalid payload accepted")
                elif not any(
                    category in validated.stderr
                    for category in ("validation error:", "validation errors:")
                ):
                    failures.append(
                        f"{fixture['name']}: validation did not run: {validated.stderr}"
                    )
                else:
                    for violation in expected:
                        if violation not in validated.stderr:
                            failures.append(
                                f"{fixture['name']}: expected {violation!r}: "
                                f"{validated.stderr}"
                            )
    for failure in failures:
        print(failure, file=sys.stderr)
    print(
        f"Workflow protobuf fixtures: {valid} valid, {invalid} invalid, "
        f"{len(failures)} failures"
    )
    return bool(failures)


if __name__ == "__main__":
    sys.exit(main())
