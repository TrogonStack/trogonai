#!/usr/bin/python3
import json
import os
import pathlib
import subprocess
import time
from multiprocessing import shared_memory


def run(*command):
    return subprocess.run(command, check=True, timeout=5, capture_output=True)


mounts = {line.split()[1]: line.split()[2] for line in pathlib.Path("/proc/mounts").read_text().splitlines()}
for path, filesystem in {"/proc": "proc", "/sys": "sysfs", "/dev": "tmpfs", "/dev/pts": "devpts", "/dev/shm": "tmpfs", "/run": "tmpfs", "/tmp": "tmpfs", "/sys/fs/cgroup": "cgroup2"}.items():
    assert mounts[path] == filesystem, (path, mounts.get(path))
assert os.readlink("/dev/ptmx") == "pts/ptmx"
master, slave = os.openpty()
assert os.ttyname(slave).startswith("/dev/pts/")
os.close(master)
os.close(slave)
first = shared_memory.SharedMemory(create=True, size=16)
try:
    first.buf[:4] = b"test"
    second = shared_memory.SharedMemory(name=first.name)
    try:
        assert bytes(second.buf[:4]) == b"test"
    finally:
        second.close()
finally:
    first.close()
    first.unlink()

root = pathlib.Path("/sys/fs/cgroup")
assert {"cpu", "memory", "pids"}.issubset(set((root / "cgroup.controllers").read_text().split()))
supervisor = root / "init-supervisor"
supervisor.mkdir()
for pid in (root / "cgroup.procs").read_text().split():
    try:
        (supervisor / "cgroup.procs").write_text(pid)
    except ProcessLookupError:
        pass
(root / "cgroup.subtree_control").write_text("+cpu +memory +pids")
workload = root / "init-workload"
workload.mkdir()
(workload / "memory.max").write_text(str(64 * 1024 * 1024))
(workload / "pids.max").write_text("16")
(workload / "cpu.max").write_text("50000 100000")
assert (workload / "memory.max").read_text().strip() == str(64 * 1024 * 1024)
run("unshare", "--mount", "--uts", "--ipc", "--net", "--pid", "--fork", "--mount-proc", "/bin/sh", "-c", "test $$ -eq 1 && test -w /tmp")

overlay = pathlib.Path("/run/overlay-proof")
overlay.mkdir()
for name in ("lower", "upper", "work", "merged"):
    (overlay / name).mkdir()
(overlay / "lower/base").write_text("base")
run("mount", "-t", "overlay", "overlay", "-o", f"lowerdir={overlay}/lower,upperdir={overlay}/upper,workdir={overlay}/work", str(overlay / "merged"))
try:
    assert (overlay / "merged/base").read_text() == "base"
    (overlay / "merged/base").write_text("private")
    assert (overlay / "lower/base").read_text() == "base"
finally:
    run("umount", str(overlay / "merged"))
run("nft", "add", "table", "inet", "init_proof")
run("nft", "delete", "table", "inet", "init_proof")

os.mkfifo("/run/orphan-gate")
run("/bin/sh", "-c", "/bin/sh -c 'read token < /run/orphan-gate' >/dev/null 2>&1 & echo $! > /run/orphan-pid")
orphan = int(pathlib.Path("/run/orphan-pid").read_text())
assert "PPid:\t1\n" in pathlib.Path(f"/proc/{orphan}/status").read_text()
with open("/run/orphan-gate", "w") as gate:
    gate.write("exit\n")
deadline = time.monotonic() + 2
while pathlib.Path(f"/proc/{orphan}").exists():
    assert time.monotonic() < deadline, "PID 1 did not reap its orphan"

print(json.dumps({"guest_init_mounts": True, "pty_and_shared_memory": True, "writable_cgroup2": True, "nested_namespaces": True, "overlay_and_netfilter": True, "orphan_reaped": True}), flush=True)
