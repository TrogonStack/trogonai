#!/usr/bin/env python3
"""Records what CPython's `urlopen` opens and reads over FTP from a scripted loopback server, so the Rust Python
dialect's FTP retrieval is checked against CPython instead of against a reading of its source.

usage: python3 -I python_ftp_goldens.py OUT_FILE [--check]

Run it with CPython 3.12.16 on macOS: the goldens keep the macOS `errno` and resolver texts, which
`src/python_ftplib_tests.rs` swaps for what its own platform raises.

Each case names a URL, the steps of each control connection the server accepts in turn, and the proxy variables
`urlopen` sees. `send` writes the Latin-1 text, or the `[text, repeat]` chunks, `recv` reads and records one command
line, `data` accepts a data connection, writes the text and closes it, `drop` accepts one and closes it unused, `pause`
waits that many milliseconds and `close` closes the control connection. After its last step, or a reply the closed
client refused, the server records the lines the client still sends until it closes. `{PORT}` stands for the control
port and `{PASV}` for the data address the passive replies name. A case records the body `response.read()` answered,
the error opening raised, or the error reading or closing the response raised, and the lines of each connection.

ftplib reads its control connection through a buffered text reader, so a closing reply that arrives in the same read
as the reply before the data is decoded with it. That changes nothing while both are UTF-8, so only the case whose
closing reply is not UTF-8 depends on whether they arrive apart, and it waits `SETTLE` before that reply.

A case the client fails before the script runs out ends at the reply it fails on whenever a read would follow a
later reply. The client closes as it fails, and a read after a reply sent past that close records an empty line or
fails with a reset depending on which reaches the server first, so how many empty reads the case records would
depend on the loopback too.
"""

import base64
import json
import os
import re
import socket
import sys
import threading
import time
import urllib.request

args = sys.argv[1:]
if not args or args[0].startswith("-"):
    sys.stderr.write("usage: python_ftp_goldens.py OUT_FILE [--check]\n")
    sys.exit(2)
out, flags = args[0], args[1:]

if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 12, 16) or sys.platform != "darwin":
    sys.stderr.write("python_ftp_goldens.py records with CPython 3.12.16 on macOS\n")
    sys.exit(2)

DATA_TIMEOUT = 5
"""How long the server waits for a data connection."""

TAIL_TIMEOUT = 3
"""How long the server keeps reading once its steps are done."""

STEP_TIMEOUT = 10
"""How long the server waits for a command of its steps, so a client that waits too ends the connection."""

ACCEPT_TIMEOUT = 10
"""How long the server waits for the next control connection of a case."""


def send(text):
    return ["send", text]


RECV = ["recv"]


def data(text):
    return ["data", text]


DROP = ["drop"]


def pause(milliseconds):
    return ["pause", milliseconds]


CLOSE = ["close"]

SETTLE = pause(200)
"""The wait before a closing reply that is not UTF-8, so it arrives in a read of its own.

It is a wait and not a handshake because, in CPython 3.12.16, the client sends nothing between reading the reply
before the data and reading the closing reply. `FTP.ntransfercmd` reads the `150` (Lib/ftplib.py:359-369).
`ftpwrapper.retrfile` then wraps the data socket in `addclosehook(conn.makefile('rb'), self.file_close)` and calls
`conn.close()` (Lib/urllib/request.py:2504-2506), which leaves the socket open while the file holds it
(Lib/socket.py:491-495, 501-505). The response only reads the data, and closing it runs `file_close`, whose
`endtransfer` reads the closing reply with `voidresp` (Lib/urllib/request.py:2510-2525) before `addclosehook.close`
closes the data file (Lib/urllib/response.py:45-54). Whether CPython raises opening or reading therefore depends on
how TCP splits the reads, and the golden records the branch where the two replies arrive in separate reads."""


LOGIN = [send("220 ready\r\n"), RECV, send("331 password\r\n"), RECV, send("230 in\r\n"), RECV]
AUTHENTICATE = [send("331 password\r\n"), RECV, send("230 in\r\n"), RECV]
SETUP = [send("250 ok\r\n"), RECV, send("200 type\r\n"), RECV]
PASSIVE = [send("227 Entering Passive Mode ({PASV})\r\n"), RECV]
FILE = [send("150 opening (13 bytes)\r\n"), data('{"ftp": true}'), send("226 done\r\n")]


def case(name, url, *connections):
    return {"name": name, "url": url, "script": list(connections), "env": []}


def proxied(name, url, variable, steps):
    """A URL `urlopen` sends to the server as the `ftp` proxy the variable names."""
    return {"name": name, "url": url, "env": [[variable, "ftp://127.0.0.1:{PORT}"]], "script": [steps]}


CASES = [
    case('a file', 'ftp://127.0.0.1:{PORT}/a.json', [*LOGIN, *SETUP, *PASSIVE, *FILE]),
    case('a file in directories', 'ftp://127.0.0.1:{PORT}/d%31/d2/a.json', [*LOGIN, *SETUP, *PASSIVE, *FILE]),
    case('credentials', 'ftp://us%65r:pw@127.0.0.1:{PORT}/a.json', [*LOGIN, *SETUP, *PASSIVE, *FILE]),
    case('a user without a password', 'ftp://user@127.0.0.1:{PORT}/a.json', [*LOGIN, *SETUP, *PASSIVE, *FILE]),
    case('type a', 'ftp://127.0.0.1:{PORT}/a.json;type=a', [*LOGIN, *SETUP, *PASSIVE, *FILE]),
    case('type D upper case attribute', 'ftp://127.0.0.1:{PORT}/a.json;TYPE=D',
        [*LOGIN, *SETUP, send('200 type\r\n'), RECV, send('257 "/"\r\n'), RECV, send('250 ok\r\n'), RECV,
         send('250 ok\r\n'), RECV, *PASSIVE, send('150 list\r\n'), data('{}'), send('226 done\r\n')],
    ),
    case('type x is ignored', 'ftp://127.0.0.1:{PORT}/a.json;type=x;y', [*LOGIN, *SETUP, *PASSIVE, *FILE]),
    case('a directory URL lists', 'ftp://127.0.0.1:{PORT}/dir/',
        [*LOGIN, *SETUP, send('200 type\r\n'), RECV, *PASSIVE, send('125 list\r\n'), data('[1]'),
         send('226 done\r\n')],
    ),
    case('a greeting that waits', 'ftp://127.0.0.1:{PORT}/a.json', [pause(400), *LOGIN, *SETUP, *PASSIVE, *FILE]),
    case('a multi-line greeting', 'ftp://127.0.0.1:{PORT}/a.json',
        [send('220-one\r\n220 not the end\r\n220-two\r\n220 end\r\n'), RECV, send('331 password\r\n')],
    ),
    case('a greeting with bare line ends', 'ftp://127.0.0.1:{PORT}/a.json',
        [send('220 a\r'), pause(200), send('\n'), RECV, *AUTHENTICATE, *SETUP, *PASSIVE, *FILE],
    ),
    case('a greeting ending in a lone CR', 'ftp://127.0.0.1:{PORT}/a.json',
        [send('220 a\r331 b\n'), RECV, RECV, send('230 in\r\n'), RECV, *SETUP, *PASSIVE, *FILE],
    ),
    case('a busy server', 'ftp://127.0.0.1:{PORT}/a.json', [send('421 Service not available\r\n')]),
    case('a refusing greeting', 'ftp://127.0.0.1:{PORT}/a.json', [send('500 no\r\n')]),
    case('an odd greeting', 'ftp://127.0.0.1:{PORT}/a.json', [send('600 what\r\n')]),
    case('a greeting the server closes before', 'ftp://127.0.0.1:{PORT}/a.json', [CLOSE]),
    case('a greeting too long', 'ftp://127.0.0.1:{PORT}/a.json', [send([['220 ', 1], ['x', 9000], ['\r\n', 1]])]),
    case('a greeting of exactly the limit', 'ftp://127.0.0.1:{PORT}/a.json',
        [send([['220 ', 1], ['x', 8187], ['\r\n', 1]]), RECV, *AUTHENTICATE, *SETUP, *PASSIVE, *FILE],
    ),
    case('a greeting that is not UTF-8', 'ftp://127.0.0.1:{PORT}/a.json', [send('220 \xff\r\n')]),
    case('a greeting in UTF-8', 'ftp://127.0.0.1:{PORT}/a.json',
        [send('220 caf\xc3\xa9\r\n'), RECV, *AUTHENTICATE, *SETUP, *PASSIVE, *FILE],
    ),
    case('a login refused', 'ftp://127.0.0.1:{PORT}/a.json',
        [send('220 ready\r\n'), RECV, send('530 Login incorrect.\r\n')],
    ),
    case('a login that needs an account', 'ftp://127.0.0.1:{PORT}/a.json',
        [send('220 ready\r\n'), RECV, send('331 pw\r\n'), RECV, send('332 acct\r\n'), RECV, send('230 ok\r\n'), RECV,
         *SETUP, *PASSIVE, *FILE],
    ),
    case('a login answered 1xx', 'ftp://127.0.0.1:{PORT}/a.json', [send('220 ready\r\n'), RECV, send('150 what\r\n')]),
    case('a directory refused', 'ftp://127.0.0.1:{PORT}/d/a.json', [*LOGIN, send('550 no dir\r\n')]),
    case('a type the server refuses then accepts after reconnecting', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, send('250 ok\r\n'), RECV, send('500 bad\r\n')],
        [*LOGIN, *SETUP, *PASSIVE, *FILE],
    ),
    case('a type refused twice', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, send('250 ok\r\n'), RECV, send('500 bad\r\n')],
        [*LOGIN, send('250 ok\r\n'), RECV, send('504 bad\r\n')],
    ),
    case('passive mode refused', 'ftp://127.0.0.1:{PORT}/a.json', [*LOGIN, *SETUP, send('500 no pasv\r\n')]),
    case('passive mode answered 2xx without numbers', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, send('227 nothing\r\n')],
    ),
    case('passive mode answered 200', 'ftp://127.0.0.1:{PORT}/a.json', [*LOGIN, *SETUP, send('200 ({PASV})\r\n')]),
    case('a passive address the client ignores', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, send('227 =10,9,8,7,{PASV}\r\n'), RECV, send('150 x\r\n'), data('{}'),
         send('226 done\r\n')],
    ),
    case('a file the server lacks lists a directory instead', 'ftp://127.0.0.1:{PORT}/sub',
        [*LOGIN, *SETUP, *PASSIVE, send('550 not a file\r\n'), DROP, RECV, send('200 type\r\n'), RECV,
         send('257 "/h""o" is cwd\r\n'), RECV, send('250 ok\r\n'), RECV, send('250 ok\r\n'), RECV, *PASSIVE,
         send('150 list\r\n'), data('[2]'), send('226 done\r\n')],
    ),
    case('a file the server lacks and no directory', 'ftp://127.0.0.1:{PORT}/sub',
        [*LOGIN, *SETUP, *PASSIVE, send('550 not a file\r\n'), DROP, RECV, send('200 type\r\n'), RECV,
         send('257 /odd\r\n'), RECV, send('550 no such\r\n'), RECV, send('250 ok\r\n')],
    ),
    case('a pwd that is not 257', 'ftp://127.0.0.1:{PORT}/sub',
        [*LOGIN, *SETUP, *PASSIVE, send('550 not a file\r\n'), DROP, RECV, send('200 type\r\n'), RECV,
         send('250 here\r\n'), RECV, send('550 no such\r\n'), RECV, send('250 ok\r\n')],
    ),
    case('a retrieval refused otherwise', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, *PASSIVE, send('530 not you\r\n')],
    ),
    case('a retrieval refused for now', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, *PASSIVE, send('451 later\r\n')],
    ),
    case('a retrieval answered 2xx first', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, *PASSIVE, send('200 early\r\n'), send('150 now\r\n'), data('{}'), send('226 done\r\n')],
    ),
    case('a retrieval answered 2xx twice', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, *PASSIVE, send('200 early\r\n'), send('250 again\r\n')],
    ),
    case('a retrieval size with too many digits', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, *PASSIVE, send([['150 x (', 1], ['1', 4301], [' BYTES)\r\n', 1]])],
    ),
    case('a passive port past 65535', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, send('227 (127,0,0,1,256,1)\r\n')],
    ),
    case('a passive port of huge digits', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, send([['227 (127,0,0,1,', 1], ['9', 30], [',1)\r\n', 1]])],
    ),
    case('a passive port nothing listens on', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, send('227 (127,0,0,1,0,1)\r\n')],
    ),
    case('a directory with a newline', 'ftp://127.0.0.1:{PORT}/d%0A/a.json',
        [send('220 ready\r\n'), RECV, send('331 password\r\n'), RECV, send('230 in\r\n')],
    ),
    case('a closing reply that never comes', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, *PASSIVE, send('150 x\r\n'), data('{}'), CLOSE],
    ),
    case('a greeting one past the limit', 'ftp://127.0.0.1:{PORT}/a.json',
        [send([['220 ', 1], ['x', 8188], ['\r\n', 1]])],
    ),
    case('a greeting of multibyte characters at the limit', 'ftp://127.0.0.1:{PORT}/a.json',
        [send([['220 ', 1], ['\xc3\xa9', 8187], ['\r\n', 1]]), RECV, *AUTHENTICATE, *SETUP, *PASSIVE, *FILE],
    ),
    case('a greeting of multibyte characters past the limit', 'ftp://127.0.0.1:{PORT}/a.json',
        [send([['220 ', 1], ['\xc3\xa9', 8188], ['\r\n', 1]])],
    ),
    case('a greeting with a bad continuation byte', 'ftp://127.0.0.1:{PORT}/a.json', [send('220 \xc3(\r\n')]),
    case('a greeting cut inside a character', 'ftp://127.0.0.1:{PORT}/a.json', [send('220 \xc3'), CLOSE]),
    case('a greeting with a character split across reads', 'ftp://127.0.0.1:{PORT}/a.json',
        [send('220 caf\xc3'), pause(200), send('\xa9\r\n'), RECV, *AUTHENTICATE, *SETUP, *PASSIVE, *FILE],
    ),
    case('a bad byte in a later read', 'ftp://127.0.0.1:{PORT}/a.json',
        [send('220-a\r\n'), pause(200), send('b\xff\r\n')],
    ),
    case('a bad byte after a split character', 'ftp://127.0.0.1:{PORT}/a.json',
        [send('220-a\xc3'), pause(200), send('\xa9\xe2\x82(\r\n')],
    ),
    case('a passive number with too many digits', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, send([['227 (127,0,0,1,', 1], ['1', 4301], [',1)\r\n', 1]])],
    ),
    case('a passive number at the digit limit', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, send([['227 (127,0,0,1,', 1], ['1', 4300], [',1)\r\n', 1]])],
    ),
    case('a retrieval size at the digit limit', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, *PASSIVE, send([['150 x (', 1], ['1', 4300], [' BYTES)\r\n', 1]]), data('{}'),
         send('226 done\r\n')],
    ),
    proxied('an http URL through an ftp proxy', 'http://example.invalid/a.json', 'http_proxy',
            [*LOGIN, *SETUP, *PASSIVE, *FILE]),
    proxied('an https URL through an ftp proxy', 'https://example.invalid/a.json', 'https_proxy',
            [send('220 ready\r\n')]),
    proxied('an ftp URL through an ftp proxy', 'ftp://example.invalid/a.json', 'ftp_proxy',
            [*LOGIN, *SETUP, *PASSIVE, *FILE]),
    case('a refused server', 'ftp://127.0.0.1:1/a.json'),
    case('a parent directory', 'ftp://127.0.0.1:{PORT}/../a.json',
        [*LOGIN, send('250 up\r\n'), RECV, send('200 type\r\n'), RECV, *PASSIVE, *FILE],
    ),
    case('a parent directory without CDUP', 'ftp://127.0.0.1:{PORT}/../a.json',
        [*LOGIN, send('500 unknown\r\n'), RECV, *SETUP, *PASSIVE, *FILE],
    ),
    case('a parent directory refused', 'ftp://127.0.0.1:{PORT}/../a.json', [*LOGIN, send('550 no\r\n')]),
    case('a type answered with a byte that is not UTF-8', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, send('250 ok\r\n'), RECV, send('200 \xff\r\n')],
    ),
    case('a directory listing refused for now', 'ftp://127.0.0.1:{PORT}/sub',
        [*LOGIN, *SETUP, *PASSIVE, send('550 not a file\r\n'), DROP, RECV, send('200 type\r\n'), RECV,
         send('257 "/"\r\n'), RECV, send('450 busy\r\n'), RECV, send('250 ok\r\n')],
    ),
    case('a directory listing whose way back fails', 'ftp://127.0.0.1:{PORT}/sub',
        [*LOGIN, *SETUP, *PASSIVE, send('550 not a file\r\n'), DROP, RECV, send('200 type\r\n'), RECV,
         send('257 "/"\r\n'), RECV, send('550 no\r\n'), RECV, send('421 bye\r\n')],
    ),
    case('a directory listing of an unterminated name', 'ftp://127.0.0.1:{PORT}/sub',
        [*LOGIN, *SETUP, *PASSIVE, send('550 not a file\r\n'), DROP, RECV, send('200 type\r\n'), RECV,
         send('257 "/abc\r\n'), RECV, send('250 ok\r\n'), RECV, send('250 ok\r\n'), RECV, *PASSIVE,
         send('150 list\r\n'), data('[3]'), send('226 done\r\n')],
    ),
    case('a closing reply that is not UTF-8', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, *PASSIVE, send('150 x\r\n'), data('{}'), SETTLE, send('226 \xff\r\n')],
    ),
    case('a closing reply refused', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, *PASSIVE, send('150 x\r\n'), data('{}'), send('451 abort\r\n')],
    ),
    case('an anonymous password of a dash', 'ftp://anonymous:-@127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, *PASSIVE, *FILE],
    ),
    case('an empty user with a password', 'ftp://:pw@127.0.0.1:{PORT}/a.json', [*LOGIN, *SETUP, *PASSIVE, *FILE]),
    case('a named user with an empty password', 'ftp://user:@127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, *PASSIVE, *FILE],
    ),
    case('a retrieval size the last parenthesis gives', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, *PASSIVE, send([['150 x (', 1], ['1', 4301], [' bytes) (2 bytes)\r\n', 1]]), data('{}'),
         send('226 done\r\n')],
    ),
    case('a retrieval size on a continuation line', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, *PASSIVE, send([['150-a\r\n150 b (', 1], ['1', 4301], [' bytes)\r\n', 1]]), data('{}'),
         send('226 done\r\n')],
    ),
    case('a retrieval size after a bad parenthesis', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, *PASSIVE, send([['150 x (', 1], ['1', 4301], [' bytes) (3 bites)\r\n', 1]])],
    ),
    case('a file name in UTF-8', 'ftp://127.0.0.1:{PORT}/caf%C3%A9.json', [*LOGIN, *SETUP, *PASSIVE, *FILE]),
    case('a file name with a carriage return', 'ftp://127.0.0.1:{PORT}/a%0D.json',
        [*LOGIN, *SETUP, send('227 Entering Passive Mode ({PASV})\r\n')],
    ),
    case('a multi-line reply that another code does not end', 'ftp://127.0.0.1:{PORT}/a.json',
        [send('220-a\r\n221 b\r\n220-c\r\n220 d\r\n'), RECV, *AUTHENTICATE, *SETUP, *PASSIVE, *FILE],
    ),
    case('a passive reply after other digits', 'ftp://127.0.0.1:{PORT}/a.json',
        [*LOGIN, *SETUP, send('227 7,8 then ({PASV})\r\n'), RECV, send('150 x\r\n'), data('{}'),
         send('226 done\r\n')],
    ),
    case('a reply split after a carriage return', 'ftp://127.0.0.1:{PORT}/a.json',
        [send('220 a\r'), pause(200), send('331 b\r\n'), RECV, RECV, send('230 in\r\n'), RECV, *SETUP, *PASSIVE, *FILE],
    ),
    case('a carriage return the close leaves at the limit', 'ftp://127.0.0.1:{PORT}/a.json',
        [send([['220-', 1], ['x', 8188], ['\r', 1]]), pause(200), CLOSE],
    ),
]


def payload(argument):
    """The text a `send` step writes."""
    if isinstance(argument, str):
        return argument
    return "".join(text * repeat for text, repeat in argument)


class Server:
    """One control connection at a time, each running its steps in turn and then recording what the client still
    sends until it closes, with a data listener the passive replies name."""

    def __init__(self, script):
        self.control = socket.create_server(("127.0.0.1", 0))
        self.control.settimeout(ACCEPT_TIMEOUT)
        self.data = socket.create_server(("127.0.0.1", 0))
        self.data.settimeout(DATA_TIMEOUT)
        self.port = self.control.getsockname()[1]
        data_port = self.data.getsockname()[1]
        self.pasv = f"127,0,0,1,{data_port >> 8},{data_port & 255}"
        self.script = script
        self.commands = []
        self.thread = threading.Thread(target=self.serve)
        self.thread.start()

    def serve(self):
        with self.control, self.data:
            for steps in self.script:
                try:
                    connection, _ = self.control.accept()
                except OSError:
                    return
                with connection:
                    self.run(connection, steps)

    def run(self, connection, steps):
        """The steps, then the lines the client sends until it closes. The reader closes before the connection, so
        the client sees the close at once."""
        lines = []
        self.commands.append(lines)
        connection.settimeout(STEP_TIMEOUT)
        with connection.makefile("rb") as reader:
            if not self.steps(connection, reader, steps, lines):
                return
            connection.settimeout(TAIL_TIMEOUT)
            try:
                while line := reader.readline():
                    lines.append(line.decode("latin-1"))
            except OSError:
                pass

    def steps(self, connection, reader, steps, lines):
        """Whether the client's remaining lines are recorded: after the last step or a reply the closed client
        refused, but not after any other error or a `close`."""
        for kind, *argument in steps:
            try:
                if kind == "send":
                    connection.sendall(payload(argument[0]).replace("{PASV}", self.pasv).encode("latin-1"))
                elif kind == "recv":
                    lines.append(reader.readline().decode("latin-1"))
                elif kind == "data":
                    stream, _ = self.data.accept()
                    with stream:
                        stream.sendall(argument[0].encode("latin-1"))
                elif kind == "drop":
                    self.data.accept()[0].close()
                elif kind == "pause":
                    time.sleep(argument[0] / 1000)
                else:
                    return False
            except OSError:
                return kind == "send"
        return True


def raised(error):
    return {"class": type(error).__name__, "text": str(error)}


def opener(env):
    """The opener of a case. One without proxy variables gets an empty proxy mapping, since `getproxies` falls back to
    the macOS system settings once the environment names no proxy. One with them keeps the mapping `ProxyHandler`
    reads from the environment, because `proxy_bypass` reads the environment too and only falls back to the system
    settings when it names no proxy."""
    if env:
        return urllib.request.build_opener()
    return urllib.request.build_opener(urllib.request.ProxyHandler({}))


def outcome(url, env):
    """What opening and reading the URL gave. Each error is written down inside its `except` clause, so its traceback
    and the connections it holds are freed as the clause ends."""
    try:
        response = opener(env).open(url)
    except Exception as error:
        return {"open_error": raised(error)}
    try:
        with response:
            body = response.read()
    except Exception as error:
        return {"read_error": raised(error)}
    return {"body": base64.b64encode(body).decode("ascii")}


def record(case):
    """The case with its outcome and commands, with the ports of this run written back as placeholders. A fresh
    opener reads the proxy variables, which `urlopen` would read once for every case."""
    server = Server(case["script"])

    def filled(text):
        return text.replace("{PORT}", str(server.port)).replace("{PASV}", server.pasv)

    def placeholders(text):
        return re.sub(rf"(?<!\d){server.port}(?!\d)", "{PORT}", text.replace(server.pasv, "{PASV}"))

    for name in [name for name in os.environ if name.lower().endswith("_proxy")]:
        del os.environ[name]
    os.environ.update({name: filled(value) for name, value in case["env"]})
    result = outcome(filled(case["url"]), case["env"])
    server.thread.join()
    for error in result.values():
        if isinstance(error, dict):
            error["text"] = placeholders(error["text"])
    commands = [[placeholders(line) for line in lines] for lines in server.commands]
    return {**case, **result, "commands": commands}


body = json.dumps({"cases": [record(case) for case in CASES]}, indent=1) + "\n"

if "--check" in flags:
    with open(out, encoding="utf-8") as handle:
        if handle.read() != body:
            sys.stderr.write(f"{out} is stale: run python_ftp_goldens.py without --check\n")
            sys.exit(1)
else:
    with open(out, "w", encoding="utf-8") as handle:
        handle.write(body)
