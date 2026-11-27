#!/usr/bin/python3
# PID 1 of the Firecracker guest (`init=/fc-agent` on the kernel command line). There is no systemd,
# so the guest is ready as soon as this listens. Each vsock connection sends one line, `spawn` (run
# `python3 /hello.py` as a new process) or `exec` (run /hello.py's code in this already-warm
# interpreter), and gets the program's output back before the agent closes the stream.
import contextlib
import io
import socket
import subprocess

PORT = 5000
HELLO = "/hello.py"
with open(HELLO) as source:
    code = compile(source.read(), HELLO, "exec")


def run(mode):
    if mode == b"spawn":
        return subprocess.run(["/usr/bin/python3", HELLO], stdout=subprocess.PIPE, stderr=subprocess.STDOUT).stdout
    if mode == b"exec":
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            exec(code, {"__name__": "__main__"})
        return out.getvalue().encode()
    return b"unknown mode: " + mode


listener = socket.socket(socket.AF_VSOCK, socket.SOCK_STREAM)
listener.bind((socket.VMADDR_CID_ANY, PORT))
listener.listen(16)
print("fc-agent: listening", flush=True)
# PID 1 must not exit: the kernel would panic and Firecracker stop.
while True:
    try:
        conn, _ = listener.accept()
        with conn:
            conn.sendall(run(conn.makefile("rb").readline().strip()))
    except Exception as error:
        print(f"fc-agent: {error!r}", flush=True)
