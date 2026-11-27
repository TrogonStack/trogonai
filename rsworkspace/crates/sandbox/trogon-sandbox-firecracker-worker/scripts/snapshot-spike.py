#!/usr/bin/env python3
"""Validate portable disk and RAM restoration against a real Firecracker binary."""

import argparse
import http.client
import json
import pathlib
import shutil
import signal
import socket
import subprocess
import tempfile
import time


class UnixHttp(http.client.HTTPConnection):
    def __init__(self, path, vsock=False):
        super().__init__("localhost", timeout=15)
        self.path = str(path)
        self.vsock = vsock

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(self.timeout)
        self.sock.connect(self.path)
        if self.vsock:
            self.sock.sendall(b"CONNECT 8110\n")
            response = b""
            while not response.endswith(b"\n"):
                chunk = self.sock.recv(1)
                if not chunk:
                    raise RuntimeError("vsock handshake closed")
                response += chunk
                if len(response) > 64:
                    raise RuntimeError("oversized vsock handshake")
            if not response.startswith(b"OK "):
                raise RuntimeError(f"vsock handshake: {response!r}")


def call(path, method, route, body=None, vsock=False):
    connection = UnixHttp(path, vsock)
    try:
        connection.request(method, route, None if body is None else json.dumps(body), {"Content-Type": "application/json"})
        response = connection.getresponse()
        payload = response.read()
        if response.status >= 300:
            raise RuntimeError(f"{route}: HTTP {response.status}: {payload!r}")
        return payload
    finally:
        connection.close()


def ready(vm, process):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"VMM exited: {(vm / 'vmm.log').read_text()[-2000:]}")
        try:
            call(vm / "v.sock", "GET", "/readyz", vsock=True)
            return
        except (OSError, http.client.HTTPException, RuntimeError):
            time.sleep(0.025)
    raise TimeoutError("guest did not answer within 30 seconds")


def launch(binary, vm):
    vm.mkdir()
    with (vm / "vmm.log").open("wb") as log:
        process = subprocess.Popen([str(binary), "--api-sock", str(vm / "api.sock")], cwd=vm, stdout=log, stderr=subprocess.STDOUT)
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError("VMM exited before opening API")
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as probe:
                probe.connect(str(vm / "api.sock"))
            return process
        except OSError:
            time.sleep(0.025)
    stop(process)
    raise TimeoutError("VMM API did not appear within 5 seconds")


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=3)


def copy_disk(source, destination):
    subprocess.run(["cp", "--reflink=auto", "--sparse=always", str(source), str(destination)], check=True, timeout=30)


def execute(vm, lease, command):
    call(vm / "v.sock", "POST", "/_sandbox/v1/lease/bind", {"lease_id": lease, "env": {}, "working_dir": "/"}, True)
    raw = call(vm / "v.sock", "POST", "/_sandbox/v1/exec", {
        "lease_id": lease, "command": ["sh", "-c", command], "cwd": "/", "env": {},
        "timeout_seconds": 5, "detached": False, "stream_output": False,
    }, True)
    responses = [json.loads(line) for line in raw.splitlines() if line]
    output = next((frame for frame in responses if frame.get("outcome") == "completed"), None)
    if output is None:
        output = next((frame for frame in responses if frame.get("status") == "completed"), None)
    if output is None or output.get("exit_code") != 0:
        raise RuntimeError(f"guest command failed: {raw!r}")
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--firecracker", required=True, type=pathlib.Path)
    parser.add_argument("--kernel", required=True, type=pathlib.Path)
    parser.add_argument("--rootfs", required=True, type=pathlib.Path)
    parser.add_argument("--state-dir", required=True, type=pathlib.Path)
    args = parser.parse_args()
    args.state_dir.mkdir(parents=True, exist_ok=True)
    signal.signal(signal.SIGALRM, lambda *_: (_ for _ in ()).throw(TimeoutError("snapshot spike exceeded 120 seconds")))
    signal.alarm(120)
    active = []
    with tempfile.TemporaryDirectory(prefix="fc-snapshot-", dir=args.state_dir.resolve()) as scratch:
        root = pathlib.Path(scratch)
        try:
            vm = root / "source"
            process = launch(args.firecracker.resolve(), vm)
            active.append(process)
            copy_disk(args.rootfs.resolve(), vm / "rootfs.ext4")
            for route, body in [
                ("/boot-source", {"kernel_image_path": str(args.kernel.resolve()), "boot_args": "console=ttyS0 reboot=k panic=1 ate.actor_id=snapshot-spike"}),
                ("/drives/rootfs", {"drive_id": "rootfs", "path_on_host": "rootfs.ext4", "is_root_device": True, "is_read_only": False}),
                ("/machine-config", {"vcpu_count": 2, "mem_size_mib": 512}),
                ("/vsock", {"guest_cid": 3, "uds_path": "v.sock"}),
                ("/actions", {"action_type": "InstanceStart"}),
            ]:
                call(vm / "api.sock", "PUT", route, body)
            ready(vm, process)
            execute(vm, "source", "printf disk-proof > /snapshot-disk; printf ram-proof > /run/snapshot-ram; sync")
            call(vm / "api.sock", "PATCH", "/vm", {"state": "Paused"})
            started = time.monotonic()
            call(vm / "api.sock", "PUT", "/snapshot/create", {"snapshot_type": "Full", "snapshot_path": str(root / "state"), "mem_file_path": str(root / "memory")})
            create_ms = (time.monotonic() - started) * 1000
            copy_disk(vm / "rootfs.ext4", root / "rootfs.ext4")
            stop(process)
            shutil.rmtree(vm)
            samples = []
            for iteration in range(3):
                restored = root / f"restored-{iteration}"
                process = launch(args.firecracker.resolve(), restored)
                active.append(process)
                copy_disk(root / "rootfs.ext4", restored / "rootfs.ext4")
                started = time.monotonic()
                call(restored / "api.sock", "PUT", "/snapshot/load", {"snapshot_path": str(root / "state"), "mem_backend": {"backend_type": "File", "backend_path": str(root / "memory")}, "resume_vm": True})
                ready(restored, process)
                samples.append(round((time.monotonic() - started) * 1000, 2))
                execute(restored, f"restore-{iteration}", 'test "$(cat /snapshot-disk)" = disk-proof && test "$(cat /run/snapshot-ram)" = ram-proof && printf changed > /snapshot-disk && printf changed > /run/snapshot-ram')
                stop(process)
                shutil.rmtree(restored)
            print(json.dumps({"snapshot_create_ms": round(create_ms, 2), "snapshot_load_to_ready_ms": samples, "disk_and_ram_preserved": True, "independent_restores": True}))
        finally:
            for process in active:
                stop(process)
    signal.alarm(0)


if __name__ == "__main__":
    main()
