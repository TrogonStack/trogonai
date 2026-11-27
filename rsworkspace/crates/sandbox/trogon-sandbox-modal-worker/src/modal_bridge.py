#!/usr/bin/env python3
"""One-shot JSON bridge onto Modal's official Python SDK.

Modal has no official Rust SDK, so the worker shells out to this script, which uses
only documented `modal` Python APIs.

Protocol: `argv[1]` is the op, the request object arrives as JSON on stdin, and exactly
one JSON object is written to stdout:

    {"ok": true, "result": {...}}
    {"ok": false, "error": {"kind": "...", "message": "..."}}

Credentials come from `MODAL_TOKEN_ID` / `MODAL_TOKEN_SECRET` in the environment; this
script never reads them itself and never echoes the environment back to the caller.
"""

import base64
import json
import sys


def _fail(kind, message):
    json.dump({"ok": False, "error": {"kind": kind, "message": str(message)}}, sys.stdout)
    sys.stdout.write("\n")
    sys.stdout.flush()
    sys.exit(0)


def _ok(result):
    json.dump({"ok": True, "result": result}, sys.stdout)
    sys.stdout.write("\n")
    sys.stdout.flush()
    sys.exit(0)


def _state_of(sandbox):
    """Map Modal's exit-code polling onto the state string the Rust side expects.

    `Sandbox.poll()` returns None while the sandbox is still running and the exit code
    once it has finished.
    """
    code = sandbox.poll()
    if code is None:
        return "running", None
    return "finished", code


def op_create(modal, req):
    app = modal.App.lookup(req["app_name"], create_if_missing=True)
    if req.get("image_id"):
        # A filesystem snapshot taken by `snapshot`; the id is the `modal.Image` object id.
        image = modal.Image.from_id(req["image_id"])
    else:
        image = modal.Image.from_registry(req["image"])

    kwargs = {
        "app": app,
        "image": image,
        "timeout": int(req["timeout_seconds"]),
        "workdir": req["workdir"],
    }
    if req.get("environment"):
        kwargs["environment_name"] = req["environment"]
    if req.get("env"):
        kwargs["env"] = dict(req["env"])
    if req.get("cpu_cores") is not None:
        kwargs["cpu"] = float(req["cpu_cores"])
    if req.get("memory_mib") is not None:
        kwargs["memory"] = int(req["memory_mib"])
    if req.get("region"):
        kwargs["region"] = req["region"]
    if req.get("block_network"):
        kwargs["block_network"] = True
    if req.get("outbound_cidr_allowlist"):
        kwargs["outbound_cidr_allowlist"] = list(req["outbound_cidr_allowlist"])
    if req.get("encrypted_ports"):
        kwargs["encrypted_ports"] = [int(p) for p in req["encrypted_ports"]]

    sandbox = modal.Sandbox.create(**kwargs)
    if req.get("tags"):
        sandbox.set_tags(dict(req["tags"]))

    state, exit_code = _state_of(sandbox)
    return {"id": sandbox.object_id, "state": state, "exit_code": exit_code}


def op_get(modal, req):
    sandbox = modal.Sandbox.from_id(req["sandbox_id"])
    state, exit_code = _state_of(sandbox)
    return {"id": req["sandbox_id"], "state": state, "exit_code": exit_code}


def op_exec(modal, req):
    sandbox = modal.Sandbox.from_id(req["sandbox_id"])
    process = sandbox.exec(*req["command"])
    stdout = process.stdout.read()
    stderr = process.stderr.read()
    exit_code = process.wait()
    return {"stdout": stdout, "stderr": stderr, "exit_code": exit_code}


def op_write_file(modal, req):
    sandbox = modal.Sandbox.from_id(req["sandbox_id"])
    remote_path = req["remote_path"]
    parent = remote_path.rsplit("/", 1)[0]
    if parent:
        sandbox.filesystem.make_directory(parent, create_parents=True)
    sandbox.filesystem.write_bytes(base64.b64decode(req["data_b64"]), remote_path)
    return {"remote_path": remote_path}


def op_terminate(modal, req):
    sandbox = modal.Sandbox.from_id(req["sandbox_id"])
    sandbox.terminate()
    return {"terminated": True}


def op_tunnels(modal, req):
    """Read the TLS tunnels Modal opened for the sandbox's `encrypted_ports`.

    `Sandbox.tunnels()` maps container port to a `Tunnel` carrying `host`, `port` and
    `url`. Some SDK releases expose `url` as a method rather than a property, so both
    are handled.
    """
    sandbox = modal.Sandbox.from_id(req["sandbox_id"])
    tunnels = []
    for port, tunnel in sandbox.tunnels(timeout=50).items():
        url = tunnel.url() if callable(tunnel.url) else tunnel.url
        tunnels.append(
            {"port": int(port), "host": tunnel.host, "tls_port": int(tunnel.port), "url": url}
        )
    return {"tunnels": tunnels}


def op_snapshot(modal, req):
    """Snapshot the sandbox filesystem and return the resulting image id.

    `Sandbox.snapshot_filesystem(timeout, *, ttl)` returns a `modal.Image` retained for
    `ttl` seconds; `Image.from_id` loads it back, which is what `create` does for
    `image_id`.
    """
    sandbox = modal.Sandbox.from_id(req["sandbox_id"])
    image = sandbox.snapshot_filesystem(
        timeout=int(req["timeout_seconds"]), ttl=int(req["ttl_seconds"])
    )
    return {"image_id": image.object_id}


OPS = {
    "create": op_create,
    "get": op_get,
    "exec": op_exec,
    "write_file": op_write_file,
    "terminate": op_terminate,
    "tunnels": op_tunnels,
    "snapshot": op_snapshot,
}


def main():
    if len(sys.argv) != 2:
        _fail("usage", "expected exactly one argument: the op name")
    op = sys.argv[1]
    if op not in OPS:
        _fail("usage", "unknown op: %s" % op)

    try:
        req = json.load(sys.stdin)
    except ValueError as exc:
        _fail("protocol", exc)

    try:
        import modal
    except ImportError as exc:
        _fail("missing_sdk", "the modal Python SDK is not importable: %s" % exc)

    try:
        _ok(OPS[op](modal, req))
    except SystemExit:
        raise
    except Exception as exc:
        _fail(type(exc).__name__, exc)


if __name__ == "__main__":
    main()
