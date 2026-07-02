#!/usr/bin/env python3
"""Deterministic GitHub Actions workflow generator for this repo.

The source of truth is ``.github/ci/workflows.toml`` plus the pinned action
registry ``.github/ci/actions.toml``. This script renders the YAML under
``.github/workflows`` for the workflows listed in the manifest, so those
workflows share one reviewed action-pin table instead of being hand pinned
independently.

Usage::

    python3 devops/ci/generate_workflows.py --write   # regenerate YAML
    python3 devops/ci/generate_workflows.py --check   # fail on drift

Scope (WI-14 in MS_AGENT_GOV_TOOLKIT_WORKITEMS.md): this generator only
covers the workflows listed under ``[[workflow]]`` in workflows.toml. The
hand-written workflows listed under ``unmanaged`` in that file are
intentionally out of scope, same as upstream AGT's own generator covers one
workflow out of roughly forty hand-written files. The pattern is what
transfers here, not a full migration.

The renderer is deterministic and dependency free (stdlib only). It enforces
full SHA pinned actions resolved from the registry and fails closed on any
manifest error.
"""

from __future__ import annotations

import argparse
import sys
import tomllib
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[2]
MANIFEST_PATH = REPO_ROOT / ".github" / "ci" / "workflows.toml"
ACTIONS_PATH = REPO_ROOT / ".github" / "ci" / "actions.toml"

SHA_RE_LEN = 40


class GenerationError(Exception):
    """Raised when the manifest or registry is invalid."""


def _load_toml(path: Path) -> dict[str, Any]:
    try:
        with path.open("rb") as handle:
            return tomllib.load(handle)
    except FileNotFoundError as exc:
        raise GenerationError(f"missing required file: {path}") from exc
    except tomllib.TOMLDecodeError as exc:
        raise GenerationError(f"invalid TOML in {path}: {exc}") from exc


def _load_actions(path: Path) -> dict[str, str]:
    """Return a map of action registry key to its pinned `uses` value."""
    raw = _load_toml(path)
    registry: dict[str, str] = {}
    for key, entry in raw.items():
        if not isinstance(entry, dict) or "uses" not in entry:
            raise GenerationError(f"action '{key}' must define a uses value")
        uses = entry["uses"]
        at_index = uses.rfind("@")
        if at_index == -1 or len(uses) - at_index - 1 != SHA_RE_LEN:
            raise GenerationError(
                f"action '{key}' is not pinned to a 40 character SHA: {uses}"
            )
        sha = uses[at_index + 1 :]
        if not all(c in "0123456789abcdef" for c in sha):
            raise GenerationError(
                f"action '{key}' is not pinned to a 40 character SHA: {uses}"
            )
        comment = entry.get("comment")
        registry[key] = f"{uses} # {comment}" if comment else uses
    return registry


def _resolve_uses(step: dict[str, Any], actions: dict[str, str]) -> str:
    key = step["uses"]
    if step.get("uses_raw"):
        return key
    if key not in actions:
        raise GenerationError(f"step references unknown action registry key: {key}")
    return actions[key]


def _render_with(with_map: dict[str, Any], indent: str) -> list[str]:
    lines = [f"{indent}with:"]
    for k, v in with_map.items():
        lines.append(f"{indent}  {k}: {v}")
    return lines


def _render_env(env_map: dict[str, Any], indent: str) -> list[str]:
    lines = [f"{indent}env:"]
    for k, v in env_map.items():
        lines.append(f"{indent}  {k}: {v}")
    return lines


def _render_run_body(run_body: str, indent: str) -> list[str]:
    body = run_body.strip("\n")
    if "\n" in body:
        lines = [f"{indent}run: |"]
        for line in body.splitlines():
            lines.append(f"{indent}  {line}" if line else "")
        return lines
    return [f"{indent}run: {body}"]


def _render_step(step: dict[str, Any], actions: dict[str, str]) -> list[str]:
    """Render one step. Field order on the wire: name/id, if, uses, shell,
    with, run (+ working-directory), env - matching the hand-written source
    files this generator round-trips against."""
    indent = "      "
    name = step.get("name")
    step_id = step.get("id")
    uses = step.get("uses")
    run = step.get("run")

    lines: list[str] = []
    if name:
        lines.append(f"{indent}- name: {name}")
    elif step_id:
        lines.append(f"{indent}- id: {step_id}")
    elif uses:
        lines.append(f"{indent}- uses: {_resolve_uses(step, actions)}")
    prefix = f"{indent}  "

    if_cond = step.get("if_")
    if if_cond:
        lines.append(f"{prefix}if: {if_cond}")

    if uses and (name or step_id):
        lines.append(f"{prefix}uses: {_resolve_uses(step, actions)}")

    shell = step.get("shell")
    if shell:
        lines.append(f"{prefix}shell: {shell}")

    with_map = step.get("with")
    if with_map:
        lines += _render_with(with_map, prefix)

    working_directory = step.get("working_directory")
    working_directory_before_run = step.get("working_directory_before_run", False)
    if working_directory and working_directory_before_run:
        lines.append(f"{prefix}working-directory: {working_directory}")
    if run is not None:
        lines += _render_run_body(run, prefix)
        if working_directory and not working_directory_before_run:
            lines.append(f"{prefix}working-directory: {working_directory}")

    env_map = step.get("env")
    if env_map:
        lines += _render_env(env_map, prefix)

    return lines


def _render_permissions(perms: dict[str, Any] | None, indent: str = "") -> list[str]:
    if perms is None:
        return []
    if perms.get("read_all"):
        comment = perms.get("comment")
        lines = []
        if comment:
            for line in comment.splitlines():
                lines.append(f"{indent}# {line}")
        lines.append(f"{indent}permissions: read-all")
        return lines
    lines = [f"{indent}permissions:"]
    for scope, level in perms.items():
        if scope == "comment":
            continue
        lines.append(f"{indent}  {scope}: {level}")
    return lines


def _render_trigger(trigger: str, on: dict[str, Any]) -> list[str]:
    if trigger == "push":
        return ["  push:", "    branches: [" + ", ".join(on["push_branches"]) + "]"]
    if trigger == "workflow_dispatch":
        return ["  workflow_dispatch:"]
    if trigger == "schedule":
        quote = on.get("cron_quote", '"')
        return ["  schedule:"] + [
            f"    - cron: {quote}{cron}{quote}" for cron in on["schedule"]
        ]
    if trigger == "pull_request":
        lines = ["  pull_request:"]
        if "pull_request_types" in on:
            lines.append("    types: [" + ", ".join(on["pull_request_types"]) + "]")
        if "pull_request_branches" in on:
            lines.append("    branches: [" + ", ".join(on["pull_request_branches"]) + "]")
        if "pull_request_paths" in on:
            lines.append("    paths:")
            for path in on["pull_request_paths"]:
                lines.append(f"      - '{path}'")
        return lines
    raise GenerationError(f"unknown trigger: {trigger}")


def _default_trigger_order(on: dict[str, Any]) -> list[str]:
    order = []
    if "push_branches" in on:
        order.append("push")
    if "schedule" in on:
        order.append("schedule")
    if "pull_request_types" in on or "pull_request_branches" in on or "pull_request_paths" in on:
        order.append("pull_request")
    if on.get("workflow_dispatch"):
        order.append("workflow_dispatch")
    return order


def _render_on(on: dict[str, Any]) -> list[str]:
    lines = ["on:"]
    order = on.get("trigger_order") or _default_trigger_order(on)
    for trigger in order:
        lines += _render_trigger(trigger, on)
    return lines


def _render_job(job: dict[str, Any], actions: dict[str, str]) -> list[str]:
    job_id = job.get("id")
    if not job_id:
        raise GenerationError("every job needs an id")
    lines = [f"  {job_id}:"]
    if job.get("name"):
        lines.append(f"    name: {job['name']}")
    lines.append("    runs-on: ubuntu-latest")
    if job.get("permissions"):
        lines.append("    permissions:")
        for scope, level in job["permissions"].items():
            lines.append(f"      {scope}: {level}")
    lines.append("    steps:")
    steps = job.get("step", [])
    if not steps:
        raise GenerationError(f"job '{job_id}' has no steps")
    for step in steps:
        lines += _render_step(step, actions)
        lines.append("")
    if lines and lines[-1] == "":
        lines.pop()
    return lines


def render_workflow(workflow: dict[str, Any], actions: dict[str, str]) -> str:
    name = workflow.get("name")
    if not name:
        raise GenerationError("every workflow needs a name")
    output = workflow.get("output", "")
    if not output.startswith(".github/workflows/") or not output.endswith(".yml"):
        raise GenerationError(f"workflow '{name}' output must be a .github/workflows/*.yml path")

    lines: list[str] = []
    banner_comment = workflow.get("banner_comment")
    if banner_comment:
        lines.append(f"# {banner_comment}")
    lines.append(f"name: {name}")
    lines.append("")

    on = workflow.get("on", {})
    lines += _render_on(on)
    lines.append("")

    concurrency = workflow.get("concurrency")
    if concurrency:
        lines.append("concurrency:")
        lines.append(f"  group: {concurrency['group']}")
        lines.append(f"  cancel-in-progress: {str(concurrency['cancel_in_progress']).lower()}")
        lines.append("")

    permissions = workflow.get("permissions")
    lines += _render_permissions(permissions, "")
    lines.append("")

    lines.append("jobs:")
    jobs = workflow.get("job", [])
    if not jobs:
        raise GenerationError(f"workflow '{name}' has no jobs")
    for index, job in enumerate(jobs):
        if index:
            lines.append("")
        lines += _render_job(job, actions)

    return "\n".join(lines) + "\n"


def build_outputs() -> dict[Path, str]:
    actions = _load_actions(ACTIONS_PATH)
    manifest = _load_toml(MANIFEST_PATH)
    workflows = manifest.get("workflow", [])
    if not workflows:
        raise GenerationError("manifest defines no [[workflow]] entries")
    outputs: dict[Path, str] = {}
    seen_ids: set[str] = set()
    for workflow in workflows:
        wid = workflow.get("id", "")
        if wid in seen_ids:
            raise GenerationError(f"duplicate workflow id: {wid}")
        seen_ids.add(wid)
        outputs[REPO_ROOT / workflow["output"]] = render_workflow(workflow, actions)
    return outputs


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--write", action="store_true", help="write generated workflow YAML")
    group.add_argument("--check", action="store_true", help="fail if committed YAML drifts")
    args = parser.parse_args(argv)

    try:
        outputs = build_outputs()
    except GenerationError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    if args.write:
        for path, content in outputs.items():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")
            print(f"wrote {path.relative_to(REPO_ROOT)}")
        return 0

    drifted: list[str] = []
    for path, content in outputs.items():
        rel = path.relative_to(REPO_ROOT)
        if not path.exists():
            drifted.append(f"{rel} (missing, run --write)")
        elif path.read_text(encoding="utf-8") != content:
            drifted.append(f"{rel} (out of date, run --write)")
    if drifted:
        print("error: generated workflows are out of date:", file=sys.stderr)
        for item in drifted:
            print(f"  - {item}", file=sys.stderr)
        return 1
    print("generated workflows are up to date")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
