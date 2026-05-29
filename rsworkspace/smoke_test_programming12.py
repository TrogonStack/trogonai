#!/usr/bin/env python3
"""
smoke_test_programming12.py — File-tool precision, session lifecycle, model-in-wire.

  S119. acp path traversal       read_file "../../etc/passwd" → error "path is outside"
  S120. acp write nested dirs    write_file "sub/deep/s120.txt" → parent dirs created
  S121. acp read offset+limit    offset=2 limit=3 → lines 3-5 only (exact line numbers)
  S122. acp str_replace 0 occ    old_str absent → "Error: 'old_str' not found (0 occurrences)"
  S123. acp str_replace 2 occ    2 occurrences → "not unique — found 2 occurrences"
  S124. codex fork + list_children  fork → list_children returns fork_id
  S125. codex close + list_sessions close → session absent from list
  S126. codex fork no pending_history  import then fork → fork state.pending_history is null
  S127. OR set_model wire         set_model → new model appears in /chat/completions "model" field
  S128. xai fork + list_children  fork → list_children returns fork_id
  S129. xai fork cwd              fork with cwd=dir_b → write_file creates file in dir_b
  S130. OR str_replace 0 occ      old_str absent → error containing "0 occurrences"
  S131. OR write nested dirs      write_file "a/b/s131.txt" → parent dirs created on disk
  S132. OR fork cwd               fork with cwd=dir_b → write_file creates file in dir_b
  S133. xai resume no cwd update  resume with different cwd → file tools still use original cwd
  S134. acp fork + list_children  fork → list_children returns fork_id

Usage:
    NATS_URL=nats://localhost:4333 python3 smoke_test_programming12.py
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
BASE           = f"prog12.{os.getpid()}"
MOCK_OR_PORT   = 19838
MOCK_XAI_PORT  = 19840
MOCK_ACP_PORT  = 19842
PROMPT_TIMEOUT = 45.0
NATS_TIMEOUT   = 12.0
RUNNER_READY   = 14.0

green, red, yellow, cyan, reset = "\033[32m", "\033[31m", "\033[33m", "\033[36m", "\033[0m"
PASS = 0
FAIL = 0
procs: dict = {}

BIN_ACP   = os.path.join(RSDIR, "target/debug/trogon-acp-runner")
BIN_OR    = os.path.join(RSDIR, "target/debug/trogon-openrouter-runner")
BIN_XAI   = os.path.join(RSDIR, "target/debug/trogon-xai-runner")
BIN_CODEX = os.path.join(RSDIR, "target/debug/trogon-codex-runner")
BIN_MOCK  = os.path.join(RSDIR, "target/debug/mock_codex_server")


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

def acp_sse(text: str) -> bytes:
    chunks = [
        "event: message_start\ndata: " + json.dumps({
            "type": "message_start",
            "message": {"id": "msg-s12", "type": "message", "role": "assistant",
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
            "delta": {"stop_reason": "end_turn", "stop_sequence": None},
            "usage": {"output_tokens": 5},
        }) + "\n\n",
        "event: message_stop\ndata: " + json.dumps({"type": "message_stop"}) + "\n\n",
    ]
    return "".join(chunks).encode()


def acp_sse_tool(call_id: str, tool_name: str, tool_input_json: str) -> bytes:
    chunks = [
        "event: message_start\ndata: " + json.dumps({
            "type": "message_start",
            "message": {"id": "msg-tool-s12", "type": "message", "role": "assistant",
                        "content": [], "model": "claude-sonnet-4-6", "stop_reason": None,
                        "usage": {"input_tokens": 10, "output_tokens": 0}},
        }) + "\n\n",
        "event: content_block_start\ndata: " + json.dumps({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "tool_use", "id": call_id,
                              "name": tool_name, "input": {}},
        }) + "\n\n",
        "event: content_block_delta\ndata: " + json.dumps({
            "type": "content_block_delta", "index": 0,
            "delta": {"type": "input_json_delta", "partial_json": tool_input_json},
        }) + "\n\n",
        "event: content_block_stop\ndata: " + json.dumps(
            {"type": "content_block_stop", "index": 0}) + "\n\n",
        "event: message_delta\ndata: " + json.dumps({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use"},
            "usage": {"output_tokens": 5},
        }) + "\n\n",
        "event: message_stop\ndata: " + json.dumps({"type": "message_stop"}) + "\n\n",
    ]
    return "".join(chunks).encode()


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


def or_sse_tool(call_id: str, fn_name: str, arguments_str: str) -> bytes:
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


def xai_sse(text: str, resp_id: str = "resp-s12") -> bytes:
    chunks = [
        "data: " + json.dumps({"id": resp_id, "type": "message.delta",
                               "delta": {"text": text}}) + "\n\n",
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


# ── Mock servers ──────────────────────────────────────────────────────────────

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


def start_mock(port: int, handler_cls):
    s = HTTPServer(("127.0.0.1", port), handler_cls)
    threading.Thread(target=s.serve_forever, daemon=True).start()
    return s


# ── Runner factories ──────────────────────────────────────────────────────────

def acp_env(prefix: str) -> dict:
    return {
        "ACP_PREFIX":         prefix,
        "ANTHROPIC_BASE_URL": f"http://127.0.0.1:{MOCK_ACP_PORT}",
        "ANTHROPIC_API_KEY":  "fake-acp-key",
    }


def or_env(prefix: str, model: str = "anthropic/claude-sonnet-4-6", **extra) -> dict:
    env = {
        "ACP_PREFIX":               prefix,
        "OPENROUTER_BASE_URL":      f"http://127.0.0.1:{MOCK_OR_PORT}",
        "OPENROUTER_API_KEY":       "fake-or-key",
        "OPENROUTER_DEFAULT_MODEL": model,
        "OPENROUTER_MODELS": (
            f"{model}:Default Model,"
            "openai/gpt-4o:GPT-4o,"
            "anthropic/claude-sonnet-4-6:Claude Sonnet 4.6"
        ),
    }
    env.update(extra)
    return env


def xai_env(prefix: str, model: str = "grok-3", **extra) -> dict:
    env = {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_XAI_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": model,
        "XAI_MODELS":        f"{model}:{model},grok-3-mini:Grok-3 Mini,grok-2:Grok-2",
    }
    env.update(extra)
    return env


def codex_env(prefix: str, model: str = "o4-mini", **extra) -> dict:
    env = {
        "ACP_PREFIX":          prefix,
        "CODEX_BIN":           BIN_MOCK,
        "CODEX_DEFAULT_MODEL": model,
        "CODEX_MODELS":        f"{model}:{model},o3:o3,o4-mini:o4-mini",
        "OPENAI_API_KEY":      "fake-openai-key",
    }
    env.update(extra)
    return env


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


async def _js_session_cmd(nc, subject: str, payload: dict, timeout: float = 12.0) -> dict:
    """Fork/close/resume use pub-sub with X-Req-Id header (same pattern as set_model)."""
    parts = subject.split(".")
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
        try:
            await sub.unsubscribe()
        except Exception:
            pass


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


async def list_sessions(nc, prefix: str) -> dict:
    return await nats_req(nc, f"{prefix}.agent.session.list", {})


def _acp_tool_result(calls: list, call_index: int) -> str:
    """Extract the tool_result text from the acp API call at call_index."""
    msgs = calls[call_index].get("messages", [])
    result = ""
    for msg in msgs:
        content = msg.get("content", [])
        if isinstance(content, list):
            for block in content:
                if isinstance(block, dict) and block.get("type") == "tool_result":
                    raw = block.get("content", "")
                    if isinstance(raw, str):
                        result += raw
                    elif isinstance(raw, list):
                        for sub in raw:
                            if isinstance(sub, dict) and sub.get("type") == "text":
                                result += sub.get("text", "")
                            elif isinstance(sub, str):
                                result += sub
    return result


def _or_tool_result(calls: list, call_index: int, tool_index: int = 0) -> str:
    """Extract the tool message content from OR API call at call_index."""
    msgs = calls[call_index].get("messages", [])
    tool_msgs = [m for m in msgs if m.get("role") == "tool"]
    if tool_index < len(tool_msgs):
        return str(tool_msgs[tool_index].get("content", ""))
    return ""


def _xai_fn_output(calls: list, call_index: int) -> str:
    """Extract function_call_output output from xai API call at call_index."""
    items = calls[call_index].get("input", [])
    for item in items:
        if isinstance(item, dict) and item.get("type") == "function_call_output":
            return str(item.get("output", ""))
    return ""


# ═══════════════════════════════════════════════════════════════════════════════
# S119 — acp path traversal: read_file "../../etc/passwd" → error
# ═══════════════════════════════════════════════════════════════════════════════

async def s119_acp_path_traversal(nc):
    section("S119: acp path traversal — read_file outside cwd → error")
    prefix = f"{BASE}.acp_s119"
    MockACP.reset()

    tmp_dir = tempfile.mkdtemp(prefix="smoke12_s119_")
    try:
        c1 = "acp-c1-s119"
        MockACP.push(acp_sse_tool(c1, "read_file",
                                  json.dumps({"path": "../../etc/passwd"})))
        MockACP.push(acp_sse("traversal rejected s119"))

        start_runner("s119", BIN_ACP, acp_env(prefix))
        try:
            sid = await wait_runner(nc, prefix, cwd=tmp_dir)
            if not sid:
                fail("S119: session.new timed out"); return

            await send_set_mode(nc, prefix, sid, "bypassPermissions")
            resp = await send_prompt(nc, prefix, sid, "read outside cwd s119")
            if prompt_err(resp):
                fail("S119: prompt failed", str(resp)[:80]); return
            ok("S119: prompt completed")

            calls = MockACP.requests()
            if len(calls) < 2:
                fail("S119: expected 2 acp API calls", f"got {len(calls)}"); return

            result = _acp_tool_result(calls, 1)
            info(f"S119: tool result = {result[:100]!r}")
            if "path is outside the working directory" in result:
                ok("S119: read_file ../../etc/passwd → 'path is outside the working directory'")
            else:
                fail("S119: expected path traversal error", result[:100])

        finally:
            stop_runner("s119")
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


# ═══════════════════════════════════════════════════════════════════════════════
# S120 — acp write_file nested dirs: write_file "sub/deep/s120.txt" → dirs created
# ═══════════════════════════════════════════════════════════════════════════════

async def s120_acp_write_nested_dirs(nc):
    section("S120: acp write_file nested dirs — parent directories auto-created")
    prefix = f"{BASE}.acp_s120"
    MockACP.reset()

    tmp_dir = tempfile.mkdtemp(prefix="smoke12_s120_")
    try:
        c1 = "acp-c1-s120"
        nested_content = "nested file content s120"
        MockACP.push(acp_sse_tool(c1, "write_file",
                                  json.dumps({"path": "sub/deep/s120.txt",
                                              "content": nested_content})))
        MockACP.push(acp_sse("nested write done s120"))

        start_runner("s120", BIN_ACP, acp_env(prefix))
        try:
            sid = await wait_runner(nc, prefix, cwd=tmp_dir)
            if not sid:
                fail("S120: session.new timed out"); return

            await send_set_mode(nc, prefix, sid, "bypassPermissions")
            resp = await send_prompt(nc, prefix, sid, "write nested file s120")
            if prompt_err(resp):
                fail("S120: prompt failed", str(resp)[:80]); return
            ok("S120: prompt completed")

            expected = os.path.join(tmp_dir, "sub", "deep", "s120.txt")
            if os.path.exists(expected):
                ok("S120: nested file created on disk (parent dirs auto-created)")
                with open(expected) as f:
                    content = f.read()
                if content == nested_content:
                    ok("S120: nested file content correct")
                else:
                    fail("S120: nested file content wrong", content[:80])
            else:
                fail("S120: nested file not found on disk")

            # write_file result should be "OK"
            calls = MockACP.requests()
            if len(calls) >= 2:
                result = _acp_tool_result(calls, 1)
                if result.strip() == "OK":
                    ok("S120: write_file tool result = 'OK'")
                else:
                    fail("S120: write_file should return 'OK'", result[:60])

        finally:
            stop_runner("s120")
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


# ═══════════════════════════════════════════════════════════════════════════════
# S121 — acp read_file offset+limit: only lines 3-5 returned (numbered)
# ═══════════════════════════════════════════════════════════════════════════════

async def s121_acp_read_offset_limit(nc):
    section("S121: acp read_file offset+limit — lines 3-5 returned with line numbers")
    prefix = f"{BASE}.acp_s121"
    MockACP.reset()

    tmp_dir = tempfile.mkdtemp(prefix="smoke12_s121_")
    try:
        # Create file with 8 numbered lines
        test_file = os.path.join(tmp_dir, "multiline_s121.txt")
        with open(test_file, "w") as f:
            for i in range(1, 9):
                f.write(f"line{i}\n")

        c1 = "acp-c1-s121"
        # offset=2 (0-based) → starts at index 2 → line 3; limit=3 → 3 lines (3, 4, 5)
        MockACP.push(acp_sse_tool(c1, "read_file",
                                  json.dumps({"path": "multiline_s121.txt",
                                              "offset": 2, "limit": 3})))
        MockACP.push(acp_sse("partial read done s121"))

        start_runner("s121", BIN_ACP, acp_env(prefix))
        try:
            sid = await wait_runner(nc, prefix, cwd=tmp_dir)
            if not sid:
                fail("S121: session.new timed out"); return

            await send_set_mode(nc, prefix, sid, "bypassPermissions")
            resp = await send_prompt(nc, prefix, sid, "read lines 3-5 s121")
            if prompt_err(resp):
                fail("S121: prompt failed", str(resp)[:80]); return
            ok("S121: prompt completed")

            calls = MockACP.requests()
            if len(calls) < 2:
                fail("S121: expected 2 acp API calls", f"got {len(calls)}"); return

            result = _acp_tool_result(calls, 1)
            info(f"S121: read_file result = {result!r}")

            # Lines 3, 4, 5 should appear with their line numbers
            if "line3" in result and "line4" in result and "line5" in result:
                ok("S121: result contains lines 3, 4, 5")
            else:
                fail("S121: result should contain line3, line4, line5", result[:100])

            # Lines 1, 2, 6 should NOT appear
            if "line1" not in result and "line2" not in result and "line6" not in result:
                ok("S121: result does NOT contain lines 1, 2, or 6 (offset+limit respected)")
            else:
                fail("S121: result has lines outside offset+limit range", result[:100])

            # Line numbers should be present (format: "     3\tline3")
            if "     3\t" in result or "3\tline3" in result:
                ok("S121: line numbers present in output (line 3)")
            else:
                fail("S121: expected line number '3' in output", result[:100])

        finally:
            stop_runner("s121")
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


# ═══════════════════════════════════════════════════════════════════════════════
# S122 — acp str_replace 0 occurrences: specific error message
# ═══════════════════════════════════════════════════════════════════════════════

async def s122_acp_str_replace_zero_occ(nc):
    section("S122: acp str_replace 0 occurrences → 'not found in file (0 occurrences)'")
    prefix = f"{BASE}.acp_s122"
    MockACP.reset()

    tmp_dir = tempfile.mkdtemp(prefix="smoke12_s122_")
    try:
        test_file = os.path.join(tmp_dir, "hello_s122.txt")
        with open(test_file, "w") as f:
            f.write("hello world s122")

        c1 = "acp-c1-s122"
        MockACP.push(acp_sse_tool(c1, "str_replace",
                                  json.dumps({"path": "hello_s122.txt",
                                              "old_str": "NOTPRESENT_XYZ_s122",
                                              "new_str": "anything"})))
        MockACP.push(acp_sse("str_replace error handled s122"))

        start_runner("s122", BIN_ACP, acp_env(prefix))
        try:
            sid = await wait_runner(nc, prefix, cwd=tmp_dir)
            if not sid:
                fail("S122: session.new timed out"); return

            await send_set_mode(nc, prefix, sid, "bypassPermissions")
            resp = await send_prompt(nc, prefix, sid, "replace absent string s122")
            if prompt_err(resp):
                fail("S122: prompt failed", str(resp)[:80]); return
            ok("S122: prompt completed")

            calls = MockACP.requests()
            if len(calls) < 2:
                fail("S122: expected 2 acp API calls", f"got {len(calls)}"); return

            result = _acp_tool_result(calls, 1)
            info(f"S122: str_replace result = {result!r}")
            if "0 occurrences" in result:
                ok("S122: str_replace with absent old_str → '0 occurrences' error")
            else:
                fail("S122: expected '0 occurrences' in error", result[:100])

            if "not found" in result.lower() or "Error:" in result:
                ok("S122: error prefix present")
            else:
                fail("S122: expected 'Error:' or 'not found'", result[:100])

        finally:
            stop_runner("s122")
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


# ═══════════════════════════════════════════════════════════════════════════════
# S123 — acp str_replace 2 occurrences: "not unique — found 2 occurrences"
# ═══════════════════════════════════════════════════════════════════════════════

async def s123_acp_str_replace_multi_occ(nc):
    section("S123: acp str_replace 2 occurrences → 'not unique — found 2 occurrences'")
    prefix = f"{BASE}.acp_s123"
    MockACP.reset()

    tmp_dir = tempfile.mkdtemp(prefix="smoke12_s123_")
    try:
        test_file = os.path.join(tmp_dir, "dupe_s123.txt")
        with open(test_file, "w") as f:
            f.write("abc\nabc\n")  # two occurrences of "abc"

        c1 = "acp-c1-s123"
        MockACP.push(acp_sse_tool(c1, "str_replace",
                                  json.dumps({"path": "dupe_s123.txt",
                                              "old_str": "abc",
                                              "new_str": "xyz"})))
        MockACP.push(acp_sse("str_replace non-unique handled s123"))

        start_runner("s123", BIN_ACP, acp_env(prefix))
        try:
            sid = await wait_runner(nc, prefix, cwd=tmp_dir)
            if not sid:
                fail("S123: session.new timed out"); return

            await send_set_mode(nc, prefix, sid, "bypassPermissions")
            resp = await send_prompt(nc, prefix, sid, "replace non-unique string s123")
            if prompt_err(resp):
                fail("S123: prompt failed", str(resp)[:80]); return
            ok("S123: prompt completed")

            calls = MockACP.requests()
            if len(calls) < 2:
                fail("S123: expected 2 acp API calls", f"got {len(calls)}"); return

            result = _acp_tool_result(calls, 1)
            info(f"S123: str_replace result = {result!r}")
            if "not unique" in result or "is not unique" in result:
                ok("S123: str_replace with 2 occurrences → 'not unique' error")
            else:
                fail("S123: expected 'not unique' in error", result[:100])

            if "2 occurrences" in result:
                ok("S123: error message includes '2 occurrences'")
            else:
                fail("S123: expected '2 occurrences' in error", result[:100])

            # File should be unchanged
            with open(test_file) as f:
                content = f.read()
            if "abc" in content and "xyz" not in content:
                ok("S123: file not modified when str_replace fails")
            else:
                fail("S123: file was modified despite str_replace error", content[:60])

        finally:
            stop_runner("s123")
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


# ═══════════════════════════════════════════════════════════════════════════════
# S124 — codex fork + list_children: fork → list_children returns fork_id
# ═══════════════════════════════════════════════════════════════════════════════

async def s124_codex_fork_list_children(nc):
    section("S124: codex fork + list_children — fork_id appears in children list")
    prefix = f"{BASE}.cx_s124"

    start_runner("s124", BIN_CODEX, codex_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S124: session.new timed out"); return

        fork_resp = await fork_session(nc, prefix, sid, cwd="/tmp")
        if "_error" in fork_resp or "code" in fork_resp:
            fail("S124: fork_session failed", str(fork_resp)[:80]); return

        fork_id = fork_resp.get("sessionId")
        if not fork_id:
            fail("S124: fork_session did not return sessionId", str(fork_resp)[:80]); return
        ok(f"S124: fork_session returned fork_id={fork_id[:8]}...")

        children_resp = await nats_req(
            nc, f"{prefix}.agent.ext.session/list_children",
            {"sessionId": sid},
        )
        if "_error" in children_resp or "code" in children_resp:
            fail("S124: list_children failed", str(children_resp)[:80]); return

        children = children_resp.get("children", [])
        info(f"S124: children of parent = {children}")
        if fork_id in children:
            ok("S124: list_children includes fork_id")
        else:
            fail("S124: fork_id not in list_children", f"children={children}, fork={fork_id[:8]}")

        # Parent should not be in its own children list
        if sid not in children:
            ok("S124: parent session not in its own children list")
        else:
            fail("S124: parent should not list itself as a child")

    finally:
        stop_runner("s124")


# ═══════════════════════════════════════════════════════════════════════════════
# S125 — codex close + list_sessions: close → session absent from list
# ═══════════════════════════════════════════════════════════════════════════════

async def s125_codex_close_list_sessions(nc):
    section("S125: codex close + list_sessions — closed session not in list")
    prefix = f"{BASE}.cx_s125"

    start_runner("s125", BIN_CODEX, codex_env(prefix))
    try:
        sid1 = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid1:
            fail("S125: session 1 timed out"); return

        sid2 = await new_session(nc, prefix, cwd="/tmp")
        if not sid2:
            fail("S125: session 2 failed"); return

        r_list1 = await list_sessions(nc, prefix)
        sessions1 = [s.get("sessionId") for s in r_list1.get("sessions", [])]
        info(f"S125: sessions before close = {[s[:8] for s in sessions1]}")
        if sid1 in sessions1 and sid2 in sessions1:
            ok("S125: both sessions present before close")
        else:
            fail("S125: expected both sessions in list", str(sessions1)[:80])

        close_resp = await close_session(nc, prefix, sid2)
        if "_error" in close_resp or "code" in close_resp:
            fail("S125: close_session failed", str(close_resp)[:80]); return
        ok("S125: close_session succeeded")

        r_list2 = await list_sessions(nc, prefix)
        sessions2 = [s.get("sessionId") for s in r_list2.get("sessions", [])]
        info(f"S125: sessions after close = {[s[:8] for s in sessions2]}")
        if sid2 not in sessions2:
            ok("S125: closed session not in list_sessions")
        else:
            fail("S125: closed session still in list_sessions")

        if sid1 in sessions2:
            ok("S125: non-closed session still in list_sessions")
        else:
            fail("S125: non-closed session was removed")

    finally:
        stop_runner("s125")


# ═══════════════════════════════════════════════════════════════════════════════
# S126 — codex fork doesn't copy pending_history: import then fork → fork state null
# ═══════════════════════════════════════════════════════════════════════════════

async def s126_codex_fork_no_pending_history(nc):
    section("S126: codex fork doesn't copy pending_history — fork state has null pending")
    prefix = f"{BASE}.cx_s126"

    start_runner("s126", BIN_CODEX, codex_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S126: session.new timed out"); return

        # Import portable messages → sets pending_history on the source session
        portable = [
            {"role": "user",      "text": "hello s126", "blocks": [{"type": "text", "text": "hello s126"}]},
            {"role": "assistant", "text": "hi s126",    "blocks": [{"type": "text", "text": "hi s126"}]},
        ]
        import_resp = await nats_req(
            nc, f"{prefix}.agent.ext.session/import",
            {"sessionId": sid, "messages": portable},
        )
        if "_error" in import_resp or "code" in import_resp:
            fail("S126: import failed", str(import_resp)[:80]); return
        ok("S126: import succeeded (pending_history set on source)")

        # Verify source has pending_history after import
        src_state = await nats_req(
            nc, f"{prefix}.agent.ext.session/get_state",
            {"sessionId": sid},
        )
        src_pending = src_state.get("pending_history")
        if src_pending is not None:
            ok(f"S126: source session has pending_history ({len(src_pending)} messages)")
        else:
            fail("S126: source should have pending_history after import")

        # Fork the session
        fork_resp = await fork_session(nc, prefix, sid, cwd="/tmp")
        if "_error" in fork_resp or "code" in fork_resp:
            fail("S126: fork_session failed", str(fork_resp)[:80]); return

        fork_id = fork_resp.get("sessionId")
        if not fork_id:
            fail("S126: fork_session did not return sessionId"); return
        ok(f"S126: fork_session returned fork_id={fork_id[:8]}...")

        # Get state of fork — pending_history should be null
        fork_state = await nats_req(
            nc, f"{prefix}.agent.ext.session/get_state",
            {"sessionId": fork_id},
        )
        if "_error" in fork_state or "code" in fork_state:
            fail("S126: get_state of fork failed", str(fork_state)[:80]); return

        fork_pending = fork_state.get("pending_history")
        if fork_pending is None:
            ok("S126: forked session has pending_history=null (not copied from source)")
        else:
            fail("S126: fork should NOT copy pending_history",
                 f"got {len(fork_pending)} messages")

        # Fork should start with first_turn=true
        fork_first_turn = fork_state.get("first_turn", False)
        if fork_first_turn:
            ok("S126: forked session has first_turn=true")
        else:
            fail("S126: forked session should have first_turn=true")

    finally:
        stop_runner("s126")


# ═══════════════════════════════════════════════════════════════════════════════
# S127 — OR set_model → model in /chat/completions wire request
# ═══════════════════════════════════════════════════════════════════════════════

async def s127_or_set_model_wire(nc):
    section("S127: OR set_model → new model appears in /chat/completions 'model' field")
    prefix = f"{BASE}.or_s127"
    MockOR.reset()

    start_runner("s127", BIN_OR,
                 or_env(prefix, model="anthropic/claude-sonnet-4-6"))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S127: session.new timed out"); return

        # Switch model to gpt-4o (in models list)
        sm = await send_set_model(nc, prefix, sid, "openai/gpt-4o")
        if "code" in sm or "_error" in sm:
            fail("S127: set_model failed", str(sm)[:80]); return
        ok("S127: set_model 'openai/gpt-4o' → OK")

        # Queue response and send prompt
        MockOR.push(or_sse("response after set_model s127"))
        resp = await send_prompt(nc, prefix, sid, "prompt after set_model s127")
        if prompt_err(resp):
            fail("S127: prompt failed", str(resp)[:80]); return
        ok("S127: prompt completed")

        calls = MockOR.requests()
        if not calls:
            fail("S127: no OR API calls captured"); return

        model_in_request = calls[0].get("model")
        info(f"S127: model in /chat/completions = {model_in_request!r}")
        if model_in_request == "openai/gpt-4o":
            ok("S127: new model 'openai/gpt-4o' used in /chat/completions request")
        else:
            fail("S127: expected 'openai/gpt-4o' in request", f"got {model_in_request!r}")

    finally:
        stop_runner("s127")


# ═══════════════════════════════════════════════════════════════════════════════
# S128 — xai fork + list_children: fork → list_children returns fork_id
# ═══════════════════════════════════════════════════════════════════════════════

async def s128_xai_fork_list_children(nc):
    section("S128: xai fork + list_children — fork_id appears in children list")
    prefix = f"{BASE}.xai_s128"
    MockXAI.reset()

    start_runner("s128", BIN_XAI, xai_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S128: session.new timed out"); return

        # Send a prompt first to create some history
        MockXAI.push(xai_sse("response before fork s128", "resp-s128-pre"))
        r = await send_prompt(nc, prefix, sid, "prompt before fork s128")
        if prompt_err(r):
            fail("S128: pre-fork prompt failed", str(r)[:80]); return

        fork_resp = await fork_session(nc, prefix, sid, cwd="/tmp")
        if "_error" in fork_resp or "code" in fork_resp:
            fail("S128: fork_session failed", str(fork_resp)[:80]); return

        fork_id = fork_resp.get("sessionId")
        if not fork_id:
            fail("S128: fork_session did not return sessionId"); return
        ok(f"S128: fork_session returned fork_id={fork_id[:8]}...")

        children_resp = await nats_req(
            nc, f"{prefix}.agent.ext.session/list_children",
            {"sessionId": sid},
        )
        if "_error" in children_resp or "code" in children_resp:
            fail("S128: list_children failed", str(children_resp)[:80]); return

        children = children_resp.get("children", [])
        info(f"S128: children of parent = {children}")
        if fork_id in children:
            ok("S128: list_children includes fork_id")
        else:
            fail("S128: fork_id not in list_children",
                 f"children={children}, fork={fork_id[:8]}")

        # Fork's own children should be empty
        fork_children_resp = await nats_req(
            nc, f"{prefix}.agent.ext.session/list_children",
            {"sessionId": fork_id},
        )
        fork_children = fork_children_resp.get("children", [])
        if len(fork_children) == 0:
            ok("S128: fork session has no children")
        else:
            fail("S128: fork should have no children", str(fork_children))

    finally:
        stop_runner("s128")


# ═══════════════════════════════════════════════════════════════════════════════
# S129 — xai fork cwd: fork with dir_b → write_file creates file in dir_b
# ═══════════════════════════════════════════════════════════════════════════════

async def s129_xai_fork_cwd(nc):
    section("S129: xai fork cwd — forked session uses fork's cwd for file ops")
    prefix = f"{BASE}.xai_s129"
    MockXAI.reset()

    dir_a = tempfile.mkdtemp(prefix="smoke12_s129a_")
    dir_b = tempfile.mkdtemp(prefix="smoke12_s129b_")
    try:
        # Create parent session with cwd=dir_a
        start_runner("s129", BIN_XAI, xai_env(prefix))
        try:
            sid = await wait_runner(nc, prefix, cwd=dir_a)
            if not sid:
                fail("S129: session.new timed out"); return

            # Fork with cwd=dir_b
            fork_resp = await fork_session(nc, prefix, sid, cwd=dir_b)
            if "_error" in fork_resp or "code" in fork_resp:
                fail("S129: fork_session failed", str(fork_resp)[:80]); return

            fork_id = fork_resp.get("sessionId")
            if not fork_id:
                fail("S129: fork_session did not return sessionId"); return
            ok(f"S129: fork created with cwd=dir_b")

            # Queue: write_file in fork session
            r1 = "resp-s129-fn"
            c1 = "call-s129-write"
            MockXAI.push(xai_sse_fn(r1, c1, "write_file",
                                    json.dumps({"path": "fork_file_s129.txt",
                                                "content": "written in fork cwd s129"})))
            MockXAI.push(xai_sse("fork write done s129", "resp-s129-final"))

            resp = await send_prompt(nc, prefix, fork_id, "write file in fork s129")
            if prompt_err(resp):
                fail("S129: fork prompt failed", str(resp)[:80]); return
            ok("S129: fork prompt completed")

            # File should be in dir_b, not dir_a
            file_b = os.path.join(dir_b, "fork_file_s129.txt")
            file_a = os.path.join(dir_a, "fork_file_s129.txt")
            if os.path.exists(file_b):
                ok("S129: write_file created file in fork's cwd (dir_b)")
                with open(file_b) as f:
                    content = f.read()
                if content == "written in fork cwd s129":
                    ok("S129: fork file content correct")
                else:
                    fail("S129: fork file content wrong", content[:60])
            else:
                fail("S129: file not found in fork cwd (dir_b)")

            if not os.path.exists(file_a):
                ok("S129: file was NOT created in source session cwd (dir_a)")
            else:
                fail("S129: file unexpectedly created in source session cwd (dir_a)")

        finally:
            stop_runner("s129")
    finally:
        shutil.rmtree(dir_a, ignore_errors=True)
        shutil.rmtree(dir_b, ignore_errors=True)


# ═══════════════════════════════════════════════════════════════════════════════
# S130 — OR str_replace 0 occurrences: error in tool result
# ═══════════════════════════════════════════════════════════════════════════════

async def s130_or_str_replace_zero_occ(nc):
    section("S130: OR str_replace 0 occurrences → '0 occurrences' error in tool result")
    prefix = f"{BASE}.or_s130"
    MockOR.reset()

    tmp_dir = tempfile.mkdtemp(prefix="smoke12_s130_")
    try:
        test_file = os.path.join(tmp_dir, "content_s130.txt")
        with open(test_file, "w") as f:
            f.write("hello world for or s130")

        c1 = "or-c1-s130"
        MockOR.push(or_sse_tool(c1, "str_replace",
                                json.dumps({"path": "content_s130.txt",
                                            "old_str": "NOTPRESENT_OR_s130",
                                            "new_str": "replaced"})))
        MockOR.push(or_sse("or str_replace error handled s130"))

        start_runner("s130", BIN_OR, or_env(prefix))
        try:
            sid = await wait_runner(nc, prefix, cwd=tmp_dir)
            if not sid:
                fail("S130: session.new timed out"); return

            resp = await send_prompt(nc, prefix, sid, "replace absent string in or s130")
            if prompt_err(resp):
                fail("S130: prompt failed", str(resp)[:80]); return
            ok("S130: prompt completed")

            calls = MockOR.requests()
            if len(calls) < 2:
                fail("S130: expected 2 OR API calls", f"got {len(calls)}"); return

            result = _or_tool_result(calls, 1)
            info(f"S130: str_replace tool result = {result[:100]!r}")
            if "0 occurrences" in result:
                ok("S130: OR str_replace absent old_str → '0 occurrences' error")
            else:
                fail("S130: expected '0 occurrences' in OR str_replace error", result[:100])

        finally:
            stop_runner("s130")
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


# ═══════════════════════════════════════════════════════════════════════════════
# S131 — OR write_file nested dirs: file created with parent dirs
# ═══════════════════════════════════════════════════════════════════════════════

async def s131_or_write_nested_dirs(nc):
    section("S131: OR write_file nested dirs — parent directories auto-created")
    prefix = f"{BASE}.or_s131"
    MockOR.reset()

    tmp_dir = tempfile.mkdtemp(prefix="smoke12_s131_")
    try:
        c1 = "or-c1-s131"
        nested_content = "nested or content s131"
        MockOR.push(or_sse_tool(c1, "write_file",
                                json.dumps({"path": "a/b/s131.txt",
                                            "content": nested_content})))
        MockOR.push(or_sse("or nested write done s131"))

        start_runner("s131", BIN_OR, or_env(prefix))
        try:
            sid = await wait_runner(nc, prefix, cwd=tmp_dir)
            if not sid:
                fail("S131: session.new timed out"); return

            resp = await send_prompt(nc, prefix, sid, "write nested file or s131")
            if prompt_err(resp):
                fail("S131: prompt failed", str(resp)[:80]); return
            ok("S131: prompt completed")

            expected = os.path.join(tmp_dir, "a", "b", "s131.txt")
            if os.path.exists(expected):
                ok("S131: nested file created on disk (OR auto-created parent dirs)")
                with open(expected) as f:
                    content = f.read()
                if content == nested_content:
                    ok("S131: nested file content correct")
                else:
                    fail("S131: nested file content wrong", content[:60])
            else:
                fail("S131: nested file not found on disk")

            # write_file OR result should be "OK" in tool message
            calls = MockOR.requests()
            if len(calls) >= 2:
                result = _or_tool_result(calls, 1)
                if result.strip() == "OK":
                    ok("S131: OR write_file tool result = 'OK'")
                else:
                    fail("S131: OR write_file should return 'OK'", result[:60])

        finally:
            stop_runner("s131")
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


# ═══════════════════════════════════════════════════════════════════════════════
# S132 — OR fork cwd: fork with dir_b → file ops use fork's cwd
# ═══════════════════════════════════════════════════════════════════════════════

async def s132_or_fork_cwd(nc):
    section("S132: OR fork cwd — forked session uses fork's cwd for file ops")
    prefix = f"{BASE}.or_s132"
    MockOR.reset()

    dir_a = tempfile.mkdtemp(prefix="smoke12_s132a_")
    dir_b = tempfile.mkdtemp(prefix="smoke12_s132b_")
    try:
        start_runner("s132", BIN_OR, or_env(prefix))
        try:
            sid = await wait_runner(nc, prefix, cwd=dir_a)
            if not sid:
                fail("S132: session.new timed out"); return

            # Fork with cwd=dir_b
            fork_resp = await fork_session(nc, prefix, sid, cwd=dir_b)
            if "_error" in fork_resp or "code" in fork_resp:
                fail("S132: fork_session failed", str(fork_resp)[:80]); return

            fork_id = fork_resp.get("sessionId")
            if not fork_id:
                fail("S132: fork_session did not return sessionId"); return
            ok(f"S132: OR fork created with cwd=dir_b")

            # Queue: write_file in fork session
            c1 = "or-c1-s132"
            MockOR.push(or_sse_tool(c1, "write_file",
                                    json.dumps({"path": "fork_file_or_s132.txt",
                                                "content": "written in or fork cwd s132"})))
            MockOR.push(or_sse("or fork write done s132"))

            resp = await send_prompt(nc, prefix, fork_id, "write file in or fork s132")
            if prompt_err(resp):
                fail("S132: fork prompt failed", str(resp)[:80]); return
            ok("S132: OR fork prompt completed")

            # File should be in dir_b
            file_b = os.path.join(dir_b, "fork_file_or_s132.txt")
            file_a = os.path.join(dir_a, "fork_file_or_s132.txt")
            if os.path.exists(file_b):
                ok("S132: OR write_file created file in fork's cwd (dir_b)")
                with open(file_b) as f:
                    content = f.read()
                if content == "written in or fork cwd s132":
                    ok("S132: OR fork file content correct")
                else:
                    fail("S132: OR fork file content wrong", content[:60])
            else:
                fail("S132: file not found in OR fork cwd (dir_b)")

            if not os.path.exists(file_a):
                ok("S132: file was NOT created in source session cwd (dir_a)")
            else:
                fail("S132: file unexpectedly in source session cwd (dir_a)")

        finally:
            stop_runner("s132")
    finally:
        shutil.rmtree(dir_a, ignore_errors=True)
        shutil.rmtree(dir_b, ignore_errors=True)


# ═══════════════════════════════════════════════════════════════════════════════
# S133 — xai resume no cwd update: resume with different cwd → tools use original cwd
# ═══════════════════════════════════════════════════════════════════════════════

async def s133_xai_resume_no_cwd_update(nc):
    section("S133: xai resume no cwd update — resume with new cwd; tools still use original")
    prefix = f"{BASE}.xai_s133"
    MockXAI.reset()

    dir_a = tempfile.mkdtemp(prefix="smoke12_s133_")
    try:
        # Create a file in dir_a
        test_file = os.path.join(dir_a, "original_s133.txt")
        with open(test_file, "w") as f:
            f.write("content in original cwd s133")

        start_runner("s133", BIN_XAI, xai_env(prefix))
        try:
            # Create session with cwd=dir_a
            sid = await wait_runner(nc, prefix, cwd=dir_a)
            if not sid:
                fail("S133: session.new timed out"); return

            # Resume session with a completely different cwd
            # (xai resume_session ignores cwd — just verifies session exists)
            resume_resp = await resume_session(nc, prefix, sid, cwd="/tmp/nonexistent_s133_xyz")
            if "_error" in resume_resp or "code" in resume_resp:
                fail("S133: resume_session failed", str(resume_resp)[:80]); return
            ok("S133: resume_session with different cwd → OK")

            # Now send a prompt that reads the file from the original cwd
            r1 = "resp-s133-fn"
            c1 = "call-s133-read"
            MockXAI.push(xai_sse_fn(r1, c1, "read_file",
                                    json.dumps({"path": "original_s133.txt"})))
            MockXAI.push(xai_sse("read from original cwd s133", "resp-s133-final"))

            resp = await send_prompt(nc, prefix, sid, "read original_s133.txt s133")
            if prompt_err(resp):
                fail("S133: prompt after resume failed", str(resp)[:80]); return
            ok("S133: prompt after resume completed")

            calls = MockXAI.requests()
            if len(calls) < 2:
                fail("S133: expected 2 xai API calls", f"got {len(calls)}"); return

            # The 2nd API call's input should have a function_call_output with file content
            fn_output = _xai_fn_output(calls, 1)
            info(f"S133: read_file output = {fn_output[:80]!r}")
            if "content in original cwd s133" in fn_output:
                ok("S133: read_file used original cwd after resume (cwd not updated)")
            else:
                fail("S133: read_file should use original cwd", fn_output[:100])

        finally:
            stop_runner("s133")
    finally:
        shutil.rmtree(dir_a, ignore_errors=True)


# ═══════════════════════════════════════════════════════════════════════════════
# S134 — acp fork + list_children: fork → list_children returns fork_id
# ═══════════════════════════════════════════════════════════════════════════════

async def s134_acp_fork_list_children(nc):
    section("S134: acp fork + list_children — fork_id appears in children list")
    prefix = f"{BASE}.acp_s134"
    MockACP.reset()

    # Send a prompt first to establish history so export/import works
    MockACP.push(acp_sse("acp response before fork s134"))

    start_runner("s134", BIN_ACP, acp_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S134: session.new timed out"); return

        # Send one prompt to establish session state
        resp = await send_prompt(nc, prefix, sid, "prompt before fork s134")
        if prompt_err(resp):
            fail("S134: pre-fork prompt failed", str(resp)[:80]); return

        # Fork the session
        fork_resp = await fork_session(nc, prefix, sid, cwd="/tmp")
        if "_error" in fork_resp or "code" in fork_resp:
            fail("S134: fork_session failed", str(fork_resp)[:80]); return

        fork_id = fork_resp.get("sessionId")
        if not fork_id:
            fail("S134: fork_session did not return sessionId"); return
        ok(f"S134: acp fork_session returned fork_id={fork_id[:8]}...")

        # list_children of the parent
        children_resp = await nats_req(
            nc, f"{prefix}.agent.ext.session/list_children",
            {"sessionId": sid},
        )
        if "_error" in children_resp or "code" in children_resp:
            fail("S134: list_children failed", str(children_resp)[:80]); return

        children = children_resp.get("children", [])
        info(f"S134: acp children of parent = {children}")
        if fork_id in children:
            ok("S134: acp list_children includes fork_id")
        else:
            fail("S134: fork_id not in acp list_children",
                 f"children={children}, fork={fork_id[:8]}")

        # Second fork — two children
        fork_resp2 = await fork_session(nc, prefix, sid, cwd="/tmp")
        fork_id2 = fork_resp2.get("sessionId")
        if fork_id2:
            children_resp2 = await nats_req(
                nc, f"{prefix}.agent.ext.session/list_children",
                {"sessionId": sid},
            )
            children2 = children_resp2.get("children", [])
            if fork_id in children2 and fork_id2 in children2:
                ok("S134: acp list_children includes both forks after second fork")
            else:
                fail("S134: expected both forks in list_children", str(children2)[:80])
        else:
            info("S134: second fork failed, skipping two-child check")

    finally:
        stop_runner("s134")


# ═══════════════════════════════════════════════════════════════════════════════
# Main
# ═══════════════════════════════════════════════════════════════════════════════

async def main():
    print("=== smoke_test_programming12.py — file-tool precision, lifecycle, model-in-wire ===")
    print(f"  prefix : {BASE}")
    print(f"  nats   : {NATS_URL}")
    print(f"  mocks  : acp:{MOCK_ACP_PORT}  or:{MOCK_OR_PORT}  xai:{MOCK_XAI_PORT}")

    start_mock(MOCK_ACP_PORT, MockACP)
    start_mock(MOCK_OR_PORT, MockOR)
    start_mock(MOCK_XAI_PORT, MockXAI)
    info("mock servers started")

    nc = await nats_lib.connect(NATS_URL)
    try:
        await s119_acp_path_traversal(nc)
        await s120_acp_write_nested_dirs(nc)
        await s121_acp_read_offset_limit(nc)
        await s122_acp_str_replace_zero_occ(nc)
        await s123_acp_str_replace_multi_occ(nc)
        await s124_codex_fork_list_children(nc)
        await s125_codex_close_list_sessions(nc)
        await s126_codex_fork_no_pending_history(nc)
        await s127_or_set_model_wire(nc)
        await s128_xai_fork_list_children(nc)
        await s129_xai_fork_cwd(nc)
        await s130_or_str_replace_zero_occ(nc)
        await s131_or_write_nested_dirs(nc)
        await s132_or_fork_cwd(nc)
        await s133_xai_resume_no_cwd_update(nc)
        await s134_acp_fork_list_children(nc)
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
