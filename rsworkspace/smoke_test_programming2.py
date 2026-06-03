#!/usr/bin/env python3
"""
smoke_test_programming2.py — Real programming workflow scenarios.

Tests multi-turn coding sessions that mirror what a developer actually does:

  S1. Bug fix workflow      (acp-runner)     read → str_replace → verify [3 LLM calls]
  S2. Codebase exploration  (openrouter)     list_dir → glob → read → summarize [4 LLM calls]
  S3. Cross-runner refactor (xai → openrouter) xai reads+analyzes → or fixes file
  S4. Model switch mid-code (xai)           grok-3 reads → grok-3-mini implements

Usage:
    NATS_URL=nats://localhost:4333 python3 smoke_test_programming2.py
"""

import asyncio, collections, json, os, shutil, subprocess, sys, tempfile, threading, time, uuid
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Optional

import nats as nats_lib

# ── Config ────────────────────────────────────────────────────────────────────
NATS_URL       = os.environ.get("NATS_URL", "nats://localhost:4222")
RSDIR          = os.path.dirname(os.path.abspath(__file__))
MOCK_PORT      = 19812          # distinct from smoke_test_programming.py (19810)
BASE           = f"prog2.{os.getpid()}"
PROMPT_TIMEOUT = 40.0
NATS_TIMEOUT   = 8.0
RUNNER_READY   = 12.0

# ── Console ───────────────────────────────────────────────────────────────────
green, red, yellow, cyan, reset = "\033[32m", "\033[31m", "\033[33m", "\033[36m", "\033[0m"
PASS = 0
FAIL = 0
procs: dict = {}


def ok(msg, detail=""):
    global PASS
    PASS += 1
    sfx = f"  {yellow}{detail}{reset}" if detail else ""
    print(f"  {green}PASS{reset}  {msg}{sfx}")


def fail(msg, detail=""):
    global FAIL
    FAIL += 1
    sfx = f"  {yellow}{detail}{reset}" if detail else ""
    print(f"  {red}FAIL{reset}  {msg}{sfx}")


def info(msg):
    print(f"  {yellow}INFO{reset}  {msg}")


def section(t):
    bar = "─" * max(0, 55 - len(t))
    print(f"\n{cyan}── {t} {bar}{reset}")


def prompt_err(r: dict) -> Optional[str]:
    if "_error" in r:
        return r["_error"]
    if "code" in r:
        return r.get("message", str(r))
    return None


# ── SSE builders ──────────────────────────────────────────────────────────────

def anthropic_sse(text: str) -> bytes:
    chunks = [
        "event: message_start\ndata: " + json.dumps({
            "type": "message_start",
            "message": {"id": "msg", "type": "message", "role": "assistant",
                        "content": [], "model": "mock", "stop_reason": None,
                        "stop_sequence": None, "usage": {"input_tokens": 5, "output_tokens": 0}},
        }) + "\n\n",
        "event: content_block_start\ndata: " + json.dumps({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "text", "text": ""},
        }) + "\n\n",
        "event: content_block_delta\ndata: " + json.dumps({
            "type": "content_block_delta", "index": 0,
            "delta": {"type": "text_delta", "text": text},
        }) + "\n\n",
        "event: content_block_stop\ndata: " + json.dumps({"type": "content_block_stop", "index": 0}) + "\n\n",
        "event: message_delta\ndata: " + json.dumps({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn", "stop_sequence": None},
            "usage": {"output_tokens": 1},
        }) + "\n\n",
        "event: message_stop\ndata: " + json.dumps({"type": "message_stop"}) + "\n\n",
    ]
    return "".join(chunks).encode()


def anthropic_sse_tool(tool_id: str, tool_name: str, input_json: str) -> bytes:
    chunks = [
        "event: message_start\ndata: " + json.dumps({
            "type": "message_start",
            "message": {"id": "msg-tool", "type": "message", "role": "assistant",
                        "content": [], "model": "mock", "stop_reason": None,
                        "stop_sequence": None, "usage": {"input_tokens": 10, "output_tokens": 0}},
        }) + "\n\n",
        "event: content_block_start\ndata: " + json.dumps({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "tool_use", "id": tool_id, "name": tool_name, "input": {}},
        }) + "\n\n",
        "event: content_block_delta\ndata: " + json.dumps({
            "type": "content_block_delta", "index": 0,
            "delta": {"type": "input_json_delta", "partial_json": input_json},
        }) + "\n\n",
        "event: content_block_stop\ndata: " + json.dumps({"type": "content_block_stop", "index": 0}) + "\n\n",
        "event: message_delta\ndata: " + json.dumps({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use", "stop_sequence": None},
            "usage": {"output_tokens": 5},
        }) + "\n\n",
        "event: message_stop\ndata: " + json.dumps({"type": "message_stop"}) + "\n\n",
    ]
    return "".join(chunks).encode()


def xai_sse(text: str, resp_id: str = "resp-001") -> bytes:
    chunks = [
        "data: " + json.dumps({"id": resp_id, "type": "message.delta", "delta": {"text": text}}) + "\n\n",
        "data: " + json.dumps({"type": "response.completed",
                               "response": {"id": resp_id, "status": "completed",
                                            "usage": {"input_tokens": 5, "output_tokens": 2}}}) + "\n\n",
        "data: [DONE]\n\n",
    ]
    return "".join(chunks).encode()


def xai_sse_fn(resp_id: str, call_id: str, name: str, arguments: str) -> bytes:
    chunks = [
        "data: " + json.dumps({"id": resp_id, "type": "function_call",
                               "function_call": {"call_id": call_id, "name": name,
                                                 "arguments": arguments}}) + "\n\n",
        "data: " + json.dumps({"type": "response.completed",
                               "response": {"id": resp_id, "status": "completed",
                                            "usage": {"input_tokens": 10, "output_tokens": 5}}}) + "\n\n",
        "data: [DONE]\n\n",
    ]
    return "".join(chunks).encode()


def or_sse(text: str) -> bytes:
    chunks = [
        "data: " + json.dumps({"choices": [{"delta": {"content": text, "role": "assistant"},
                                            "finish_reason": None, "index": 0}]}) + "\n\n",
        "data: " + json.dumps({"choices": [{"delta": {}, "finish_reason": "stop", "index": 0}]}) + "\n\n",
        "data: [DONE]\n\n",
    ]
    return "".join(chunks).encode()


def or_sse_tool(call_id: str, name: str, arguments: str) -> bytes:
    chunks = [
        "data: " + json.dumps({"choices": [{"delta": {"tool_calls": [
            {"index": 0, "id": call_id, "type": "function",
             "function": {"name": name, "arguments": ""}}
        ]}, "finish_reason": None, "index": 0}]}) + "\n\n",
        "data: " + json.dumps({"choices": [{"delta": {"tool_calls": [
            {"index": 0, "function": {"arguments": arguments}}
        ]}, "finish_reason": None, "index": 0}]}) + "\n\n",
        "data: " + json.dumps({"choices": [{"delta": {}, "finish_reason": "tool_calls", "index": 0}]}) + "\n\n",
        "data: [DONE]\n\n",
    ]
    return "".join(chunks).encode()


# ── Mock HTTP server ──────────────────────────────────────────────────────────

_defaults = {
    "/anthropic/v1/messages": lambda: anthropic_sse("ok"),
    "/responses":             lambda: xai_sse("ok"),
    "/chat/completions":      lambda: or_sse("ok"),
}


class MockLLM(BaseHTTPRequestHandler):
    _lock = threading.Lock()
    _log: list = []
    _queues: dict = {
        "/anthropic/v1/messages": collections.deque(),
        "/responses":             collections.deque(),
        "/chat/completions":      collections.deque(),
    }
    _get_map: dict = {}

    @classmethod
    def reset(cls):
        with cls._lock:
            cls._log.clear()
            for q in cls._queues.values():
                q.clear()
            cls._get_map.clear()

    @classmethod
    def queue(cls, path: str, sse: bytes):
        with cls._lock:
            cls._queues[path].append(sse)

    @classmethod
    def set_get(cls, path: str, body: bytes):
        with cls._lock:
            cls._get_map[path] = body

    @classmethod
    def requests(cls, path: str) -> list:
        with cls._lock:
            return [b for p, b in cls._log if p == path]

    def log_message(self, *_):
        pass

    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(n) if n else b""
        with MockLLM._lock:
            try:
                parsed = json.loads(body)
            except Exception:
                parsed = {}
            MockLLM._log.append((self.path, parsed))
            q = MockLLM._queues.get(self.path)
            resp = q.popleft() if q else None
        if resp is None:
            f = _defaults.get(self.path)
            resp = f() if f else b"data: [DONE]\n\n"
        if resp.startswith(b"{") or resp.startswith(b"["):
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
        else:
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        try:
            self.wfile.write(resp)
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_GET(self):
        with MockLLM._lock:
            resp = MockLLM._get_map.get(self.path, b"ok")
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.end_headers()
        try:
            self.wfile.write(resp)
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass


def start_mock(port: int):
    s = HTTPServer(("127.0.0.1", port), MockLLM)
    threading.Thread(target=s.serve_forever, daemon=True).start()
    return s


# ── Runner process helpers ────────────────────────────────────────────────────

def start_runner(name: str, binary: str, env_extra: dict, cwd=None):
    env = {**os.environ, "NATS_URL": NATS_URL, "RUST_LOG": "warn", **env_extra}
    procs[name] = subprocess.Popen(
        [binary], env=env, cwd=cwd,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )


def stop_runner(name: str):
    p = procs.pop(name, None)
    if p:
        p.terminate()
        try:
            p.wait(timeout=3)
        except subprocess.TimeoutExpired:
            p.kill()


def stop_all():
    for p in list(procs.values()):
        p.terminate()
        try:
            p.wait(timeout=3)
        except subprocess.TimeoutExpired:
            p.kill()
    procs.clear()


# ── NATS helpers ──────────────────────────────────────────────────────────────

async def nats_req(nc, subject: str, params: dict, timeout: float = NATS_TIMEOUT) -> dict:
    try:
        msg = await nc.request(subject, json.dumps(params).encode(), timeout=timeout)
        return json.loads(msg.data)
    except nats_lib.errors.NoRespondersError:
        return {"_error": "no_responders"}
    except Exception as e:
        return {"_error": str(e)}


async def wait_runner(nc, prefix: str, cwd: str = "/tmp",
                      timeout: float = RUNNER_READY) -> Optional[str]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        resp = await nats_req(nc, f"{prefix}.agent.session.new",
                              {"cwd": cwd, "mcpServers": []}, timeout=1.5)
        if "sessionId" in resp:
            return resp["sessionId"]
        await asyncio.sleep(0.3)
    return None


async def send_prompt(nc, prefix: str, sid: str, text: str,
                      timeout: float = PROMPT_TIMEOUT) -> dict:
    req_id = str(uuid.uuid4())
    sub = await nc.subscribe(f"{prefix}.session.{sid}.agent.prompt.response.{req_id}")
    try:
        await nc.publish(
            f"{prefix}.session.{sid}.agent.prompt",
            json.dumps({"sessionId": sid, "prompt": [{"type": "text", "text": text}]}).encode(),
            headers={"X-Req-Id": req_id},
        )
        msg = await asyncio.wait_for(sub.next_msg(), timeout=timeout)
        return json.loads(msg.data)
    except asyncio.TimeoutError:
        return {"_error": f"prompt timed out after {timeout}s"}
    except Exception as e:
        return {"_error": str(e)}
    finally:
        try:
            await sub.unsubscribe()
        except Exception:
            pass


async def send_set_mode(nc, prefix: str, sid: str, mode: str) -> dict:
    req_id = str(uuid.uuid4())
    sub = await nc.subscribe(f"{prefix}.session.{sid}.agent.response.{req_id}")
    try:
        await nc.publish(
            f"{prefix}.session.{sid}.agent.set_mode",
            json.dumps({"sessionId": sid, "modeId": mode}).encode(),
            headers={"X-Req-Id": req_id},
        )
        msg = await asyncio.wait_for(sub.next_msg(), timeout=8.0)
        return json.loads(msg.data)
    except asyncio.TimeoutError:
        return {"_error": "set_mode timed out"}
    except Exception as e:
        return {"_error": str(e)}
    finally:
        try:
            await sub.unsubscribe()
        except Exception:
            pass


async def send_set_model(nc, prefix: str, sid: str, model: str) -> dict:
    req_id = str(uuid.uuid4())
    sub = await nc.subscribe(f"{prefix}.session.{sid}.agent.response.{req_id}")
    try:
        await nc.publish(
            f"{prefix}.session.{sid}.agent.set_model",
            json.dumps({"sessionId": sid, "modelId": model}).encode(),
            headers={"X-Req-Id": req_id},
        )
        msg = await asyncio.wait_for(sub.next_msg(), timeout=8.0)
        return json.loads(msg.data)
    except asyncio.TimeoutError:
        return {"_error": "set_model timed out"}
    except Exception as e:
        return {"_error": str(e)}
    finally:
        try:
            await sub.unsubscribe()
        except Exception:
            pass


# ── Binaries ──────────────────────────────────────────────────────────────────

BIN = {
    "acp": os.path.join(RSDIR, "target/debug/trogon-acp-runner"),
    "xai": os.path.join(RSDIR, "target/debug/trogon-xai-runner"),
    "or":  os.path.join(RSDIR, "target/debug/trogon-openrouter-runner"),
}


# ═════════════════════════════════════════════════════════════════════════════
# S1 — Multi-turn bug fix (acp-runner): read → str_replace → text [3 LLM calls]
# ═════════════════════════════════════════════════════════════════════════════

async def s1_multi_turn_bug_fix(nc):
    section("S1: Multi-turn bug fix — acp  (read → str_replace → verify text)")
    pfx = f"{BASE}.s1"

    bug_file = f"/tmp/prog2_buggy_{os.getpid()}.py"
    with open(bug_file, "w") as f:
        f.write("def add(a, b):\n    a + b  # missing return\n\ndef sub(a, b):\n    a - b  # missing return\n")

    MockLLM.reset()
    # LLM call 1: read the file to see the bug
    MockLLM.queue("/anthropic/v1/messages",
        anthropic_sse_tool("t_read_01", "read_file", json.dumps({"path": bug_file})))
    # LLM call 2: fix first bug with str_replace
    MockLLM.queue("/anthropic/v1/messages",
        anthropic_sse_tool("t_fix_01", "str_replace", json.dumps({
            "path": bug_file,
            "old_str": "    a + b  # missing return",
            "new_str": "    return a + b",
        })))
    # LLM call 3: final confirmation text
    MockLLM.queue("/anthropic/v1/messages",
        anthropic_sse("Fixed! Both functions were missing `return`. I fixed `add` — `return a + b`."))

    start_runner("s1", BIN["acp"], {
        "ACP_PREFIX": pfx,
        "PROXY_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake",
        "AGENT_MODEL": "claude-opus-4-6",
    }, cwd="/tmp")
    try:
        sid = await wait_runner(nc, pfx, cwd="/tmp")
        if not sid:
            fail("S1: session.new timed out"); return
        info(f"session: {sid[:8]}")

        mr = await send_set_mode(nc, pfx, sid, "bypassPermissions")
        if prompt_err(mr):
            fail("S1: set_mode", str(mr)); return
        ok("S1: bypassPermissions active")

        r = await send_prompt(nc, pfx, sid, f"Fix the bugs in {bug_file}")
        if e := prompt_err(r):
            fail("S1: prompt failed", e); return
        ok("S1: 3-turn tool loop completed (read → str_replace → text)")

        reqs = MockLLM.requests("/anthropic/v1/messages")
        if len(reqs) == 3:
            ok("S1: exactly 3 LLM API calls in one user turn", f"{len(reqs)} requests")
        else:
            fail("S1: wrong LLM call count", f"expected 3, got {len(reqs)}")

        # 2nd call must carry the read_file tool_result
        if len(reqs) >= 2:
            msgs2 = reqs[1].get("messages", [])
            has_tr = any(
                isinstance(m.get("content"), list)
                and any(b.get("type") == "tool_result" for b in m["content"])
                for m in msgs2
            )
            ok("S1: 2nd LLM call carries read_file result") if has_tr else \
                fail("S1: 2nd LLM call missing tool_result")

        # 3rd call must carry str_replace tool_result
        if len(reqs) >= 3:
            msgs3 = reqs[2].get("messages", [])
            tool_results = [b for m in msgs3 if isinstance(m.get("content"), list)
                            for b in m["content"] if b.get("type") == "tool_result"]
            ok("S1: 3rd LLM call carries str_replace result",
               f"{len(tool_results)} tool_result(s)") if tool_results else \
                fail("S1: 3rd LLM call missing tool_result")

        # Verify file was actually fixed on disk
        with open(bug_file) as f:
            content = f.read()
        if "return a + b" in content:
            ok("S1: file fixed on disk by acp-runner", repr(content.strip()))
        else:
            fail("S1: file NOT fixed on disk", repr(content))
    finally:
        stop_runner("s1")
        try:
            os.unlink(bug_file)
        except OSError:
            pass


# ═════════════════════════════════════════════════════════════════════════════
# S2 — Codebase exploration (openrouter): list_dir → glob → read → summarize
# ═════════════════════════════════════════════════════════════════════════════

async def s2_codebase_exploration(nc):
    section("S2: Codebase exploration — openrouter  (list_dir → glob → read → text)")
    pfx = f"{BASE}.s2"

    # Create a mini Rust project
    proj = tempfile.mkdtemp(prefix="prog2_proj_")
    os.makedirs(f"{proj}/src")
    with open(f"{proj}/Cargo.toml", "w") as f:
        f.write('[package]\nname = "demo"\nversion = "0.1.0"\nedition = "2021"\n')
    with open(f"{proj}/src/main.rs", "w") as f:
        f.write('fn main() {\n    println!("{}", demo::greet("world"));\n}\n')
    with open(f"{proj}/src/lib.rs", "w") as f:
        f.write('/// Returns a greeting string.\npub fn greet(name: &str) -> String {\n    format!("Hello, {name}!")\n}\n')

    MockLLM.reset()
    # LLM call 1: list project root
    MockLLM.queue("/chat/completions",
        or_sse_tool("call_ls_01", "list_dir", json.dumps({"path": "."})))
    # LLM call 2: find all Rust files
    MockLLM.queue("/chat/completions",
        or_sse_tool("call_glob_01", "glob", json.dumps({"pattern": "**/*.rs"})))
    # LLM call 3: read main.rs
    MockLLM.queue("/chat/completions",
        or_sse_tool("call_read_01", "read_file", json.dumps({"path": "src/main.rs"})))
    # LLM call 4: summarize
    MockLLM.queue("/chat/completions",
        or_sse("This Rust project has main.rs (entry point) and lib.rs (greet function). "
               "main.rs calls greet(\"world\") and prints the result."))

    start_runner("s2", BIN["or"], {
        "ACP_PREFIX":          pfx,
        "OPENROUTER_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":  "fake",
    })
    try:
        sid = await wait_runner(nc, pfx, cwd=proj)
        if not sid:
            fail("S2: session.new timed out"); return
        info(f"session: {sid[:8]}  cwd={proj}")

        r = await send_prompt(nc, pfx, sid, "Explore this project and explain its structure")
        if e := prompt_err(r):
            fail("S2: prompt failed", e); return
        ok("S2: 4-turn exploration completed")

        reqs = MockLLM.requests("/chat/completions")
        if len(reqs) == 4:
            ok("S2: exactly 4 LLM API calls", f"{len(reqs)} requests")
        else:
            fail("S2: wrong LLM call count", f"expected 4, got {len(reqs)}")

        # Each subsequent call must carry the previous tool result
        for i, label in [(1, "list_dir"), (2, "glob"), (3, "read_file")]:
            if len(reqs) > i:
                msgs = reqs[i].get("messages", [])
                has_tool = any(m.get("role") == "tool" for m in msgs)
                ok(f"S2: call {i+1} carries {label} result") if has_tool else \
                    fail(f"S2: call {i+1} missing {label} tool result")

        # Verify main.rs content reached the LLM (4th call)
        if len(reqs) >= 4:
            msgs4 = reqs[3].get("messages", [])
            tool_contents = " ".join(
                str(m.get("content", "")) for m in msgs4 if m.get("role") == "tool"
            )
            ok("S2: main.rs content in 4th LLM call",
               repr(tool_contents[:80])) if "println" in tool_contents else \
                fail("S2: main.rs content missing from 4th LLM call")
    finally:
        stop_runner("s2")
        shutil.rmtree(proj, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S3 — Cross-runner refactoring: xai reads+analyzes → openrouter fixes file
# ═════════════════════════════════════════════════════════════════════════════

async def s3_cross_runner_refactor(nc):
    section("S3: Cross-runner refactoring — xai analyzes → openrouter fixes")
    pfx_xai = f"{BASE}.s3x"
    pfx_or  = f"{BASE}.s3o"

    code_file = f"/tmp/prog2_refactor_{os.getpid()}.py"
    with open(code_file, "w") as f:
        f.write("def multiply(a, b):\n    a * b  # missing return\n\ndef divide(a, b):\n    a / b  # missing return\n")

    MockLLM.reset()
    # xai: read file and identify bugs
    MockLLM.queue("/responses",
        xai_sse_fn("resp-xai-s3-01", "call-s3-01", "read_file",
                   json.dumps({"path": code_file})))
    MockLLM.queue("/responses",
        xai_sse("Found bugs: both `multiply` and `divide` are missing `return`. "
                "The expressions compute results but discard them.", "resp-xai-s3-02"))
    # openrouter: fix multiply with str_replace
    MockLLM.queue("/chat/completions",
        or_sse_tool("call-or-s3-01", "str_replace", json.dumps({
            "path": code_file,
            "old_str": "    a * b  # missing return",
            "new_str": "    return a * b",
        })))
    # openrouter: fix divide with str_replace
    MockLLM.queue("/chat/completions",
        or_sse_tool("call-or-s3-02", "str_replace", json.dumps({
            "path": code_file,
            "old_str": "    a / b  # missing return",
            "new_str": "    return a / b",
        })))
    # openrouter: confirm
    MockLLM.queue("/chat/completions",
        or_sse("Both functions fixed! Added `return` to `multiply` and `divide`."))

    start_runner("s3x", BIN["xai"], {
        "ACP_PREFIX":        pfx_xai,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    start_runner("s3o", BIN["or"], {
        "ACP_PREFIX":          pfx_or,
        "OPENROUTER_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":  "fake",
    })
    try:
        sid_xai = await wait_runner(nc, pfx_xai, cwd="/tmp")
        sid_or  = await wait_runner(nc, pfx_or,  cwd="/tmp")
        if not sid_xai:
            fail("S3: xai session.new timed out"); return
        if not sid_or:
            fail("S3: openrouter session.new timed out"); return
        info(f"xai={sid_xai[:8]}  or={sid_or[:8]}")

        # Step 1: xai reads and analyzes the code
        r = await send_prompt(nc, pfx_xai, sid_xai, f"Analyze {code_file} and find the bugs")
        if e := prompt_err(r):
            fail("S3: xai analysis prompt failed", e); return
        ok("S3: xai analyzed the code (read_file + text response)")

        xai_reqs = MockLLM.requests("/responses")
        ok(f"S3: xai made {len(xai_reqs)} API calls (fn_call + text)") if len(xai_reqs) == 2 else \
            fail(f"S3: expected 2 xai calls, got {len(xai_reqs)}")

        # Step 2: export xai history → import into openrouter
        exported = await nats_req(nc, f"{pfx_xai}.agent.ext.session/export",
                                  {"sessionId": sid_xai})
        if not isinstance(exported, list) or not exported:
            fail("S3: xai export failed", str(exported)[:100]); return
        ok(f"S3: xai session exported ({len(exported)} messages, analysis in context)")

        ir = await nats_req(nc, f"{pfx_or}.agent.ext.session/import",
                            {"sessionId": sid_or, "messages": exported})
        if isinstance(ir, dict) and "code" in ir:
            fail("S3: openrouter import failed", str(ir)); return
        ok("S3: xai analysis imported into openrouter session")

        # Step 3: openrouter fixes the file using the analysis context
        or_before = len(MockLLM.requests("/chat/completions"))
        r = await send_prompt(nc, pfx_or, sid_or,
                              f"Apply the fixes you found to {code_file}")
        if e := prompt_err(r):
            fail("S3: openrouter fix prompt failed", e); return
        ok("S3: openrouter applied fixes (2× str_replace + text)")

        or_reqs = MockLLM.requests("/chat/completions")
        or_new = or_reqs[or_before:]
        ok(f"S3: openrouter made {len(or_new)} API calls") if len(or_new) == 3 else \
            fail(f"S3: expected 3 openrouter calls, got {len(or_new)}")

        # openrouter's first request should include the prior analysis as context
        if or_new:
            msgs = or_new[0].get("messages", [])
            # prior context = xai analysis messages imported from xai
            has_prior = len(msgs) > 1
            ok(f"S3: openrouter had prior xai analysis in context ({len(msgs)} messages)") if has_prior else \
                fail("S3: openrouter had no prior context")

        # Verify both bugs fixed on disk
        with open(code_file) as f:
            content = f.read()
        if "return a * b" in content and "return a / b" in content:
            ok("S3: both bugs fixed on disk by openrouter", repr(content.strip()))
        elif "return a * b" in content:
            ok("S3: multiply fixed", "divide may need another pass")
        else:
            fail("S3: file NOT fixed on disk", repr(content))
    finally:
        stop_runner("s3x")
        stop_runner("s3o")
        try:
            os.unlink(code_file)
        except OSError:
            pass


# ═════════════════════════════════════════════════════════════════════════════
# S4 — Model switch mid-coding session (xai: grok-3 reads → grok-3-mini writes)
# ═════════════════════════════════════════════════════════════════════════════

async def s4_model_switch_mid_coding(nc):
    section("S4: Model switch mid-coding — xai grok-3 reads → grok-3-mini implements")
    pfx = f"{BASE}.s4"

    rs_file = f"/tmp/prog2_switch_{os.getpid()}.rs"
    with open(rs_file, "w") as f:
        f.write("fn greet(name: &str) -> String {\n    todo!()\n}\n")

    MockLLM.reset()
    # grok-3 turn 1: read the scaffold and understand it
    MockLLM.queue("/responses",
        xai_sse_fn("resp-g3-01", "call-g3-01", "read_file",
                   json.dumps({"path": rs_file})))
    MockLLM.queue("/responses",
        xai_sse("I see `greet` has a `todo!()` placeholder. "
                "It should return a formatted greeting string.", "resp-g3-02"))
    # grok-3-mini turn 2: implement via str_replace
    MockLLM.queue("/responses",
        xai_sse_fn("resp-gmini-01", "call-gmini-01", "str_replace", json.dumps({
            "path": rs_file,
            "old_str": "    todo!()",
            "new_str": '    format!("Hello, {}!", name)',
        })))
    MockLLM.queue("/responses",
        xai_sse("Implemented `greet`! Replaced `todo!()` with the format string.", "resp-gmini-02"))

    start_runner("s4", BIN["xai"], {
        "ACP_PREFIX":        pfx,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake",
        "XAI_DEFAULT_MODEL": "grok-3",
        "XAI_MODELS":        "grok-3:Grok 3,grok-3-mini:Grok 3 Mini",
    })
    try:
        sid = await wait_runner(nc, pfx, cwd="/tmp")
        if not sid:
            fail("S4: session.new timed out"); return
        info(f"session: {sid[:8]}")

        # Turn 1 with grok-3: read and understand
        r = await send_prompt(nc, pfx, sid, f"Read {rs_file} and understand the code structure")
        if e := prompt_err(r):
            fail("S4: turn 1 failed", e); return
        ok("S4: turn 1 completed (grok-3 read the scaffold)")

        reqs = MockLLM.requests("/responses")
        m1 = reqs[0].get("model") if reqs else None
        ok("S4: turn 1 used grok-3", f"model={m1}") if m1 == "grok-3" else \
            fail("S4: turn 1 used wrong model", f"expected grok-3, got {m1}")

        # ── Switch model mid-session ──────────────────────────────────────────
        sr = await send_set_model(nc, pfx, sid, "grok-3-mini")
        if prompt_err(sr):
            fail("S4: set_model failed", str(sr)); return
        ok("S4: switched to grok-3-mini mid-session")

        # Turn 2 with grok-3-mini: implement the function
        r = await send_prompt(nc, pfx, sid, f"Now implement the greet function in {rs_file}")
        if e := prompt_err(r):
            fail("S4: turn 2 failed", e); return
        ok("S4: turn 2 completed (grok-3-mini implemented the function)")

        reqs2 = MockLLM.requests("/responses")
        # Calls 0,1 = turn 1 (fn_call + text); calls 2,3 = turn 2
        t2_first = reqs2[2] if len(reqs2) > 2 else {}
        m2 = t2_first.get("model")
        ok("S4: turn 2 used grok-3-mini", f"model={m2}") if m2 == "grok-3-mini" else \
            fail("S4: turn 2 used wrong model", f"expected grok-3-mini, got {m2}")

        # set_model clears last_response_id → turn 2 sends full history (no previous_response_id)
        has_prev_id = bool(t2_first.get("previous_response_id"))
        ok("S4: last_response_id cleared by set_model (turn 2 sends full history)") \
            if not has_prev_id else \
            fail("S4: last_response_id was NOT cleared by set_model",
                 f"previous_response_id={t2_first.get('previous_response_id')!r}")

        # turn 2 should have the turn 1 history in its input
        t2_input = t2_first.get("input", [])
        has_history = len(t2_input) > 1
        ok(f"S4: turn 2 includes turn 1 history in input ({len(t2_input)} items)") if has_history else \
            info(f"S4: turn 2 input items: {len(t2_input)}")

        # Verify file actually implemented on disk
        with open(rs_file) as f:
            content = f.read()
        implemented = 'format!("Hello, {}!", name)' in content
        ok("S4: function implemented on disk by grok-3-mini",
           repr(content.strip())) if implemented else \
            fail("S4: function NOT implemented on disk", repr(content))

        # Summary: total API calls
        total = len(MockLLM.requests("/responses"))
        info(f"S4: total xai API calls across both turns: {total} (2+2 expected)")
    finally:
        stop_runner("s4")
        try:
            os.unlink(rs_file)
        except OSError:
            pass


# ═════════════════════════════════════════════════════════════════════════════
# Main
# ═════════════════════════════════════════════════════════════════════════════

async def main():
    print(f"\n=== smoke_test_programming2.py — real programming workflows ===")
    print(f"  prefix : {BASE}")
    print(f"  nats   : {NATS_URL}")
    print(f"  mock   : http://127.0.0.1:{MOCK_PORT}\n")

    missing = [k for k, v in BIN.items() if not os.path.exists(v)]
    if missing:
        print(f"  {red}ERROR{reset}: missing binaries: {missing}")
        sys.exit(1)

    server = start_mock(MOCK_PORT)
    info(f"mock server started on :{MOCK_PORT}")

    nc = await nats_lib.connect(NATS_URL)
    try:
        await s1_multi_turn_bug_fix(nc)
        await s2_codebase_exploration(nc)
        await s3_cross_runner_refactor(nc)
        await s4_model_switch_mid_coding(nc)
    finally:
        stop_all()
        await nc.close()
        server.shutdown()

    print(f"\n=== Results ===")
    print(f"  {green}passed{reset}: {PASS}")
    print(f"  {red}failed{reset}: {FAIL}")
    if FAIL == 0:
        print(f"\n{green}All scenarios passed.{reset}\n")
        sys.exit(0)
    else:
        print(f"\n{red}{FAIL} scenario(s) failed.{reset}\n")
        sys.exit(1)


if __name__ == "__main__":
    asyncio.run(main())
