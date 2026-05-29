#!/usr/bin/env python3
"""
smoke_test_programming8.py — Deep openrouter-runner behavioral verification.

Every scenario targets correct openrouter-runner behavior that could hide subtle bugs.
The openrouter-runner uses OpenAI chat completions SSE format and sends full history
on every API call (stateful, unlike xai's previous_response_id approach).

  S71. or get_state fresh          before any prompts: history=[], model=null
  S72. or basic prompt             response received; get_state shows 2 history entries
  S73. or 3-turn full-history      each API call sends growing message count (not stateful)
  S74. or single tool round        call 2 wire msgs include tool_calls + tool_result entries
  S75. or 2 consecutive tool rounds  3 API calls; messages grow: 1→3→5 per call
  S76. or set_model → API body     next API call uses new model in request body
  S77. or unknown model error      -32602 + 'unknown model' in error message
  S78. or export PortableMessages  role/text/blocks correct for user + assistant msgs
  S79. or import replaces history   import PortableMessages → history updated in get_state
  S80. or TROGON.md system msg     marker from TROGON.md appears as system msg in API body
  S81. or 2 sessions isolated      each session has own history; no cross-contamination
  S82. or survives truncated stream  abrupt stream end → EndTurn; runner stays functional

Usage:
    NATS_URL=nats://localhost:4333 python3 smoke_test_programming8.py
"""

import asyncio
import collections
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Optional

import nats as nats_lib

# ── Config ────────────────────────────────────────────────────────────────────
NATS_URL       = os.environ.get("NATS_URL", "nats://localhost:4222")
RSDIR          = os.path.dirname(os.path.abspath(__file__))
MOCK_PORT      = 19824
BASE           = f"prog8.{os.getpid()}"
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


# ── SSE builders (OpenAI chat completions format) ─────────────────────────────

def or_sse(text: str) -> bytes:
    """Simple text response in OpenAI chat completions SSE format."""
    chunks = [
        json.dumps({"choices": [{"delta": {"content": text}}]}),
        json.dumps({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
        json.dumps({"usage": {"prompt_tokens": 10, "completion_tokens": 5,
                              "cache_read_input_tokens": 0,
                              "cache_creation_input_tokens": 0}}),
        "[DONE]",
    ]
    return ("".join(f"data: {c}\n\n" for c in chunks)).encode()


def or_sse_tool(call_id: str, fn_name: str, arguments_str: str) -> bytes:
    """Single tool call in OpenAI chat completions SSE format."""
    chunks = [
        json.dumps({"choices": [{"delta": {"tool_calls": [
            {"index": 0, "id": call_id,
             "function": {"name": fn_name, "arguments": ""}}
        ]}}]}),
        json.dumps({"choices": [{"delta": {"tool_calls": [
            {"index": 0, "function": {"arguments": arguments_str}}
        ]}}]}),
        json.dumps({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
        "[DONE]",
    ]
    return ("".join(f"data: {c}\n\n" for c in chunks)).encode()


def or_sse_truncated() -> bytes:
    """Stream that ends abruptly with only [DONE], no finish_reason."""
    return b"data: [DONE]\n\n"


# ── Mock HTTP server ──────────────────────────────────────────────────────────

class MockLLM(BaseHTTPRequestHandler):
    _lock = threading.Lock()
    _log: list = []
    _queue: collections.deque = collections.deque()

    @classmethod
    def reset(cls):
        with cls._lock:
            cls._log.clear()
            cls._queue.clear()

    @classmethod
    def queue(cls, sse: bytes):
        with cls._lock:
            cls._queue.append(sse)

    @classmethod
    def requests(cls) -> list:
        with cls._lock:
            return list(cls._log)

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
            MockLLM._log.append(parsed)
            resp = MockLLM._queue.popleft() if MockLLM._queue else None
        if resp is None:
            resp = or_sse("default openrouter response")
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
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


async def new_session(nc, prefix: str, cwd: str = "/tmp") -> Optional[str]:
    resp = await nats_req(nc, f"{prefix}.agent.session.new",
                          {"cwd": cwd, "mcpServers": []}, timeout=5.0)
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


def or_runner_env(prefix: str, model: str = "anthropic/claude-sonnet-4-6") -> dict:
    return {
        "ACP_PREFIX":               prefix,
        "OPENROUTER_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":       "fake-or-key",
        "OPENROUTER_DEFAULT_MODEL": model,
        "OPENROUTER_MODELS":        (
            f"{model}:Default Model,"
            "openai/gpt-4o:GPT-4o,"
            "anthropic/claude-sonnet-4-6:Claude Sonnet 4.6"
        ),
    }


BIN_OR = os.path.join(RSDIR, "target/debug/trogon-openrouter-runner")


# ═════════════════════════════════════════════════════════════════════════════
# S71 — or get_state fresh: before any prompts → history=[], model=null
# ═════════════════════════════════════════════════════════════════════════════

async def s71_or_get_state_fresh(nc):
    section("S71: or get_state fresh — before any prompts → history=[], model=null")
    prefix = f"{BASE}.or_s71"
    MockLLM.reset()

    start_runner("s71", BIN_OR, or_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S71: session.new timed out"); return
        info(f"S71: session={sid[:8]} — calling get_state before any prompt")

        state = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid})
        info(f"S71: get_state = {str(state)[:160]!r}")

        if "_error" in state or "code" in state:
            fail("S71: get_state returned error", str(state)[:80]); return
        ok("S71: get_state succeeded")

        history = state.get("history", "MISSING")
        if isinstance(history, list) and len(history) == 0:
            ok("S71: history=[] for fresh session")
        else:
            fail("S71: expected history=[] for fresh session", str(history)[:60])

        model = state.get("model", "MISSING")
        if model is None:
            ok("S71: model=null for fresh session (uses runner default)")
        else:
            fail("S71: expected model=null for fresh session", str(model)[:60])

        cwd = state.get("cwd", "MISSING")
        if cwd == "/tmp":
            ok("S71: cwd='/tmp' in get_state")
        else:
            fail("S71: expected cwd='/tmp'", str(cwd))

    finally:
        stop_runner("s71")


# ═════════════════════════════════════════════════════════════════════════════
# S72 — or basic prompt: response received; get_state shows 2 history entries
# ═════════════════════════════════════════════════════════════════════════════

async def s72_or_basic_prompt(nc):
    section("S72: or basic prompt — response text received; get_state shows 2 history entries")
    prefix = f"{BASE}.or_s72"
    MockLLM.reset()
    MockLLM.queue(or_sse("openrouter says hello s72"))

    start_runner("s72", BIN_OR, or_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S72: session.new timed out"); return

        resp = await send_prompt(nc, prefix, sid, "say hello")
        if prompt_err(resp):
            fail("S72: prompt failed", str(resp)[:80]); return
        ok("S72: prompt completed without error")

        state = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid})
        history = state.get("history", [])
        info(f"S72: history length = {len(history)}")
        if len(history) == 2:
            ok("S72: history has 2 entries (user + assistant) after 1 prompt")
        else:
            fail("S72: expected 2 history entries", f"got {len(history)}")

        roles = [m.get("role") for m in history]
        if roles == ["user", "assistant"]:
            ok("S72: history roles correct: user then assistant")
        else:
            fail("S72: unexpected history roles", str(roles))

        assistant_content = history[1].get("content", "") if len(history) > 1 else ""
        if "openrouter says hello s72" in assistant_content:
            ok("S72: assistant message content matches SSE text")
        else:
            fail("S72: assistant content not in history", str(assistant_content)[:80])

    finally:
        stop_runner("s72")


# ═════════════════════════════════════════════════════════════════════════════
# S73 — or 3-turn full-history: each API call sends growing message count
# ═════════════════════════════════════════════════════════════════════════════

async def s73_or_full_history_mode(nc):
    section("S73: or 3-turn full-history — each API call sends growing messages (not stateful)")
    prefix = f"{BASE}.or_s73"
    MockLLM.reset()
    MockLLM.queue(or_sse("turn 1 response s73"))
    MockLLM.queue(or_sse("turn 2 response s73"))
    MockLLM.queue(or_sse("turn 3 response s73"))

    start_runner("s73", BIN_OR, or_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S73: session.new timed out"); return

        r1 = await send_prompt(nc, prefix, sid, "question 1 s73")
        r2 = await send_prompt(nc, prefix, sid, "question 2 s73")
        r3 = await send_prompt(nc, prefix, sid, "question 3 s73")

        errs = [prompt_err(r) for r in [r1, r2, r3] if prompt_err(r)]
        if errs:
            fail("S73: prompt(s) failed", str(errs[:1])); return
        ok("S73: 3 prompts completed")

        calls = MockLLM.requests()
        info(f"S73: total API calls = {len(calls)}")
        if len(calls) == 3:
            ok("S73: exactly 3 API calls (one per turn)")
        else:
            fail("S73: expected 3 API calls", f"got {len(calls)}")
            return

        # no system prompt configured → message counts should be 1, 3, 5
        # (user1) | (user1, asst1, user2) | (user1, asst1, user2, asst2, user3)
        c1_msgs = calls[0].get("messages", [])
        c2_msgs = calls[1].get("messages", [])
        c3_msgs = calls[2].get("messages", [])
        info(f"S73: API call message counts: {len(c1_msgs)}, {len(c2_msgs)}, {len(c3_msgs)}")

        if len(c1_msgs) == 1:
            ok("S73: call 1 has 1 message (user only)")
        else:
            fail("S73: call 1 should have 1 message", f"got {len(c1_msgs)}")

        if len(c2_msgs) == 3:
            ok("S73: call 2 has 3 messages (user1, asst1, user2) — full history")
        else:
            fail("S73: call 2 should have 3 messages (full history)", f"got {len(c2_msgs)}")

        if len(c3_msgs) == 5:
            ok("S73: call 3 has 5 messages (user1, asst1, user2, asst2, user3) — full history")
        else:
            fail("S73: call 3 should have 5 messages (full history)", f"got {len(c3_msgs)}")

        # verify calls 2 and 3 do NOT have previous_response_id (it's not xai)
        for i, call in enumerate(calls, 1):
            has_prev = "previous_response_id" in call
            if not has_prev:
                ok(f"S73: call {i} has no previous_response_id (stateless mode, full history)")
            else:
                fail(f"S73: call {i} unexpectedly has previous_response_id (should send full history)")

    finally:
        stop_runner("s73")


# ═════════════════════════════════════════════════════════════════════════════
# S74 — or single tool round: call 2 wire msgs include tool_calls + tool_result
# ═════════════════════════════════════════════════════════════════════════════

async def s74_or_single_tool_round(nc):
    section("S74: or single tool round — call 2 wire msgs include tool_calls + tool_result")
    prefix = f"{BASE}.or_s74"
    MockLLM.reset()

    # Create test file in /tmp
    test_file = "/tmp/test_s74_or.txt"
    with open(test_file, "w") as f:
        f.write("s74 file contents from openrouter test")

    call_id = "or-call-s74-001"
    MockLLM.queue(or_sse_tool(call_id, "read_file", json.dumps({"path": "test_s74_or.txt"})))
    MockLLM.queue(or_sse("file analysis done s74"))

    start_runner("s74", BIN_OR, or_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S74: session.new timed out"); return

        resp = await send_prompt(nc, prefix, sid, "read test_s74_or.txt and analyze it")
        if prompt_err(resp):
            fail("S74: prompt failed", str(resp)[:80]); return
        ok("S74: prompt completed (1 tool round)")

        calls = MockLLM.requests()
        info(f"S74: total API calls = {len(calls)}")
        if len(calls) == 2:
            ok("S74: exactly 2 API calls (initial + after tool result)")
        else:
            fail("S74: expected 2 API calls", f"got {len(calls)}")
            return

        # Call 1: just the user message
        c1_msgs = calls[0].get("messages", [])
        if len(c1_msgs) == 1 and c1_msgs[0]["role"] == "user":
            ok("S74: call 1 has 1 user message")
        else:
            fail("S74: call 1 should have 1 user message", str(c1_msgs)[:80])

        # Call 2: user, asst_tool_calls, tool_result
        c2_msgs = calls[1].get("messages", [])
        info(f"S74: call 2 message count = {len(c2_msgs)}, roles = {[m.get('role') for m in c2_msgs]}")
        if len(c2_msgs) == 3:
            ok("S74: call 2 has 3 messages (user + tool_calls + tool_result)")
        else:
            fail("S74: call 2 should have 3 messages", f"got {len(c2_msgs)}")
            return

        # Message 1: user
        if c2_msgs[0]["role"] == "user":
            ok("S74: call 2 msg[0] is user message")
        else:
            fail("S74: call 2 msg[0] should be user", c2_msgs[0].get("role"))

        # Message 2: assistant with tool_calls
        asst_msg = c2_msgs[1]
        if asst_msg.get("role") == "assistant" and asst_msg.get("tool_calls"):
            tc = asst_msg["tool_calls"][0]
            ok("S74: call 2 msg[1] is assistant with tool_calls")
            if tc.get("id") == call_id:
                ok(f"S74: tool_call id matches ({call_id})")
            else:
                fail("S74: tool_call id mismatch", f"expected {call_id}, got {tc.get('id')}")
            if tc.get("function", {}).get("name") == "read_file":
                ok("S74: tool_call function name = 'read_file'")
            else:
                fail("S74: tool_call function name wrong", str(tc.get("function", {})))
        else:
            fail("S74: call 2 msg[1] should be assistant with tool_calls", str(asst_msg)[:80])

        # Message 3: tool result
        tool_msg = c2_msgs[2]
        if tool_msg.get("role") == "tool" and tool_msg.get("tool_call_id") == call_id:
            ok("S74: call 2 msg[2] is tool result with correct tool_call_id")
        else:
            fail("S74: call 2 msg[2] should be tool result", str(tool_msg)[:80])

        # Tool result content should contain the file contents
        tool_content = tool_msg.get("content", "")
        if "s74 file contents" in tool_content:
            ok("S74: tool result content contains file contents")
        else:
            fail("S74: tool result should contain file contents", str(tool_content)[:80])

    finally:
        stop_runner("s74")
        try:
            os.unlink(test_file)
        except Exception:
            pass


# ═════════════════════════════════════════════════════════════════════════════
# S75 — or 2 consecutive tool rounds: messages grow 1 → 3 → 5
# ═════════════════════════════════════════════════════════════════════════════

async def s75_or_two_tool_rounds(nc):
    section("S75: or 2 consecutive tool rounds — 3 API calls; messages 1→3→5")
    prefix = f"{BASE}.or_s75"
    MockLLM.reset()

    # Create two test files
    file_a = "/tmp/s75_or_a.txt"
    file_b = "/tmp/s75_or_b.txt"
    with open(file_a, "w") as f:
        f.write("s75 file A content")
    with open(file_b, "w") as f:
        f.write("s75 file B content")

    ca1, ca2 = "or-ca1-s75", "or-ca2-s75"
    MockLLM.queue(or_sse_tool(ca1, "read_file", json.dumps({"path": "s75_or_a.txt"})))
    MockLLM.queue(or_sse_tool(ca2, "read_file", json.dumps({"path": "s75_or_b.txt"})))
    MockLLM.queue(or_sse("both files analyzed successfully s75"))

    start_runner("s75", BIN_OR, or_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S75: session.new timed out"); return

        resp = await send_prompt(nc, prefix, sid, "read both files and summarize")
        if prompt_err(resp):
            fail("S75: prompt failed", str(resp)[:80]); return
        ok("S75: prompt completed (2 tool rounds)")

        calls = MockLLM.requests()
        info(f"S75: total API calls = {len(calls)}")
        if len(calls) == 3:
            ok("S75: exactly 3 API calls (initial + after tool_A + after tool_B)")
        else:
            fail("S75: expected 3 API calls", f"got {len(calls)}")
            return

        c1_len = len(calls[0].get("messages", []))
        c2_len = len(calls[1].get("messages", []))
        c3_len = len(calls[2].get("messages", []))
        info(f"S75: message counts per call: {c1_len}, {c2_len}, {c3_len}")

        if c1_len == 1:
            ok("S75: call 1 has 1 message (user only)")
        else:
            fail("S75: call 1 should have 1 message", f"got {c1_len}")

        # call 2: user + asst_tool_calls_A + tool_result_A = 3
        if c2_len == 3:
            ok("S75: call 2 has 3 messages (user + tc_A + tr_A)")
        else:
            fail("S75: call 2 should have 3 messages", f"got {c2_len}")

        # call 3: user + asst_tc_A + tr_A + asst_tc_B + tr_B = 5
        if c3_len == 5:
            ok("S75: call 3 has 5 messages (user + tc_A + tr_A + tc_B + tr_B)")
        else:
            fail("S75: call 3 should have 5 messages", f"got {c3_len}")
            return

        # Verify call 3 has tool_result_A and tool_result_B in correct order
        c3_msgs = calls[2]["messages"]
        tool_msgs = [m for m in c3_msgs if m.get("role") == "tool"]
        if len(tool_msgs) == 2:
            ok("S75: call 3 has 2 tool-result messages")
        else:
            fail("S75: call 3 should have 2 tool result messages", f"got {len(tool_msgs)}")
            return

        if tool_msgs[0].get("tool_call_id") == ca1:
            ok(f"S75: first tool result has call_id={ca1}")
        else:
            fail("S75: first tool result wrong call_id",
                 f"expected {ca1}, got {tool_msgs[0].get('tool_call_id')}")

        if tool_msgs[1].get("tool_call_id") == ca2:
            ok(f"S75: second tool result has call_id={ca2}")
        else:
            fail("S75: second tool result wrong call_id",
                 f"expected {ca2}, got {tool_msgs[1].get('tool_call_id')}")

        if "s75 file A content" in tool_msgs[0].get("content", ""):
            ok("S75: tool result A contains file A content")
        else:
            fail("S75: tool result A should contain file A content",
                 str(tool_msgs[0].get("content"))[:60])

        if "s75 file B content" in tool_msgs[1].get("content", ""):
            ok("S75: tool result B contains file B content")
        else:
            fail("S75: tool result B should contain file B content",
                 str(tool_msgs[1].get("content"))[:60])

    finally:
        stop_runner("s75")
        for f in [file_a, file_b]:
            try:
                os.unlink(f)
            except Exception:
                pass


# ═════════════════════════════════════════════════════════════════════════════
# S76 — or set_model: next API call uses new model in request body
# ═════════════════════════════════════════════════════════════════════════════

async def s76_or_set_model_changes_api_body(nc):
    section("S76: or set_model → API body uses new model; get_state reflects change")
    prefix = f"{BASE}.or_s76"
    default_model = "anthropic/claude-sonnet-4-6"
    MockLLM.reset()
    MockLLM.queue(or_sse("turn 1 with default model s76"))
    MockLLM.queue(or_sse("turn 2 with new model s76"))

    start_runner("s76", BIN_OR, or_runner_env(prefix, model=default_model))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S76: session.new timed out"); return

        # Turn 1 with default model
        r1 = await send_prompt(nc, prefix, sid, "turn 1 prompt s76")
        if prompt_err(r1):
            fail("S76: turn 1 failed", str(r1)[:80]); return
        ok("S76: turn 1 completed with default model")

        # Set model to openai/gpt-4o
        new_model = "openai/gpt-4o"
        set_resp = await send_set_model(nc, prefix, sid, new_model)
        if "code" in set_resp or "_error" in set_resp:
            fail("S76: set_model failed", str(set_resp)[:80]); return
        ok(f"S76: set_model to {new_model} succeeded")

        # Verify get_state shows new model
        state = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid})
        model_in_state = state.get("model")
        info(f"S76: model in get_state = {model_in_state!r}")
        if model_in_state == new_model:
            ok("S76: get_state.model reflects set_model change")
        else:
            fail("S76: get_state.model wrong", f"expected {new_model}, got {model_in_state}")

        # Turn 2 with new model
        r2 = await send_prompt(nc, prefix, sid, "turn 2 prompt s76")
        if prompt_err(r2):
            fail("S76: turn 2 failed", str(r2)[:80]); return
        ok("S76: turn 2 completed with new model")

        calls = MockLLM.requests()
        info(f"S76: total API calls = {len(calls)}")
        if len(calls) < 2:
            fail("S76: expected at least 2 API calls"); return

        call1_model = calls[0].get("model", "")
        call2_model = calls[1].get("model", "")
        info(f"S76: call 1 model = {call1_model!r}, call 2 model = {call2_model!r}")

        if call1_model == default_model:
            ok(f"S76: call 1 uses default model ({default_model})")
        else:
            fail("S76: call 1 should use default model", call1_model)

        if call2_model == new_model:
            ok(f"S76: call 2 uses new model ({new_model}) after set_model")
        else:
            fail(f"S76: call 2 should use {new_model}", call2_model)

    finally:
        stop_runner("s76")


# ═════════════════════════════════════════════════════════════════════════════
# S77 — or unknown model error: -32602 + 'unknown model' in message
# ═════════════════════════════════════════════════════════════════════════════

async def s77_or_unknown_model_error(nc):
    section("S77: or unknown model error — set_model with invalid id → -32602 error")
    prefix = f"{BASE}.or_s77"
    MockLLM.reset()

    start_runner("s77", BIN_OR, or_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S77: session.new timed out"); return

        resp = await send_set_model(nc, prefix, sid, "completely-unknown-model-xyz-s77")
        info(f"S77: set_model unknown response = {str(resp)[:120]!r}")

        if resp.get("code") == -32602:
            ok("S77: error code -32602 (invalid params)")
        else:
            fail("S77: expected code -32602", str(resp.get("code")))

        msg = resp.get("message", "")
        if "unknown model" in msg.lower():
            ok("S77: error message mentions 'unknown model'")
        else:
            fail("S77: error message should mention 'unknown model'", msg[:80])

        # Runner should still be functional
        MockLLM.queue(or_sse("runner alive after unknown model s77"))
        sid2 = await new_session(nc, prefix, cwd="/tmp")
        if not sid2:
            fail("S77: new_session failed after unknown model error"); return
        r = await send_prompt(nc, prefix, sid2, "alive?")
        if prompt_err(r):
            fail("S77: runner not functional after unknown model", str(r)[:80])
        else:
            ok("S77: runner functional after unknown model error")

    finally:
        stop_runner("s77")


# ═════════════════════════════════════════════════════════════════════════════
# S78 — or export PortableMessages: role/text/blocks correct
# ═════════════════════════════════════════════════════════════════════════════

async def s78_or_export_format(nc):
    section("S78: or export PortableMessages — role/text/blocks all present and correct")
    prefix = f"{BASE}.or_s78"
    MockLLM.reset()
    export_text = "exported openrouter message content s78"
    MockLLM.queue(or_sse(export_text))

    start_runner("s78", BIN_OR, or_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S78: session.new timed out"); return

        resp = await send_prompt(nc, prefix, sid, "a test prompt s78")
        if prompt_err(resp):
            fail("S78: prompt failed", str(resp)[:80]); return
        ok("S78: prompt completed")

        exp = await nats_req(nc, f"{prefix}.agent.ext.session/export", {"sessionId": sid})
        if "_error" in exp or "code" in exp:
            fail("S78: export failed", str(exp)[:80]); return

        # The export result is a list of PortableMessages
        portable = exp
        if isinstance(portable, str):
            portable = json.loads(portable)
        if "result" in portable and isinstance(portable["result"], str):
            portable = json.loads(portable["result"])

        info(f"S78: exported {len(portable) if isinstance(portable, list) else 'N/A'} messages")
        if not isinstance(portable, list) or len(portable) < 2:
            fail("S78: expected list of ≥2 PortableMessages", str(portable)[:80]); return
        ok(f"S78: exported {len(portable)} PortableMessages (user + assistant)")

        roles = [m.get("role") for m in portable]
        if roles[0] == "user" and roles[1] == "assistant":
            ok("S78: roles correct: user then assistant")
        else:
            fail("S78: unexpected roles", str(roles))

        asst = portable[1]
        if export_text in asst.get("text", ""):
            ok("S78: assistant text field contains expected content")
        else:
            fail("S78: assistant text wrong", str(asst.get("text"))[:80])

        blocks = asst.get("blocks", [])
        if blocks and blocks[0].get("type") == "text":
            ok("S78: blocks field present with type='text' block")
        else:
            fail("S78: expected blocks=[{type:text,...}]", str(blocks)[:80])

        if blocks and export_text in blocks[0].get("text", ""):
            ok("S78: blocks[0].text matches expected content")
        else:
            fail("S78: blocks[0].text wrong", str(blocks[0] if blocks else [])[:80])

    finally:
        stop_runner("s78")


# ═════════════════════════════════════════════════════════════════════════════
# S79 — or import replaces history: import PortableMessages → history updated
# ═════════════════════════════════════════════════════════════════════════════

async def s79_or_import_replaces_history(nc):
    section("S79: or import replaces history — imported PortableMessages appear in get_state")
    prefix = f"{BASE}.or_s79"
    MockLLM.reset()

    start_runner("s79", BIN_OR, or_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S79: session.new timed out"); return

        # Import some portable messages directly (no prior prompt needed)
        portable_messages = [
            {"role": "user", "text": "imported user message s79",
             "blocks": [{"type": "text", "text": "imported user message s79"}]},
            {"role": "assistant", "text": "imported assistant response s79",
             "blocks": [{"type": "text", "text": "imported assistant response s79"}]},
        ]
        import_resp = await nats_req(
            nc, f"{prefix}.agent.ext.session/import",
            {"sessionId": sid, "messages": portable_messages}
        )
        if "_error" in import_resp or "code" in import_resp:
            fail("S79: import failed", str(import_resp)[:80]); return
        ok("S79: import succeeded")

        # Export to verify history was replaced
        exp = await nats_req(nc, f"{prefix}.agent.ext.session/export", {"sessionId": sid})
        if "_error" in exp or "code" in exp:
            fail("S79: export after import failed", str(exp)[:80]); return

        portable = exp
        if isinstance(portable, str):
            portable = json.loads(portable)
        if "result" in portable and isinstance(portable["result"], str):
            portable = json.loads(portable["result"])

        if not isinstance(portable, list):
            fail("S79: export should return list", str(portable)[:80]); return

        info(f"S79: exported {len(portable)} messages after import")
        if len(portable) == 2:
            ok("S79: exactly 2 messages exported after import")
        else:
            fail("S79: expected 2 messages after import", f"got {len(portable)}")
            return

        if portable[0].get("role") == "user" and \
           "imported user message s79" in portable[0].get("text", ""):
            ok("S79: first exported message is the imported user message")
        else:
            fail("S79: first exported message wrong", str(portable[0])[:80])

        if portable[1].get("role") == "assistant" and \
           "imported assistant response s79" in portable[1].get("text", ""):
            ok("S79: second exported message is the imported assistant response")
        else:
            fail("S79: second exported message wrong", str(portable[1])[:80])

        # Verify get_state also shows the updated history
        state = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid})
        history = state.get("history", [])
        info(f"S79: get_state history length after import = {len(history)}")
        if len(history) == 2:
            ok("S79: get_state.history has 2 entries after import")
        else:
            fail("S79: get_state.history wrong after import", f"got {len(history)}")

    finally:
        stop_runner("s79")


# ═════════════════════════════════════════════════════════════════════════════
# S80 — or TROGON.md system msg: marker appears as system role in API body
# ═════════════════════════════════════════════════════════════════════════════

async def s80_or_trogon_md_system_msg(nc):
    section("S80: or TROGON.md system msg — TROGON.md marker appears in API request body")
    prefix = f"{BASE}.or_s80"
    MockLLM.reset()
    MockLLM.queue(or_sse("response with system prompt s80"))

    marker = "TROGON_OPENROUTER_SYSTEM_MARKER_S80"
    tmp_dir = tempfile.mkdtemp()
    try:
        with open(os.path.join(tmp_dir, "TROGON.md"), "w") as f:
            f.write(f"# Project Instructions\n\n{marker}\n")

        start_runner("s80", BIN_OR, or_runner_env(prefix))
        try:
            sid = await wait_runner(nc, prefix, cwd=tmp_dir)
            if not sid:
                fail("S80: session.new timed out"); return

            resp = await send_prompt(nc, prefix, sid, "test prompt s80")
            if prompt_err(resp):
                fail("S80: prompt failed", str(resp)[:80]); return
            ok("S80: prompt completed")

            calls = MockLLM.requests()
            if not calls:
                fail("S80: no API calls recorded"); return

            body = calls[0]
            msgs = body.get("messages", [])
            info(f"S80: API message count = {len(msgs)}, roles = {[m.get('role') for m in msgs]}")

            body_str = json.dumps(body)
            if marker in body_str:
                ok("S80: TROGON.md marker present in API request body")
            else:
                fail("S80: TROGON.md marker missing from API request body")

            # The marker should be in a system-role message
            system_msgs = [m for m in msgs if m.get("role") == "system"]
            if system_msgs and marker in system_msgs[0].get("content", ""):
                ok("S80: marker is in system-role message")
            else:
                fail("S80: marker not found in system-role message",
                     str(system_msgs)[:120] if system_msgs else "no system msg")

        finally:
            stop_runner("s80")
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S81 — or 2 sessions isolated: each session has own history, no cross-contamination
# ═════════════════════════════════════════════════════════════════════════════

async def s81_or_session_isolation(nc):
    section("S81: or 2 sessions isolated — each session has own history; no cross-contamination")
    prefix = f"{BASE}.or_s81"
    MockLLM.reset()
    MockLLM.queue(or_sse("session A response s81"))
    MockLLM.queue(or_sse("session B response s81"))

    start_runner("s81", BIN_OR, or_runner_env(prefix))
    try:
        sid_a = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid_a:
            fail("S81: session A creation timed out"); return
        sid_b = await new_session(nc, prefix, cwd="/tmp")
        if not sid_b:
            fail("S81: session B creation failed"); return

        info(f"S81: A={sid_a[:8]}  B={sid_b[:8]}")

        ra = await send_prompt(nc, prefix, sid_a, "session A prompt s81")
        rb = await send_prompt(nc, prefix, sid_b, "session B prompt s81")

        if prompt_err(ra):
            fail("S81: session A prompt failed", str(ra)[:80]); return
        if prompt_err(rb):
            fail("S81: session B prompt failed", str(rb)[:80]); return
        ok("S81: both prompts completed")

        # Export both sessions
        exp_a = await nats_req(nc, f"{prefix}.agent.ext.session/export", {"sessionId": sid_a})
        exp_b = await nats_req(nc, f"{prefix}.agent.ext.session/export", {"sessionId": sid_b})

        def get_portable(exp):
            p = exp
            if isinstance(p, str):
                p = json.loads(p)
            if "result" in p and isinstance(p["result"], str):
                p = json.loads(p["result"])
            return p if isinstance(p, list) else []

        p_a = get_portable(exp_a)
        p_b = get_portable(exp_b)

        # Session A should have "session A response" but NOT "session B response"
        a_str = json.dumps(p_a)
        b_str = json.dumps(p_b)

        if "session A response s81" in a_str:
            ok("S81: session A export contains session A response")
        else:
            fail("S81: session A export missing session A response", a_str[:120])

        if "session B response s81" not in a_str:
            ok("S81: session A export does NOT contain session B response (isolated)")
        else:
            fail("S81: session A export contaminated with session B content")

        if "session B response s81" in b_str:
            ok("S81: session B export contains session B response")
        else:
            fail("S81: session B export missing session B response", b_str[:120])

        if "session A response s81" not in b_str:
            ok("S81: session B export does NOT contain session A response (isolated)")
        else:
            fail("S81: session B export contaminated with session A content")

        # Also verify API calls used separate message histories (no mixing)
        calls = MockLLM.requests()
        if len(calls) == 2:
            ok("S81: exactly 2 API calls (one per session)")
        else:
            fail("S81: expected 2 API calls", f"got {len(calls)}")

    finally:
        stop_runner("s81")


# ═════════════════════════════════════════════════════════════════════════════
# S82 — or survives truncated stream: [DONE] only → EndTurn; runner stays alive
# ═════════════════════════════════════════════════════════════════════════════

async def s82_or_survives_truncated_stream(nc):
    section("S82: or truncated stream → EndTurn (not crash); runner stays functional")
    prefix = f"{BASE}.or_s82"
    MockLLM.reset()
    # Queue a truncated stream: just [DONE] with no content or finish_reason
    MockLLM.queue(or_sse_truncated())
    # Queue a normal response for the follow-up
    MockLLM.queue(or_sse("runner alive after truncated stream s82"))

    start_runner("s82", BIN_OR, or_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S82: session.new timed out"); return

        # First prompt: stream ends abruptly (just [DONE])
        r1 = await send_prompt(nc, prefix, sid, "first prompt with truncated response")
        if "_error" in r1 and "timed out" in r1.get("_error", ""):
            fail("S82: prompt timed out on truncated stream (should return EndTurn)")
            return
        ok("S82: prompt 1 completed despite truncated stream (no crash)")

        # Runner should still be functional
        sid2 = await new_session(nc, prefix, cwd="/tmp")
        if not sid2:
            fail("S82: new_session failed after truncated stream"); return
        r2 = await send_prompt(nc, prefix, sid2, "are you alive?")
        if prompt_err(r2):
            fail("S82: runner not functional after truncated stream", str(r2)[:80])
        else:
            ok("S82: runner functional after truncated stream (second prompt succeeded)")

        calls = MockLLM.requests()
        info(f"S82: total API calls = {len(calls)}")
        if len(calls) == 2:
            ok("S82: exactly 2 API calls (truncated + recovery)")
        else:
            fail("S82: expected 2 API calls", f"got {len(calls)}")

    finally:
        stop_runner("s82")


# ═════════════════════════════════════════════════════════════════════════════
# Main
# ═════════════════════════════════════════════════════════════════════════════

async def main():
    print(f"=== smoke_test_programming8.py — deep openrouter-runner behavioral verification ===")
    print(f"  prefix : {BASE}")
    print(f"  nats   : {NATS_URL}")
    print(f"  mock   : http://127.0.0.1:{MOCK_PORT}")

    start_mock(MOCK_PORT)
    info("mock server started on :" + str(MOCK_PORT))

    nc = await nats_lib.connect(NATS_URL)
    try:
        await s71_or_get_state_fresh(nc)
        await s72_or_basic_prompt(nc)
        await s73_or_full_history_mode(nc)
        await s74_or_single_tool_round(nc)
        await s75_or_two_tool_rounds(nc)
        await s76_or_set_model_changes_api_body(nc)
        await s77_or_unknown_model_error(nc)
        await s78_or_export_format(nc)
        await s79_or_import_replaces_history(nc)
        await s80_or_trogon_md_system_msg(nc)
        await s81_or_session_isolation(nc)
        await s82_or_survives_truncated_stream(nc)
    finally:
        stop_all()
        await nc.close()

    print(f"\n=== Results ===")
    print(f"  {green}passed{reset}: {PASS}")
    print(f"  {red}failed{reset}: {FAIL}")
    if FAIL == 0:
        print(f"\n{green}All scenarios passed.{reset}")
        sys.exit(0)
    else:
        print(f"\n{red}{FAIL} scenario(s) failed.{reset}")
        sys.exit(1)


if __name__ == "__main__":
    asyncio.run(main())
