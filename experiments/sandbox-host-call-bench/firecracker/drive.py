#!/usr/bin/env python3
"""Times a Python hello world in Firecracker microVMs for firecracker.sh.

Every sample starts a new `firecracker` process and ends when the guest agent (agent.py, the
guest's init) has sent "ok" back over vsock. Prints tab-separated lines for firecracker.sh to
format: `row<TAB>metric<TAB>µs µs ...` or `failed<TAB>metric<TAB>reason`.
"""

import argparse
import contextlib
import http.client
import json
import pathlib
import shutil
import socket
import subprocess
import time

PORT = 5000
BOOT_ARGS = "console=ttyS0 reboot=k panic=1 pci=off quiet ro init=/fc-agent"


def now_us():
    return time.perf_counter_ns() // 1000


class UnixHttp(http.client.HTTPConnection):
    def __init__(self, path):
        super().__init__("localhost", timeout=30)
        self.unix_path = path

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(self.timeout)
        self.sock.connect(self.unix_path)


def api(vm, method, route, body):
    connection = UnixHttp(str(vm / "api.sock"))
    try:
        connection.request(method, route, json.dumps(body), {"Content-Type": "application/json"})
        response = connection.getresponse()
        payload = response.read()
        if response.status >= 300:
            raise RuntimeError(f"{method} {route}: HTTP {response.status}: {payload[:300]!r}")
    finally:
        connection.close()


def log_tail(vm):
    return (vm / "vmm.log").read_text(errors="replace")[-600:]


def wait_for(vm, process, what, attempt, timeout):
    """Calls attempt() until it returns a value, polling every 50 µs."""
    deadline = time.monotonic() + timeout
    while True:
        if process.poll() is not None:
            raise RuntimeError(f"firecracker exited before {what}: {log_tail(vm)}")
        try:
            return attempt()
        except OSError:
            if time.monotonic() > deadline:
                raise TimeoutError(f"no {what} within {timeout} s: {log_tail(vm)}")
            time.sleep(0.00005)


@contextlib.contextmanager
def firecracker(binary, vm):
    """Starts firecracker in a fresh vm directory (its sockets are relative to it), waits for the
    API socket, and on exit kills it and removes the directory."""
    vm.mkdir()
    with (vm / "vmm.log").open("wb") as log:
        process = subprocess.Popen([binary, "--api-sock", "api.sock"], cwd=vm, stdout=log, stderr=subprocess.STDOUT)
    try:

        def probe():
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
                sock.connect(str(vm / "api.sock"))

        wait_for(vm, process, "API socket", probe, 5)
        yield process
    finally:
        process.kill()
        process.wait()
        shutil.rmtree(vm)


def connect_agent(vm, process, timeout):
    """Opens a stream to the guest agent through Firecracker's hybrid vsock socket. Firecracker
    closes the stream when nothing listens on the port yet, which is retried."""

    def attempt():
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            sock.settimeout(30)
            sock.connect(str(vm / "v.sock"))
            sock.sendall(f"CONNECT {PORT}\n".encode())
            reply = sock.makefile("rb").readline()
            if not reply.startswith(b"OK "):
                raise ConnectionError(f"vsock handshake: {reply!r}")
            return sock
        except BaseException:
            sock.close()
            raise

    return wait_for(vm, process, "guest agent", attempt, timeout)


def ask(sock, mode):
    with sock:
        sock.sendall(mode.encode() + b"\n")
        reply = b"".join(iter(lambda: sock.recv(4096), b""))
    if reply != b"ok\n":
        raise RuntimeError(f"guest {mode} returned {reply[:300]!r}")


def boot(args, vm, process):
    """Configures and starts a launched VM, then runs `python3 hello.py` in it. Returns the
    (configure, boot, round trip) times."""
    t0 = now_us()
    for route, body in [
        ("/boot-source", {"kernel_image_path": str(args.kernel), "boot_args": BOOT_ARGS}),
        ("/drives/rootfs", {"drive_id": "rootfs", "path_on_host": str(args.rootfs), "is_root_device": True, "is_read_only": True}),
        ("/machine-config", {"vcpu_count": args.vcpus, "mem_size_mib": args.mem_mib}),
        ("/vsock", {"guest_cid": 3, "uds_path": "v.sock"}),
        ("/actions", {"action_type": "InstanceStart"}),
    ]:
        api(vm, "PUT", route, body)
    t1 = now_us()
    sock = connect_agent(vm, process, 30)
    t2 = now_us()
    ask(sock, "spawn")
    return t1 - t0, t2 - t1, now_us() - t2


def emit_rows(prefix, parts, samples):
    """One row for the whole sample, then one per part."""
    print("row", f"{prefix}: total", " ".join(str(sum(s)) for s in samples), sep="\t")
    for part, column in zip(parts, zip(*samples)):
        print("row", f"{prefix}: of which {part}", " ".join(map(str, column)), sep="\t")


def measured(metric, body, *body_args):
    """Runs body; a failure becomes a failed row, as in hluk.sh. Returns whether it succeeded."""
    try:
        body(*body_args)
        return True
    except Exception as error:
        print("failed", metric, repr(error).replace("\t", " "), sep="\t")
        return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--firecracker", required=True, type=pathlib.Path)
    parser.add_argument("--kernel", required=True, type=pathlib.Path)
    parser.add_argument("--rootfs", required=True, type=pathlib.Path)
    parser.add_argument("--work", required=True, type=pathlib.Path)
    parser.add_argument("--runs", type=int, default=30)
    parser.add_argument("--vcpus", type=int, default=1)
    parser.add_argument("--mem-mib", type=int, default=256)
    args = parser.parse_args()
    for name in ("firecracker", "kernel", "rootfs", "work"):
        setattr(args, name, getattr(args, name).resolve())
    vm = args.work / "vm"
    snapshot = args.work / "snapshot"

    def cold():
        samples = []
        for _ in range(args.runs):
            t0 = now_us()
            with firecracker(args.firecracker, vm) as process:
                samples.append((now_us() - t0, *boot(args, vm, process)))
        emit_rows("cold boot → python3 hello.py printed ok", [
            "firecracker spawn to API socket",
            "configure + InstanceStart (5 API calls)",
            "kernel boot + init to agent accepting on vsock",
            "vsock round trip running python3 hello.py",
        ], samples)

    def take_snapshot():
        # Boot, run both modes once so the interpreter and python3's pages are in guest memory,
        # then pause and save a full snapshot.
        with firecracker(args.firecracker, vm) as process:
            boot(args, vm, process)
            ask(connect_agent(vm, process, 10), "exec")
            api(vm, "PATCH", "/vm", {"state": "Paused"})
            snapshot.mkdir()
            t0 = now_us()
            api(vm, "PUT", "/snapshot/create", {
                "snapshot_type": "Full",
                "snapshot_path": str(snapshot / "state"),
                "mem_file_path": str(snapshot / "memory"),
            })
            print("row", "snapshot create (Full, paused guest)", now_us() - t0, sep="\t")

    def restored(mode, what):
        samples = []
        for _ in range(args.runs):
            t0 = now_us()
            with firecracker(args.firecracker, vm) as process:
                t1 = now_us()
                api(vm, "PUT", "/snapshot/load", {
                    "snapshot_path": str(snapshot / "state"),
                    "mem_backend": {"backend_type": "File", "backend_path": str(snapshot / "memory")},
                    "resume_vm": True,
                })
                t2 = now_us()
                sock = connect_agent(vm, process, 10)
                t3 = now_us()
                ask(sock, mode)
                samples.append((t1 - t0, t2 - t1, t3 - t2, now_us() - t3))
        emit_rows(f"snapshot restore → {what} printed ok", [
            "firecracker spawn to API socket",
            "PUT /snapshot/load with resume (memory file mapped, not copied)",
            "vsock connect to the resumed agent",
            f"vsock round trip running {what}",
        ], samples)

    measured("cold boot", cold)
    if measured("snapshot create", take_snapshot):
        for mode, what in [("spawn", "python3 hello.py"), ("exec", "hello.py in the warm agent")]:
            measured(f"snapshot restore → {what}", restored, mode, what)


if __name__ == "__main__":
    main()
