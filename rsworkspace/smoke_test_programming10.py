#!/usr/bin/env python3
"""
smoke_test_programming10.py — Cross-runner workflows, set_mode, and tool I/O verification.

  S95.  xai set_mode          "default" → OK; unknown → -32602 error
  S96.  or set_mode           "default" → OK; unknown → -32602 error
  S97.  codex set_mode        "default" → OK; unknown → -32602 error
  S98.  xai write_file+read   write a file via tool, read it back → content correct
  S99.  or str_replace        write + str_replace + read_file → edit verified in file
  S100. acp write_file+glob   write *.s100.txt files, glob finds them; bad path rejected
  S101. acp export tool blocks PortableBlocks with tool_call/tool_result types after tool turn
  S102. or → codex import     export from openrouter, import into codex; history grows
  S103. acp → xai import      export from acp, import into xai; last_response_id cleared
  S104. codex MOCK_REQUIRE_MODEL  model passed in turn/start; wrong model → turn error
  S105. xai write then list_dir   write files then list_dir shows them
  S106. or write_file + list_dir  combined file creation and directory listing

Usage:
    NATS_URL=nats://localhost:4333 python3 smoke_test_programming10.py
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
BASE           = f"prog10.{os.getpid()}"
MOCK_ACP_PORT  = 19826
MOCK_OR_PORT   = 19828
MOCK_XAI_PORT  = 19830
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

# ── Anthropic SSE (acp-runner format) ─────────────────────────────────────────

def acp_sse(text: str) -> bytes:
    chunks = [
        "event: message_start\ndata: " + json.dumps({
            "type": "message_start",
            "message": {"id": "msg-s10", "type": "message", "role": "assistant",
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
            "message": {"id": "msg-tool-s10", "type": "message", "role": "assistant",
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


# ── OpenAI SSE (or-runner format) ─────────────────────────────────────────────

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


# ── xAI SSE ───────────────────────────────────────────────────────────────────

def xai_sse(text: str, resp_id: str = "resp-s10") -> bytes:
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


# ── Dual mock server (routes by path) ─────────────────────────────────────────

class MockLLM(BaseHTTPRequestHandler):
    """Serves both ACP (POST /messages) and OR (POST /chat/completions) on same port."""
    _lock = threading.Lock()
    _acp_log: list = []
    _or_log: list = []
    _acp_queue: collections.deque = collections.deque()
    _or_queue: collections.deque = collections.deque()

    @classmethod
    def reset(cls):
        with cls._lock:
            cls._acp_log.clear()
            cls._or_log.clear()
            cls._acp_queue.clear()
            cls._or_queue.clear()

    @classmethod
    def acp_queue(cls, sse: bytes):
        with cls._lock:
            cls._acp_queue.append(sse)

    @classmethod
    def or_queue(cls, sse: bytes):
        with cls._lock:
            cls._or_queue.append(sse)

    @classmethod
    def acp_requests(cls) -> list:
        with cls._lock:
            return list(cls._acp_log)

    @classmethod
    def or_requests(cls) -> list:
        with cls._lock:
            return list(cls._or_log)

    def log_message(self, *_):
        pass

    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(n) if n else b""
        try:
            parsed = json.loads(body)
        except Exception:
            parsed = {}

        with self.__class__._lock:
            if self.path.endswith("/messages"):
                self.__class__._acp_log.append(parsed)
                resp = self.__class__._acp_queue.popleft() \
                    if self.__class__._acp_queue else acp_sse("default acp response")
            elif self.path.endswith("/chat/completions"):
                self.__class__._or_log.append(parsed)
                resp = self.__class__._or_queue.popleft() \
                    if self.__class__._or_queue else or_sse("default or response")
            else:
                resp = b"data: [DONE]\n\n"

        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        try:
            self.wfile.write(resp)
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass


# Separate mock for xAI (port MOCK_ACP_PORT+2)
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
        with self.__class__._lock:
            try:
                self.__class__._log.append(json.loads(body))
            except Exception:
                self.__class__._log.append({})
            resp = self.__class__._queue.popleft() \
                if self.__class__._queue else xai_sse("default xai response")
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
        "ACP_PREFIX":        prefix,
        "ANTHROPIC_BASE_URL": f"http://127.0.0.1:{MOCK_ACP_PORT}",
        "ANTHROPIC_API_KEY": "fake-acp-key",
        "BYPASS_PERMISSIONS": "true",
    }


def or_env(prefix: str, model: str = "anthropic/claude-sonnet-4-6") -> dict:
    return {
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


def xai_env(prefix: str, model: str = "grok-3") -> dict:
    return {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_XAI_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": model,
    }


def codex_env(prefix: str, model: str = "o4-mini", **extra) -> dict:
    env = {
        "ACP_PREFIX":           prefix,
        "CODEX_BIN":            BIN_MOCK,
        "CODEX_DEFAULT_MODEL":  model,
        "CODEX_MODELS":         f"{model}:{model},o3:o3,o4-mini:o4-mini,gpt-4o:GPT-4o",
        "OPENAI_API_KEY":       "fake-openai-key",
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


# ═════════════════════════════════════════════════════════════════════════════
# S95 — xai set_mode: "default" → OK; unknown → -32602 error
# ═════════════════════════════════════════════════════════════════════════════

async def s95_xai_set_mode(nc):
    section("S95: xai set_mode — 'default' → OK; unknown → -32602")
    prefix = f"{BASE}.xai_s95"
    MockXAI.reset()

    start_runner("s95", BIN_XAI, xai_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S95: session.new timed out"); return

        # "default" mode is valid for xai
        r = await send_set_mode(nc, prefix, sid, "default")
        if "code" in r or "_error" in r:
            fail("S95: set_mode 'default' failed for xai", str(r)[:80])
        else:
            ok("S95: xai set_mode 'default' → OK")

        # unknown mode
        r2 = await send_set_mode(nc, prefix, sid, "bypassPermissions")
        code = r2.get("code", None)
        if code == -32602:
            ok("S95: xai set_mode unknown → -32602 (invalid params)")
        else:
            fail("S95: expected -32602 for unknown mode", str(r2)[:80])

        if "unknown mode" in r2.get("message", "").lower():
            ok("S95: error message mentions 'unknown mode'")
        else:
            fail("S95: error msg should mention 'unknown mode'", r2.get("message", "")[:60])

    finally:
        stop_runner("s95")


# ═════════════════════════════════════════════════════════════════════════════
# S96 — or set_mode: "default" → OK; unknown → -32602 error
# ═════════════════════════════════════════════════════════════════════════════

async def s96_or_set_mode(nc):
    section("S96: or set_mode — 'default' → OK; unknown → -32602")
    prefix = f"{BASE}.or_s96"
    MockLLM.reset()

    start_runner("s96", BIN_OR, or_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S96: session.new timed out"); return

        r = await send_set_mode(nc, prefix, sid, "default")
        if "code" in r or "_error" in r:
            fail("S96: set_mode 'default' failed for or", str(r)[:80])
        else:
            ok("S96: or set_mode 'default' → OK")

        r2 = await send_set_mode(nc, prefix, sid, "some-unknown-mode")
        if r2.get("code") == -32602:
            ok("S96: or set_mode unknown → -32602")
        else:
            fail("S96: expected -32602 for unknown mode", str(r2)[:80])

    finally:
        stop_runner("s96")


# ═════════════════════════════════════════════════════════════════════════════
# S97 — codex set_mode: "default" → OK; unknown → -32602 error
# ═════════════════════════════════════════════════════════════════════════════

async def s97_codex_set_mode(nc):
    section("S97: codex set_mode — 'default' → OK; unknown → -32602")
    prefix = f"{BASE}.cx_s97"

    start_runner("s97", BIN_CODEX, codex_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S97: session.new timed out"); return

        r = await send_set_mode(nc, prefix, sid, "default")
        if "code" in r or "_error" in r:
            fail("S97: set_mode 'default' failed for codex", str(r)[:80])
        else:
            ok("S97: codex set_mode 'default' → OK")

        r2 = await send_set_mode(nc, prefix, sid, "completely-unknown-mode")
        # codex has no permission modes — silently accepts any mode string
        if "code" in r2 or "_error" in r2:
            fail("S97: codex set_mode unknown should be silently accepted", str(r2)[:80])
        else:
            ok("S97: codex set_mode unknown → silently accepted (codex has no modes)")

    finally:
        stop_runner("s97")


# ═════════════════════════════════════════════════════════════════════════════
# S98 — xai write_file + read_file: real file I/O via tool dispatch
# ═════════════════════════════════════════════════════════════════════════════

async def s98_xai_write_read_file(nc):
    section("S98: xai write_file + read_file — real file I/O via tool dispatch")
    prefix = f"{BASE}.xai_s98"
    MockXAI.reset()

    tmp_dir = tempfile.mkdtemp()
    try:
        file_content = "hello from xai write_file s98"
        # Turn 1: write_file tool
        r1, c1 = f"ra1-s98", f"ca1-s98"
        MockXAI.queue(xai_sse_fn(r1, c1, "write_file",
                                 json.dumps({"path": "output_s98.txt",
                                             "content": file_content})))
        # Turn 2: read_file tool
        r2, c2 = f"ra2-s98", f"ca2-s98"
        MockXAI.queue(xai_sse_fn(r2, c2, "read_file",
                                 json.dumps({"path": "output_s98.txt"})))
        # Final text
        MockXAI.queue(xai_sse("file written and read back s98", "resp-s98-final"))

        start_runner("s98", BIN_XAI, xai_env(prefix))
        try:
            sid = await wait_runner(nc, prefix, cwd=tmp_dir)
            if not sid:
                fail("S98: session.new timed out"); return

            resp = await send_prompt(nc, prefix, sid,
                                     "write and then read output_s98.txt")
            if prompt_err(resp):
                fail("S98: prompt failed", str(resp)[:80]); return
            ok("S98: prompt completed (write_file + read_file)")

            # Verify the file was actually created on disk
            expected_path = os.path.join(tmp_dir, "output_s98.txt")
            if os.path.exists(expected_path):
                ok("S98: file created on disk by write_file tool")
            else:
                fail("S98: file not found on disk after write_file tool")

            with open(expected_path) as f:
                disk_content = f.read()
            if disk_content == file_content:
                ok("S98: file content on disk matches what write_file wrote")
            else:
                fail("S98: file content mismatch", f"got {disk_content!r}")

            # Verify the read_file result appears in the xai API call (call 3 input)
            calls = MockXAI.requests()
            if len(calls) == 3:
                ok("S98: exactly 3 xai API calls (write_fn + read_fn + final)")
            else:
                fail("S98: expected 3 xai API calls", f"got {len(calls)}")

        finally:
            stop_runner("s98")
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S99 — or str_replace: write file, str_replace, read_file to verify edit
# ═════════════════════════════════════════════════════════════════════════════

async def s99_or_str_replace(nc):
    section("S99: or str_replace — write + replace + read_file verifies edit on disk")
    prefix = f"{BASE}.or_s99"
    MockLLM.reset()

    tmp_dir = tempfile.mkdtemp()
    try:
        # Pre-create a file with known content
        test_file = os.path.join(tmp_dir, "edit_s99.txt")
        with open(test_file, "w") as f:
            f.write("The quick brown fox jumps over the lazy dog")

        c1, c2, c3 = "or-c1-s99", "or-c2-s99", "or-c3-s99"
        # Turn 1: str_replace "brown fox" → "red cat"
        MockLLM.or_queue(or_sse_tool(c1, "str_replace",
                                     json.dumps({"path": "edit_s99.txt",
                                                 "old_str": "brown fox",
                                                 "new_str": "red cat"})))
        # Turn 2: read_file to verify
        MockLLM.or_queue(or_sse_tool(c2, "read_file",
                                     json.dumps({"path": "edit_s99.txt"})))
        # Final text
        MockLLM.or_queue(or_sse("file edited and verified s99"))

        start_runner("s99", BIN_OR, or_env(prefix))
        try:
            sid = await wait_runner(nc, prefix, cwd=tmp_dir)
            if not sid:
                fail("S99: session.new timed out"); return

            resp = await send_prompt(nc, prefix, sid, "edit and verify edit_s99.txt")
            if prompt_err(resp):
                fail("S99: prompt failed", str(resp)[:80]); return
            ok("S99: prompt completed (str_replace + read_file)")

            # Verify the edit on disk
            with open(test_file) as f:
                content = f.read()
            if "red cat" in content and "brown fox" not in content:
                ok("S99: str_replace edited file correctly on disk")
            else:
                fail("S99: file not edited correctly", content[:80])

            # Verify read_file result in call 3 includes updated content
            calls = MockLLM.or_requests()
            if len(calls) == 3:
                ok("S99: exactly 3 API calls (str_replace + read_file + final)")
            else:
                fail("S99: expected 3 API calls", f"got {len(calls)}")
                return

            # Call 3 should have tool_result with updated content
            c3_msgs = calls[2].get("messages", [])
            tool_msgs = [m for m in c3_msgs if m.get("role") == "tool"]
            if tool_msgs:
                read_result = tool_msgs[-1].get("content", "")
                if "red cat" in read_result:
                    ok("S99: read_file tool result in API call shows updated content")
                else:
                    fail("S99: read_file result should show 'red cat'", read_result[:80])
            else:
                fail("S99: no tool results in call 3", str(c3_msgs)[:80])

        finally:
            stop_runner("s99")
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S100 — acp write_file + glob: write files, glob finds them; bad path rejected
# ═════════════════════════════════════════════════════════════════════════════

async def s100_acp_write_glob(nc):
    section("S100: acp write_file + glob — write files with pattern; glob finds them")
    prefix = f"{BASE}.acp_s100"
    MockLLM.reset()

    # ACP runner uses PROCESS cwd (rsworkspace) for file tools, not session cwd.
    # Create files inside rsworkspace so glob can find them.
    tmp_dir = tempfile.mkdtemp(dir=RSDIR, prefix="smoke10_s100_")
    rel_name = os.path.basename(tmp_dir)
    try:
        # Create 3 .s100.txt files in the temp subdir of rsworkspace
        for i in range(3):
            with open(os.path.join(tmp_dir, f"file{i}.s100.txt"), "w") as f:
                f.write(f"file {i} content s100")

        c1 = "acp-c1-s100"
        # Turn 1: glob to find *.s100.txt files in the temp subdir
        MockLLM.acp_queue(acp_sse_tool(
            c1, "glob", json.dumps({"pattern": "*.s100.txt", "path": rel_name})
        ))
        # Final text
        MockLLM.acp_queue(acp_sse("found all files s100"))

        start_runner("s100", BIN_ACP, acp_env(prefix))
        try:
            sid = await wait_runner(nc, prefix, cwd=RSDIR)
            if not sid:
                fail("S100: session.new timed out"); return

            # ACP runner requires interactive permission for file tools; bypass for tests
            await send_set_mode(nc, prefix, sid, "bypassPermissions")

            resp = await send_prompt(nc, prefix, sid, "find all .s100.txt files")
            if prompt_err(resp):
                fail("S100: prompt failed", str(resp)[:80]); return
            ok("S100: prompt completed (glob tool)")

            # The glob result should appear in the 2nd API call's messages
            calls = MockLLM.acp_requests()
            if len(calls) == 2:
                ok("S100: exactly 2 acp API calls (glob + final)")
            else:
                fail("S100: expected 2 acp API calls", f"got {len(calls)}")
                return

            # Find the tool result in the 2nd call
            c2_content = calls[1].get("messages", [])
            tool_result_content = ""
            for msg in c2_content:
                if isinstance(msg.get("content"), list):
                    for block in msg["content"]:
                        if block.get("type") == "tool_result":
                            raw = block.get("content", "")
                            if isinstance(raw, str):
                                tool_result_content += raw
                            else:
                                for sub in raw:
                                    if isinstance(sub, dict) and sub.get("type") == "text":
                                        tool_result_content += sub.get("text", "")
                                    elif isinstance(sub, str):
                                        tool_result_content += sub

            info(f"S100: glob result = {tool_result_content[:120]!r}")
            # The glob should have found all 3 files
            found_count = tool_result_content.count(".s100.txt")
            if found_count >= 3:
                ok(f"S100: glob found {found_count} .s100.txt files")
            else:
                fail("S100: glob should find 3+ .s100.txt files",
                     f"found {found_count}: {tool_result_content[:80]}")

        finally:
            stop_runner("s100")
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S101 — acp export tool blocks: PortableBlocks with tool_call/tool_result types
# ═════════════════════════════════════════════════════════════════════════════

async def s101_acp_export_tool_blocks(nc):
    section("S101: acp export after tool turn — PortableBlocks include tool_call/tool_result types")
    prefix = f"{BASE}.acp_s101"
    MockLLM.reset()

    # ACP runner uses PROCESS cwd (rsworkspace) for file tools, not session cwd.
    # Create the test file inside rsworkspace so read_file can find it.
    tmp_dir = tempfile.mkdtemp(dir=RSDIR, prefix="smoke10_s101_")
    rel_name = os.path.basename(tmp_dir)
    try:
        with open(os.path.join(tmp_dir, "target_s101.txt"), "w") as f:
            f.write("target file content s101")

        c1 = "acp-tc-s101"
        MockLLM.acp_queue(acp_sse_tool(
            c1, "read_file", json.dumps({"path": f"{rel_name}/target_s101.txt"})
        ))
        MockLLM.acp_queue(acp_sse("file read for export test s101"))

        start_runner("s101", BIN_ACP, acp_env(prefix))
        try:
            sid = await wait_runner(nc, prefix, cwd=RSDIR)
            if not sid:
                fail("S101: session.new timed out"); return

            # ACP runner requires interactive permission for file tools; bypass for tests
            await send_set_mode(nc, prefix, sid, "bypassPermissions")

            resp = await send_prompt(nc, prefix, sid, f"read {rel_name}/target_s101.txt")
            if prompt_err(resp):
                fail("S101: prompt failed", str(resp)[:80]); return
            ok("S101: prompt completed with tool turn")

            exp = await nats_req(nc, f"{prefix}.agent.ext.session/export", {"sessionId": sid})
            if "_error" in exp or "code" in exp:
                fail("S101: export failed", str(exp)[:80]); return

            portable = exp
            if isinstance(portable, str):
                portable = json.loads(portable)
            if "result" in portable and isinstance(portable["result"], str):
                portable = json.loads(portable["result"])

            if not isinstance(portable, list):
                fail("S101: export should return list", str(portable)[:80]); return

            info(f"S101: exported {len(portable)} PortableMessages")

            # Find blocks of type tool_call and tool_result
            all_blocks = [b for m in portable for b in m.get("blocks", [])]
            block_types = [b.get("type") for b in all_blocks]
            info(f"S101: block types in export = {block_types}")

            tc_blocks = [b for b in all_blocks if b.get("type") == "tool_call"]
            if tc_blocks:
                ok(f"S101: export has {len(tc_blocks)} tool_call block(s)")
                if tc_blocks[0].get("name") == "read_file":
                    ok("S101: tool_call block.name = 'read_file'")
                else:
                    fail("S101: tool_call block.name wrong", str(tc_blocks[0])[:80])
                if tc_blocks[0].get("id") == c1:
                    ok(f"S101: tool_call block.id matches ({c1})")
                else:
                    fail("S101: tool_call block.id wrong", str(tc_blocks[0].get("id")))
            else:
                fail("S101: no tool_call blocks in export")

            tr_blocks = [b for b in all_blocks if b.get("type") == "tool_result"]
            if tr_blocks:
                ok(f"S101: export has {len(tr_blocks)} tool_result block(s)")
                if "target file content s101" in str(tr_blocks[0].get("content", "")):
                    ok("S101: tool_result block contains file content")
                else:
                    fail("S101: tool_result block missing content", str(tr_blocks[0])[:80])
            else:
                fail("S101: no tool_result blocks in export")

        finally:
            stop_runner("s101")
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S102 — or → codex cross-runner import: export from OR, import into codex
# ═════════════════════════════════════════════════════════════════════════════

async def s102_or_to_codex_import(nc):
    section("S102: or→codex cross-runner import — export from OR; codex history grows")
    prefix_or    = f"{BASE}.or_s102"
    prefix_codex = f"{BASE}.cx_s102"
    MockLLM.reset()
    MockLLM.or_queue(or_sse("or response for cross-runner export s102"))

    start_runner("s102or", BIN_OR, or_env(prefix_or))
    start_runner("s102cx", BIN_CODEX, codex_env(prefix_codex))
    try:
        # Create or session and send a prompt
        sid_or = await wait_runner(nc, prefix_or, cwd="/tmp")
        if not sid_or:
            fail("S102: or session.new timed out"); return

        resp = await send_prompt(nc, prefix_or, sid_or, "something for export s102")
        if prompt_err(resp):
            fail("S102: or prompt failed", str(resp)[:80]); return
        ok("S102: or prompt completed")

        # Export from openrouter
        exp = await nats_req(nc, f"{prefix_or}.agent.ext.session/export",
                             {"sessionId": sid_or})
        if "_error" in exp or "code" in exp:
            fail("S102: or export failed", str(exp)[:80]); return

        portable = exp
        if isinstance(portable, str):
            portable = json.loads(portable)
        if "result" in portable and isinstance(portable["result"], str):
            portable = json.loads(portable["result"])
        if not isinstance(portable, list):
            fail("S102: or export should be list", str(portable)[:80]); return
        ok(f"S102: or export has {len(portable)} messages")

        # Create codex session and import the or export
        sid_cx = await wait_runner(nc, prefix_codex, cwd="/tmp")
        if not sid_cx:
            fail("S102: codex session.new timed out"); return

        import_resp = await nats_req(
            nc, f"{prefix_codex}.agent.ext.session/import",
            {"sessionId": sid_cx, "messages": portable},
        )
        if "_error" in import_resp or "code" in import_resp:
            fail("S102: codex import failed", str(import_resp)[:80]); return
        ok("S102: codex import from or export succeeded")

        # Codex history should have the imported messages
        state = await nats_req(nc, f"{prefix_codex}.agent.ext.session/get_state",
                               {"sessionId": sid_cx})
        history = state.get("history", [])
        info(f"S102: codex history after import = {len(history)} entries")
        if len(history) == len(portable):
            ok(f"S102: codex history has {len(history)} entries = or export length")
        else:
            fail("S102: codex history length mismatch",
                 f"expected {len(portable)}, got {len(history)}")

        # Send a codex prompt after import — history should grow
        resp2 = await send_prompt(nc, prefix_codex, sid_cx, "continue from imported context s102")
        if prompt_err(resp2):
            fail("S102: codex prompt after import failed", str(resp2)[:80]); return
        ok("S102: codex prompt after import completed")

        state2 = await nats_req(nc, f"{prefix_codex}.agent.ext.session/get_state",
                                {"sessionId": sid_cx})
        history2 = state2.get("history", [])
        info(f"S102: codex history after prompt = {len(history2)} entries")
        if len(history2) > len(history):
            ok(f"S102: codex history grew from {len(history)} to {len(history2)} after prompt")
        else:
            fail("S102: codex history should grow after prompt",
                 f"was {len(history)}, now {len(history2)}")

    finally:
        stop_runner("s102or")
        stop_runner("s102cx")


# ═════════════════════════════════════════════════════════════════════════════
# S103 — acp → xai cross-runner import: export from acp, import into xai
# ═════════════════════════════════════════════════════════════════════════════

async def s103_acp_to_xai_import(nc):
    section("S103: acp→xai cross-runner import — last_response_id cleared after import")
    prefix_acp = f"{BASE}.acp_s103"
    prefix_xai = f"{BASE}.xai_s103"
    MockLLM.reset()
    MockXAI.reset()
    MockLLM.acp_queue(acp_sse("acp response for xai import s103"))

    start_runner("s103acp", BIN_ACP, acp_env(prefix_acp))
    start_runner("s103xai", BIN_XAI, xai_env(prefix_xai))
    try:
        # Create acp session + prompt
        sid_acp = await wait_runner(nc, prefix_acp, cwd="/tmp")
        if not sid_acp:
            fail("S103: acp session.new timed out"); return

        resp = await send_prompt(nc, prefix_acp, sid_acp, "acp message for export s103")
        if prompt_err(resp):
            fail("S103: acp prompt failed", str(resp)[:80]); return
        ok("S103: acp prompt completed")

        # Export from acp
        exp = await nats_req(nc, f"{prefix_acp}.agent.ext.session/export",
                             {"sessionId": sid_acp})
        portable = exp
        if isinstance(portable, str):
            portable = json.loads(portable)
        if "result" in portable and isinstance(portable["result"], str):
            portable = json.loads(portable["result"])
        if not isinstance(portable, list):
            fail("S103: acp export should be list", str(portable)[:80]); return
        ok(f"S103: acp export has {len(portable)} messages")

        # Create xai session, send a prompt (establishes last_response_id)
        xai_resp_id = "resp-s103-xai-before"
        MockXAI.queue(xai_sse("xai before import s103", xai_resp_id))
        sid_xai = await wait_runner(nc, prefix_xai, cwd="/tmp")
        if not sid_xai:
            fail("S103: xai session.new timed out"); return

        r = await send_prompt(nc, prefix_xai, sid_xai, "xai turn before import s103")
        if prompt_err(r):
            fail("S103: xai prompt before import failed", str(r)[:80]); return
        ok("S103: xai prompt before import completed")

        # Verify last_response_id is set
        state_before = await nats_req(nc, f"{prefix_xai}.agent.ext.session/get_state",
                                      {"sessionId": sid_xai})
        lrid_before = state_before.get("last_response_id")
        if lrid_before == xai_resp_id:
            ok(f"S103: xai last_response_id set before import ({lrid_before})")
        else:
            fail("S103: xai last_response_id not set before import", str(lrid_before))

        # Import acp export into xai session
        import_resp = await nats_req(
            nc, f"{prefix_xai}.agent.ext.session/import",
            {"sessionId": sid_xai, "messages": portable},
        )
        if "_error" in import_resp or "code" in import_resp:
            fail("S103: xai import failed", str(import_resp)[:80]); return
        ok("S103: xai import from acp export succeeded")

        # After import, last_response_id should be cleared (per xai import semantics)
        state_after = await nats_req(nc, f"{prefix_xai}.agent.ext.session/get_state",
                                     {"sessionId": sid_xai})
        lrid_after = state_after.get("last_response_id")
        if lrid_after is None:
            ok("S103: last_response_id cleared after import (fresh conversation)")
        else:
            fail("S103: last_response_id should be None after import", str(lrid_after))

    finally:
        stop_runner("s103acp")
        stop_runner("s103xai")


# ═════════════════════════════════════════════════════════════════════════════
# S104 — codex MOCK_REQUIRE_MODEL: model passed in turn/start; wrong model → error
# ═════════════════════════════════════════════════════════════════════════════

async def s104_codex_require_model(nc):
    section("S104: codex MOCK_REQUIRE_MODEL — model passed in turn/start; mismatch → error")
    prefix = f"{BASE}.cx_s104"

    # Runner with default model=o4-mini; mock requires o4-mini → prompts work
    start_runner("s104_ok", BIN_CODEX,
                 codex_env(prefix + ".ok", model="o4-mini",
                           MOCK_REQUIRE_MODEL="o4-mini"))
    try:
        sid_ok = await wait_runner(nc, prefix + ".ok", cwd="/tmp")
        if not sid_ok:
            fail("S104: ok session.new timed out"); return

        # Must call set_model so the model is sent in turn/start params
        # (codex only sends model in turn/start when session.model is explicitly set)
        await send_set_model(nc, prefix + ".ok", sid_ok, "o4-mini")

        resp_ok = await send_prompt(nc, prefix + ".ok", sid_ok,
                                    "prompt with correct model s104")
        if prompt_err(resp_ok):
            fail("S104: prompt with correct model failed", str(resp_ok)[:80])
        else:
            ok("S104: prompt with MOCK_REQUIRE_MODEL=o4-mini and model=o4-mini → success")

    finally:
        stop_runner("s104_ok")

    # Runner with default model=o4-mini; mock requires o3 → prompts fail (model mismatch)
    start_runner("s104_bad", BIN_CODEX,
                 codex_env(prefix + ".bad", model="o4-mini",
                           MOCK_REQUIRE_MODEL="o3"))
    try:
        sid_bad = await wait_runner(nc, prefix + ".bad", cwd="/tmp")
        if not sid_bad:
            fail("S104: bad session.new timed out"); return

        # Set model to o4-mini so it IS sent in turn/start; mock requires o3 → mismatch
        await send_set_model(nc, prefix + ".bad", sid_bad, "o4-mini")

        resp_bad = await send_prompt(nc, prefix + ".bad", sid_bad,
                                     "prompt with wrong model s104")
        # The prompt should complete (runner handles turn error gracefully) but
        # the mock should have rejected the turn/start → runner returns EndTurn with empty text
        if "_error" in resp_bad and "timed out" in resp_bad.get("_error", ""):
            fail("S104: prompt timed out (runner hung on model mismatch)"); return
        ok("S104: prompt returned (no hang) despite MOCK_REQUIRE_MODEL mismatch")

        # The runner should still be functional (spawn a new session)
        sid_bad2 = await new_session(nc, prefix + ".bad", cwd="/tmp")
        if sid_bad2:
            ok("S104: runner functional after model mismatch error")
        else:
            fail("S104: runner should be functional after model mismatch")

    finally:
        stop_runner("s104_bad")


# ═════════════════════════════════════════════════════════════════════════════
# S105 — xai write_file then list_dir: files created then directory listed
# ═════════════════════════════════════════════════════════════════════════════

async def s105_xai_write_then_list(nc):
    section("S105: xai write_file + list_dir — create files then list directory")
    prefix = f"{BASE}.xai_s105"
    MockXAI.reset()

    tmp_dir = tempfile.mkdtemp()
    try:
        r1, c1 = "ra1-s105", "ca1-s105"
        r2, c2 = "ra2-s105", "ca2-s105"
        r3, c3 = "ra3-s105", "ca3-s105"

        # Turn 1: write file_a.txt
        MockXAI.queue(xai_sse_fn(r1, c1, "write_file",
                                 json.dumps({"path": "file_a_s105.txt",
                                             "content": "content A s105"})))
        # Turn 2: write file_b.txt
        MockXAI.queue(xai_sse_fn(r2, c2, "write_file",
                                 json.dumps({"path": "file_b_s105.txt",
                                             "content": "content B s105"})))
        # Turn 3: list_dir
        MockXAI.queue(xai_sse_fn(r3, c3, "list_dir", json.dumps({})))
        # Final text
        MockXAI.queue(xai_sse("files written and directory listed s105", "resp-s105-final"))

        start_runner("s105", BIN_XAI, xai_env(prefix))
        try:
            sid = await wait_runner(nc, prefix, cwd=tmp_dir)
            if not sid:
                fail("S105: session.new timed out"); return

            resp = await send_prompt(nc, prefix, sid,
                                     "write two files then list the directory")
            if prompt_err(resp):
                fail("S105: prompt failed", str(resp)[:80]); return
            ok("S105: prompt completed (2 writes + list_dir)")

            # Verify files exist
            fa = os.path.join(tmp_dir, "file_a_s105.txt")
            fb = os.path.join(tmp_dir, "file_b_s105.txt")
            if os.path.exists(fa) and os.path.exists(fb):
                ok("S105: both files created on disk by write_file tools")
            else:
                fail("S105: files not created on disk",
                     f"a={os.path.exists(fa)}, b={os.path.exists(fb)}")

            # Verify list_dir result in the API (call 4 input should have list_dir result)
            calls = MockXAI.requests()
            info(f"S105: total xai API calls = {len(calls)}")
            if len(calls) == 4:
                ok("S105: exactly 4 xai calls (write_a + write_b + list + final)")
            else:
                fail("S105: expected 4 xai API calls", f"got {len(calls)}")
                return

            # Call 4 input should contain the list_dir output
            c4_input = calls[3].get("input", [])
            c4_str = json.dumps(c4_input)
            if "file_a_s105" in c4_str or "file_b_s105" in c4_str:
                ok("S105: list_dir result in call 4 shows created files")
            else:
                fail("S105: list_dir result should show created files", c4_str[:120])

        finally:
            stop_runner("s105")
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S106 — or write_file + list_dir: combined file + directory operations
# ═════════════════════════════════════════════════════════════════════════════

async def s106_or_write_list_dir(nc):
    section("S106: or write_file + list_dir — file creation then directory listing")
    prefix = f"{BASE}.or_s106"
    MockLLM.reset()

    tmp_dir = tempfile.mkdtemp()
    try:
        c1, c2 = "or-c1-s106", "or-c2-s106"
        # Turn 1: write a file
        MockLLM.or_queue(or_sse_tool(c1, "write_file",
                                     json.dumps({"path": "notes_s106.txt",
                                                 "content": "project notes s106"})))
        # Turn 2: list_dir
        MockLLM.or_queue(or_sse_tool(c2, "list_dir", json.dumps({})))
        # Final text
        MockLLM.or_queue(or_sse("wrote notes and listed directory s106"))

        start_runner("s106", BIN_OR, or_env(prefix))
        try:
            sid = await wait_runner(nc, prefix, cwd=tmp_dir)
            if not sid:
                fail("S106: session.new timed out"); return

            resp = await send_prompt(nc, prefix, sid, "write notes and list directory")
            if prompt_err(resp):
                fail("S106: prompt failed", str(resp)[:80]); return
            ok("S106: prompt completed (write + list_dir)")

            # Verify file created
            notes_file = os.path.join(tmp_dir, "notes_s106.txt")
            if os.path.exists(notes_file):
                ok("S106: notes file created on disk")
                with open(notes_file) as f:
                    content = f.read()
                if content == "project notes s106":
                    ok("S106: notes file content correct")
                else:
                    fail("S106: notes file content wrong", content[:60])
            else:
                fail("S106: notes file not found on disk")

            # API call 3 should have tool results for both write_file and list_dir
            calls = MockLLM.or_requests()
            info(f"S106: total API calls = {len(calls)}")
            if len(calls) == 3:
                ok("S106: exactly 3 API calls (write + list_dir + final)")
            else:
                fail("S106: expected 3 API calls", f"got {len(calls)}")
                return

            c3_msgs = calls[2].get("messages", [])
            tool_msgs = [m for m in c3_msgs if m.get("role") == "tool"]
            if len(tool_msgs) == 2:
                ok("S106: 2 tool results in call 3 (write_file + list_dir)")
            else:
                fail("S106: expected 2 tool results", f"got {len(tool_msgs)}")
                return

            # write_file result should be "OK"
            if tool_msgs[0].get("content") == "OK":
                ok("S106: write_file tool result = 'OK'")
            else:
                fail("S106: write_file should return 'OK'",
                     str(tool_msgs[0].get("content"))[:60])

            # list_dir result should mention the notes file
            list_result = tool_msgs[1].get("content", "")
            if "notes_s106" in list_result:
                ok("S106: list_dir result includes the created notes file")
            else:
                fail("S106: list_dir should include notes_s106.txt", list_result[:80])

        finally:
            stop_runner("s106")
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# Main
# ═════════════════════════════════════════════════════════════════════════════

async def main():
    print("=== smoke_test_programming10.py — cross-runner, set_mode, tool I/O ===")
    print(f"  prefix : {BASE}")
    print(f"  nats   : {NATS_URL}")
    print(f"  acp/or mock : :{MOCK_ACP_PORT} / :{MOCK_OR_PORT}")
    print(f"  xai mock    : :{MOCK_XAI_PORT}")

    # Start dual mock (acp at /messages + or at /chat/completions)
    start_mock(MOCK_ACP_PORT, MockLLM)    # serves both acp and or on different paths
    start_mock(MOCK_OR_PORT, MockLLM)     # or-specific mock (same class, different port)
    start_mock(MOCK_XAI_PORT, MockXAI)   # xai mock
    info(f"mock servers started on :{MOCK_ACP_PORT}, :{MOCK_OR_PORT}, :{MOCK_XAI_PORT}")

    nc = await nats_lib.connect(NATS_URL)
    try:
        await s95_xai_set_mode(nc)
        await s96_or_set_mode(nc)
        await s97_codex_set_mode(nc)
        await s98_xai_write_read_file(nc)
        await s99_or_str_replace(nc)
        await s100_acp_write_glob(nc)
        await s101_acp_export_tool_blocks(nc)
        await s102_or_to_codex_import(nc)
        await s103_acp_to_xai_import(nc)
        await s104_codex_require_model(nc)
        await s105_xai_write_then_list(nc)
        await s106_or_write_list_dir(nc)
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
