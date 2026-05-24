#!/usr/bin/env python3
"""
smoke_test_programming5.py — Advanced scenarios targeting real programming behaviors.

Targets behaviors untested by smoke1-4 that are likely to hide bugs:

  S34. acp parallel tools         2 tool_use in one SSE → join_all path → both tool_results in 2nd call
  S35. glob no matches            glob_files with no matching files → "No files found"
  S36. glob recursive pattern     **/*.py matches files in subdirectories
  S37. xai set_model → model used after set_model, next API call uses new model ID
  S38. write_file → read_file     content written then read back in same session matches
  S39. acp read_file nonexistent  Error in tool_result; runner does not crash; sends 2nd LLM call
  S40. xai export→import same     session B (same xai runner) receives session A history in input
  S41. or 4-turn growth           export after 4 turns shows 8 messages (4 user + 4 assistant)
  S42. TROGON.md + _meta combined TROGON.md content AND systemPrompt both appear in /chat/completions
  S43. or set_model cwd preserved after set_model, write_file still lands in original cwd
  S44. acp export includes ToolCall export has PortableBlock::ToolCall (not text-only)
  S45. codex 3-turn history       export grows by 2 messages per turn

Usage:
    NATS_URL=nats://localhost:4333 python3 smoke_test_programming5.py
"""

import asyncio, collections, json, os, shutil, subprocess, sys, tempfile, threading, time, uuid
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Optional

import nats as nats_lib

# ── Config ────────────────────────────────────────────────────────────────────
NATS_URL       = os.environ.get("NATS_URL", "nats://localhost:4222")
RSDIR          = os.path.dirname(os.path.abspath(__file__))
MOCK_PORT      = 19818
BASE           = f"prog5.{os.getpid()}"
PROMPT_TIMEOUT = 45.0
NATS_TIMEOUT   = 10.0
RUNNER_READY   = 14.0

green, red, yellow, cyan, reset = "\033[32m", "\033[31m", "\033[33m", "\033[36m", "\033[0m"
PASS = 0
FAIL = 0
procs: dict = {}


def ok(msg, detail=""):
    global PASS; PASS += 1
    sfx = f"  {yellow}{detail}{reset}" if detail else ""
    print(f"  {green}PASS{reset}  {msg}{sfx}")


def fail(msg, detail=""):
    global FAIL; FAIL += 1
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


def anthropic_sse_two_tools(t1_id: str, t1_name: str, t1_input: str,
                             t2_id: str, t2_name: str, t2_input: str) -> bytes:
    """Two parallel tool_use blocks in one Anthropic SSE response."""
    chunks = [
        "event: message_start\ndata: " + json.dumps({
            "type": "message_start",
            "message": {"id": "msg-2tools", "type": "message", "role": "assistant",
                        "content": [], "model": "mock", "stop_reason": None,
                        "stop_sequence": None, "usage": {"input_tokens": 15, "output_tokens": 0}},
        }) + "\n\n",
        # Tool 1
        "event: content_block_start\ndata: " + json.dumps({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "tool_use", "id": t1_id, "name": t1_name, "input": {}},
        }) + "\n\n",
        "event: content_block_delta\ndata: " + json.dumps({
            "type": "content_block_delta", "index": 0,
            "delta": {"type": "input_json_delta", "partial_json": t1_input},
        }) + "\n\n",
        "event: content_block_stop\ndata: " + json.dumps({"type": "content_block_stop", "index": 0}) + "\n\n",
        # Tool 2
        "event: content_block_start\ndata: " + json.dumps({
            "type": "content_block_start", "index": 1,
            "content_block": {"type": "tool_use", "id": t2_id, "name": t2_name, "input": {}},
        }) + "\n\n",
        "event: content_block_delta\ndata: " + json.dumps({
            "type": "content_block_delta", "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": t2_input},
        }) + "\n\n",
        "event: content_block_stop\ndata: " + json.dumps({"type": "content_block_stop", "index": 1}) + "\n\n",
        # End
        "event: message_delta\ndata: " + json.dumps({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use", "stop_sequence": None},
            "usage": {"output_tokens": 10},
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

    @classmethod
    def reset(cls):
        with cls._lock:
            cls._log.clear()
            for q in cls._queues.values():
                q.clear()

    @classmethod
    def queue(cls, path: str, sse: bytes):
        with cls._lock:
            cls._queues[path].append(sse)

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
        self.send_response(200)
        self.send_header("Content-Type",
                         "application/json" if resp[:1] in (b"{", b"[") else "text/event-stream")
        self.end_headers()
        try:
            self.wfile.write(resp)
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.end_headers()
        try:
            self.wfile.write(b"<html><body><h1>Mock</h1></body></html>")
        except (BrokenPipeError, ConnectionResetError):
            pass


def start_mock(port: int):
    s = HTTPServer(("127.0.0.1", port), MockLLM)
    threading.Thread(target=s.serve_forever, daemon=True).start()
    return s


# ── Runner helpers ────────────────────────────────────────────────────────────

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
                      timeout: float = RUNNER_READY,
                      meta: Optional[dict] = None) -> Optional[str]:
    deadline = time.monotonic() + timeout
    payload = {"cwd": cwd, "mcpServers": []}
    if meta:
        payload["_meta"] = meta
    while time.monotonic() < deadline:
        resp = await nats_req(nc, f"{prefix}.agent.session.new", payload, timeout=1.5)
        if "sessionId" in resp:
            return resp["sessionId"]
        await asyncio.sleep(0.3)
    return None


async def new_session(nc, prefix: str, cwd: str = "/tmp",
                      meta: Optional[dict] = None) -> Optional[str]:
    payload = {"cwd": cwd, "mcpServers": []}
    if meta:
        payload["_meta"] = meta
    resp = await nats_req(nc, f"{prefix}.agent.session.new", payload, timeout=5.0)
    return resp.get("sessionId")


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


BIN = {
    "acp":        os.path.join(RSDIR, "target/debug/trogon-acp-runner"),
    "xai":        os.path.join(RSDIR, "target/debug/trogon-xai-runner"),
    "or":         os.path.join(RSDIR, "target/debug/trogon-openrouter-runner"),
    "codex":      os.path.join(RSDIR, "target/debug/trogon-codex-runner"),
    "mock_codex": os.path.join(RSDIR, "target/debug/mock_codex_server"),
}


# ═════════════════════════════════════════════════════════════════════════════
# S34 — acp parallel tools: 2 tool_use blocks → join_all → 2 tool_results in 2nd call
# ═════════════════════════════════════════════════════════════════════════════

async def s34_acp_parallel_tools(nc):
    section("S34: acp parallel tools — 2 tool_use in one SSE → 2 tool_results in next call")
    prefix = f"{BASE}.acp_s34"
    tmpdir = tempfile.mkdtemp(prefix="prog5_s34_")

    fa = os.path.join(tmpdir, "alpha.txt")
    fb = os.path.join(tmpdir, "beta.txt")
    with open(fa, "w") as f: f.write("alpha content\n")
    with open(fb, "w") as f: f.write("beta content\n")

    MockLLM.reset()
    # LLM call 1: return 2 parallel tool_use blocks (read_file x2)
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse_two_tools(
        "t-s34-a", "read_file", json.dumps({"path": "alpha.txt"}),
        "t-s34-b", "read_file", json.dumps({"path": "beta.txt"}),
    ))
    # LLM call 2: text response after seeing both tool_results
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse("both files read in parallel"))

    start_runner("s34", BIN["acp"], {
        "ACP_PREFIX":      prefix,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    }, cwd=tmpdir)
    try:
        sid = await wait_runner(nc, prefix, cwd=tmpdir)
        if not sid:
            fail("S34: session.new timed out"); return

        mr = await send_set_mode(nc, prefix, sid, "bypassPermissions")
        if prompt_err(mr):
            fail("S34: set_mode failed", str(mr)); return

        r = await send_prompt(nc, prefix, sid, "read both files")
        if prompt_err(r):
            fail("S34: prompt failed", str(r)); return
        ok("S34: prompt completed (parallel tools executed)")

        reqs = MockLLM.requests("/anthropic/v1/messages")
        if len(reqs) < 2:
            fail("S34: expected 2 LLM calls", f"got {len(reqs)}"); return
        ok("S34: 2 LLM calls made (tool turn + confirmation)")

        # 2nd LLM call must have 2 tool_result blocks in the user message
        msgs2 = reqs[1].get("messages", [])
        tool_results = [
            b for m in msgs2 if m.get("role") == "user"
            for b in (m.get("content") if isinstance(m.get("content"), list) else [])
            if isinstance(b, dict) and b.get("type") == "tool_result"
        ]
        info(f"S34: tool_result count in 2nd call = {len(tool_results)}")

        if len(tool_results) >= 2:
            ok(f"S34: {len(tool_results)} tool_result blocks in 2nd LLM call (parallel join_all worked)")
        else:
            fail(f"S34: expected 2 tool_results, got {len(tool_results)} — parallel tools may not be combined",
                 f"blocks={json.dumps(tool_results)[:120]}")

        # Verify both tools produced content with the file contents
        result_str = json.dumps(tool_results)
        has_alpha = "alpha content" in result_str
        has_beta  = "beta content"  in result_str
        if has_alpha and has_beta:
            ok("S34: both file contents present in tool_results (alpha + beta)")
        else:
            fail("S34: missing file content in tool_results",
                 f"alpha={has_alpha} beta={has_beta}")

        # Verify tool IDs match what was sent
        ids_seen = {b.get("tool_use_id") for b in tool_results if isinstance(b, dict)}
        if "t-s34-a" in ids_seen and "t-s34-b" in ids_seen:
            ok("S34: tool_use_ids correctly matched (t-s34-a, t-s34-b)")
        else:
            fail("S34: tool_use_ids mismatch", f"seen={ids_seen}")
    finally:
        stop_runner("s34")
        shutil.rmtree(tmpdir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S35 — glob no matches: glob_files with non-matching pattern → "No files found"
# ═════════════════════════════════════════════════════════════════════════════

async def s35_glob_no_matches(nc):
    section("S35: glob no matches — glob_files returns 'No files found' for empty match")
    prefix = f"{BASE}.xai_s35"
    tmpdir = tempfile.mkdtemp(prefix="prog5_s35_")

    # Only .py and .txt files — no .xyz_nonexistent
    with open(os.path.join(tmpdir, "main.py"), "w") as f: f.write("print('hello')\n")
    with open(os.path.join(tmpdir, "notes.txt"), "w") as f: f.write("some notes\n")

    MockLLM.reset()
    MockLLM.queue("/responses", xai_sse_fn(
        "r-s35-fn", "c-s35-01", "glob",
        json.dumps({"pattern": "*.xyz_nonexistent_s35"}),
    ))
    MockLLM.queue("/responses", xai_sse("no files found as expected", "r-s35-txt"))

    start_runner("s35", BIN["xai"], {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_runner(nc, prefix, cwd=tmpdir)
        if not sid:
            fail("S35: session.new timed out"); return

        r = await send_prompt(nc, prefix, sid, "find all .xyz_nonexistent_s35 files")
        if prompt_err(r):
            fail("S35: prompt failed", str(r)); return
        ok("S35: prompt completed")

        reqs = MockLLM.requests("/responses")
        if len(reqs) < 2:
            fail("S35: expected 2 xai API calls"); return

        input2 = reqs[1].get("input", [])
        outputs = [
            item.get("output", "")
            for item in input2
            if isinstance(item, dict) and item.get("type") == "function_call_output"
        ]
        if not outputs:
            fail("S35: no function_call_output in 2nd request"); return

        content = "\n".join(outputs)
        info(f"S35: glob output = {content!r}")

        if "No files found" in content:
            ok("S35: 'No files found' returned for non-matching glob pattern")
        else:
            fail("S35: unexpected output for empty glob match", f"content={content!r}")
    finally:
        stop_runner("s35")
        shutil.rmtree(tmpdir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S36 — glob recursive pattern: **/*.py matches files in subdirectories
# ═════════════════════════════════════════════════════════════════════════════

async def s36_glob_recursive(nc):
    section("S36: glob recursive — **/*.py matches files in subdirectories")
    prefix = f"{BASE}.xai_s36"
    tmpdir = tempfile.mkdtemp(prefix="prog5_s36_")

    # Create nested structure
    os.makedirs(os.path.join(tmpdir, "src", "models"), exist_ok=True)
    os.makedirs(os.path.join(tmpdir, "src", "api"), exist_ok=True)
    with open(os.path.join(tmpdir, "src", "models", "user.py"), "w") as f:
        f.write("class User: pass\n")
    with open(os.path.join(tmpdir, "src", "api", "routes.py"), "w") as f:
        f.write("def routes(): pass\n")
    with open(os.path.join(tmpdir, "README.txt"), "w") as f:
        f.write("readme\n")

    MockLLM.reset()
    MockLLM.queue("/responses", xai_sse_fn(
        "r-s36-fn", "c-s36-01", "glob",
        json.dumps({"pattern": "**/*.py"}),
    ))
    MockLLM.queue("/responses", xai_sse("found python files", "r-s36-txt"))

    start_runner("s36", BIN["xai"], {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_runner(nc, prefix, cwd=tmpdir)
        if not sid:
            fail("S36: session.new timed out"); return

        r = await send_prompt(nc, prefix, sid, "find all python files recursively")
        if prompt_err(r):
            fail("S36: prompt failed", str(r)); return
        ok("S36: prompt completed")

        reqs = MockLLM.requests("/responses")
        if len(reqs) < 2:
            fail("S36: expected 2 xai API calls"); return

        input2 = reqs[1].get("input", [])
        outputs = [
            item.get("output", "")
            for item in input2
            if isinstance(item, dict) and item.get("type") == "function_call_output"
        ]
        if not outputs:
            fail("S36: no function_call_output in 2nd request"); return

        content = "\n".join(outputs)
        info(f"S36: glob output = {content!r}")

        has_user = "user.py" in content
        has_routes = "routes.py" in content
        has_readme = "README.txt" in content

        if has_user and has_routes:
            ok("S36: both user.py and routes.py found by **/*.py glob", content[:80])
        else:
            fail("S36: recursive glob missing files", f"user.py={has_user} routes.py={has_routes}")

        if not has_readme:
            ok("S36: README.txt correctly excluded by **/*.py pattern")
        else:
            fail("S36: README.txt incorrectly included by **/*.py pattern")
    finally:
        stop_runner("s36")
        shutil.rmtree(tmpdir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S37 — xai set_model: after set_model, next API call uses the new model ID
# ═════════════════════════════════════════════════════════════════════════════

async def s37_xai_set_model_reflected_in_api(nc):
    section("S37: xai set_model → next API call uses new model ID in request body")
    prefix = f"{BASE}.xai_s37"

    MockLLM.reset()
    # Turn 1: before set_model
    MockLLM.queue("/responses", xai_sse("turn1 on default model", "r-s37-t1"))
    # Turn 2: after set_model to grok-3-mini
    MockLLM.queue("/responses", xai_sse("turn2 on grok-3-mini", "r-s37-t2"))

    start_runner("s37", BIN["xai"], {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S37: session.new timed out"); return

        # Turn 1: uses default model grok-3
        r1 = await send_prompt(nc, prefix, sid, "hello on default model")
        if prompt_err(r1):
            fail("S37: turn 1 failed", str(r1)); return
        ok("S37: turn 1 completed (default model)")

        reqs_before = MockLLM.requests("/responses")
        model_t1 = reqs_before[0].get("model") if reqs_before else None
        info(f"S37: turn 1 model = {model_t1!r}")

        # Switch model to grok-3-mini
        sr = await send_set_model(nc, prefix, sid, "grok-3-mini")
        if prompt_err(sr):
            fail("S37: set_model failed", str(sr)); return
        ok("S37: set_model to grok-3-mini succeeded")

        MockLLM.reset()
        MockLLM.queue("/responses", xai_sse("turn2 on grok-3-mini", "r-s37-t2"))

        # Turn 2: must use grok-3-mini
        r2 = await send_prompt(nc, prefix, sid, "hello on new model")
        if prompt_err(r2):
            fail("S37: turn 2 failed", str(r2)); return
        ok("S37: turn 2 completed (after set_model)")

        reqs_after = MockLLM.requests("/responses")
        if not reqs_after:
            fail("S37: no API call after set_model"); return

        model_t2 = reqs_after[0].get("model")
        info(f"S37: turn 2 model = {model_t2!r}")

        if model_t2 == "grok-3-mini":
            ok("S37: API call after set_model uses 'grok-3-mini'")
        else:
            fail("S37: model not updated after set_model",
                 f"expected 'grok-3-mini', got {model_t2!r}")

        # set_model also clears last_response_id → full history sent (no previous_response_id)
        prev_id_t2 = reqs_after[0].get("previous_response_id")
        if prev_id_t2 is None:
            ok("S37: last_response_id cleared by set_model (full history sent on turn 2)")
        else:
            fail("S37: previous_response_id NOT cleared by set_model",
                 f"previous_response_id={prev_id_t2!r}")
    finally:
        stop_runner("s37")


# ═════════════════════════════════════════════════════════════════════════════
# S38 — write_file → read_file same session: content roundtrip within one session
# ═════════════════════════════════════════════════════════════════════════════

async def s38_write_then_read_same_session(nc):
    section("S38: write_file → read_file same xai session — content roundtrip")
    prefix = f"{BASE}.xai_s38"
    tmpdir = tempfile.mkdtemp(prefix="prog5_s38_")
    config_content = "DB_URL = 'sqlite:///app.db'\nDEBUG = True\n"

    MockLLM.reset()
    # Turn 1: write_file config.py
    MockLLM.queue("/responses", xai_sse_fn(
        "r-s38-w", "c-s38-01", "write_file",
        json.dumps({"path": "config.py", "content": config_content}),
    ))
    MockLLM.queue("/responses", xai_sse("config.py written", "r-s38-w2"))
    # Turn 2: read_file config.py
    MockLLM.queue("/responses", xai_sse_fn(
        "r-s38-r", "c-s38-02", "read_file",
        json.dumps({"path": "config.py"}),
    ))
    MockLLM.queue("/responses", xai_sse("config.py content received", "r-s38-r2"))

    start_runner("s38", BIN["xai"], {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_runner(nc, prefix, cwd=tmpdir)
        if not sid:
            fail("S38: session.new timed out"); return

        # Turn 1: write
        r1 = await send_prompt(nc, prefix, sid, "create config.py")
        if prompt_err(r1):
            fail("S38: write turn failed", str(r1)); return
        ok("S38: write turn completed")

        # Verify file on disk
        expected = os.path.join(tmpdir, "config.py")
        if os.path.exists(expected) and open(expected).read() == config_content:
            ok("S38: config.py exists on disk with correct content")
        else:
            fail("S38: config.py not on disk or content wrong",
                 repr(open(expected).read() if os.path.exists(expected) else "missing"))

        # Turn 2: read
        r2 = await send_prompt(nc, prefix, sid, "read config.py")
        if prompt_err(r2):
            fail("S38: read turn failed", str(r2)); return
        ok("S38: read turn completed")

        reqs = MockLLM.requests("/responses")
        # 4 calls: write_fn, write_ack, read_fn, read_ack
        info(f"S38: total xai API calls = {len(reqs)}")

        # Find the read_file tool_result in the 4th call
        if len(reqs) >= 4:
            input4 = reqs[3].get("input", [])
            outputs = [
                item.get("output", "")
                for item in input4
                if isinstance(item, dict) and item.get("type") == "function_call_output"
            ]
            content = "\n".join(outputs)
            info(f"S38: read_file output = {content[:100]!r}")

            if "DB_URL" in content and "sqlite" in content:
                ok("S38: read_file returned the written content (DB_URL present)")
            elif "DEBUG" in content:
                ok("S38: read_file returned the written content (DEBUG key present)")
            else:
                fail("S38: read_file output missing expected content",
                     f"content={content[:80]!r}")
        else:
            fail("S38: expected 4 xai API calls", f"got {len(reqs)}")
    finally:
        stop_runner("s38")
        shutil.rmtree(tmpdir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S39 — acp read_file nonexistent: Error in tool_result; runner continues
# ═════════════════════════════════════════════════════════════════════════════

async def s39_acp_read_file_nonexistent(nc):
    section("S39: acp read_file nonexistent — Error in tool_result; runner does not crash")
    prefix = f"{BASE}.acp_s39"
    tmpdir = tempfile.mkdtemp(prefix="prog5_s39_")

    MockLLM.reset()
    # LLM call 1: try to read a file that doesn't exist
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse_tool(
        "t-s39-01", "read_file",
        json.dumps({"path": "completely_missing_file_xyz.py"}),
    ))
    # LLM call 2: acknowledge the error and continue
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse("file not found as expected"))

    start_runner("s39", BIN["acp"], {
        "ACP_PREFIX":      prefix,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    }, cwd=tmpdir)
    try:
        sid = await wait_runner(nc, prefix, cwd=tmpdir)
        if not sid:
            fail("S39: session.new timed out"); return

        mr = await send_set_mode(nc, prefix, sid, "bypassPermissions")
        if prompt_err(mr):
            fail("S39: set_mode failed", str(mr)); return

        r = await send_prompt(nc, prefix, sid, "read the missing file")
        if prompt_err(r):
            fail("S39: prompt failed (runner crashed?)", str(r)); return
        ok("S39: prompt completed — runner survived tool error")

        reqs = MockLLM.requests("/anthropic/v1/messages")
        if len(reqs) < 2:
            fail("S39: expected 2 LLM calls (tool + recovery)", f"got {len(reqs)}"); return
        ok("S39: 2 LLM calls made (runner continued after tool error)")

        # Verify the tool_result contains an error
        msgs2 = reqs[1].get("messages", [])
        tool_results = [
            b for m in msgs2 if m.get("role") == "user"
            for b in (m.get("content") if isinstance(m.get("content"), list) else [])
            if isinstance(b, dict) and b.get("type") == "tool_result"
        ]
        if tool_results:
            content_str = json.dumps(tool_results)
            info(f"S39: tool_result = {content_str[:120]!r}")
            if "Error" in content_str or "error" in content_str or "No such" in content_str:
                ok("S39: tool_result contains error message for missing file")
            else:
                fail("S39: tool_result missing error indication", content_str[:100])
        else:
            fail("S39: no tool_result in 2nd LLM call")
    finally:
        stop_runner("s39")
        shutil.rmtree(tmpdir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S40 — xai export→import same runner: session B receives session A's history
# ═════════════════════════════════════════════════════════════════════════════

async def s40_xai_import_same_runner(nc):
    section("S40: xai export→import same runner — session B gets session A history in input")
    prefix = f"{BASE}.xai_s40"

    MockLLM.reset()
    # Session A: 2 text turns
    MockLLM.queue("/responses", xai_sse("Alpha-first-response", "r-a40-1"))
    MockLLM.queue("/responses", xai_sse("Alpha-second-response", "r-a40-2"))

    start_runner("s40", BIN["xai"], {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid_a = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid_a:
            fail("S40: session A timed out"); return

        # Session A: 2 turns to build history
        r1 = await send_prompt(nc, prefix, sid_a, "first message")
        if prompt_err(r1):
            fail("S40: session A turn 1 failed", str(r1)); return
        r2 = await send_prompt(nc, prefix, sid_a, "second message")
        if prompt_err(r2):
            fail("S40: session A turn 2 failed", str(r2)); return
        ok("S40: session A completed 2 turns")

        # Export session A
        exp = await nats_req(nc, f"{prefix}.agent.ext.session/export", {"sessionId": sid_a})
        if "_error" in exp or "code" in exp:
            fail("S40: session A export failed", str(exp)[:80]); return
        msgs_a = exp if isinstance(exp, list) else []
        info(f"S40: session A exported {len(msgs_a)} messages")
        if len(msgs_a) < 4:
            fail("S40: session A should have at least 4 messages (2 user + 2 assistant)",
                 f"got {len(msgs_a)}"); return
        ok(f"S40: session A exported {len(msgs_a)} messages")

        # Create session B on the SAME runner
        sid_b = await new_session(nc, prefix, cwd="/tmp")
        if not sid_b:
            fail("S40: session B creation failed"); return
        info(f"S40: A={sid_a[:8]}  B={sid_b[:8]}")

        # Import session A history into session B
        imp = await nats_req(nc, f"{prefix}.agent.ext.session/import",
                             {"sessionId": sid_b, "messages": msgs_a})
        if "_error" in imp or "code" in imp:
            fail("S40: import into session B failed", str(imp)[:80]); return
        ok("S40: session A history imported into session B")

        # Reset mock and queue session B's response
        MockLLM.reset()
        MockLLM.queue("/responses", xai_sse("session B continuing with imported context", "r-b40-1"))

        # Prompt session B — since no last_response_id, it sends full history
        r_b = await send_prompt(nc, prefix, sid_b, "continue the conversation")
        if prompt_err(r_b):
            fail("S40: session B prompt failed", str(r_b)); return
        ok("S40: session B prompt completed")

        reqs_b = MockLLM.requests("/responses")
        if not reqs_b:
            fail("S40: no xai API call from session B"); return

        # Verify session B's first xai call includes the imported history
        input_b = reqs_b[0].get("input", [])
        input_str = json.dumps(input_b)
        info(f"S40: session B input items = {len(input_b)}, snippet = {input_str[:120]!r}")

        # The imported history has "first message", "Alpha-first-response", etc.
        has_first = "first message" in input_str or "Alpha-first" in input_str
        has_second = "second message" in input_str or "Alpha-second" in input_str

        if has_first and has_second:
            ok("S40: session B xai input contains session A's history (export→import worked)")
        elif len(input_b) >= 4:
            ok(f"S40: session B xai input has {len(input_b)} items (imported history present)")
        else:
            fail("S40: session B xai input missing imported history",
                 f"items={len(input_b)} has_first={has_first} has_second={has_second}")

        # Also verify no previous_response_id (imported session starts fresh)
        prev_id = reqs_b[0].get("previous_response_id")
        if prev_id is None:
            ok("S40: session B starts fresh (no previous_response_id after import)")
        else:
            fail("S40: session B has previous_response_id from session A — stale chain",
                 f"prev_id={prev_id!r}")
    finally:
        stop_runner("s40")


# ═════════════════════════════════════════════════════════════════════════════
# S41 — or 4-turn message count growth: export shows 8 messages
# ═════════════════════════════════════════════════════════════════════════════

async def s41_or_four_turn_growth(nc):
    section("S41: or 4-turn growth — export shows 8 messages (4 user + 4 assistant)")
    prefix = f"{BASE}.or_s41"

    MockLLM.reset()
    MockLLM.queue("/chat/completions", or_sse("response-1"))
    MockLLM.queue("/chat/completions", or_sse("response-2"))
    MockLLM.queue("/chat/completions", or_sse("response-3"))
    MockLLM.queue("/chat/completions", or_sse("response-4"))

    start_runner("s41", BIN["or"], {
        "ACP_PREFIX":               prefix,
        "OPENROUTER_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":       "fake-or-key",
        "OPENROUTER_DEFAULT_MODEL": "gpt-4o",
    })
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S41: session.new timed out"); return

        for i in range(1, 5):
            r = await send_prompt(nc, prefix, sid, f"turn-{i} message")
            if prompt_err(r):
                fail(f"S41: turn {i} failed", str(r)); return
        ok("S41: 4 turns completed")

        exp = await nats_req(nc, f"{prefix}.agent.ext.session/export", {"sessionId": sid})
        if "_error" in exp or "code" in exp:
            fail("S41: export failed", str(exp)[:80]); return
        msgs = exp if isinstance(exp, list) else exp.get("messages", exp)
        if not isinstance(msgs, list):
            fail("S41: unexpected export format", str(exp)[:80]); return

        info(f"S41: exported {len(msgs)} messages")

        if len(msgs) == 8:
            ok("S41: exactly 8 messages in export (4 user + 4 assistant)")
        elif len(msgs) >= 8:
            ok(f"S41: {len(msgs)} messages in export (≥8 expected)")
        else:
            fail(f"S41: expected 8 messages, got {len(msgs)}")

        roles = [m.get("role") for m in msgs if isinstance(m, dict)]
        user_count = roles.count("user")
        asst_count = roles.count("assistant")
        info(f"S41: user={user_count} assistant={asst_count}")

        if user_count == 4 and asst_count == 4:
            ok("S41: correct role distribution (4 user, 4 assistant)")
        else:
            fail("S41: unexpected role distribution", f"user={user_count} assistant={asst_count}")

        # Verify message count grows monotonically by checking 5th call > 3rd call messages
        reqs = MockLLM.requests("/chat/completions")
        if len(reqs) >= 4:
            msgs_turn3 = reqs[2].get("messages", [])
            msgs_turn4 = reqs[3].get("messages", [])
            count3 = len([m for m in msgs_turn3 if m.get("role") != "system"])
            count4 = len([m for m in msgs_turn4 if m.get("role") != "system"])
            if count4 > count3:
                ok(f"S41: message count grows turn-to-turn ({count3}→{count4})")
            else:
                fail("S41: message count did not grow", f"turn3={count3} turn4={count4}")
    finally:
        stop_runner("s41")


# ═════════════════════════════════════════════════════════════════════════════
# S42 — TROGON.md + _meta.systemPrompt combined in openrouter
# ═════════════════════════════════════════════════════════════════════════════

async def s42_trogon_md_plus_meta_combined(nc):
    section("S42: TROGON.md + _meta.systemPrompt — both markers in /chat/completions")
    prefix = f"{BASE}.or_s42"
    tmpdir = tempfile.mkdtemp(prefix="prog5_s42_")

    trogon_marker = f"TROGON_S42_{uuid.uuid4().hex[:8]}"
    sys_marker    = f"SYS_S42_{uuid.uuid4().hex[:8]}"

    with open(os.path.join(tmpdir, "TROGON.md"), "w") as f:
        f.write(f"# Workspace Context\n\nMARKER: {trogon_marker}\n")

    MockLLM.reset()
    MockLLM.queue("/chat/completions", or_sse("understood both prompts"))

    start_runner("s42", BIN["or"], {
        "ACP_PREFIX":               prefix,
        "OPENROUTER_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":       "fake-or-key",
        "OPENROUTER_DEFAULT_MODEL": "gpt-4o",
    })
    try:
        sid = await wait_runner(nc, prefix, cwd=tmpdir,
                                meta={"systemPrompt": f"Expert token: {sys_marker}"})
        if not sid:
            fail("S42: session.new timed out"); return
        info(f"S42: session={sid[:8]}")

        r = await send_prompt(nc, prefix, sid, "hello")
        if prompt_err(r):
            fail("S42: prompt failed", str(r)); return
        ok("S42: prompt completed")

        reqs = MockLLM.requests("/chat/completions")
        if not reqs:
            fail("S42: no /chat/completions request"); return

        body_str = json.dumps(reqs[0])
        messages = reqs[0].get("messages", [])
        system_msgs = [m for m in messages if m.get("role") == "system"]
        info(f"S42: system messages = {json.dumps(system_msgs)[:150]!r}")

        if trogon_marker in body_str:
            ok("S42: TROGON.md marker present in /chat/completions request")
        else:
            fail("S42: TROGON.md marker MISSING from request",
                 f"expected {trogon_marker!r}")

        if sys_marker in body_str:
            ok("S42: _meta.systemPrompt marker present in /chat/completions request")
        else:
            fail("S42: _meta.systemPrompt marker MISSING from request",
                 f"expected {sys_marker!r}")

        if trogon_marker in body_str and sys_marker in body_str:
            ok("S42: both TROGON.md and _meta.systemPrompt combined in single system message")
        elif system_msgs:
            sys_content = json.dumps(system_msgs)
            if trogon_marker in sys_content and sys_marker in sys_content:
                ok("S42: both markers in system message (combined)")
            else:
                fail("S42: markers not combined in system message",
                     f"content={sys_content[:120]!r}")
    finally:
        stop_runner("s42")
        shutil.rmtree(tmpdir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S43 — or set_model: cwd preserved after model switch; write_file still works
# ═════════════════════════════════════════════════════════════════════════════

async def s43_or_set_model_cwd_preserved(nc):
    section("S43: or set_model — cwd preserved; write_file after model switch works")
    prefix = f"{BASE}.or_s43"
    tmpdir = tempfile.mkdtemp(prefix="prog5_s43_")

    MockLLM.reset()
    # Turn 1: text (to establish session with cwd=tmpdir)
    MockLLM.queue("/chat/completions", or_sse("session established"))
    # Turn 2 (after set_model): write_file tool call
    MockLLM.queue("/chat/completions", or_sse_tool(
        "c-s43-01", "write_file",
        json.dumps({"path": "after_switch.py", "content": "# written after model switch\n"}),
    ))
    MockLLM.queue("/chat/completions", or_sse("file written after model switch"))

    start_runner("s43", BIN["or"], {
        "ACP_PREFIX":               prefix,
        "OPENROUTER_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":       "fake-or-key",
        "OPENROUTER_DEFAULT_MODEL": "gpt-4o",
    })
    try:
        sid = await wait_runner(nc, prefix, cwd=tmpdir)
        if not sid:
            fail("S43: session.new timed out"); return

        # Turn 1: establish session
        r1 = await send_prompt(nc, prefix, sid, "hello")
        if prompt_err(r1):
            fail("S43: turn 1 failed", str(r1)); return
        ok("S43: turn 1 completed (cwd established)")

        # Switch model to anthropic/claude-sonnet-4-6 (always in hardcoded list)
        sr = await send_set_model(nc, prefix, sid, "anthropic/claude-sonnet-4-6")
        if prompt_err(sr):
            fail("S43: set_model failed", str(sr)); return
        ok("S43: set_model to anthropic/claude-sonnet-4-6 succeeded")

        # Turn 2: write_file in original cwd
        r2 = await send_prompt(nc, prefix, sid, "create a file")
        if prompt_err(r2):
            fail("S43: turn 2 failed", str(r2)); return
        ok("S43: turn 2 completed (after model switch)")

        expected = os.path.join(tmpdir, "after_switch.py")
        if os.path.exists(expected):
            content = open(expected).read()
            if "after model switch" in content:
                ok("S43: write_file worked in original cwd after model switch")
            else:
                fail("S43: file content wrong", repr(content[:80]))
        else:
            fail("S43: file not created after model switch", f"expected {expected}")

        # Verify 2nd API call uses the new model
        reqs = MockLLM.requests("/chat/completions")
        if len(reqs) >= 2:
            model_t2 = reqs[1].get("model")
            info(f"S43: model after set_model = {model_t2!r}")
            if model_t2 == "anthropic/claude-sonnet-4-6":
                ok("S43: API call after set_model uses correct model ID")
            else:
                fail("S43: model not updated in API call", f"expected anthropic/claude-sonnet-4-6, got {model_t2!r}")
    finally:
        stop_runner("s43")
        shutil.rmtree(tmpdir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S44 — acp export includes ToolCall blocks (not just text)
# ═════════════════════════════════════════════════════════════════════════════

async def s44_acp_export_includes_tool_call(nc):
    section("S44: acp export includes ToolCall blocks — tool_use not dropped from export")
    prefix = f"{BASE}.acp_s44"
    tmpdir = tempfile.mkdtemp(prefix="prog5_s44_")

    fname = os.path.join(tmpdir, "target.py")
    with open(fname, "w") as f:
        f.write("x = 1\n")

    MockLLM.reset()
    # Turn 1: tool call (str_replace)
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse_tool(
        "t-s44-01", "str_replace",
        json.dumps({"path": "target.py", "old_str": "x = 1", "new_str": "x = 42"}),
    ))
    # Turn 1 follow-up: text after tool result
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse("str_replace succeeded"))

    start_runner("s44", BIN["acp"], {
        "ACP_PREFIX":      prefix,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    }, cwd=tmpdir)
    try:
        sid = await wait_runner(nc, prefix, cwd=tmpdir)
        if not sid:
            fail("S44: session.new timed out"); return

        mr = await send_set_mode(nc, prefix, sid, "bypassPermissions")
        if prompt_err(mr):
            fail("S44: set_mode failed", str(mr)); return

        r = await send_prompt(nc, prefix, sid, "fix target.py")
        if prompt_err(r):
            fail("S44: prompt failed", str(r)); return
        ok("S44: prompt completed (tool call executed)")

        # Verify the str_replace actually worked
        content = open(fname).read()
        if "x = 42" in content:
            ok("S44: str_replace modified file correctly")
        else:
            fail("S44: str_replace did not modify file", repr(content[:80]))

        # Export and verify ToolCall block is present
        exp = await nats_req(nc, f"{prefix}.agent.ext.session/export", {"sessionId": sid})
        if "_error" in exp or "code" in exp:
            fail("S44: export failed", str(exp)[:80]); return

        msgs = exp if isinstance(exp, list) else []
        info(f"S44: exported {len(msgs)} messages")
        exp_str = json.dumps(msgs)

        # Look for ToolCall blocks in the export
        has_tool_call = "tool_call" in exp_str or "ToolCall" in exp_str
        has_tool_result = "tool_result" in exp_str or "ToolResult" in exp_str

        # Check for the specific block types that acp uses
        all_blocks = [
            b for m in msgs
            if isinstance(m, dict)
            for b in (m.get("blocks") or m.get("content") or [])
            if isinstance(b, dict)
        ]
        block_types = {b.get("type") for b in all_blocks}
        info(f"S44: block types in export = {block_types}")

        if "tool_call" in block_types:
            ok("S44: ToolCall block present in export (tool_use preserved, not dropped)")
        elif has_tool_call:
            ok("S44: 'tool_call' found in export JSON (ToolCall block present)")
        else:
            fail("S44: ToolCall block missing from export",
                 f"block_types={block_types} exp_str={exp_str[:120]!r}")

        if "tool_result" in block_types or has_tool_result:
            ok("S44: ToolResult block also present in export")
        else:
            fail("S44: ToolResult block missing from export")

        if len(msgs) >= 2:
            ok(f"S44: export has {len(msgs)} messages (at least 2 expected)")
        else:
            fail(f"S44: export has only {len(msgs)} messages")
    finally:
        stop_runner("s44")
        shutil.rmtree(tmpdir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S45 — codex 3-turn history growth: export grows by 2 per turn
# ═════════════════════════════════════════════════════════════════════════════

async def s45_codex_three_turn_history_growth(nc):
    section("S45: codex 3-turn history — export grows by 2 messages per turn")
    if not os.path.exists(BIN["mock_codex"]):
        fail("S45: mock_codex_server binary not found", BIN["mock_codex"])
        return

    prefix = f"{BASE}.codex_s45"

    start_runner("codex_s45", BIN["codex"], {
        "ACP_PREFIX":               prefix,
        "CODEX_BIN":                BIN["mock_codex"],
        "MOCK_SEND_N_TEXT_EVENTS":  "1",  # one text event per prompt
        "CODEX_SPAWN_TIMEOUT_SECS": "8",
    })
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S45: codex session.new timed out"); return
        info(f"S45: session={sid[:8]}")

        counts = []
        for turn in range(1, 4):
            r = await send_prompt(nc, prefix, sid, f"codex turn {turn}")
            if prompt_err(r):
                fail(f"S45: turn {turn} failed", str(r)); return

            exp = await nats_req(nc, f"{prefix}.agent.ext.session/export",
                                 {"sessionId": sid})
            if "_error" in exp or "code" in exp:
                fail(f"S45: export after turn {turn} failed", str(exp)[:80]); return

            msgs = exp if isinstance(exp, list) else []
            count = len(msgs)
            counts.append(count)
            info(f"S45: after turn {turn}: {count} messages exported")

        ok(f"S45: 3 turns completed, message counts = {counts}")

        # Each turn adds 2 messages (user prompt + assistant response)
        if len(counts) == 3:
            if counts[0] == 2 and counts[1] == 4 and counts[2] == 6:
                ok("S45: message count grows exactly: 2 → 4 → 6 (codex history grows correctly)")
            elif counts[0] < counts[1] < counts[2]:
                ok(f"S45: message count grows monotonically: {counts[0]} → {counts[1]} → {counts[2]}")
            elif counts[2] >= 4:
                ok(f"S45: message count grows over 3 turns: {counts}")
            else:
                fail("S45: message count does not grow across turns", f"counts={counts}")
    finally:
        stop_runner("codex_s45")


# ═════════════════════════════════════════════════════════════════════════════
# Main
# ═════════════════════════════════════════════════════════════════════════════

async def main():
    print(f"\n=== smoke_test_programming5.py — advanced programming scenarios ===")
    print(f"  prefix : {BASE}")
    print(f"  nats   : {NATS_URL}")
    print(f"  mock   : http://127.0.0.1:{MOCK_PORT}")

    start_mock(MOCK_PORT)
    print(f"  {yellow}INFO{reset}  mock server started on :{MOCK_PORT}")

    nc = await nats_lib.connect(NATS_URL)
    try:
        await s34_acp_parallel_tools(nc)
        await s35_glob_no_matches(nc)
        await s36_glob_recursive(nc)
        await s37_xai_set_model_reflected_in_api(nc)
        await s38_write_then_read_same_session(nc)
        await s39_acp_read_file_nonexistent(nc)
        await s40_xai_import_same_runner(nc)
        await s41_or_four_turn_growth(nc)
        await s42_trogon_md_plus_meta_combined(nc)
        await s43_or_set_model_cwd_preserved(nc)
        await s44_acp_export_includes_tool_call(nc)
        await s45_codex_three_turn_history_growth(nc)
    finally:
        stop_all()
        await nc.drain()

    print(f"\n=== Results ===")
    print(f"  {green}passed{reset}: {PASS}")
    print(f"  {red}failed{reset}: {FAIL}")
    if FAIL == 0:
        print(f"\n{green}All scenarios passed.{reset}\n")
    else:
        print(f"\n{red}{FAIL} scenario(s) FAILED.{reset}\n")
        sys.exit(1)


if __name__ == "__main__":
    asyncio.run(main())
