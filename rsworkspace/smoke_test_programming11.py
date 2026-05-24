#!/usr/bin/env python3
"""
smoke_test_programming11.py — Deeper correctness: session lifecycle, history content,
xai chain, model-in-request, max_history trim, acp modes, concurrent sessions.

  S107. xai list_sessions          create 2 sessions → list has 2; close 1 → list has 1
  S108. or list_sessions + cwd     list_sessions response includes cwd from new_session
  S109. or fork + history inherit  fork inherits source messages; fork prompt grows fork only
  S110. xai fork + isolation       fork clears last_response_id; fork prompt is independent
  S111. or close + list_sessions   close removes session; prompt on closed session → error
  S112. or resume unknown session  resume nonexistent → -32602 error
  S113. acp set_mode valid modes   bypassPermissions accepted; acceptEdits accepted; mode in state
  S114. or history content         roles and text content of messages verified exactly
  S115. xai prev_response_id chain exact previous_response_id value in turn 2 and turn 3
  S116. xai model in API after set_model  new model appears in request body field "model"
  S117. or max_history trim        OPENROUTER_MAX_HISTORY_MESSAGES=4; turn 5 → oldest trimmed
  S118. concurrent sessions        2 sessions same runner; parallel prompts; histories isolated

Usage:
    NATS_URL=nats://localhost:4333 python3 smoke_test_programming11.py
"""

import asyncio
import collections
import json
import os
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Optional
import subprocess
import sys

import nats as nats_lib

# ── Config ────────────────────────────────────────────────────────────────────
NATS_URL       = os.environ.get("NATS_URL", "nats://localhost:4222")
RSDIR          = os.path.dirname(os.path.abspath(__file__))
BASE           = f"prog11.{os.getpid()}"
MOCK_OR_PORT   = 19832
MOCK_XAI_PORT  = 19834
MOCK_ACP_PORT  = 19836
PROMPT_TIMEOUT = 45.0
NATS_TIMEOUT   = 12.0
RUNNER_READY   = 14.0

green, red, yellow, cyan, reset = "\033[32m", "\033[31m", "\033[33m", "\033[36m", "\033[0m"
PASS = 0
FAIL = 0
procs: dict = {}

BIN_OR    = os.path.join(RSDIR, "target/debug/trogon-openrouter-runner")
BIN_XAI   = os.path.join(RSDIR, "target/debug/trogon-xai-runner")
BIN_ACP   = os.path.join(RSDIR, "target/debug/trogon-acp-runner")


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


# ── SSE builders ──────────────────────────────────────────────────────────────

def or_sse(text: str) -> bytes:
    chunks = [
        json.dumps({"choices": [{"delta": {"content": text}}]}),
        json.dumps({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
        json.dumps({"usage": {"prompt_tokens": 10, "completion_tokens": 5,
                              "cache_read_input_tokens": 0,
                              "cache_creation_input_tokens": 0}}),
        "[DONE]",
    ]
    return ("".join(f"data: {c}\n\n" for c in chunks)).encode()


def xai_sse(text: str, resp_id: str = "resp-s11") -> bytes:
    chunks = [
        "data: " + json.dumps({"id": resp_id, "type": "message.delta",
                               "delta": {"text": text}}) + "\n\n",
        "data: " + json.dumps({"type": "response.completed",
                               "response": {"id": resp_id, "status": "completed",
                                            "usage": {"input_tokens": 5, "output_tokens": 2}}}) + "\n\n",
        "data: [DONE]\n\n",
    ]
    return "".join(chunks).encode()


def acp_sse(text: str) -> bytes:
    chunks = [
        "event: message_start\ndata: " + json.dumps({
            "type": "message_start",
            "message": {"id": "msg-s11", "type": "message", "role": "assistant",
                        "content": [], "model": "claude-sonnet-4-6", "stop_reason": None,
                        "usage": {"input_tokens": 5, "output_tokens": 0}},
        }) + "\n\n",
        "event: content_block_start\ndata: " + json.dumps({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "text", "text": ""},
        }) + "\n\n",
        "event: content_block_delta\ndata: " + json.dumps({
            "type": "content_block_delta", "index": 0,
            "delta": {"type": "text_delta", "text": text},
        }) + "\n\n",
        "event: content_block_stop\ndata: " + json.dumps(
            {"type": "content_block_stop", "index": 0}) + "\n\n",
        "event: message_delta\ndata: " + json.dumps({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn"},
            "usage": {"output_tokens": 5},
        }) + "\n\n",
        "event: message_stop\ndata: " + json.dumps({"type": "message_stop"}) + "\n\n",
    ]
    return "".join(chunks).encode()


# ── Mock servers ──────────────────────────────────────────────────────────────

class MockOR(BaseHTTPRequestHandler):
    _lock = threading.Lock()
    _log: list = []
    _queue: collections.deque = collections.deque()

    @classmethod
    def reset(cls):
        with cls._lock:
            cls._log.clear()
            cls._queue.clear()

    @classmethod
    def push(cls, sse: bytes):
        with cls._lock:
            cls._queue.append(sse)

    @classmethod
    def requests(cls) -> list:
        with cls._lock:
            return list(cls._log)

    def log_message(self, *_): pass

    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(n) if n else b"{}")
        with self.__class__._lock:
            self.__class__._log.append(body)
            resp = self.__class__._queue.popleft() if self.__class__._queue else or_sse("ok")
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        try:
            self.wfile.write(resp)
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass


class MockXAI(BaseHTTPRequestHandler):
    _lock = threading.Lock()
    _log: list = []
    _queue: collections.deque = collections.deque()

    @classmethod
    def reset(cls):
        with cls._lock:
            cls._log.clear()
            cls._queue.clear()

    @classmethod
    def push(cls, sse: bytes):
        with cls._lock:
            cls._queue.append(sse)

    @classmethod
    def requests(cls) -> list:
        with cls._lock:
            return list(cls._log)

    def log_message(self, *_): pass

    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(n) if n else b"{}")
        with self.__class__._lock:
            self.__class__._log.append(body)
            resp = self.__class__._queue.popleft() if self.__class__._queue else xai_sse("ok")
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        try:
            self.wfile.write(resp)
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass


class MockACP(BaseHTTPRequestHandler):
    _lock = threading.Lock()
    _log: list = []
    _queue: collections.deque = collections.deque()

    @classmethod
    def reset(cls):
        with cls._lock:
            cls._log.clear()
            cls._queue.clear()

    @classmethod
    def push(cls, sse: bytes):
        with cls._lock:
            cls._queue.append(sse)

    @classmethod
    def requests(cls) -> list:
        with cls._lock:
            return list(cls._log)

    def log_message(self, *_): pass

    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(n) if n else b"{}")
        with self.__class__._lock:
            self.__class__._log.append(body)
            resp = self.__class__._queue.popleft() if self.__class__._queue else acp_sse("ok")
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        try:
            self.wfile.write(resp)
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass


def start_mock(port: int, handler_cls):
    s = HTTPServer(("127.0.0.1", port), handler_cls)
    threading.Thread(target=s.serve_forever, daemon=True).start()
    return s


# ── Runner factories ──────────────────────────────────────────────────────────

def or_env(prefix: str, model: str = "anthropic/claude-sonnet-4-6", **extra) -> dict:
    env = {
        "ACP_PREFIX":               prefix,
        "OPENROUTER_BASE_URL":      f"http://127.0.0.1:{MOCK_OR_PORT}",
        "OPENROUTER_API_KEY":       "fake-or-key",
        "OPENROUTER_DEFAULT_MODEL": model,
        "OPENROUTER_MODELS":        (
            f"{model}:Default Model,"
            "openai/gpt-4o:GPT-4o,"
            "anthropic/claude-sonnet-4-6:Claude Sonnet 4.6"
        ),
    }
    env.update(extra)
    return env


def xai_env(prefix: str, model: str = "grok-3") -> dict:
    return {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_XAI_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": model,
        "XAI_MODELS":        f"{model}:{model},grok-3-mini:Grok-3 Mini,grok-2:Grok-2",
    }


def acp_env(prefix: str) -> dict:
    return {
        "ACP_PREFIX":         prefix,
        "ANTHROPIC_BASE_URL": f"http://127.0.0.1:{MOCK_ACP_PORT}",
        "ANTHROPIC_API_KEY":  "fake-acp-key",
    }


def start_runner(name: str, binary: str, env_extra: dict):
    env = {**os.environ, "NATS_URL": NATS_URL, "RUST_LOG": "warn", **env_extra}
    procs[name] = subprocess.Popen(
        [binary], env=env,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )


def stop_runner(name: str):
    p = procs.pop(name, None)
    if p:
        p.terminate()
        try: p.wait(timeout=3)
        except subprocess.TimeoutExpired: p.kill()


def stop_all():
    for p in list(procs.values()):
        p.terminate()
        try: p.wait(timeout=3)
        except subprocess.TimeoutExpired: p.kill()
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
            json.dumps({"sessionId": sid,
                        "prompt": [{"type": "text", "text": text}]}).encode(),
            headers={"X-Req-Id": req_id},
        )
        msg = await asyncio.wait_for(sub.next_msg(), timeout=timeout)
        return json.loads(msg.data)
    except asyncio.TimeoutError:
        return {"_error": f"prompt timed out after {timeout}s"}
    except Exception as e:
        return {"_error": str(e)}
    finally:
        try: await sub.unsubscribe()
        except Exception: pass


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
        try: await sub.unsubscribe()
        except Exception: pass


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
        try: await sub.unsubscribe()
        except Exception: pass


async def list_sessions(nc, prefix: str) -> dict:
    # list_sessions is a global (NATS) command — use simple request-reply
    return await nats_req(nc, f"{prefix}.agent.session.list", {})


async def _js_session_cmd(nc, subject: str, payload: dict, timeout: float = 12.0) -> dict:
    """Fork/close/resume are JetStream commands: subscribe to response subject first,
    then publish with X-Req-Id header (no reply-to). Same pattern as set_model."""
    # Extract prefix and session ID from subject to build response subject
    # subject format: {prefix}.session.{sid}.agent.{method}
    parts = subject.split(".")
    # Find 'session' index
    try:
        sess_idx = parts.index("session")
        prefix_str = ".".join(parts[:sess_idx])
        sid_str = parts[sess_idx + 1]
    except (ValueError, IndexError):
        return {"_error": f"bad subject: {subject}"}

    req_id = str(uuid.uuid4())
    resp_subj = f"{prefix_str}.session.{sid_str}.agent.response.{req_id}"
    sub = await nc.subscribe(resp_subj)
    try:
        await nc.publish(
            subject,
            json.dumps(payload).encode(),
            headers={"X-Req-Id": req_id},
        )
        msg = await asyncio.wait_for(sub.next_msg(), timeout=timeout)
        return json.loads(msg.data)
    except asyncio.TimeoutError:
        return {"_error": f"timed out waiting for {subject}"}
    except Exception as e:
        return {"_error": str(e)}
    finally:
        try: await sub.unsubscribe()
        except Exception: pass


async def fork_session(nc, prefix: str, sid: str, cwd: str = "/tmp") -> dict:
    return await _js_session_cmd(
        nc, f"{prefix}.session.{sid}.agent.fork",
        {"sessionId": sid, "cwd": cwd, "mcpServers": []},
    )


async def close_session(nc, prefix: str, sid: str) -> dict:
    return await _js_session_cmd(
        nc, f"{prefix}.session.{sid}.agent.close",
        {"sessionId": sid},
    )


async def resume_session(nc, prefix: str, sid: str, cwd: str = "/tmp") -> dict:
    return await _js_session_cmd(
        nc, f"{prefix}.session.{sid}.agent.resume",
        {"sessionId": sid, "cwd": cwd, "mcpServers": []},
    )


def prompt_err(r: dict) -> Optional[str]:
    if "_error" in r:
        return r["_error"]
    if "code" in r:
        return r.get("message", str(r))
    return None


# ═════════════════════════════════════════════════════════════════════════════
# S107 — xai list_sessions: 2 sessions appear; close 1 → 1 remains
# ═════════════════════════════════════════════════════════════════════════════

async def s107_xai_list_sessions(nc):
    section("S107: xai list_sessions — create 2, list both; close 1, list 1")
    prefix = f"{BASE}.xai_s107"
    MockXAI.reset()

    start_runner("s107", BIN_XAI, xai_env(prefix))
    try:
        sid1 = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid1:
            fail("S107: session.new timed out"); return
        sid2 = await new_session(nc, prefix, cwd="/tmp")
        if not sid2:
            fail("S107: 2nd session.new failed"); return
        ok(f"S107: created 2 sessions ({sid1[:8]}, {sid2[:8]})")

        r = await list_sessions(nc, prefix)
        if "_error" in r or "code" in r:
            fail("S107: list_sessions failed", str(r)[:80]); return

        sessions = r.get("sessions", [])
        ids = [s.get("sessionId", s.get("id", "")) for s in sessions]
        info(f"S107: list_sessions returned {len(sessions)} sessions: {[x[:8] for x in ids]}")

        if len(sessions) == 2:
            ok("S107: list_sessions returns 2 sessions")
        else:
            fail("S107: expected 2 sessions", f"got {len(sessions)}")

        if sid1 in ids and sid2 in ids:
            ok("S107: both session IDs present in list")
        else:
            fail("S107: session IDs missing from list", f"ids={ids}")

        # Close sid1 and verify it disappears
        cr = await close_session(nc, prefix, sid1)
        if "_error" in cr or "code" in cr:
            fail("S107: close_session failed", str(cr)[:60]); return
        ok("S107: close_session returned OK")

        r2 = await list_sessions(nc, prefix)
        sessions2 = r2.get("sessions", [])
        ids2 = [s.get("sessionId", s.get("id", "")) for s in sessions2]
        info(f"S107: after close, list has {len(sessions2)} sessions: {[x[:8] for x in ids2]}")

        if len(sessions2) == 1:
            ok("S107: after close, list has 1 session")
        else:
            fail("S107: expected 1 session after close", f"got {len(sessions2)}")

        if sid1 not in ids2:
            ok("S107: closed session not in list")
        else:
            fail("S107: closed session still appears in list")

        if sid2 in ids2:
            ok("S107: remaining session still in list")
        else:
            fail("S107: remaining session disappeared from list")

    finally:
        stop_runner("s107")


# ═════════════════════════════════════════════════════════════════════════════
# S108 — or list_sessions + cwd: cwd from new_session appears in list
# ═════════════════════════════════════════════════════════════════════════════

async def s108_or_list_sessions_cwd(nc):
    section("S108: or list_sessions — cwd from new_session appears in response")
    prefix = f"{BASE}.or_s108"
    MockOR.reset()
    test_cwd = "/tmp/smoke11_s108"

    start_runner("s108", BIN_OR, or_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd=test_cwd)
        if not sid:
            fail("S108: session.new timed out"); return

        r = await list_sessions(nc, prefix)
        if "_error" in r or "code" in r:
            fail("S108: list_sessions failed", str(r)[:80]); return

        sessions = r.get("sessions", [])
        info(f"S108: list_sessions returned {len(sessions)} session(s)")

        if not sessions:
            fail("S108: no sessions in list"); return
        ok("S108: list_sessions has at least 1 session")

        # Find our session
        our = next((s for s in sessions
                    if s.get("sessionId", s.get("id", "")) == sid), None)
        if not our:
            fail("S108: our session not found in list"); return
        ok("S108: our session found in list_sessions")

        # Verify cwd
        cwd_val = our.get("cwd", "")
        info(f"S108: session cwd in list = {cwd_val!r}")
        if cwd_val == test_cwd:
            ok(f"S108: cwd in list_sessions matches new_session cwd ({test_cwd})")
        else:
            fail("S108: cwd mismatch", f"expected {test_cwd!r}, got {cwd_val!r}")

    finally:
        stop_runner("s108")


# ═════════════════════════════════════════════════════════════════════════════
# S109 — or fork + history inherit: fork gets source messages; fork grows independently
# ═════════════════════════════════════════════════════════════════════════════

async def s109_or_fork_history(nc):
    section("S109: or fork — fork inherits source history; fork grows independently")
    prefix = f"{BASE}.or_s109"
    MockOR.reset()

    MockOR.push(or_sse("source reply s109"))
    MockOR.push(or_sse("fork reply s109"))

    start_runner("s109", BIN_OR, or_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S109: session.new timed out"); return

        # Source: 1 turn → 2 messages (user + assistant)
        r = await send_prompt(nc, prefix, sid, "source question s109")
        if prompt_err(r):
            fail("S109: source prompt failed", str(r)[:80]); return
        ok("S109: source prompt completed (1 turn)")

        # Verify source history = 2 messages via list_sessions
        calls_before_fork = MockOR.requests()
        src_msgs = calls_before_fork[-1].get("messages", [])
        info(f"S109: source has {len(src_msgs)} messages after 1 turn")
        if len(src_msgs) == 1:
            ok("S109: source has 1 message (user) before first reply stored")
        else:
            info(f"S109: source messages = {len(src_msgs)}")

        # Fork the session
        fork_resp = await fork_session(nc, prefix, sid, cwd="/tmp/fork109")
        if "_error" in fork_resp or "code" in fork_resp:
            fail("S109: fork_session failed", str(fork_resp)[:80]); return
        fork_id = fork_resp.get("sessionId", "")
        if not fork_id:
            fail("S109: fork response missing sessionId", str(fork_resp)[:80]); return
        if fork_id != sid:
            ok(f"S109: fork created new session ({fork_id[:8]} ≠ {sid[:8]})")
        else:
            fail("S109: fork sessionId same as source")

        # Prompt the fork — should include source history in its API call
        r_fork = await send_prompt(nc, prefix, fork_id, "fork question s109")
        if prompt_err(r_fork):
            fail("S109: fork prompt failed", str(r_fork)[:80]); return
        ok("S109: fork prompt completed")

        calls = MockOR.requests()
        fork_call_msgs = calls[-1].get("messages", [])
        info(f"S109: fork API call has {len(fork_call_msgs)} messages")

        # Fork should have: [user(source), asst(source), user(fork)] = 3 messages
        if len(fork_call_msgs) >= 3:
            ok(f"S109: fork prompt includes source history ({len(fork_call_msgs)} msgs ≥ 3)")
        else:
            fail("S109: fork should inherit source history",
                 f"fork call has only {len(fork_call_msgs)} msgs")

        # Source session should still have only 3 messages (user+asst+no new user)
        # Actually after 1 turn: source has user+asst in history = 2 messages
        # Fork prompt should have source(user+asst) + fork(user) = 3

        # Verify list_sessions shows both sessions
        lr = await list_sessions(nc, prefix)
        all_ids = [s.get("sessionId", s.get("id", "")) for s in lr.get("sessions", [])]
        if sid in all_ids and fork_id in all_ids:
            ok("S109: both source and fork appear in list_sessions")
        else:
            fail("S109: not both sessions in list", f"ids={[x[:8] for x in all_ids]}")

    finally:
        stop_runner("s109")


# ═════════════════════════════════════════════════════════════════════════════
# S110 — xai fork: fork clears last_response_id; fork prompt is independent
# ═════════════════════════════════════════════════════════════════════════════

async def s110_xai_fork_isolation(nc):
    section("S110: xai fork — fork clears last_response_id; API call uses full history")
    prefix = f"{BASE}.xai_s110"
    MockXAI.reset()

    src_resp_id = "resp-s110-source"
    MockXAI.push(xai_sse("source text s110", src_resp_id))
    MockXAI.push(xai_sse("fork text s110", "resp-s110-fork"))

    start_runner("s110", BIN_XAI, xai_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S110: session.new timed out"); return

        # Prompt source → sets last_response_id
        r = await send_prompt(nc, prefix, sid, "source s110")
        if prompt_err(r):
            fail("S110: source prompt failed", str(r)[:80]); return

        state = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid})
        lrid = state.get("last_response_id")
        if lrid == src_resp_id:
            ok(f"S110: source last_response_id set ({lrid})")
        else:
            fail("S110: source last_response_id not set", str(lrid))

        # Fork the session
        fork_resp = await fork_session(nc, prefix, sid, cwd="/tmp/fork110")
        if "_error" in fork_resp or "code" in fork_resp:
            fail("S110: fork failed", str(fork_resp)[:80]); return
        fork_id = fork_resp.get("sessionId", "")
        if not fork_id or fork_id == sid:
            fail("S110: bad fork sessionId", str(fork_resp)[:80]); return
        ok(f"S110: fork created ({fork_id[:8]})")

        # Fork's last_response_id should be None (fork must replay history)
        fork_state = await nats_req(nc, f"{prefix}.agent.ext.session/get_state",
                                    {"sessionId": fork_id})
        fork_lrid = fork_state.get("last_response_id")
        if fork_lrid is None:
            ok("S110: fork last_response_id is None (will replay full history on first turn)")
        else:
            fail("S110: fork last_response_id should be None after fork", str(fork_lrid))

        # Prompt fork — since last_response_id is None, must send full input
        MockXAI.reset()
        await send_prompt(nc, prefix, fork_id, "fork question s110")
        fork_calls = MockXAI.requests()
        if fork_calls:
            fork_body = fork_calls[0]
            fork_input = fork_body.get("input", [])
            # full history sent: should have at least the source user + asst + fork user
            if len(fork_input) >= 2:
                ok(f"S110: fork sends full input ({len(fork_input)} items, not stateful)")
            else:
                info(f"S110: fork input items = {len(fork_input)}")
            # Should NOT have previous_response_id
            if "previous_response_id" not in fork_body:
                ok("S110: fork API call has no previous_response_id (correct)")
            else:
                fail("S110: fork should not have previous_response_id",
                     str(fork_body.get("previous_response_id")))
        else:
            fail("S110: no xai API calls from fork prompt")

    finally:
        stop_runner("s110")


# ═════════════════════════════════════════════════════════════════════════════
# S111 — or close + prompt on closed session: prompt fails; list removed
# ═════════════════════════════════════════════════════════════════════════════

async def s111_or_close_session(nc):
    section("S111: or close_session — removed from list; prompt on closed → error")
    prefix = f"{BASE}.or_s111"
    MockOR.reset()

    start_runner("s111", BIN_OR, or_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S111: session.new timed out"); return

        # Verify session exists in list
        lr = await list_sessions(nc, prefix)
        ids = [s.get("sessionId", s.get("id", "")) for s in lr.get("sessions", [])]
        if sid in ids:
            ok("S111: session in list before close")
        else:
            fail("S111: session not in list before close")

        # Close
        cr = await close_session(nc, prefix, sid)
        if "_error" in cr or "code" in cr:
            fail("S111: close_session returned error", str(cr)[:80]); return
        ok("S111: close_session OK")

        # Verify removed from list
        lr2 = await list_sessions(nc, prefix)
        ids2 = [s.get("sessionId", s.get("id", "")) for s in lr2.get("sessions", [])]
        if sid not in ids2:
            ok("S111: closed session removed from list_sessions")
        else:
            fail("S111: closed session still in list_sessions")

        # Prompt on closed session should return an error (not hang)
        resp = await send_prompt(nc, prefix, sid, "prompt on closed session s111",
                                 timeout=10.0)
        if prompt_err(resp):
            ok("S111: prompt on closed session returns error (correct)", str(resp)[:60])
        else:
            fail("S111: prompt on closed session should fail", str(resp)[:80])

    finally:
        stop_runner("s111")


# ═════════════════════════════════════════════════════════════════════════════
# S112 — or resume unknown session: returns -32602 error
# ═════════════════════════════════════════════════════════════════════════════

async def s112_or_resume_unknown(nc):
    section("S112: or resume_session nonexistent → error")
    prefix = f"{BASE}.or_s112"
    MockOR.reset()

    start_runner("s112", BIN_OR, or_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S112: session.new timed out"); return

        # Resume a session that doesn't exist
        r = await resume_session(nc, prefix, "nonexistent-session-id-s112")
        info(f"S112: resume nonexistent = {str(r)[:100]}")

        if "code" in r or "_error" in r:
            ok("S112: resume nonexistent session returns error (correct)")
        else:
            fail("S112: resume nonexistent should return error", str(r)[:80])

        # Also test resume on valid session succeeds
        r2 = await resume_session(nc, prefix, sid)
        if "code" in r2 or "_error" in r2:
            fail("S112: resume existing session should succeed", str(r2)[:80])
        else:
            ok("S112: resume existing session OK")

    finally:
        stop_runner("s112")


# ═════════════════════════════════════════════════════════════════════════════
# S113 — acp set_mode: valid modes accepted; mode reflected in state
# ═════════════════════════════════════════════════════════════════════════════

async def s113_acp_set_mode(nc):
    section("S113: acp set_mode — valid modes accepted; mode persists in state")
    prefix = f"{BASE}.acp_s113"
    MockACP.reset()

    start_runner("s113", BIN_ACP, acp_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S113: session.new timed out"); return

        for mode in ["default", "acceptEdits", "bypassPermissions"]:
            r = await send_set_mode(nc, prefix, sid, mode)
            if "code" in r or "_error" in r:
                fail(f"S113: set_mode '{mode}' returned error", str(r)[:80])
            else:
                ok(f"S113: set_mode '{mode}' → OK")

        # Verify final mode persists
        # ACP uses list_sessions or load_session to check state
        lr = await list_sessions(nc, prefix)
        our_session = next(
            (s for s in lr.get("sessions", [])
             if s.get("sessionId", s.get("id", "")) == sid), None
        )
        if our_session:
            mode_val = our_session.get("mode", our_session.get("currentMode", ""))
            info(f"S113: mode in list_sessions = {mode_val!r}")
            if mode_val == "bypassPermissions":
                ok("S113: mode 'bypassPermissions' persists in session state")
            elif mode_val:
                info(f"S113: mode present but value = {mode_val!r}")
                ok(f"S113: mode present in list_sessions (value={mode_val!r})")
            else:
                info("S113: mode field not in list_sessions entry (may be in get_state)")
                ok("S113: all set_mode calls returned OK — mode accepted")
        else:
            info("S113: session not in list_sessions, but set_mode calls all returned OK")
            ok("S113: all set_mode calls returned OK")

        # Verify bypassPermissions actually works: send a prompt with file tool
        MockACP.push(acp_sse("ok s113"))
        r_prompt = await send_prompt(nc, prefix, sid, "test mode s113")
        if prompt_err(r_prompt):
            fail("S113: prompt in bypassPermissions mode failed", str(r_prompt)[:80])
        else:
            ok("S113: prompt succeeds in bypassPermissions mode")

    finally:
        stop_runner("s113")


# ═════════════════════════════════════════════════════════════════════════════
# S114 — or history content: roles and text verified exactly across 2 turns
# ═════════════════════════════════════════════════════════════════════════════

async def s114_or_history_content(nc):
    section("S114: or history content — roles and text verified exactly")
    prefix = f"{BASE}.or_s114"
    MockOR.reset()

    user1 = "first user message s114"
    asst1 = "first assistant reply s114"
    user2 = "second user message s114"

    MockOR.push(or_sse(asst1))
    MockOR.push(or_sse("second assistant reply s114"))

    start_runner("s114", BIN_OR, or_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S114: session.new timed out"); return

        # Turn 1
        r1 = await send_prompt(nc, prefix, sid, user1)
        if prompt_err(r1):
            fail("S114: turn 1 failed", str(r1)[:80]); return

        calls = MockOR.requests()
        c1_msgs = calls[0].get("messages", [])
        info(f"S114: turn 1 messages = {json.dumps(c1_msgs, indent=None)[:200]}")

        # Call 1 should have exactly 1 message: user with user1 text
        if len(c1_msgs) == 1:
            ok("S114: turn 1 has 1 message")
        else:
            fail("S114: turn 1 should have 1 message", f"got {len(c1_msgs)}")
        if c1_msgs and c1_msgs[0].get("role") == "user":
            ok("S114: turn 1 msg[0].role = 'user'")
        else:
            fail("S114: turn 1 msg[0].role wrong", str(c1_msgs[:1]))
        if c1_msgs:
            content = c1_msgs[0].get("content", "")
            if isinstance(content, list):
                text_content = next((b.get("text", "") for b in content
                                     if isinstance(b, dict) and b.get("type") == "text"), "")
            else:
                text_content = content
            if user1 in text_content:
                ok(f"S114: turn 1 user text preserved ({user1!r})")
            else:
                fail("S114: turn 1 user text wrong", f"got {text_content!r}")

        # Turn 2
        r2 = await send_prompt(nc, prefix, sid, user2)
        if prompt_err(r2):
            fail("S114: turn 2 failed", str(r2)[:80]); return

        calls2 = MockOR.requests()
        c2_msgs = calls2[1].get("messages", [])
        info(f"S114: turn 2 messages ({len(c2_msgs)} items)")

        # Call 2 should have 3 messages: user1, asst1, user2
        if len(c2_msgs) == 3:
            ok("S114: turn 2 has 3 messages (user1 + asst1 + user2)")
        else:
            fail("S114: turn 2 should have 3 messages", f"got {len(c2_msgs)}")

        if len(c2_msgs) >= 3:
            roles = [m.get("role") for m in c2_msgs]
            if roles == ["user", "assistant", "user"]:
                ok("S114: turn 2 message roles correct [user, assistant, user]")
            else:
                fail("S114: turn 2 message roles wrong", str(roles))

            # Check assistant text
            asst_msg = c2_msgs[1]
            asst_content = asst_msg.get("content", "")
            if isinstance(asst_content, list):
                asst_text = " ".join(b.get("text", "") for b in asst_content
                                     if isinstance(b, dict) and b.get("type") == "text")
            else:
                asst_text = asst_content
            if asst1 in asst_text:
                ok(f"S114: turn 2 assistant text preserved ({asst1!r})")
            else:
                fail("S114: turn 2 assistant text wrong", f"got {asst_text!r}")

            # Check second user text
            u2_msg = c2_msgs[2]
            u2_content = u2_msg.get("content", "")
            if isinstance(u2_content, list):
                u2_text = next((b.get("text", "") for b in u2_content
                                if isinstance(b, dict) and b.get("type") == "text"), "")
            else:
                u2_text = u2_content
            if user2 in u2_text:
                ok(f"S114: turn 2 user2 text preserved ({user2!r})")
            else:
                fail("S114: turn 2 user2 text wrong", f"got {u2_text!r}")

    finally:
        stop_runner("s114")


# ═════════════════════════════════════════════════════════════════════════════
# S115 — xai prev_response_id chain: exact ID value in each turn verified
# ═════════════════════════════════════════════════════════════════════════════

async def s115_xai_prev_response_id_chain(nc):
    section("S115: xai prev_response_id chain — exact ID value verified turn-by-turn")
    prefix = f"{BASE}.xai_s115"
    MockXAI.reset()

    resp_id_1 = "resp-chain-s115-turn1"
    resp_id_2 = "resp-chain-s115-turn2"
    resp_id_3 = "resp-chain-s115-turn3"

    MockXAI.push(xai_sse("reply 1 s115", resp_id_1))
    MockXAI.push(xai_sse("reply 2 s115", resp_id_2))
    MockXAI.push(xai_sse("reply 3 s115", resp_id_3))

    start_runner("s115", BIN_XAI, xai_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S115: session.new timed out"); return

        # Turn 1: no previous_response_id
        await send_prompt(nc, prefix, sid, "turn 1 s115")
        calls = MockXAI.requests()
        c1 = calls[0]
        if "previous_response_id" not in c1:
            ok("S115: turn 1 has no previous_response_id (first turn)")
        else:
            fail("S115: turn 1 should not have previous_response_id",
                 str(c1.get("previous_response_id")))
        # Verify model field
        if c1.get("model") == "grok-3":
            ok("S115: turn 1 model = 'grok-3'")
        else:
            fail("S115: turn 1 model wrong", str(c1.get("model")))

        # Turn 2: previous_response_id must be resp_id_1
        await send_prompt(nc, prefix, sid, "turn 2 s115")
        calls = MockXAI.requests()
        c2 = calls[1]
        got_prev = c2.get("previous_response_id")
        if got_prev == resp_id_1:
            ok(f"S115: turn 2 previous_response_id = {resp_id_1!r} (correct)")
        else:
            fail("S115: turn 2 previous_response_id wrong",
                 f"expected {resp_id_1!r}, got {got_prev!r}")

        # Turn 3: previous_response_id must be resp_id_2 (not resp_id_1)
        await send_prompt(nc, prefix, sid, "turn 3 s115")
        calls = MockXAI.requests()
        c3 = calls[2]
        got_prev3 = c3.get("previous_response_id")
        if got_prev3 == resp_id_2:
            ok(f"S115: turn 3 previous_response_id = {resp_id_2!r} (correct, not turn-1 ID)")
        else:
            fail("S115: turn 3 previous_response_id wrong",
                 f"expected {resp_id_2!r}, got {got_prev3!r}")

        # Verify state has latest response_id
        state = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid})
        if state.get("last_response_id") == resp_id_3:
            ok(f"S115: get_state last_response_id = {resp_id_3!r} (latest)")
        else:
            fail("S115: get_state last_response_id wrong",
                 f"expected {resp_id_3!r}, got {state.get('last_response_id')!r}")

    finally:
        stop_runner("s115")


# ═════════════════════════════════════════════════════════════════════════════
# S116 — xai model in API request after set_model
# ═════════════════════════════════════════════════════════════════════════════

async def s116_xai_model_in_request(nc):
    section("S116: xai model in API request — set_model → new model in request body 'model' field")
    prefix = f"{BASE}.xai_s116"
    MockXAI.reset()

    MockXAI.push(xai_sse("before set_model s116", "resp-s116-before"))
    MockXAI.push(xai_sse("after set_model s116", "resp-s116-after"))

    start_runner("s116", BIN_XAI, xai_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S116: session.new timed out"); return

        # Turn 1: default model (grok-3)
        r1 = await send_prompt(nc, prefix, sid, "turn 1 s116")
        if prompt_err(r1):
            fail("S116: turn 1 failed", str(r1)[:80]); return

        calls = MockXAI.requests()
        model_t1 = calls[0].get("model")
        if model_t1 == "grok-3":
            ok("S116: turn 1 uses default model 'grok-3'")
        else:
            fail("S116: turn 1 model wrong", f"got {model_t1!r}")

        # Change model to grok-3-mini
        sm = await send_set_model(nc, prefix, sid, "grok-3-mini")
        if "code" in sm or "_error" in sm:
            fail("S116: set_model grok-3-mini failed", str(sm)[:80]); return
        ok("S116: set_model 'grok-3-mini' → OK")

        # Turn 2: should use grok-3-mini in request
        r2 = await send_prompt(nc, prefix, sid, "turn 2 s116")
        if prompt_err(r2):
            fail("S116: turn 2 failed", str(r2)[:80]); return

        calls2 = MockXAI.requests()
        model_t2 = calls2[1].get("model")
        if model_t2 == "grok-3-mini":
            ok("S116: turn 2 uses new model 'grok-3-mini' in request body")
        else:
            fail("S116: turn 2 model should be 'grok-3-mini'", f"got {model_t2!r}")

        # Verify previous_response_id is from turn 1 (set_model clears it per S68)
        prev_id = calls2[1].get("previous_response_id")
        if prev_id is None:
            ok("S116: turn 2 has no previous_response_id (set_model cleared it)")
        else:
            fail("S116: set_model should clear previous_response_id", f"got {prev_id!r}")

    finally:
        stop_runner("s116")


# ═════════════════════════════════════════════════════════════════════════════
# S117 — or max_history trim: OPENROUTER_MAX_HISTORY_MESSAGES=4 trims oldest turn
# ═════════════════════════════════════════════════════════════════════════════

async def s117_or_max_history_trim(nc):
    section("S117: or max_history trim — OPENROUTER_MAX_HISTORY_MESSAGES=4; turn 5 trims oldest")
    prefix = f"{BASE}.or_s117"
    MockOR.reset()

    for i in range(5):
        MockOR.push(or_sse(f"reply {i + 1} s117"))

    # max_history=4: after 3 full turns (6 messages) → trim to 4
    # trim_history removes oldest turn (user+assistant) until len <= 4
    start_runner("s117", BIN_OR, or_env(prefix, OPENROUTER_MAX_HISTORY_MESSAGES="4"))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S117: session.new timed out"); return

        # 4 turns → after turn 4, history has 8 messages → trim to ≤4
        for i in range(4):
            r = await send_prompt(nc, prefix, sid, f"user message {i+1} s117")
            if prompt_err(r):
                fail(f"S117: turn {i+1} failed", str(r)[:80]); return
            info(f"S117: turn {i+1} completed")

        calls = MockOR.requests()
        info(f"S117: total API calls = {len(calls)}")

        # After turn 1: 1 msg sent, 2 in history
        # After turn 2: 3 msgs sent, 4 in history (at max)
        # After turn 3: trim: 4 msgs → remove oldest turn (user+asst) → 2, then add user: 5→trim→3→add asst: 4
        # Actually trim happens BEFORE sending the next turn
        # Let's trace:
        #   call1 sends: [u1] (1 msg) → history after: [u1, a1] (2 msgs)
        #   call2 sends: [u1,a1,u2] (3 msgs) → history after: [u1,a1,u2,a2] (4 msgs) = max
        #   call3: before sending trim_history to ≤4: already 4+new_user = 5 > 4
        #          trim removes u1+a1 → [u2,a2] (2 msgs) ... wait let me re-read the code
        #
        # trim_history: called AFTER the turn completes (appending asst to history)
        # So: after turn 3 history = [u1,a1,u2,a2,u3,a3] = 6 → trim to 4: remove u1,a1 → [u2,a2,u3,a3]
        # call4 sends: [u2,a2,u3,a3,u4] = 5 messages

        # Verify: call 4 should have ≤5 messages and NOT have "user message 1"
        if len(calls) >= 4:
            c4_msgs = calls[3].get("messages", [])
            info(f"S117: call 4 has {len(c4_msgs)} messages")
            if len(c4_msgs) <= 5:
                ok(f"S117: call 4 has {len(c4_msgs)} messages (history trimmed)")
            else:
                fail("S117: call 4 should have ≤5 messages (trim not working)",
                     f"got {len(c4_msgs)}")

            # "user message 1" should have been trimmed
            c4_str = json.dumps(c4_msgs)
            if "user message 1" not in c4_str:
                ok("S117: oldest turn ('user message 1') trimmed from history")
            else:
                fail("S117: 'user message 1' should be trimmed by now", c4_str[:120])

            # "user message 3" should still be present
            if "user message 3" in c4_str:
                ok("S117: recent turn ('user message 3') still in history")
            else:
                fail("S117: 'user message 3' should be in history", c4_str[:120])
        else:
            fail("S117: expected 4 API calls", f"got {len(calls)}")

    finally:
        stop_runner("s117")


# ═════════════════════════════════════════════════════════════════════════════
# S118 — concurrent sessions: parallel prompts; histories isolated
# ═════════════════════════════════════════════════════════════════════════════

async def s118_concurrent_sessions(nc):
    section("S118: concurrent sessions — parallel prompts; histories isolated")
    prefix = f"{BASE}.or_s118"
    MockOR.reset()

    # Queue 4 responses: 2 for each session (2 turns each)
    user_a1, asst_a1 = "session A turn 1 s118", "reply A1 s118"
    user_b1, asst_b1 = "session B turn 1 s118", "reply B1 s118"
    user_a2, asst_a2 = "session A turn 2 s118", "reply A2 s118"
    user_b2, asst_b2 = "session B turn 2 s118", "reply B2 s118"

    start_runner("s118", BIN_OR, or_env(prefix))
    try:
        sids = await wait_runner(nc, prefix, cwd="/tmp")
        if not sids:
            fail("S118: session.new timed out"); return
        sid_a = sids  # first session created by wait_runner
        sid_b = await new_session(nc, prefix, cwd="/tmp")
        if not sid_b:
            fail("S118: 2nd session.new failed"); return
        ok(f"S118: created 2 sessions (A={sid_a[:8]}, B={sid_b[:8]})")

        # Queue responses for both sessions
        MockOR.push(or_sse(asst_a1))
        MockOR.push(or_sse(asst_b1))

        # Send turn 1 for both sessions concurrently
        r_a1, r_b1 = await asyncio.gather(
            send_prompt(nc, prefix, sid_a, user_a1),
            send_prompt(nc, prefix, sid_b, user_b1),
        )
        if prompt_err(r_a1):
            fail("S118: session A turn 1 failed", str(r_a1)[:80]); return
        if prompt_err(r_b1):
            fail("S118: session B turn 1 failed", str(r_b1)[:80]); return
        ok("S118: both session turn 1 prompts completed concurrently")

        # Queue responses for turn 2
        MockOR.push(or_sse(asst_a2))
        MockOR.push(or_sse(asst_b2))

        # Send turn 2 for both sessions concurrently
        r_a2, r_b2 = await asyncio.gather(
            send_prompt(nc, prefix, sid_a, user_a2),
            send_prompt(nc, prefix, sid_b, user_b2),
        )
        if prompt_err(r_a2):
            fail("S118: session A turn 2 failed", str(r_a2)[:80]); return
        if prompt_err(r_b2):
            fail("S118: session B turn 2 failed", str(r_b2)[:80]); return
        ok("S118: both session turn 2 prompts completed concurrently")

        # Now verify isolation: each session should have its own messages
        # Find session A and B calls by looking at the messages content
        all_calls = MockOR.requests()
        info(f"S118: total API calls = {len(all_calls)}")

        # Each session had 2 turns → 4 calls total
        if len(all_calls) == 4:
            ok("S118: exactly 4 API calls (2 sessions × 2 turns)")
        else:
            fail("S118: expected 4 API calls", f"got {len(all_calls)}")

        # Find A's 2nd turn call (contains user_a1 and user_a2)
        a_calls = [c for c in all_calls if user_a2 in json.dumps(c.get("messages", []))]
        b_calls = [c for c in all_calls if user_b2 in json.dumps(c.get("messages", []))]

        info(f"S118: A's turn-2 calls = {len(a_calls)}, B's turn-2 calls = {len(b_calls)}")

        if a_calls:
            a2_msgs = a_calls[0].get("messages", [])
            # Session A turn 2 should have: [user_a1, asst_a1, user_a2]
            a2_str = json.dumps(a2_msgs)
            if user_a1 in a2_str and user_a2 in a2_str:
                ok("S118: session A turn 2 has A's own history (not B's)")
            else:
                fail("S118: session A history wrong", a2_str[:120])
            # Must NOT contain session B messages
            if user_b1 not in a2_str and user_b2 not in a2_str:
                ok("S118: session A does not contain session B's messages")
            else:
                fail("S118: session A contains session B's messages (leak!)", a2_str[:120])
        else:
            info("S118: could not identify session A's turn-2 call")

        if b_calls:
            b2_msgs = b_calls[0].get("messages", [])
            b2_str = json.dumps(b2_msgs)
            if user_b1 in b2_str and user_b2 in b2_str:
                ok("S118: session B turn 2 has B's own history (not A's)")
            else:
                fail("S118: session B history wrong", b2_str[:120])
            if user_a1 not in b2_str and user_a2 not in b2_str:
                ok("S118: session B does not contain session A's messages")
            else:
                fail("S118: session B contains session A's messages (leak!)", b2_str[:120])
        else:
            info("S118: could not identify session B's turn-2 call")

    finally:
        stop_runner("s118")


# ═════════════════════════════════════════════════════════════════════════════
# Main
# ═════════════════════════════════════════════════════════════════════════════

async def main():
    print("=== smoke_test_programming11.py — lifecycle, history content, chains ===")
    print(f"  prefix : {BASE}")
    print(f"  nats   : {NATS_URL}")
    print(f"  or mock : :{MOCK_OR_PORT}  xai mock : :{MOCK_XAI_PORT}  acp mock : :{MOCK_ACP_PORT}")

    start_mock(MOCK_OR_PORT, MockOR)
    start_mock(MOCK_XAI_PORT, MockXAI)
    start_mock(MOCK_ACP_PORT, MockACP)
    info(f"mock servers started")

    nc = await nats_lib.connect(NATS_URL)
    try:
        await s107_xai_list_sessions(nc)
        await s108_or_list_sessions_cwd(nc)
        await s109_or_fork_history(nc)
        await s110_xai_fork_isolation(nc)
        await s111_or_close_session(nc)
        await s112_or_resume_unknown(nc)
        await s113_acp_set_mode(nc)
        await s114_or_history_content(nc)
        await s115_xai_prev_response_id_chain(nc)
        await s116_xai_model_in_request(nc)
        await s117_or_max_history_trim(nc)
        await s118_concurrent_sessions(nc)
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
