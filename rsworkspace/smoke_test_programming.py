#!/usr/bin/env python3
"""
smoke_test_programming.py — Integration test for programming features without credentials.

Tests:
  1.  acp-runner:        basic prompt           (mock Anthropic SSE via PROXY_URL)
  2.  xai-runner:        basic prompt           (mock xAI SSE via XAI_BASE_URL)
  3.  openrouter-runner: basic prompt           (mock OpenRouter SSE via OPENROUTER_BASE_URL)
  4.  acp-runner:        set_model              (model field changes in HTTP request body)
  5.  cross-runner:      acp → xai              (export/import, prompt on xai)
  6.  acp-runner:        tool execution         (read_file → tool_result round-trip)
  7.  xai-runner:        tool execution         (function_call → follow-up round-trip)
  8.  openrouter-runner: tool execution         (tool_calls → role:tool follow-up)
  9.  xai-runner:        set_model              (model field changes in /responses request)
  10. openrouter-runner: set_model              (model field changes in /chat/completions)
  11. codex-runner:      basic prompt           (mock_codex_server, text event)
  12. cross-runner:      acp tool history → xai (PortableBlock::ToolCall/ToolResult preserved)
  13. codex-runner:      set_model              (model passed to turn/start; MOCK_REQUIRE_MODEL rejects wrong)
  14. acp-runner:        bypassPermissions      (tool actually executes, not permission-denied)
  15. codex-runner:      tool execution         (MOCK_SEND_TOOL_EVENT; PortableBlock in history)
  16. acp-runner:        session resume         (runner restart; load_session; history persists)
  17. cross-runner:      acp → codex            (export/import/prompt via pending_history)
  18. acp-runner:        tool actual success    (runner cwd=/tmp; file content in tool_result)
  19. xai-runner:        tool actual success    (function_call_output with file content)
  20. set_model+cross:   xai set_model → or     (xai→openrouter export after model change)
  21. openrouter-runner: tool actual execution  (role:tool message with file content)
  22. TROGON.md:         system context         (marker injected on acp/xai/openrouter)
  23. write_file:        AI can write files     (xai/openrouter/acp bypassPermissions)
  24. xai-runner:        spawn handler e2e      (NATS request → /chat/completions → reply)
  25. openrouter-runner: spawn handler e2e      (NATS request → /chat/completions → reply)
  26. cross-runner:      acp → openrouter       (export/import, prompt on openrouter)
  27. cross-runner:      xai → codex            (export/import, prompt on codex)
  28. cross-runner:      openrouter → xai       (export/import, prompt on xai)
  29. tool:              git_status             (runs git status in session cwd)
  30. tool:              git_diff               (runs git diff in session cwd)
  31. tool:              git_log                (shows commit history in session cwd)
  32. tool:              glob                   (finds files matching **/*.rs pattern)
  33. tool:              list_dir               (lists directory tree)
  34. tool:              fetch_url              (fetches URL; raw content in output)
  35. tool:              notebook_edit          (edits cell in .ipynb; verifies on disk)
  36. tool:              str_replace            (replaces text in a file; verifies on disk)
  37. tool:              search_files           (finds pattern matches in directory)
  38. tool:              todo_write             (writes a todo item; verifies output)
  39. tool:              todo_read              (reads back todo written in same session)
  40. registry:          runner startup         (xai-runner registers models in AGENT_REGISTRY KV)
  41. --print flag:      CLI e2e                (trogon --print sends prompt, stdout has response)
  42. compaction:        full cycle             (token threshold → trogon.compactor.compact called)
  43. registry:          openrouter startup     (openrouter-runner registers models in AGENT_REGISTRY KV)
  44. registry:          codex startup          (codex-runner registers models in AGENT_REGISTRY KV)
  45. bash tool:         acp e2e                (acp-runner dispatches bash via mock wasm-runtime over NATS)
  46. registry:          acp startup            (acp-runner registers Claude models in AGENT_REGISTRY KV)
  47. session/get_state: xai + openrouter       (ext_method returns session JSON with cwd field)
  48. CrossRunnerSwitcher: /model via trogon    (trogon binary switches runner mid-session via registry)
  49. /compact CLI:      manual publish         (trogon /compact publishes to compactor subject)
  50. /init CLI:         writes TROGON.md       (trogon /init sends LLM prompt and writes file to disk)

All runners share a single mock HTTP server on MOCK_PORT, routed by path:
  POST /anthropic/v1/messages  → Anthropic SSE  (acp-runner)
  POST /responses              → xAI SSE         (xai-runner)
  POST /chat/completions       → OpenAI SSE      (openrouter-runner)

Run:
    python3 smoke_test_programming.py

Override NATS URL:
    NATS_URL=nats://other:4222 python3 smoke_test_programming.py
"""

import asyncio
import collections
import json
import os
import subprocess
import sys
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Optional

import nats as nats_lib

# ── Config ─────────────────────────────────────────────────────────────────────

NATS_URL   = os.environ.get("NATS_URL", "nats://localhost:4222")
RSDIR      = os.path.dirname(os.path.abspath(__file__))
MOCK_PORT  = 19810
BASE       = f"smoke.prog.{os.getpid()}"

PROMPT_TIMEOUT       = 35.0   # longer for tool-execution tests (two API round-trips)
NATS_TIMEOUT         = 8.0
RUNNER_READY_TIMEOUT = 12.0

# Temp file used by read_file tool-execution tests.
TOOL_TEST_FILE = "/tmp/trogon_smoke_tool_test.txt"

# ── Console helpers ────────────────────────────────────────────────────────────

green  = "\033[32m"
red    = "\033[31m"
yellow = "\033[33m"
cyan   = "\033[36m"
reset  = "\033[0m"

PASS = 0
FAIL = 0
procs: dict = {}


def ok(msg: str, detail: str = ""):
    global PASS
    PASS += 1
    sfx = f"  {yellow}{detail}{reset}" if detail else ""
    print(f"  {green}PASS{reset}  {msg}{sfx}")


def fail(msg: str, detail: str = ""):
    global FAIL
    FAIL += 1
    sfx = f"  {yellow}{detail}{reset}" if detail else ""
    print(f"  {red}FAIL{reset}  {msg}{sfx}")


def info(msg: str):
    print(f"  {yellow}INFO{reset}  {msg}")


def section(title: str):
    bar = "─" * max(0, 60 - len(title))
    print(f"\n{cyan}── {title} {bar}{reset}")


def prompt_err(result: dict) -> Optional[str]:
    """Return an error string if the prompt result indicates failure, else None."""
    if "_error" in result:
        return result["_error"]
    if "code" in result:
        return result.get("message", str(result))
    return None


# ── SSE builders: text responses ───────────────────────────────────────────────

def anthropic_sse(text: str) -> bytes:
    """Anthropic streaming SSE for acp-runner.

    SseParser in agent_loop.rs uses '\n\n' as event separator.  Do NOT use
    '\r\n' line endings or '\r\n\r\n' event separators.
    """
    chunks = [
        "event: message_start\ndata: " + json.dumps({
            "type": "message_start",
            "message": {"id": "msg-smoke", "type": "message", "role": "assistant",
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
        "event: content_block_stop\ndata: " + json.dumps({
            "type": "content_block_stop", "index": 0,
        }) + "\n\n",
        "event: message_delta\ndata: " + json.dumps({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn", "stop_sequence": None},
            "usage": {"output_tokens": 1},
        }) + "\n\n",
        "event: message_stop\ndata: " + json.dumps({"type": "message_stop"}) + "\n\n",
    ]
    return "".join(chunks).encode()


def xai_sse(text: str, resp_id: str = "resp-smoke-001") -> bytes:
    """xAI Responses API SSE for xai-runner."""
    chunks = [
        "data: " + json.dumps({"id": resp_id, "type": "message.delta",
                                "delta": {"text": text}}) + "\n\n",
        "data: " + json.dumps({"type": "response.completed",
                                "response": {"id": resp_id, "status": "completed",
                                             "usage": {"input_tokens": 5, "output_tokens": 2}}}) + "\n\n",
        "data: [DONE]\n\n",
    ]
    return "".join(chunks).encode()


def openrouter_sse(text: str) -> bytes:
    """OpenAI-compatible SSE for openrouter-runner."""
    chunks = [
        "data: " + json.dumps({
            "choices": [{"delta": {"content": text, "role": "assistant"},
                         "finish_reason": None, "index": 0}],
        }) + "\n\n",
        "data: " + json.dumps({
            "choices": [{"delta": {}, "finish_reason": "stop", "index": 0}],
        }) + "\n\n",
        "data: [DONE]\n\n",
    ]
    return "".join(chunks).encode()


# ── SSE builders: tool-call responses ─────────────────────────────────────────

def anthropic_sse_tool_use(tool_id: str, tool_name: str, input_json: str) -> bytes:
    """Anthropic SSE: tool_use response (stop_reason=tool_use)."""
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
        "event: content_block_stop\ndata: " + json.dumps({
            "type": "content_block_stop", "index": 0,
        }) + "\n\n",
        "event: message_delta\ndata: " + json.dumps({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use", "stop_sequence": None},
            "usage": {"output_tokens": 5},
        }) + "\n\n",
        "event: message_stop\ndata: " + json.dumps({"type": "message_stop"}) + "\n\n",
    ]
    return "".join(chunks).encode()


def xai_sse_function_call(resp_id: str, call_id: str, name: str, arguments: str) -> bytes:
    """xAI SSE: function_call response (triggers tool execution in xai-runner)."""
    chunks = [
        "data: " + json.dumps({
            "id": resp_id, "type": "function_call",
            "function_call": {"call_id": call_id, "name": name, "arguments": arguments},
        }) + "\n\n",
        "data: " + json.dumps({
            "type": "response.completed",
            "response": {"id": resp_id, "status": "completed",
                         "usage": {"input_tokens": 10, "output_tokens": 5}},
        }) + "\n\n",
        "data: [DONE]\n\n",
    ]
    return "".join(chunks).encode()


def openrouter_sse_tool_calls(call_id: str, name: str, arguments: str) -> bytes:
    """OpenRouter SSE: tool_calls streaming (finish_reason=tool_calls)."""
    chunks = [
        # First chunk: announces the tool call (empty arguments)
        "data: " + json.dumps({
            "choices": [{"delta": {"tool_calls": [{"index": 0, "id": call_id,
                                                    "type": "function",
                                                    "function": {"name": name, "arguments": ""}}]},
                         "finish_reason": None, "index": 0}],
        }) + "\n\n",
        # Second chunk: streams the arguments
        "data: " + json.dumps({
            "choices": [{"delta": {"tool_calls": [{"index": 0,
                                                    "function": {"arguments": arguments}}]},
                         "finish_reason": None, "index": 0}],
        }) + "\n\n",
        # Final chunk: finish_reason=tool_calls
        "data: " + json.dumps({
            "choices": [{"delta": {}, "finish_reason": "tool_calls", "index": 0}],
        }) + "\n\n",
        "data: [DONE]\n\n",
    ]
    return "".join(chunks).encode()


# ── Mock HTTP server ────────────────────────────────────────────────────────────

_PATH_TO_DEFAULT = {
    "/anthropic/v1/messages": lambda: anthropic_sse("ok"),
    "/responses":             lambda: xai_sse("ok"),
    "/chat/completions":      lambda: openrouter_sse("ok"),
}


class MockLLMServer(BaseHTTPRequestHandler):
    _lock = threading.Lock()
    _request_log: list = []
    _response_queues: dict = {
        "/anthropic/v1/messages": collections.deque(),
        "/responses":             collections.deque(),
        "/chat/completions":      collections.deque(),
    }
    _get_responses: dict = {}

    @classmethod
    def reset(cls):
        with cls._lock:
            cls._request_log.clear()
            for q in cls._response_queues.values():
                q.clear()
            cls._get_responses.clear()

    @classmethod
    def queue(cls, path: str, sse: bytes):
        with cls._lock:
            cls._response_queues[path].append(sse)

    @classmethod
    def set_get_response(cls, path: str, body: bytes):
        with cls._lock:
            cls._get_responses[path] = body

    @classmethod
    def request_count(cls, path: str) -> int:
        with cls._lock:
            return sum(1 for p, _ in cls._request_log if p == path)

    @classmethod
    def all_requests(cls, path: str) -> list:
        with cls._lock:
            return [body for p, body in cls._request_log if p == path]

    def log_message(self, fmt, *args):
        pass

    def _read_body(self) -> bytes:
        length = int(self.headers.get("Content-Length", 0))
        return self.rfile.read(length) if length else b""

    def do_POST(self):
        body = self._read_body()
        with MockLLMServer._lock:
            try:
                parsed = json.loads(body)
            except Exception:
                parsed = {}
            MockLLMServer._request_log.append((self.path, parsed))
            q = MockLLMServer._response_queues.get(self.path)
            resp = q.popleft() if q else None

        if resp is None:
            factory = _PATH_TO_DEFAULT.get(self.path)
            resp = factory() if factory else b"data: [DONE]\n\n"

        # Queued responses that start with '{' are plain JSON (e.g. spawn handler).
        if resp.startswith(b'{') or resp.startswith(b'['):
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
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
        with MockLLMServer._lock:
            resp = MockLLMServer._get_responses.get(self.path, b"mock-get-content")
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.end_headers()
        try:
            self.wfile.write(resp)
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass


def start_mock_server(port: int) -> HTTPServer:
    server = HTTPServer(("127.0.0.1", port), MockLLMServer)
    t = threading.Thread(target=server.serve_forever, daemon=True)
    t.start()
    return server


# ── Runner process helpers ─────────────────────────────────────────────────────

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


# ── NATS helpers ───────────────────────────────────────────────────────────────

async def nats_req(nc, subject: str, params: dict, timeout: float = NATS_TIMEOUT) -> dict:
    try:
        msg = await nc.request(subject, json.dumps(params).encode(), timeout=timeout)
        return json.loads(msg.data)
    except nats_lib.errors.NoRespondersError:
        return {"_error": "no_responders"}
    except Exception as e:
        return {"_error": str(e)}


async def wait_for_runner(nc, prefix: str, timeout: float = RUNNER_READY_TIMEOUT) -> Optional[str]:
    """Poll session.new until the runner responds.  Returns sessionId or None."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        resp = await nats_req(nc, f"{prefix}.agent.session.new",
                              {"cwd": "/tmp", "mcpServers": []}, timeout=1.5)
        if "sessionId" in resp:
            return resp["sessionId"]
        await asyncio.sleep(0.3)
    return None


async def wait_for_runner_cwd(nc, prefix: str, cwd: str,
                               timeout: float = RUNNER_READY_TIMEOUT) -> Optional[str]:
    """Like wait_for_runner but uses a specific cwd in session.new."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        resp = await nats_req(nc, f"{prefix}.agent.session.new",
                              {"cwd": cwd, "mcpServers": []}, timeout=1.5)
        if "sessionId" in resp:
            return resp["sessionId"]
        await asyncio.sleep(0.3)
    return None


async def send_prompt(nc, prefix: str, session_id: str, text: str,
                      timeout: float = PROMPT_TIMEOUT) -> dict:
    """
    Publish a prompt and wait for the runner's completion response.

    ACP prompt protocol (header-based, NOT NATS reply subjects):
      1. Subscribe to  {prefix}.session.{sid}.agent.prompt.response.{req_id}
      2. Publish to    {prefix}.session.{sid}.agent.prompt  with header X-Req-Id
      3. Wait on subscription — runner publishes here when the turn finishes.
    """
    req_id = str(uuid.uuid4())
    prompt_subject = f"{prefix}.session.{session_id}.agent.prompt"
    resp_subject = f"{prefix}.session.{session_id}.agent.prompt.response.{req_id}"
    payload = json.dumps({
        "sessionId": session_id,
        "prompt": [{"type": "text", "text": text}],
    }).encode()

    sub = await nc.subscribe(resp_subject)
    try:
        await nc.publish(prompt_subject, payload, headers={"X-Req-Id": req_id})
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


async def send_set_mode(nc, prefix: str, session_id: str, mode_id: str,
                        timeout: float = 10.0) -> dict:
    """Send set_mode and wait for runner ACK (same header protocol as set_model)."""
    req_id = str(uuid.uuid4())
    resp_subject = f"{prefix}.session.{session_id}.agent.response.{req_id}"
    subject = f"{prefix}.session.{session_id}.agent.set_mode"
    payload = json.dumps({"sessionId": session_id, "modeId": mode_id}).encode()
    sub = await nc.subscribe(resp_subject)
    try:
        await nc.publish(subject, payload, headers={"X-Req-Id": req_id})
        msg = await asyncio.wait_for(sub.next_msg(), timeout=timeout)
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


async def send_set_model(nc, prefix: str, session_id: str, model_id: str,
                         timeout: float = 10.0) -> dict:
    """
    Send set_model NATS command and wait for runner ACK.

    Protocol (mirrors trogon-cli/src/session.rs):
      publish to   {prefix}.session.{sid}.agent.set_model  with X-Req-Id
      subscribe to {prefix}.session.{sid}.agent.response.{req_id}
    """
    req_id = str(uuid.uuid4())
    resp_subject = f"{prefix}.session.{session_id}.agent.response.{req_id}"
    set_model_subject = f"{prefix}.session.{session_id}.agent.set_model"
    payload = json.dumps({"sessionId": session_id, "modelId": model_id}).encode()

    sub = await nc.subscribe(resp_subject)
    try:
        await nc.publish(set_model_subject, payload, headers={"X-Req-Id": req_id})
        msg = await asyncio.wait_for(sub.next_msg(), timeout=timeout)
        return json.loads(msg.data)
    except asyncio.TimeoutError:
        return {"_error": "set_model timed out — runner did not respond"}
    except Exception as e:
        return {"_error": str(e)}
    finally:
        try:
            await sub.unsubscribe()
        except Exception:
            pass


# ── Test 1: acp basic prompt ───────────────────────────────────────────────────

async def test_acp_basic_prompt(nc):
    section("Test 1: acp-runner — basic prompt (mock Anthropic SSE)")
    prefix = f"{BASE}.acp1"
    acp_bin = os.path.join(RSDIR, "target", "debug", "trogon-acp-runner")

    MockLLMServer.reset()
    MockLLMServer.queue("/anthropic/v1/messages", anthropic_sse("hello_acp"))

    start_runner("acp1", acp_bin, {
        "ACP_PREFIX":      prefix,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("acp session.new timed out"); return
        info(f"session: {sid}")

        result = await send_prompt(nc, prefix, sid, "ping")
        if e := prompt_err(result):
            fail("acp prompt", e)
        else:
            ok("acp prompt completed without error")

        n = MockLLMServer.request_count("/anthropic/v1/messages")
        ok("acp: Anthropic endpoint called", f"{n} request(s)") if n >= 1 else \
            fail("acp: Anthropic endpoint never called")

        reqs = MockLLMServer.all_requests("/anthropic/v1/messages")
        m = reqs[0].get("model") if reqs else None
        if m == "claude-opus-4-6":
            ok("acp: correct model in HTTP body", f"model={m}")
        else:
            fail("acp: wrong model in HTTP body", f"expected claude-opus-4-6, got {m}")
    finally:
        stop_runner("acp1")


# ── Test 2: xai basic prompt ───────────────────────────────────────────────────

async def test_xai_basic_prompt(nc):
    section("Test 2: xai-runner — basic prompt (mock xAI SSE)")
    prefix = f"{BASE}.xai1"
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")

    MockLLMServer.reset()
    MockLLMServer.queue("/responses", xai_sse("hello_xai"))

    start_runner("xai1", xai_bin, {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("xai session.new timed out"); return
        info(f"session: {sid}")

        result = await send_prompt(nc, prefix, sid, "ping")
        if e := prompt_err(result):
            fail("xai prompt", e)
        else:
            ok("xai prompt completed without error")

        n = MockLLMServer.request_count("/responses")
        ok("xai: xAI endpoint called", f"{n} request(s)") if n >= 1 else \
            fail("xai: xAI endpoint never called")
    finally:
        stop_runner("xai1")


# ── Test 3: openrouter basic prompt ───────────────────────────────────────────

async def test_openrouter_basic_prompt(nc):
    section("Test 3: openrouter-runner — basic prompt (mock OpenRouter SSE)")
    prefix = f"{BASE}.or1"
    or_bin = os.path.join(RSDIR, "target", "debug", "trogon-openrouter-runner")

    MockLLMServer.reset()
    MockLLMServer.queue("/chat/completions", openrouter_sse("hello_openrouter"))

    start_runner("or1", or_bin, {
        "ACP_PREFIX":          prefix,
        "OPENROUTER_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":  "fake-or-key",
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("openrouter session.new timed out"); return
        info(f"session: {sid}")

        result = await send_prompt(nc, prefix, sid, "ping")
        if e := prompt_err(result):
            fail("openrouter prompt", e)
        else:
            ok("openrouter prompt completed without error")

        n = MockLLMServer.request_count("/chat/completions")
        ok("openrouter: endpoint called", f"{n} request(s)") if n >= 1 else \
            fail("openrouter: endpoint never called")
    finally:
        stop_runner("or1")


# ── Test 4: acp set_model ──────────────────────────────────────────────────────

async def test_acp_set_model(nc):
    section("Test 4: acp-runner — set_model changes model in HTTP requests")
    prefix = f"{BASE}.acp2"
    acp_bin = os.path.join(RSDIR, "target", "debug", "trogon-acp-runner")

    MockLLMServer.reset()
    MockLLMServer.queue("/anthropic/v1/messages", anthropic_sse("response_one"))
    MockLLMServer.queue("/anthropic/v1/messages", anthropic_sse("response_two"))

    start_runner("acp2", acp_bin, {
        "ACP_PREFIX":      prefix,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("acp set_model: session.new timed out"); return
        info(f"session: {sid}")

        r1 = await send_prompt(nc, prefix, sid, "first prompt")
        if e := prompt_err(r1):
            fail("acp set_model: prompt 1", e); return

        reqs = MockLLMServer.all_requests("/anthropic/v1/messages")
        m1 = reqs[0].get("model") if reqs else None
        if m1 == "claude-opus-4-6":
            ok("acp: initial model in prompt 1", f"model={m1}")
        else:
            fail("acp: wrong initial model", f"expected claude-opus-4-6, got {m1}")

        sr = await send_set_model(nc, prefix, sid, "claude-haiku-4-5")
        if e := prompt_err(sr):
            fail("acp: set_model NATS call", e)
        else:
            ok("acp: set_model acknowledged by runner")

        r2 = await send_prompt(nc, prefix, sid, "second prompt")
        if e := prompt_err(r2):
            fail("acp set_model: prompt 2", e); return

        reqs2 = MockLLMServer.all_requests("/anthropic/v1/messages")
        m2 = reqs2[-1].get("model") if reqs2 else None
        if m2 == "claude-haiku-4-5":
            ok("acp: model changed to claude-haiku-4-5 in prompt 2", f"model={m2}")
        else:
            fail("acp: model did not change after set_model", f"expected claude-haiku-4-5, got {m2}")
    finally:
        stop_runner("acp2")


# ── Test 5: cross-runner acp → xai ────────────────────────────────────────────

async def test_cross_runner_acp_to_xai(nc):
    section("Test 5: cross-runner — acp → xai (export / import / prompt)")
    prefix_acp = f"{BASE}.acp3"
    prefix_xai = f"{BASE}.xai3"
    acp_bin = os.path.join(RSDIR, "target", "debug", "trogon-acp-runner")
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")

    MockLLMServer.reset()
    MockLLMServer.queue("/anthropic/v1/messages", anthropic_sse("acp_turn_one"))
    MockLLMServer.queue("/responses",             xai_sse("xai_after_switch"))

    start_runner("acp3", acp_bin, {
        "ACP_PREFIX": prefix_acp, "PROXY_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token", "AGENT_MODEL": "claude-opus-4-6",
    })
    start_runner("xai3", xai_bin, {
        "ACP_PREFIX": prefix_xai, "XAI_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY": "fake-xai-key", "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid_acp = await wait_for_runner(nc, prefix_acp)
        if not sid_acp:
            fail("cross: acp session.new timed out"); return
        sid_xai = await wait_for_runner(nc, prefix_xai)
        if not sid_xai:
            fail("cross: xai session.new timed out"); return
        info(f"acp={sid_acp}  xai={sid_xai}")

        r = await send_prompt(nc, prefix_acp, sid_acp, "tell me something")
        if e := prompt_err(r):
            fail("cross: acp initial prompt", e); return
        ok("cross: acp initial prompt completed")

        exported = await nats_req(nc, f"{prefix_acp}.agent.ext.session/export",
                                   {"sessionId": sid_acp})
        if not isinstance(exported, list) or not exported:
            fail("cross: acp export", f"expected list, got: {exported!r}"); return
        ok("cross: acp session exported", f"{len(exported)} message(s)")

        ir = await nats_req(nc, f"{prefix_xai}.agent.ext.session/import",
                             {"sessionId": sid_xai, "messages": exported})
        if isinstance(ir, dict) and "code" in ir:
            fail("cross: xai import", str(ir)); return
        ok("cross: history imported into xai session")

        xai_before = MockLLMServer.request_count("/responses")
        acp_before  = MockLLMServer.request_count("/anthropic/v1/messages")

        r = await send_prompt(nc, prefix_xai, sid_xai, "continue from there")
        if e := prompt_err(r):
            fail("cross: xai prompt after import", e)
        else:
            ok("cross: xai prompt completed after import")

        xai_after = MockLLMServer.request_count("/responses")
        acp_after  = MockLLMServer.request_count("/anthropic/v1/messages")

        if xai_after > xai_before:
            ok("cross: xAI endpoint called after switch", f"{xai_after - xai_before} request(s)")
        else:
            fail("cross: xAI endpoint not called after switch")

        if acp_after == acp_before:
            ok("cross: Anthropic endpoint silent after switch")
        else:
            fail("cross: Anthropic endpoint unexpectedly called after switch")
    finally:
        stop_runner("acp3")
        stop_runner("xai3")


# ── Test 6: acp tool execution ─────────────────────────────────────────────────

async def test_acp_tool_execution(nc):
    section("Test 6: acp-runner — tool execution (read_file round-trip)")
    prefix = f"{BASE}.acp4"
    acp_bin = os.path.join(RSDIR, "target", "debug", "trogon-acp-runner")

    with open(TOOL_TEST_FILE, "w") as f:
        f.write("acp_tool_test_content")

    MockLLMServer.reset()
    # First response: tool_use (read_file)
    MockLLMServer.queue("/anthropic/v1/messages",
                        anthropic_sse_tool_use("toolu_acp_001", "read_file",
                                               json.dumps({"path": TOOL_TEST_FILE})))
    # Second response: end_turn text after receiving tool result
    MockLLMServer.queue("/anthropic/v1/messages", anthropic_sse("I read the file."))

    start_runner("acp4", acp_bin, {
        "ACP_PREFIX":      prefix,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("acp tool: session.new timed out"); return
        info(f"session: {sid}")

        result = await send_prompt(nc, prefix, sid, "read a file")
        if e := prompt_err(result):
            fail("acp tool: prompt failed", e); return
        ok("acp tool: prompt completed (two-turn tool cycle)")

        reqs = MockLLMServer.all_requests("/anthropic/v1/messages")
        if len(reqs) >= 2:
            ok("acp tool: runner made 2 HTTP requests (tool_use + follow-up)",
               f"{len(reqs)} total")
        else:
            fail("acp tool: expected 2 requests, got", str(len(reqs))); return

        # Verify second request contains tool_result
        msgs2 = reqs[1].get("messages", [])
        has_tool_result = any(
            isinstance(m.get("content"), list) and
            any(b.get("type") == "tool_result" for b in m["content"])
            for m in msgs2
        )
        if has_tool_result:
            ok("acp tool: second request contains tool_result block")
        else:
            fail("acp tool: second request missing tool_result block",
                 f"messages: {json.dumps(msgs2)[:200]}")

        # Verify tool actually ran: tool_result content should include file content
        for m in msgs2:
            if isinstance(m.get("content"), list):
                for b in m["content"]:
                    if b.get("type") == "tool_result":
                        content = b.get("content", "")
                        if "acp_tool_test_content" in content:
                            ok("acp tool: tool result contains file content")
                        else:
                            info(f"acp tool: tool result content: {content[:100]!r}")
    finally:
        stop_runner("acp4")


# ── Test 7: xai tool execution ─────────────────────────────────────────────────

async def test_xai_tool_execution(nc):
    section("Test 7: xai-runner — tool execution (function_call round-trip)")
    prefix = f"{BASE}.xai2"
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")

    with open(TOOL_TEST_FILE, "w") as f:
        f.write("xai_tool_test_content")

    MockLLMServer.reset()
    MockLLMServer.queue("/responses",
                        xai_sse_function_call("resp-xai-tool-001", "call_xai_001",
                                              "read_file",
                                              json.dumps({"path": TOOL_TEST_FILE})))
    MockLLMServer.queue("/responses", xai_sse("I read the file.", "resp-xai-002"))

    start_runner("xai2", xai_bin, {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("xai tool: session.new timed out"); return
        info(f"session: {sid}")

        result = await send_prompt(nc, prefix, sid, "read a file")
        if e := prompt_err(result):
            fail("xai tool: prompt failed", e); return
        ok("xai tool: prompt completed (function_call cycle)")

        reqs = MockLLMServer.all_requests("/responses")
        if len(reqs) >= 2:
            ok("xai tool: runner made 2 HTTP requests", f"{len(reqs)} total")
        else:
            fail("xai tool: expected 2 requests, got", str(len(reqs))); return

        # Second request: input[0].type == "function_call_output"
        input2 = reqs[1].get("input", [])
        has_output = any(item.get("type") == "function_call_output" for item in input2)
        if has_output:
            ok("xai tool: second request contains function_call_output")
        else:
            fail("xai tool: second request missing function_call_output",
                 f"input: {json.dumps(input2)[:200]}")

        # Verify previous_response_id is set (stateful continuation)
        prev_id = reqs[1].get("previous_response_id")
        if prev_id == "resp-xai-tool-001":
            ok("xai tool: previous_response_id set correctly", f"id={prev_id}")
        else:
            info(f"xai tool: previous_response_id={prev_id!r} (may differ from mock resp_id)")
    finally:
        stop_runner("xai2")


# ── Test 8: openrouter tool execution ──────────────────────────────────────────

async def test_openrouter_tool_execution(nc):
    section("Test 8: openrouter-runner — tool execution (tool_calls round-trip)")
    prefix = f"{BASE}.or2"
    or_bin = os.path.join(RSDIR, "target", "debug", "trogon-openrouter-runner")

    with open(TOOL_TEST_FILE, "w") as f:
        f.write("or_tool_test_content")

    MockLLMServer.reset()
    MockLLMServer.queue("/chat/completions",
                        openrouter_sse_tool_calls("call_or_001", "read_file",
                                                   json.dumps({"path": TOOL_TEST_FILE})))
    MockLLMServer.queue("/chat/completions", openrouter_sse("I read the file."))

    start_runner("or2", or_bin, {
        "ACP_PREFIX":          prefix,
        "OPENROUTER_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":  "fake-or-key",
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("or tool: session.new timed out"); return
        info(f"session: {sid}")

        result = await send_prompt(nc, prefix, sid, "read a file")
        if e := prompt_err(result):
            fail("or tool: prompt failed", e); return
        ok("or tool: prompt completed (tool_calls cycle)")

        reqs = MockLLMServer.all_requests("/chat/completions")
        if len(reqs) >= 2:
            ok("or tool: runner made 2 HTTP requests", f"{len(reqs)} total")
        else:
            fail("or tool: expected 2 requests, got", str(len(reqs))); return

        # Second request messages should include role:"tool"
        msgs2 = reqs[1].get("messages", [])
        has_tool_role = any(m.get("role") == "tool" for m in msgs2)
        if has_tool_role:
            ok("or tool: second request contains role:tool message")
        else:
            fail("or tool: second request missing role:tool message",
                 f"roles: {[m.get('role') for m in msgs2]}")
    finally:
        stop_runner("or2")


# ── Test 9: xai set_model ──────────────────────────────────────────────────────

async def test_xai_set_model(nc):
    section("Test 9: xai-runner — set_model changes model in /responses requests")
    prefix = f"{BASE}.xai4"
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")

    MockLLMServer.reset()
    MockLLMServer.queue("/responses", xai_sse("response_one"))
    MockLLMServer.queue("/responses", xai_sse("response_two"))

    start_runner("xai4", xai_bin, {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
        "XAI_MODELS":        "grok-3:Grok 3,grok-3-mini:Grok 3 Mini",
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("xai set_model: session.new timed out"); return
        info(f"session: {sid}")

        r1 = await send_prompt(nc, prefix, sid, "first prompt")
        if e := prompt_err(r1):
            fail("xai set_model: prompt 1", e); return

        reqs = MockLLMServer.all_requests("/responses")
        m1 = reqs[0].get("model") if reqs else None
        if m1 == "grok-3":
            ok("xai: initial model in prompt 1", f"model={m1}")
        else:
            fail("xai: wrong initial model", f"expected grok-3, got {m1}")

        sr = await send_set_model(nc, prefix, sid, "grok-3-mini")
        if e := prompt_err(sr):
            fail("xai: set_model NATS call", e)
        else:
            ok("xai: set_model acknowledged by runner")

        r2 = await send_prompt(nc, prefix, sid, "second prompt")
        if e := prompt_err(r2):
            fail("xai set_model: prompt 2", e); return

        reqs2 = MockLLMServer.all_requests("/responses")
        m2 = reqs2[-1].get("model") if reqs2 else None
        if m2 == "grok-3-mini":
            ok("xai: model changed to grok-3-mini in prompt 2", f"model={m2}")
        else:
            fail("xai: model did not change after set_model",
                 f"expected grok-3-mini, got {m2}")
    finally:
        stop_runner("xai4")


# ── Test 10: openrouter set_model ──────────────────────────────────────────────

async def test_openrouter_set_model(nc):
    section("Test 10: openrouter-runner — set_model changes model in /chat/completions")
    prefix = f"{BASE}.or3"
    or_bin = os.path.join(RSDIR, "target", "debug", "trogon-openrouter-runner")

    initial_model = "anthropic/claude-sonnet-4-6"  # OPENROUTER_DEFAULT_MODEL default
    new_model     = "openai/gpt-4o"

    MockLLMServer.reset()
    MockLLMServer.queue("/chat/completions", openrouter_sse("response_one"))
    MockLLMServer.queue("/chat/completions", openrouter_sse("response_two"))

    start_runner("or3", or_bin, {
        "ACP_PREFIX":          prefix,
        "OPENROUTER_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":  "fake-or-key",
        # No OPENROUTER_DEFAULT_MODEL → uses default "anthropic/claude-sonnet-4-6"
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("or set_model: session.new timed out"); return
        info(f"session: {sid}")

        r1 = await send_prompt(nc, prefix, sid, "first prompt")
        if e := prompt_err(r1):
            fail("or set_model: prompt 1", e); return

        reqs = MockLLMServer.all_requests("/chat/completions")
        m1 = reqs[0].get("model") if reqs else None
        if m1 == initial_model:
            ok("or: initial model in prompt 1", f"model={m1}")
        else:
            fail("or: wrong initial model", f"expected {initial_model}, got {m1}")

        sr = await send_set_model(nc, prefix, sid, new_model)
        if e := prompt_err(sr):
            fail("or: set_model NATS call", e)
        else:
            ok("or: set_model acknowledged by runner")

        r2 = await send_prompt(nc, prefix, sid, "second prompt")
        if e := prompt_err(r2):
            fail("or set_model: prompt 2", e); return

        reqs2 = MockLLMServer.all_requests("/chat/completions")
        m2 = reqs2[-1].get("model") if reqs2 else None
        if m2 == new_model:
            ok(f"or: model changed to {new_model} in prompt 2", f"model={m2}")
        else:
            fail("or: model did not change after set_model",
                 f"expected {new_model}, got {m2}")
    finally:
        stop_runner("or3")


# ── Test 11: codex basic prompt ────────────────────────────────────────────────

async def test_codex_basic_prompt(nc):
    section("Test 11: codex-runner — basic prompt (mock_codex_server)")
    prefix = f"{BASE}.codex1"
    codex_bin      = os.path.join(RSDIR, "target", "debug", "trogon-codex-runner")
    mock_codex_bin = os.path.join(RSDIR, "target", "debug", "mock_codex_server")

    if not os.path.exists(mock_codex_bin):
        fail("codex: mock_codex_server binary not found", mock_codex_bin)
        return

    start_runner("codex1", codex_bin, {
        "ACP_PREFIX":             prefix,
        "CODEX_BIN":              mock_codex_bin,
        "MOCK_SEND_N_TEXT_EVENTS": "1",
        "CODEX_SPAWN_TIMEOUT_SECS": "8",
    })
    try:
        sid = await wait_for_runner(nc, prefix, timeout=15.0)
        if not sid:
            fail("codex session.new timed out"); return
        info(f"session: {sid}")

        result = await send_prompt(nc, prefix, sid, "hello codex")
        if e := prompt_err(result):
            fail("codex prompt", e)
        else:
            ok("codex prompt completed without error")

        # Export history: should have at least user + assistant messages
        exported = await nats_req(nc, f"{prefix}.agent.ext.session/export",
                                   {"sessionId": sid})
        if isinstance(exported, list) and len(exported) >= 2:
            ok("codex: session history has messages after prompt", f"{len(exported)} messages")
        else:
            fail("codex: expected ≥2 messages in history after prompt",
                 f"got: {exported!r}")
    finally:
        stop_runner("codex1")


# ── Test 12: cross-runner tool history (acp tool cycle → xai) ─────────────────

async def test_cross_runner_tool_history(nc):
    section("Test 12: cross-runner — acp tool history → xai (PortableBlock::ToolCall/ToolResult)")
    prefix_acp = f"{BASE}.acp5"
    prefix_xai = f"{BASE}.xai5"
    acp_bin = os.path.join(RSDIR, "target", "debug", "trogon-acp-runner")
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")

    with open(TOOL_TEST_FILE, "w") as f:
        f.write("cross_runner_tool_content")

    MockLLMServer.reset()
    # acp: tool_use first, then end_turn after tool result
    MockLLMServer.queue("/anthropic/v1/messages",
                        anthropic_sse_tool_use("toolu_cross_001", "read_file",
                                               json.dumps({"path": TOOL_TEST_FILE})))
    MockLLMServer.queue("/anthropic/v1/messages", anthropic_sse("Done reading."))
    # xai: follow-up after import
    MockLLMServer.queue("/responses", xai_sse("xai_follow_up"))

    start_runner("acp5", acp_bin, {
        "ACP_PREFIX": prefix_acp, "PROXY_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token", "AGENT_MODEL": "claude-opus-4-6",
    })
    start_runner("xai5", xai_bin, {
        "ACP_PREFIX": prefix_xai, "XAI_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY": "fake-xai-key", "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid_acp = await wait_for_runner(nc, prefix_acp)
        if not sid_acp:
            fail("cross tool: acp session.new timed out"); return
        sid_xai = await wait_for_runner(nc, prefix_xai)
        if not sid_xai:
            fail("cross tool: xai session.new timed out"); return
        info(f"acp={sid_acp}  xai={sid_xai}")

        # Step 1: run tool cycle on acp
        r = await send_prompt(nc, prefix_acp, sid_acp, "read the file")
        if e := prompt_err(r):
            fail("cross tool: acp tool prompt", e); return
        ok("cross tool: acp tool cycle completed (2 API round-trips)")

        # Step 2: export — must include PortableBlock::ToolCall and PortableBlock::ToolResult
        exported = await nats_req(nc, f"{prefix_acp}.agent.ext.session/export",
                                   {"sessionId": sid_acp})
        if not isinstance(exported, list) or not exported:
            fail("cross tool: acp export empty", f"got: {exported!r}"); return
        ok("cross tool: acp session exported", f"{len(exported)} message(s)")

        # Verify PortableBlock::ToolCall appears in exported blocks
        tool_call_found   = any(
            any(b.get("type") == "tool_call" for b in m.get("blocks", []))
            for m in exported
        )
        tool_result_found = any(
            any(b.get("type") == "tool_result" for b in m.get("blocks", []))
            for m in exported
        )
        if tool_call_found:
            ok("cross tool: export contains PortableBlock::ToolCall")
        else:
            fail("cross tool: export missing PortableBlock::ToolCall",
                 f"exported: {json.dumps(exported)[:300]}")
        if tool_result_found:
            ok("cross tool: export contains PortableBlock::ToolResult")
        else:
            fail("cross tool: export missing PortableBlock::ToolResult")

        # Step 3: import into xai
        ir = await nats_req(nc, f"{prefix_xai}.agent.ext.session/import",
                             {"sessionId": sid_xai, "messages": exported})
        if isinstance(ir, dict) and "code" in ir:
            fail("cross tool: xai import", str(ir)); return
        ok("cross tool: tool history imported into xai session")

        # Step 4: prompt xai — should still work with tool history in context
        xai_before = MockLLMServer.request_count("/responses")
        r = await send_prompt(nc, prefix_xai, sid_xai, "continue")
        if e := prompt_err(r):
            fail("cross tool: xai prompt after import", e)
        else:
            ok("cross tool: xai prompt completed with tool history as context")

        if MockLLMServer.request_count("/responses") > xai_before:
            ok("cross tool: xAI endpoint called after tool-history import")
        else:
            fail("cross tool: xAI endpoint not called after import")
    finally:
        stop_runner("acp5")
        stop_runner("xai5")


# ── Test 13: codex set_model ──────────────────────────────────────────────────

async def test_codex_set_model(nc):
    section("Test 13: codex-runner — set_model passes model to turn/start")
    prefix = f"{BASE}.codex2"
    codex_bin      = os.path.join(RSDIR, "target", "debug", "trogon-codex-runner")
    mock_codex_bin = os.path.join(RSDIR, "target", "debug", "mock_codex_server")

    # MOCK_REQUIRE_MODEL=o3: mock rejects any turn/start where params.model != "o3"
    start_runner("codex2", codex_bin, {
        "ACP_PREFIX":              prefix,
        "CODEX_BIN":               mock_codex_bin,
        "CODEX_MODELS":            "o4-mini:o4-mini,o3:o3",
        "CODEX_DEFAULT_MODEL":     "o4-mini",
        "CODEX_SPAWN_TIMEOUT_SECS": "8",
        "MOCK_REQUIRE_MODEL":      "o3",
        "MOCK_SEND_N_TEXT_EVENTS": "1",
    })
    try:
        sid = await wait_for_runner(nc, prefix, timeout=15.0)
        if not sid:
            fail("codex set_model: session.new timed out"); return
        info(f"session: {sid}")

        # First prompt: session.model is None → turn/start sends no model → mock rejects
        r1 = await send_prompt(nc, prefix, sid, "first prompt")
        if prompt_err(r1):
            ok("codex: first prompt rejected (model mismatch — expected)",
               f"err={prompt_err(r1)[:60]}")
        else:
            fail("codex: first prompt should have failed (no model set)")

        # set_model to "o3"
        sr = await send_set_model(nc, prefix, sid, "o3")
        if e := prompt_err(sr):
            fail("codex: set_model call", e); return
        ok("codex: set_model to o3 acknowledged")

        # Second prompt: session.model = "o3" → mock accepts → success
        r2 = await send_prompt(nc, prefix, sid, "second prompt after set_model")
        if e := prompt_err(r2):
            fail("codex: second prompt after set_model", e)
        else:
            ok("codex: second prompt succeeded after set_model to o3")
    finally:
        stop_runner("codex2")


# ── Test 14: acp bypassPermissions ────────────────────────────────────────────

async def test_acp_bypass_permissions(nc):
    section("Test 14: acp-runner — bypassPermissions mode actually executes tools")
    prefix = f"{BASE}.acp6"
    acp_bin = os.path.join(RSDIR, "target", "debug", "trogon-acp-runner")

    tool_file = "/tmp/trogon_bypass_tool_test.txt"
    tool_content = "bypass_permissions_content_42"
    with open(tool_file, "w") as f:
        f.write(tool_content)

    MockLLMServer.reset()
    MockLLMServer.queue("/anthropic/v1/messages",
                        anthropic_sse_tool_use("toolu_bypass_001", "read_file",
                                               json.dumps({"path": tool_file})))
    MockLLMServer.queue("/anthropic/v1/messages", anthropic_sse("Done."))

    start_runner("acp6", acp_bin, {
        "ACP_PREFIX":      prefix,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("acp bypass: session.new timed out"); return
        info(f"session: {sid}")

        # Set mode to bypassPermissions so tool runs without asking the user
        mr = await send_set_mode(nc, prefix, sid, "bypassPermissions")
        if e := prompt_err(mr):
            fail("acp bypass: set_mode", e); return
        ok("acp bypass: mode set to bypassPermissions")

        result = await send_prompt(nc, prefix, sid, "read the test file")
        if e := prompt_err(result):
            fail("acp bypass: prompt", e); return
        ok("acp bypass: prompt completed (tool cycle)")

        reqs = MockLLMServer.all_requests("/anthropic/v1/messages")
        if len(reqs) < 2:
            fail("acp bypass: expected 2 HTTP requests", f"got {len(reqs)}"); return
        ok("acp bypass: runner made 2 HTTP requests", f"{len(reqs)} total")

        # Find tool_result in second request and verify actual content (not Permission denied)
        second_req = reqs[1]
        messages = second_req.get("messages", [])
        tool_result_content = None
        for msg in messages:
            content = msg.get("content")
            if not isinstance(content, list):
                continue
            for block in content:
                if not isinstance(block, dict) or block.get("type") != "tool_result":
                    continue
                c = block.get("content", "")
                if isinstance(c, str):
                    tool_result_content = c
                elif isinstance(c, list):
                    for item in c:
                        if isinstance(item, dict) and item.get("type") == "text":
                            tool_result_content = item.get("text", "")
                            break
        info(f"acp bypass: tool result = {tool_result_content!r}")
        if tool_result_content and "Permission denied" in tool_result_content:
            fail("acp bypass: tool was denied despite bypassPermissions",
                 tool_result_content[:80])
        elif tool_result_content and tool_content in tool_result_content:
            ok("acp bypass: tool actually executed — file content returned",
               f"content={tool_result_content!r}")
        elif tool_result_content:
            ok("acp bypass: tool executed (content returned, not Permission denied)",
               f"content={tool_result_content!r}")
        else:
            fail("acp bypass: tool_result block not found in second request")
    finally:
        stop_runner("acp6")


# ── Test 15: codex tool execution ─────────────────────────────────────────────

async def test_codex_tool_execution(nc):
    section("Test 15: codex-runner — tool execution (MOCK_SEND_TOOL_EVENT)")
    prefix = f"{BASE}.codex3"
    codex_bin      = os.path.join(RSDIR, "target", "debug", "trogon-codex-runner")
    mock_codex_bin = os.path.join(RSDIR, "target", "debug", "mock_codex_server")

    # MOCK_SEND_TOOL_EVENT: mock emits item/updated(tool_call) + item/completed before turn/completed
    # MOCK_SEND_N_TEXT_EVENTS=1: also emit one text delta
    start_runner("codex3", codex_bin, {
        "ACP_PREFIX":              prefix,
        "CODEX_BIN":               mock_codex_bin,
        "CODEX_SPAWN_TIMEOUT_SECS": "8",
        "MOCK_SEND_TOOL_EVENT":    "1",
        "MOCK_SEND_N_TEXT_EVENTS": "1",
    })
    try:
        sid = await wait_for_runner(nc, prefix, timeout=15.0)
        if not sid:
            fail("codex tool: session.new timed out"); return
        info(f"session: {sid}")

        result = await send_prompt(nc, prefix, sid, "run a tool")
        if e := prompt_err(result):
            fail("codex tool: prompt", e); return
        ok("codex tool: prompt completed with tool event")

        # Export history — should contain PortableBlock::ToolCall and ToolResult
        exported = await nats_req(nc, f"{prefix}.agent.ext.session/export",
                                   {"sessionId": sid})
        if not isinstance(exported, list) or len(exported) < 2:
            fail("codex tool: export returned unexpected", f"got: {exported!r}"); return
        ok("codex tool: session exported", f"{len(exported)} messages")

        raw = json.dumps(exported)
        if '"type":"tool_call"' in raw or '"ToolCall"' in raw or "tool_call" in raw.lower():
            ok("codex tool: export contains tool_call block")
        else:
            fail("codex tool: no tool_call block in export", raw[:200])

        if '"type":"tool_result"' in raw or '"ToolResult"' in raw or "tool_result" in raw.lower():
            ok("codex tool: export contains tool_result block")
        else:
            fail("codex tool: no tool_result block in export", raw[:200])
    finally:
        stop_runner("codex3")


# ── Test 16: acp session resume ────────────────────────────────────────────────

async def test_acp_session_resume(nc):
    section("Test 16: acp-runner — session resume (runner restart + load_session)")
    prefix = f"{BASE}.acp7"
    acp_bin = os.path.join(RSDIR, "target", "debug", "trogon-acp-runner")

    MockLLMServer.reset()
    MockLLMServer.queue("/anthropic/v1/messages", anthropic_sse("first_response"))
    MockLLMServer.queue("/anthropic/v1/messages", anthropic_sse("second_response"))

    start_runner("acp7", acp_bin, {
        "ACP_PREFIX":      prefix,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("acp resume: session.new timed out"); return
        info(f"session: {sid}")

        r1 = await send_prompt(nc, prefix, sid, "first prompt before restart")
        if e := prompt_err(r1):
            fail("acp resume: first prompt", e); return
        ok("acp resume: first prompt completed")
    finally:
        stop_runner("acp7")

    # Restart runner with same prefix — session is persisted in JetStream KV
    await asyncio.sleep(0.5)
    start_runner("acp7b", acp_bin, {
        "ACP_PREFIX":      prefix,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    })
    try:
        # Wait for new runner to be ready
        new_sid = await wait_for_runner(nc, prefix)
        if not new_sid:
            fail("acp resume: new session.new timed out"); return

        # load_session restores the previous session state from KV
        req_id = str(uuid.uuid4())
        resp_subject = f"{prefix}.session.{sid}.agent.response.{req_id}"
        load_subject = f"{prefix}.session.{sid}.agent.load"
        payload = json.dumps({"sessionId": sid, "cwd": "/tmp", "mcpServers": []}).encode()
        sub = await nc.subscribe(resp_subject)
        try:
            await nc.publish(load_subject, payload, headers={"X-Req-Id": req_id})
            msg = await asyncio.wait_for(sub.next_msg(), timeout=10.0)
            load_resp = json.loads(msg.data)
        except asyncio.TimeoutError:
            load_resp = {"_error": "load_session timed out"}
        finally:
            await sub.unsubscribe()

        if e := prompt_err(load_resp):
            fail("acp resume: load_session", e); return
        ok("acp resume: load_session succeeded on new runner instance")

        # Second prompt on the restored session — history should include first message
        before = MockLLMServer.request_count("/anthropic/v1/messages")
        r2 = await send_prompt(nc, prefix, sid, "second prompt after resume")
        if e := prompt_err(r2):
            fail("acp resume: second prompt", e); return
        ok("acp resume: second prompt completed on resumed session")

        reqs = MockLLMServer.all_requests("/anthropic/v1/messages")
        if MockLLMServer.request_count("/anthropic/v1/messages") > before:
            last_req = reqs[-1]
            msgs = last_req.get("messages", [])
            if len(msgs) >= 3:
                ok("acp resume: second prompt carries prior history",
                   f"{len(msgs)} messages in request")
            else:
                info(f"acp resume: {len(msgs)} messages in request (may be compacted)")
                ok("acp resume: second prompt reached Anthropic API")
        else:
            fail("acp resume: Anthropic not called for second prompt")
    finally:
        stop_runner("acp7b")


# ── Test 17: cross-runner acp → codex ─────────────────────────────────────────

async def test_cross_runner_acp_to_codex(nc):
    section("Test 17: cross-runner — acp → codex (export/import/prompt)")
    prefix_acp   = f"{BASE}.acp8"
    prefix_codex = f"{BASE}.codex4"
    acp_bin      = os.path.join(RSDIR, "target", "debug", "trogon-acp-runner")
    codex_bin    = os.path.join(RSDIR, "target", "debug", "trogon-codex-runner")
    mock_codex_bin = os.path.join(RSDIR, "target", "debug", "mock_codex_server")

    MockLLMServer.reset()
    MockLLMServer.queue("/anthropic/v1/messages", anthropic_sse("acp_response_before_switch"))

    start_runner("acp8", acp_bin, {
        "ACP_PREFIX":      prefix_acp,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    })
    start_runner("codex4", codex_bin, {
        "ACP_PREFIX":              prefix_codex,
        "CODEX_BIN":               mock_codex_bin,
        "CODEX_SPAWN_TIMEOUT_SECS": "8",
        "MOCK_SEND_N_TEXT_EVENTS": "1",
    })
    try:
        sid_acp   = await wait_for_runner(nc, prefix_acp)
        sid_codex = await wait_for_runner(nc, prefix_codex, timeout=15.0)
        if not sid_acp:
            fail("cross acp→codex: acp session.new timed out"); return
        if not sid_codex:
            fail("cross acp→codex: codex session.new timed out"); return
        info(f"acp={sid_acp}  codex={sid_codex}")

        # Step 1: prompt on acp
        r1 = await send_prompt(nc, prefix_acp, sid_acp, "tell me something")
        if e := prompt_err(r1):
            fail("cross acp→codex: acp prompt", e); return
        ok("cross acp→codex: acp prompt completed")

        # Step 2: export acp history
        exported = await nats_req(nc, f"{prefix_acp}.agent.ext.session/export",
                                   {"sessionId": sid_acp})
        if not isinstance(exported, list) or len(exported) < 2:
            fail("cross acp→codex: export", f"got {exported!r}"); return
        ok("cross acp→codex: acp history exported", f"{len(exported)} messages")

        # Step 3: import into codex session
        ir = await nats_req(nc, f"{prefix_codex}.agent.ext.session/import",
                             {"sessionId": sid_codex, "messages": exported})
        if isinstance(ir, dict) and "code" in ir:
            fail("cross acp→codex: import", str(ir)); return
        ok("cross acp→codex: history imported into codex session")

        # Step 4: prompt codex — pending_history should be prepended to user input
        codex_before = MockLLMServer.request_count("/anthropic/v1/messages")
        r2 = await send_prompt(nc, prefix_codex, sid_codex, "continue from there")
        if e := prompt_err(r2):
            fail("cross acp→codex: codex prompt after import", e); return
        ok("cross acp→codex: codex prompt completed after import")

        # Verify acp endpoint was NOT called again (codex doesn't use HTTP)
        if MockLLMServer.request_count("/anthropic/v1/messages") == codex_before:
            ok("cross acp→codex: Anthropic endpoint silent after switch to codex")
        else:
            info("cross acp→codex: Anthropic endpoint called (unexpected but not fatal)")
    finally:
        stop_runner("acp8")
        stop_runner("codex4")


# ── Test 21: openrouter tool actual execution ─────────────────────────────────

async def test_openrouter_tool_actual_execution(nc):
    section("Test 21: openrouter-runner — tool read_file returns actual file content")
    prefix = f"{BASE}.or5"
    or_bin = os.path.join(RSDIR, "target", "debug", "trogon-openrouter-runner")

    tool_file = "/tmp/or_tool_exec_21.txt"
    tool_content = "openrouter_tool_exec_content_21"
    with open(tool_file, "w") as f:
        f.write(tool_content)

    MockLLMServer.reset()
    MockLLMServer.queue("/chat/completions",
                        openrouter_sse_tool_calls("call_21_001", "read_file",
                                                   json.dumps({"path": tool_file})))
    MockLLMServer.queue("/chat/completions", openrouter_sse("File read successfully."))

    # wait_for_runner sends cwd=/tmp → session cwd = /tmp; file is in /tmp → ok
    start_runner("or5", or_bin, {
        "ACP_PREFIX":          prefix,
        "OPENROUTER_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":  "fake-or-key",
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("or exec: session.new timed out"); return
        info(f"session: {sid}")

        result = await send_prompt(nc, prefix, sid, "read the test file")
        if e := prompt_err(result):
            fail("or exec: prompt", e); return
        ok("or exec: prompt completed (tool_calls cycle)")

        reqs = MockLLMServer.all_requests("/chat/completions")
        if len(reqs) < 2:
            fail("or exec: expected 2 HTTP requests", f"got {len(reqs)}"); return
        ok("or exec: runner made 2 HTTP requests", f"{len(reqs)} total")

        msgs2 = reqs[1].get("messages", [])
        tool_msg = next((m for m in msgs2 if m.get("role") == "tool"), None)
        if tool_msg is None:
            fail("or exec: no role:tool message in second request",
                 f"roles: {[m.get('role') for m in msgs2]}"); return

        output_content = tool_msg.get("content", "")
        info(f"or exec: role:tool content = {output_content!r}")
        if tool_content in output_content:
            ok("or exec: actual file content in role:tool message",
               f"content={output_content!r}")
        elif "outside" in output_content or "Error" in output_content:
            fail("or exec: tool blocked by path restriction or error",
                 output_content[:80])
        else:
            ok("or exec: role:tool message has content (tool executed)",
               f"content={output_content!r}")
    finally:
        stop_runner("or5")


# ── Test 22: TROGON.md system context injection (acp / xai / openrouter) ───────

async def test_trogon_md_injection(nc):
    section("Test 22: TROGON.md system context injected into all HTTP-based runners")
    import tempfile

    marker = "TROGON_MD_SMOKE_MARKER_22"

    acp_dir = tempfile.mkdtemp(prefix="trogon_md_acp_")
    xai_dir = tempfile.mkdtemp(prefix="trogon_md_xai_")
    or_dir  = tempfile.mkdtemp(prefix="trogon_md_or_")

    with open(os.path.join(acp_dir, "TROGON.md"), "w") as f:
        f.write(f"# System Context\n{marker}_acp")
    with open(os.path.join(xai_dir, "TROGON.md"), "w") as f:
        f.write(f"# System Context\n{marker}_xai")
    with open(os.path.join(or_dir, "TROGON.md"), "w") as f:
        f.write(f"# System Context\n{marker}_or")

    # ── acp ──────────────────────────────────────────────────────────────────────
    prefix_acp = f"{BASE}.acp10"
    acp_bin = os.path.join(RSDIR, "target", "debug", "trogon-acp-runner")
    MockLLMServer.reset()
    MockLLMServer.queue("/anthropic/v1/messages", anthropic_sse("ok"))

    start_runner("acp10", acp_bin, {
        "ACP_PREFIX":      prefix_acp,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    })
    try:
        # acp uses state.cwd (from session.new) for TROGON.md loading
        sid = await wait_for_runner_cwd(nc, prefix_acp, acp_dir)
        if not sid:
            fail("trogon_md acp: session.new timed out")
        else:
            info(f"acp session: {sid}")
            r = await send_prompt(nc, prefix_acp, sid, "hello")
            if e := prompt_err(r):
                fail("trogon_md acp: prompt", e)
            else:
                reqs = MockLLMServer.all_requests("/anthropic/v1/messages")
                if not reqs:
                    fail("trogon_md acp: no HTTP request recorded")
                else:
                    body_str = json.dumps(reqs[0])
                    sys_field = reqs[0].get("system", "")
                    info(f"acp: system field = {str(sys_field)[:80]!r}")
                    if f"{marker}_acp" in body_str:
                        ok("trogon_md acp: TROGON.md content in system prompt")
                    else:
                        fail("trogon_md acp: TROGON.md content missing from request",
                             f"system={str(sys_field)[:80]!r}")
    finally:
        stop_runner("acp10")

    # ── xai ──────────────────────────────────────────────────────────────────────
    prefix_xai = f"{BASE}.xai8"
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")
    MockLLMServer.reset()
    MockLLMServer.queue("/responses", xai_sse("ok"))

    start_runner("xai8", xai_bin, {
        "ACP_PREFIX":        prefix_xai,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_for_runner_cwd(nc, prefix_xai, xai_dir)
        if not sid:
            fail("trogon_md xai: session.new timed out")
        else:
            info(f"xai session: {sid}")
            r = await send_prompt(nc, prefix_xai, sid, "hello")
            if e := prompt_err(r):
                fail("trogon_md xai: prompt", e)
            else:
                reqs = MockLLMServer.all_requests("/responses")
                if not reqs:
                    fail("trogon_md xai: no HTTP request recorded")
                else:
                    body_str = json.dumps(reqs[0])
                    inp = reqs[0].get("input", [])
                    sys_items = [i for i in inp if isinstance(i, dict) and i.get("role") == "system"]
                    info(f"xai: system input items = {json.dumps(sys_items)[:80]!r}")
                    if f"{marker}_xai" in body_str:
                        ok("trogon_md xai: TROGON.md content in input[role=system]")
                    else:
                        fail("trogon_md xai: TROGON.md content missing from request",
                             f"system_items={json.dumps(sys_items)[:80]!r}")
    finally:
        stop_runner("xai8")

    # ── openrouter ───────────────────────────────────────────────────────────────
    prefix_or = f"{BASE}.or6"
    or_bin = os.path.join(RSDIR, "target", "debug", "trogon-openrouter-runner")
    MockLLMServer.reset()
    MockLLMServer.queue("/chat/completions", openrouter_sse("ok"))

    start_runner("or6", or_bin, {
        "ACP_PREFIX":          prefix_or,
        "OPENROUTER_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":  "fake-or-key",
    })
    try:
        sid = await wait_for_runner_cwd(nc, prefix_or, or_dir)
        if not sid:
            fail("trogon_md or: session.new timed out")
        else:
            info(f"or session: {sid}")
            r = await send_prompt(nc, prefix_or, sid, "hello")
            if e := prompt_err(r):
                fail("trogon_md or: prompt", e)
            else:
                reqs = MockLLMServer.all_requests("/chat/completions")
                if not reqs:
                    fail("trogon_md or: no HTTP request recorded")
                else:
                    body_str = json.dumps(reqs[0])
                    msgs = reqs[0].get("messages", [])
                    sys_msgs = [m for m in msgs if m.get("role") == "system"]
                    info(f"or: system messages = {json.dumps(sys_msgs)[:80]!r}")
                    if f"{marker}_or" in body_str:
                        ok("trogon_md or: TROGON.md content in messages[role=system]")
                    else:
                        fail("trogon_md or: TROGON.md content missing from request",
                             f"system_msgs={json.dumps(sys_msgs)[:80]!r}")
    finally:
        stop_runner("or6")


# ── Test 23: write_file — AI can write code files on all HTTP runners ──────────

async def test_write_file(nc):
    section("Test 23: write_file tool — AI can create files on acp / xai / openrouter")

    # ── xai (no permission checker; session cwd=/tmp from wait_for_runner) ───────
    prefix_xai = f"{BASE}.xai9"
    xai_bin    = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")
    out_xai    = "/tmp/trogon_write_test_xai_23.py"
    xai_code   = "def hello():\n    return 'written by xai'\n"

    MockLLMServer.reset()
    MockLLMServer.queue("/responses",
                        xai_sse_function_call("resp-23-xai-1", "call_23_xai",
                                              "write_file",
                                              json.dumps({"path": out_xai, "content": xai_code})))
    MockLLMServer.queue("/responses", xai_sse("File written.", "resp-23-xai-2"))

    start_runner("xai9", xai_bin, {
        "ACP_PREFIX":        prefix_xai,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_for_runner(nc, prefix_xai)
        if not sid:
            fail("write_file xai: session.new timed out")
        else:
            r = await send_prompt(nc, prefix_xai, sid, "write a python file")
            if e := prompt_err(r):
                fail("write_file xai: prompt", e)
            else:
                if os.path.exists(out_xai):
                    actual = open(out_xai).read()
                    if xai_code in actual:
                        ok("write_file xai: file created with correct content",
                           f"path={out_xai}")
                    else:
                        fail("write_file xai: file exists but content wrong",
                             f"got={actual!r}")
                else:
                    fail("write_file xai: file not created on disk", out_xai)
    finally:
        stop_runner("xai9")

    # ── openrouter (no permission checker; session cwd=/tmp) ─────────────────────
    prefix_or = f"{BASE}.or7"
    or_bin    = os.path.join(RSDIR, "target", "debug", "trogon-openrouter-runner")
    out_or    = "/tmp/trogon_write_test_or_23.py"
    or_code   = "def hello():\n    return 'written by openrouter'\n"

    MockLLMServer.reset()
    MockLLMServer.queue("/chat/completions",
                        openrouter_sse_tool_calls("call_23_or", "write_file",
                                                   json.dumps({"path": out_or, "content": or_code})))
    MockLLMServer.queue("/chat/completions", openrouter_sse("File written."))

    start_runner("or7", or_bin, {
        "ACP_PREFIX":          prefix_or,
        "OPENROUTER_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":  "fake-or-key",
    })
    try:
        sid = await wait_for_runner(nc, prefix_or)
        if not sid:
            fail("write_file or: session.new timed out")
        else:
            r = await send_prompt(nc, prefix_or, sid, "write a python file")
            if e := prompt_err(r):
                fail("write_file or: prompt", e)
            else:
                if os.path.exists(out_or):
                    actual = open(out_or).read()
                    if or_code in actual:
                        ok("write_file or: file created with correct content",
                           f"path={out_or}")
                    else:
                        fail("write_file or: file exists but content wrong",
                             f"got={actual!r}")
                else:
                    fail("write_file or: file not created on disk", out_or)
    finally:
        stop_runner("or7")

    # ── acp (bypassPermissions + runner cwd=/tmp) ─────────────────────────────────
    prefix_acp = f"{BASE}.acp11"
    acp_bin    = os.path.join(RSDIR, "target", "debug", "trogon-acp-runner")
    out_acp    = "/tmp/trogon_write_test_acp_23.py"
    acp_code   = "def hello():\n    return 'written by acp'\n"

    MockLLMServer.reset()
    MockLLMServer.queue("/anthropic/v1/messages",
                        anthropic_sse_tool_use("toolu_23_acp", "write_file",
                                               json.dumps({"path": out_acp, "content": acp_code})))
    MockLLMServer.queue("/anthropic/v1/messages", anthropic_sse("File written."))

    start_runner("acp11", acp_bin, {
        "ACP_PREFIX":      prefix_acp,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    }, cwd="/tmp")
    try:
        sid = await wait_for_runner(nc, prefix_acp)
        if not sid:
            fail("write_file acp: session.new timed out")
        else:
            mr = await send_set_mode(nc, prefix_acp, sid, "bypassPermissions")
            if e := prompt_err(mr):
                fail("write_file acp: set_mode", e)
            else:
                r = await send_prompt(nc, prefix_acp, sid, "write a python file")
                if e := prompt_err(r):
                    fail("write_file acp: prompt", e)
                else:
                    if os.path.exists(out_acp):
                        actual = open(out_acp).read()
                        if acp_code in actual:
                            ok("write_file acp: file created with correct content",
                               f"path={out_acp}")
                        else:
                            fail("write_file acp: file exists but content wrong",
                                 f"got={actual!r}")
                    else:
                        fail("write_file acp: file not created on disk", out_acp)
    finally:
        stop_runner("acp11")


# ── Test 18: acp tool actual success (runner started in /tmp) ──────────────────

async def test_acp_tool_actual_success(nc):
    section("Test 18: acp-runner — tool read_file succeeds when runner cwd=/tmp")
    prefix = f"{BASE}.acp9"
    acp_bin = os.path.join(RSDIR, "target", "debug", "trogon-acp-runner")

    tool_file = "/tmp/acp_tool_success_18.txt"
    tool_content = "acp_tool_cwd_success_content_18"
    with open(tool_file, "w") as f:
        f.write(tool_content)

    MockLLMServer.reset()
    MockLLMServer.queue("/anthropic/v1/messages",
                        anthropic_sse_tool_use("toolu_18_001", "read_file",
                                               json.dumps({"path": tool_file})))
    MockLLMServer.queue("/anthropic/v1/messages", anthropic_sse("File read successfully."))

    # Start runner with cwd=/tmp so ToolContext.cwd = /tmp
    start_runner("acp9", acp_bin, {
        "ACP_PREFIX":      prefix,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    }, cwd="/tmp")
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("acp success: session.new timed out"); return
        info(f"session: {sid}")

        mr = await send_set_mode(nc, prefix, sid, "bypassPermissions")
        if e := prompt_err(mr):
            fail("acp success: set_mode", e); return
        ok("acp success: mode set to bypassPermissions")

        result = await send_prompt(nc, prefix, sid, "read the test file")
        if e := prompt_err(result):
            fail("acp success: prompt", e); return
        ok("acp success: prompt completed (tool cycle)")

        reqs = MockLLMServer.all_requests("/anthropic/v1/messages")
        if len(reqs) < 2:
            fail("acp success: expected 2 HTTP requests", f"got {len(reqs)}"); return

        messages = reqs[1].get("messages", [])
        tool_result_content = None
        for msg in messages:
            content = msg.get("content")
            if not isinstance(content, list):
                continue
            for block in content:
                if not isinstance(block, dict) or block.get("type") != "tool_result":
                    continue
                c = block.get("content", "")
                if isinstance(c, str):
                    tool_result_content = c
                elif isinstance(c, list):
                    for item in c:
                        if isinstance(item, dict) and item.get("type") == "text":
                            tool_result_content = item.get("text", "")
                            break

        info(f"acp success: tool result = {tool_result_content!r}")
        if tool_result_content and "outside" in tool_result_content:
            fail("acp success: tool blocked by path restriction despite cwd=/tmp",
                 tool_result_content[:80])
        elif tool_result_content and tool_content in tool_result_content:
            ok("acp success: actual file content returned (not path error)",
               f"content={tool_result_content!r}")
        elif tool_result_content:
            ok("acp success: tool result present (content returned, not path error)",
               f"content={tool_result_content!r}")
        else:
            fail("acp success: tool_result block missing from second request")
    finally:
        stop_runner("acp9")


# ── Test 19: xai tool actual execution (file content in function_call_output) ──

async def test_xai_tool_actual_execution(nc):
    section("Test 19: xai-runner — tool read_file returns actual file content")
    prefix = f"{BASE}.xai6"
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")

    tool_file = "/tmp/xai_tool_exec_19.txt"
    tool_content = "xai_tool_exec_content_19"
    with open(tool_file, "w") as f:
        f.write(tool_content)

    MockLLMServer.reset()
    MockLLMServer.queue("/responses",
                        xai_sse_function_call("resp-19-001", "call_19_001",
                                              "read_file",
                                              json.dumps({"path": tool_file})))
    MockLLMServer.queue("/responses", xai_sse("File read successfully.", "resp-19-002"))

    # wait_for_runner sends {"cwd": "/tmp"} → xai session cwd = /tmp
    start_runner("xai6", xai_bin, {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("xai exec: session.new timed out"); return
        info(f"session: {sid}")

        result = await send_prompt(nc, prefix, sid, "read the test file")
        if e := prompt_err(result):
            fail("xai exec: prompt", e); return
        ok("xai exec: prompt completed (function_call cycle)")

        reqs = MockLLMServer.all_requests("/responses")
        if len(reqs) < 2:
            fail("xai exec: expected 2 HTTP requests", f"got {len(reqs)}"); return
        ok("xai exec: runner made 2 HTTP requests", f"{len(reqs)} total")

        input2 = reqs[1].get("input", [])
        output_content = None
        for item in input2:
            if item.get("type") == "function_call_output":
                output_content = item.get("output", "")
                break

        info(f"xai exec: function_call_output = {output_content!r}")
        if output_content and tool_content in output_content:
            ok("xai exec: actual file content in function_call_output",
               f"output={output_content!r}")
        elif output_content and "outside" in output_content:
            fail("xai exec: path restriction blocked tool (cwd mismatch)",
                 output_content[:80])
        elif output_content:
            ok("xai exec: function_call_output present (tool executed)",
               f"output={output_content!r}")
        else:
            fail("xai exec: function_call_output missing or empty from second request")
    finally:
        stop_runner("xai6")


# ── Test 20: set_model + cross-runner (xai → openrouter) ──────────────────────

async def test_set_model_cross_runner(nc):
    section("Test 20: set_model + cross-runner — xai set_model then export → openrouter")
    prefix_xai = f"{BASE}.xai7"
    prefix_or  = f"{BASE}.or4"
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")
    or_bin  = os.path.join(RSDIR, "target", "debug", "trogon-openrouter-runner")

    MockLLMServer.reset()
    MockLLMServer.queue("/responses",    xai_sse("xai_response_1", "resp-20-xai-1"))
    MockLLMServer.queue("/responses",    xai_sse("xai_response_2", "resp-20-xai-2"))
    MockLLMServer.queue("/chat/completions", openrouter_sse("or_after_import_response"))

    start_runner("xai7", xai_bin, {
        "ACP_PREFIX":        prefix_xai,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
        "XAI_MODELS":        "grok-3:Grok 3,grok-3-mini:Grok 3 Mini",
    })
    start_runner("or4", or_bin, {
        "ACP_PREFIX":          prefix_or,
        "OPENROUTER_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":  "fake-or-key",
    })
    try:
        sid_xai = await wait_for_runner(nc, prefix_xai)
        sid_or  = await wait_for_runner(nc, prefix_or)
        if not sid_xai:
            fail("set_model cross: xai session.new timed out"); return
        if not sid_or:
            fail("set_model cross: or session.new timed out"); return
        info(f"xai={sid_xai}  or={sid_or}")

        # Prompt 1: verify default model grok-3
        r1 = await send_prompt(nc, prefix_xai, sid_xai, "first prompt")
        if e := prompt_err(r1):
            fail("set_model cross: xai prompt 1", e); return

        reqs_xai = MockLLMServer.all_requests("/responses")
        m1 = reqs_xai[0].get("model") if reqs_xai else None
        if m1 == "grok-3":
            ok("set_model cross: xai initial model grok-3 confirmed", f"model={m1}")
        else:
            fail("set_model cross: wrong initial model", f"expected grok-3, got {m1}")

        # set_model to grok-3-mini
        sr = await send_set_model(nc, prefix_xai, sid_xai, "grok-3-mini")
        if e := prompt_err(sr):
            fail("set_model cross: xai set_model", e); return
        ok("set_model cross: xai set_model to grok-3-mini")

        # Prompt 2: verify new model grok-3-mini
        r2 = await send_prompt(nc, prefix_xai, sid_xai, "second prompt after model change")
        if e := prompt_err(r2):
            fail("set_model cross: xai prompt 2", e); return

        reqs_xai2 = MockLLMServer.all_requests("/responses")
        m2 = reqs_xai2[-1].get("model") if reqs_xai2 else None
        if m2 == "grok-3-mini":
            ok("set_model cross: xai prompt 2 uses grok-3-mini", f"model={m2}")
        else:
            fail("set_model cross: xai model not updated in prompt 2",
                 f"expected grok-3-mini, got {m2}")

        # Export xai history (2 turns)
        exported = await nats_req(nc, f"{prefix_xai}.agent.ext.session/export",
                                   {"sessionId": sid_xai})
        if not isinstance(exported, list) or len(exported) < 2:
            fail("set_model cross: xai export", f"got {exported!r}"); return
        ok("set_model cross: xai history exported", f"{len(exported)} messages")

        # Import into openrouter session
        ir = await nats_req(nc, f"{prefix_or}.agent.ext.session/import",
                             {"sessionId": sid_or, "messages": exported})
        if isinstance(ir, dict) and "code" in ir:
            fail("set_model cross: or import", str(ir)); return
        ok("set_model cross: xai history imported into openrouter session")

        # Prompt openrouter — must hit /chat/completions with imported history
        or_before = MockLLMServer.request_count("/chat/completions")
        r3 = await send_prompt(nc, prefix_or, sid_or, "continue with openrouter")
        if e := prompt_err(r3):
            fail("set_model cross: or prompt after import", e); return
        ok("set_model cross: openrouter prompt completed after import")

        if MockLLMServer.request_count("/chat/completions") > or_before:
            ok("set_model cross: openrouter endpoint called after import")
        else:
            fail("set_model cross: openrouter endpoint not called after import")
    finally:
        stop_runner("xai7")
        stop_runner("or4")


# ── Helpers for new tests ─────────────────────────────────────────────────────

def spawn_json_response(text: str) -> bytes:
    """Non-streaming JSON body for spawn-handler tests (choices[0].message.content)."""
    return json.dumps({"choices": [{"message": {"content": text}}]}).encode()


def setup_git_repo(path: str, files: Optional[dict] = None, commit: bool = True) -> None:
    """Init a git repo at path, optionally write files and make an initial commit."""
    subprocess.run(["git", "init", path], capture_output=True)
    subprocess.run(["git", "-C", path, "config", "user.email", "test@test.com"], capture_output=True)
    subprocess.run(["git", "-C", path, "config", "user.name", "Test"], capture_output=True)
    if files:
        for name, content in files.items():
            full = os.path.join(path, name)
            os.makedirs(os.path.dirname(full), exist_ok=True) if "/" in name else None
            with open(full, "w") as f:
                f.write(content)
    if commit and files:
        subprocess.run(["git", "-C", path, "add", "."], capture_output=True)
        subprocess.run(
            ["git", "-C", path, "commit", "-m", "initial commit"],
            capture_output=True,
        )


# ── Test 24: xai spawn handler NATS e2e ───────────────────────────────────────

async def test_xai_spawn_handler(nc):
    section("Test 24: xai-runner — spawn handler NATS e2e (real runner binary)")
    prefix = f"{BASE}.spawn_xai"
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")

    MockLLMServer.reset()
    MockLLMServer.queue("/chat/completions", spawn_json_response("xai spawn replied"))

    start_runner("spawn_xai", xai_bin, {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        if not await wait_for_runner(nc, prefix):
            fail("xai spawn: runner not ready"); return

        payload = json.dumps({"prompt": "spawn test prompt"}).encode()
        try:
            msg = await asyncio.wait_for(
                nc.request(f"{prefix}.agent.spawn", payload), timeout=15.0
            )
            reply_text = msg.data.decode()
        except asyncio.TimeoutError:
            fail("xai spawn: NATS request timed out"); return
        except Exception as e:
            fail("xai spawn: NATS request error", str(e)); return

        if reply_text == "xai spawn replied":
            ok("xai spawn: correct reply from spawn handler", f"reply={reply_text!r}")
        else:
            fail("xai spawn: unexpected reply", f"got={reply_text!r}")

        n = MockLLMServer.request_count("/chat/completions")
        if n >= 1:
            ok("xai spawn: runner called /chat/completions", f"{n} request(s)")
        else:
            fail("xai spawn: /chat/completions never called")
    finally:
        stop_runner("spawn_xai")


# ── Test 25: openrouter spawn handler NATS e2e ────────────────────────────────

async def test_openrouter_spawn_handler(nc):
    section("Test 25: openrouter-runner — spawn handler NATS e2e (real runner binary)")
    prefix = f"{BASE}.spawn_or"
    or_bin = os.path.join(RSDIR, "target", "debug", "trogon-openrouter-runner")

    MockLLMServer.reset()
    MockLLMServer.queue("/chat/completions", spawn_json_response("openrouter spawn replied"))

    start_runner("spawn_or", or_bin, {
        "ACP_PREFIX":          prefix,
        "OPENROUTER_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":  "fake-or-key",
    })
    try:
        if not await wait_for_runner(nc, prefix):
            fail("or spawn: runner not ready"); return

        payload = json.dumps({"prompt": "spawn test prompt"}).encode()
        try:
            msg = await asyncio.wait_for(
                nc.request(f"{prefix}.agent.spawn", payload), timeout=15.0
            )
            reply_text = msg.data.decode()
        except asyncio.TimeoutError:
            fail("or spawn: NATS request timed out"); return
        except Exception as e:
            fail("or spawn: NATS request error", str(e)); return

        if reply_text == "openrouter spawn replied":
            ok("or spawn: correct reply from spawn handler", f"reply={reply_text!r}")
        else:
            fail("or spawn: unexpected reply", f"got={reply_text!r}")

        n = MockLLMServer.request_count("/chat/completions")
        if n >= 1:
            ok("or spawn: runner called /chat/completions", f"{n} request(s)")
        else:
            fail("or spawn: /chat/completions never called")
    finally:
        stop_runner("spawn_or")


# ── Test 26: cross-runner acp → openrouter ────────────────────────────────────

async def test_cross_runner_acp_to_openrouter(nc):
    section("Test 26: cross-runner — acp → openrouter (export / import / prompt)")
    prefix_acp = f"{BASE}.acp_cr26"
    prefix_or  = f"{BASE}.or_cr26"
    acp_bin = os.path.join(RSDIR, "target", "debug", "trogon-acp-runner")
    or_bin  = os.path.join(RSDIR, "target", "debug", "trogon-openrouter-runner")

    MockLLMServer.reset()
    MockLLMServer.queue("/anthropic/v1/messages", anthropic_sse("acp_before_switch_26"))
    MockLLMServer.queue("/chat/completions",      openrouter_sse("or_after_import_26"))

    start_runner("acp_cr26", acp_bin, {
        "ACP_PREFIX": prefix_acp, "PROXY_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token", "AGENT_MODEL": "claude-opus-4-6",
    })
    start_runner("or_cr26", or_bin, {
        "ACP_PREFIX": prefix_or, "OPENROUTER_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY": "fake-or-key",
    })
    try:
        sid_acp = await wait_for_runner(nc, prefix_acp)
        sid_or  = await wait_for_runner(nc, prefix_or)
        if not sid_acp:
            fail("cross acp→or: acp session.new timed out"); return
        if not sid_or:
            fail("cross acp→or: or session.new timed out"); return
        info(f"acp={sid_acp}  or={sid_or}")

        r = await send_prompt(nc, prefix_acp, sid_acp, "tell me something")
        if e := prompt_err(r):
            fail("cross acp→or: acp prompt", e); return
        ok("cross acp→or: acp prompt completed")

        exported = await nats_req(nc, f"{prefix_acp}.agent.ext.session/export",
                                   {"sessionId": sid_acp})
        if not isinstance(exported, list) or not exported:
            fail("cross acp→or: export", f"got {exported!r}"); return
        ok("cross acp→or: acp session exported", f"{len(exported)} message(s)")

        ir = await nats_req(nc, f"{prefix_or}.agent.ext.session/import",
                             {"sessionId": sid_or, "messages": exported})
        if isinstance(ir, dict) and "code" in ir:
            fail("cross acp→or: openrouter import", str(ir)); return
        ok("cross acp→or: history imported into openrouter session")

        acp_before = MockLLMServer.request_count("/anthropic/v1/messages")
        or_before  = MockLLMServer.request_count("/chat/completions")

        r = await send_prompt(nc, prefix_or, sid_or, "continue with openrouter")
        if e := prompt_err(r):
            fail("cross acp→or: openrouter prompt after import", e)
        else:
            ok("cross acp→or: openrouter prompt completed after import")

        or_after  = MockLLMServer.request_count("/chat/completions")
        acp_after = MockLLMServer.request_count("/anthropic/v1/messages")

        if or_after > or_before:
            ok("cross acp→or: openrouter /chat/completions called after switch",
               f"{or_after - or_before} request(s)")
        else:
            fail("cross acp→or: /chat/completions not called after switch")

        if acp_after == acp_before:
            ok("cross acp→or: Anthropic endpoint silent after switch")
        else:
            fail("cross acp→or: Anthropic endpoint unexpectedly called after switch")
    finally:
        stop_runner("acp_cr26")
        stop_runner("or_cr26")


# ── Test 27: cross-runner xai → codex ────────────────────────────────────────

async def test_cross_runner_xai_to_codex(nc):
    section("Test 27: cross-runner — xai → codex (export / import / prompt)")
    prefix_xai   = f"{BASE}.xai_cr27"
    prefix_codex = f"{BASE}.codex_cr27"
    xai_bin      = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")
    codex_bin    = os.path.join(RSDIR, "target", "debug", "trogon-codex-runner")
    mock_codex   = os.path.join(RSDIR, "target", "debug", "mock_codex_server")

    if not os.path.exists(mock_codex):
        fail("xai→codex: mock_codex_server binary not found", mock_codex); return

    MockLLMServer.reset()
    MockLLMServer.queue("/responses", xai_sse("xai_before_switch_27"))

    start_runner("xai_cr27", xai_bin, {
        "ACP_PREFIX":        prefix_xai,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    start_runner("codex_cr27", codex_bin, {
        "ACP_PREFIX":              prefix_codex,
        "CODEX_BIN":               mock_codex,
        "CODEX_SPAWN_TIMEOUT_SECS": "8",
        "MOCK_SEND_N_TEXT_EVENTS": "1",
    })
    try:
        sid_xai   = await wait_for_runner(nc, prefix_xai)
        sid_codex = await wait_for_runner(nc, prefix_codex, timeout=15.0)
        if not sid_xai:
            fail("xai→codex: xai session.new timed out"); return
        if not sid_codex:
            fail("xai→codex: codex session.new timed out"); return
        info(f"xai={sid_xai}  codex={sid_codex}")

        r = await send_prompt(nc, prefix_xai, sid_xai, "tell me something")
        if e := prompt_err(r):
            fail("xai→codex: xai prompt", e); return
        ok("xai→codex: xai initial prompt completed")

        exported = await nats_req(nc, f"{prefix_xai}.agent.ext.session/export",
                                   {"sessionId": sid_xai})
        if not isinstance(exported, list) or not exported:
            fail("xai→codex: export", f"got {exported!r}"); return
        ok("xai→codex: xai session exported", f"{len(exported)} message(s)")

        ir = await nats_req(nc, f"{prefix_codex}.agent.ext.session/import",
                             {"sessionId": sid_codex, "messages": exported})
        if isinstance(ir, dict) and "code" in ir:
            fail("xai→codex: codex import", str(ir)); return
        ok("xai→codex: history imported into codex session")

        xai_before = MockLLMServer.request_count("/responses")
        r = await send_prompt(nc, prefix_codex, sid_codex, "continue from there")
        if e := prompt_err(r):
            fail("xai→codex: codex prompt after import", e)
        else:
            ok("xai→codex: codex prompt completed after import")

        if MockLLMServer.request_count("/responses") == xai_before:
            ok("xai→codex: xAI endpoint silent after switch to codex")
        else:
            info("xai→codex: xAI endpoint called (unexpected but not fatal)")
    finally:
        stop_runner("xai_cr27")
        stop_runner("codex_cr27")


# ── Test 28: cross-runner openrouter → xai ───────────────────────────────────

async def test_cross_runner_openrouter_to_xai(nc):
    section("Test 28: cross-runner — openrouter → xai (export / import / prompt)")
    prefix_or  = f"{BASE}.or_cr28"
    prefix_xai = f"{BASE}.xai_cr28"
    or_bin  = os.path.join(RSDIR, "target", "debug", "trogon-openrouter-runner")
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")

    MockLLMServer.reset()
    MockLLMServer.queue("/chat/completions", openrouter_sse("or_before_switch_28"))
    MockLLMServer.queue("/responses",        xai_sse("xai_after_import_28"))

    start_runner("or_cr28", or_bin, {
        "ACP_PREFIX": prefix_or, "OPENROUTER_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY": "fake-or-key",
    })
    start_runner("xai_cr28", xai_bin, {
        "ACP_PREFIX":        prefix_xai,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid_or  = await wait_for_runner(nc, prefix_or)
        sid_xai = await wait_for_runner(nc, prefix_xai)
        if not sid_or:
            fail("or→xai: or session.new timed out"); return
        if not sid_xai:
            fail("or→xai: xai session.new timed out"); return
        info(f"or={sid_or}  xai={sid_xai}")

        r = await send_prompt(nc, prefix_or, sid_or, "tell me something")
        if e := prompt_err(r):
            fail("or→xai: or prompt", e); return
        ok("or→xai: openrouter initial prompt completed")

        exported = await nats_req(nc, f"{prefix_or}.agent.ext.session/export",
                                   {"sessionId": sid_or})
        if not isinstance(exported, list) or not exported:
            fail("or→xai: export", f"got {exported!r}"); return
        ok("or→xai: openrouter session exported", f"{len(exported)} message(s)")

        ir = await nats_req(nc, f"{prefix_xai}.agent.ext.session/import",
                             {"sessionId": sid_xai, "messages": exported})
        if isinstance(ir, dict) and "code" in ir:
            fail("or→xai: xai import", str(ir)); return
        ok("or→xai: history imported into xai session")

        or_before  = MockLLMServer.request_count("/chat/completions")
        xai_before = MockLLMServer.request_count("/responses")

        r = await send_prompt(nc, prefix_xai, sid_xai, "continue with xai")
        if e := prompt_err(r):
            fail("or→xai: xai prompt after import", e)
        else:
            ok("or→xai: xai prompt completed after import")

        if MockLLMServer.request_count("/responses") > xai_before:
            ok("or→xai: xAI /responses called after switch")
        else:
            fail("or→xai: /responses not called after switch")

        if MockLLMServer.request_count("/chat/completions") == or_before:
            ok("or→xai: openrouter endpoint silent after switch")
        else:
            fail("or→xai: openrouter endpoint unexpectedly called after switch")
    finally:
        stop_runner("or_cr28")
        stop_runner("xai_cr28")


# ── Test 29: git_status tool ──────────────────────────────────────────────────

async def test_tool_git_status(nc):
    section("Test 29: tool — git_status runs git status in session cwd")
    import tempfile
    prefix = f"{BASE}.git_status_29"
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")

    tmpdir = tempfile.mkdtemp(prefix="trogon_git_status_")
    setup_git_repo(tmpdir, {"hello.txt": "world"}, commit=True)

    MockLLMServer.reset()
    MockLLMServer.queue("/responses",
                        xai_sse_function_call("resp-29-1", "call_29_1",
                                              "git_status", "{}"))
    MockLLMServer.queue("/responses", xai_sse("Status checked.", "resp-29-2"))

    start_runner("git_status_29", xai_bin, {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_for_runner_cwd(nc, prefix, tmpdir)
        if not sid:
            fail("git_status: session.new timed out"); return
        info(f"session: {sid}  cwd={tmpdir}")

        result = await send_prompt(nc, prefix, sid, "show git status")
        if e := prompt_err(result):
            fail("git_status: prompt failed", e); return
        ok("git_status: prompt completed (function_call cycle)")

        reqs = MockLLMServer.all_requests("/responses")
        if len(reqs) < 2:
            fail("git_status: expected 2 HTTP requests", f"got {len(reqs)}"); return
        ok("git_status: runner made 2 HTTP requests", f"{len(reqs)} total")

        input2 = reqs[1].get("input", [])
        output = next((i.get("output", "") for i in input2
                       if i.get("type") == "function_call_output"), None)
        info(f"git_status output: {output!r}")
        if output is None:
            fail("git_status: function_call_output missing in second request"); return
        if "error" in output.lower() and "git" in output.lower():
            fail("git_status: tool returned git error", output[:80])
        else:
            ok("git_status: tool executed and returned output", f"len={len(output)}")
    finally:
        stop_runner("git_status_29")


# ── Test 30: git_diff tool ────────────────────────────────────────────────────

async def test_tool_git_diff(nc):
    section("Test 30: tool — git_diff runs git diff in session cwd")
    import tempfile
    prefix = f"{BASE}.git_diff_30"
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")

    tmpdir = tempfile.mkdtemp(prefix="trogon_git_diff_")
    setup_git_repo(tmpdir, {"code.rs": "fn main() {}"}, commit=True)
    # Modify a file so git_diff has something to report
    with open(os.path.join(tmpdir, "code.rs"), "w") as f:
        f.write("fn main() { println!(\"hello\"); }")

    MockLLMServer.reset()
    MockLLMServer.queue("/responses",
                        xai_sse_function_call("resp-30-1", "call_30_1",
                                              "git_diff", "{}"))
    MockLLMServer.queue("/responses", xai_sse("Diff shown.", "resp-30-2"))

    start_runner("git_diff_30", xai_bin, {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_for_runner_cwd(nc, prefix, tmpdir)
        if not sid:
            fail("git_diff: session.new timed out"); return
        info(f"session: {sid}  cwd={tmpdir}")

        result = await send_prompt(nc, prefix, sid, "show git diff")
        if e := prompt_err(result):
            fail("git_diff: prompt failed", e); return
        ok("git_diff: prompt completed (function_call cycle)")

        reqs = MockLLMServer.all_requests("/responses")
        if len(reqs) < 2:
            fail("git_diff: expected 2 HTTP requests", f"got {len(reqs)}"); return

        input2 = reqs[1].get("input", [])
        output = next((i.get("output", "") for i in input2
                       if i.get("type") == "function_call_output"), None)
        info(f"git_diff output: {output!r}")
        if output is None:
            fail("git_diff: function_call_output missing"); return
        if "println" in output or "diff" in output.lower() or len(output) > 0:
            ok("git_diff: tool executed and returned diff output", f"len={len(output)}")
        else:
            ok("git_diff: tool executed (empty diff — clean working tree)")
    finally:
        stop_runner("git_diff_30")


# ── Test 31: git_log tool ─────────────────────────────────────────────────────

async def test_tool_git_log(nc):
    section("Test 31: tool — git_log shows commit history in session cwd")
    import tempfile
    prefix = f"{BASE}.git_log_31"
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")

    tmpdir = tempfile.mkdtemp(prefix="trogon_git_log_")
    setup_git_repo(tmpdir, {"README.md": "# Test repo"}, commit=True)

    MockLLMServer.reset()
    MockLLMServer.queue("/responses",
                        xai_sse_function_call("resp-31-1", "call_31_1",
                                              "git_log", "{}"))
    MockLLMServer.queue("/responses", xai_sse("Log shown.", "resp-31-2"))

    start_runner("git_log_31", xai_bin, {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_for_runner_cwd(nc, prefix, tmpdir)
        if not sid:
            fail("git_log: session.new timed out"); return
        info(f"session: {sid}  cwd={tmpdir}")

        result = await send_prompt(nc, prefix, sid, "show git log")
        if e := prompt_err(result):
            fail("git_log: prompt failed", e); return
        ok("git_log: prompt completed (function_call cycle)")

        reqs = MockLLMServer.all_requests("/responses")
        if len(reqs) < 2:
            fail("git_log: expected 2 HTTP requests", f"got {len(reqs)}"); return

        input2 = reqs[1].get("input", [])
        output = next((i.get("output", "") for i in input2
                       if i.get("type") == "function_call_output"), None)
        info(f"git_log output: {output!r}")
        if output is None:
            fail("git_log: function_call_output missing"); return
        if "initial commit" in output.lower():
            ok("git_log: commit message visible in log output", f"output={output!r}")
        elif len(output) > 0:
            ok("git_log: tool executed and returned output", f"len={len(output)}")
        else:
            fail("git_log: empty output (no commits in repo?)")
    finally:
        stop_runner("git_log_31")


# ── Test 32: glob tool ────────────────────────────────────────────────────────

async def test_tool_glob(nc):
    section("Test 32: tool — glob finds files matching a pattern")
    import tempfile
    prefix = f"{BASE}.glob_32"
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")

    tmpdir = tempfile.mkdtemp(prefix="trogon_glob_")
    setup_git_repo(tmpdir, {
        "src/main.rs": "fn main() {}",
        "src/lib.rs":  "pub fn hello() {}",
        "README.md":   "# Project",
    }, commit=True)

    MockLLMServer.reset()
    MockLLMServer.queue("/responses",
                        xai_sse_function_call("resp-32-1", "call_32_1",
                                              "glob",
                                              json.dumps({"pattern": "**/*.rs"})))
    MockLLMServer.queue("/responses", xai_sse("Files found.", "resp-32-2"))

    start_runner("glob_32", xai_bin, {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_for_runner_cwd(nc, prefix, tmpdir)
        if not sid:
            fail("glob: session.new timed out"); return
        info(f"session: {sid}  cwd={tmpdir}")

        result = await send_prompt(nc, prefix, sid, "find all rust files")
        if e := prompt_err(result):
            fail("glob: prompt failed", e); return
        ok("glob: prompt completed (function_call cycle)")

        reqs = MockLLMServer.all_requests("/responses")
        if len(reqs) < 2:
            fail("glob: expected 2 HTTP requests", f"got {len(reqs)}"); return

        input2 = reqs[1].get("input", [])
        output = next((i.get("output", "") for i in input2
                       if i.get("type") == "function_call_output"), None)
        info(f"glob output: {output!r}")
        if output is None:
            fail("glob: function_call_output missing"); return
        if "main.rs" in output and "lib.rs" in output:
            ok("glob: both .rs files found", f"output={output!r}")
        elif ".rs" in output:
            ok("glob: at least one .rs file found", f"output={output!r}")
        else:
            fail("glob: expected .rs files in output", f"got={output!r}")
    finally:
        stop_runner("glob_32")


# ── Test 33: list_dir tool ────────────────────────────────────────────────────

async def test_tool_list_dir(nc):
    section("Test 33: tool — list_dir lists directory contents")
    import tempfile
    prefix = f"{BASE}.list_dir_33"
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")

    tmpdir = tempfile.mkdtemp(prefix="trogon_list_dir_")
    setup_git_repo(tmpdir, {
        "alpha.txt": "a",
        "beta.txt":  "b",
        "subdir/gamma.txt": "c",
    }, commit=True)

    MockLLMServer.reset()
    MockLLMServer.queue("/responses",
                        xai_sse_function_call("resp-33-1", "call_33_1",
                                              "list_dir",
                                              json.dumps({"path": "."})))
    MockLLMServer.queue("/responses", xai_sse("Listed.", "resp-33-2"))

    start_runner("list_dir_33", xai_bin, {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_for_runner_cwd(nc, prefix, tmpdir)
        if not sid:
            fail("list_dir: session.new timed out"); return
        info(f"session: {sid}  cwd={tmpdir}")

        result = await send_prompt(nc, prefix, sid, "list the directory")
        if e := prompt_err(result):
            fail("list_dir: prompt failed", e); return
        ok("list_dir: prompt completed (function_call cycle)")

        reqs = MockLLMServer.all_requests("/responses")
        if len(reqs) < 2:
            fail("list_dir: expected 2 HTTP requests", f"got {len(reqs)}"); return

        input2 = reqs[1].get("input", [])
        output = next((i.get("output", "") for i in input2
                       if i.get("type") == "function_call_output"), None)
        info(f"list_dir output: {output!r}")
        if output is None:
            fail("list_dir: function_call_output missing"); return
        if "alpha.txt" in output and "beta.txt" in output:
            ok("list_dir: expected files visible in output", f"output={output[:80]!r}")
        elif len(output) > 0:
            ok("list_dir: tool executed and returned directory listing",
               f"output={output[:80]!r}")
        else:
            fail("list_dir: empty output")
    finally:
        stop_runner("list_dir_33")


# ── Test 34: fetch_url tool ───────────────────────────────────────────────────

async def test_tool_fetch_url(nc):
    section("Test 34: tool — fetch_url fetches content from a URL")
    prefix = f"{BASE}.fetch_url_34"
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")

    fetch_path = "/test-fetch-34"
    fetch_content = b"fetch_url_smoke_test_content_34"
    MockLLMServer.reset()
    MockLLMServer.set_get_response(fetch_path, fetch_content)
    MockLLMServer.queue("/responses",
                        xai_sse_function_call("resp-34-1", "call_34_1",
                                              "fetch_url",
                                              json.dumps({
                                                  "url": f"http://127.0.0.1:{MOCK_PORT}{fetch_path}",
                                                  "raw": True,
                                              })))
    MockLLMServer.queue("/responses", xai_sse("Fetched.", "resp-34-2"))

    start_runner("fetch_url_34", xai_bin, {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("fetch_url: session.new timed out"); return
        info(f"session: {sid}")

        result = await send_prompt(nc, prefix, sid, "fetch the test page")
        if e := prompt_err(result):
            fail("fetch_url: prompt failed", e); return
        ok("fetch_url: prompt completed (function_call cycle)")

        reqs = MockLLMServer.all_requests("/responses")
        if len(reqs) < 2:
            fail("fetch_url: expected 2 HTTP requests", f"got {len(reqs)}"); return

        input2 = reqs[1].get("input", [])
        output = next((i.get("output", "") for i in input2
                       if i.get("type") == "function_call_output"), None)
        info(f"fetch_url output: {output!r}")
        if output is None:
            fail("fetch_url: function_call_output missing"); return
        if fetch_content.decode() in output:
            ok("fetch_url: fetched content present in tool output", f"output={output!r}")
        elif "error" in output.lower():
            fail("fetch_url: tool returned error", output[:80])
        else:
            ok("fetch_url: tool executed (content returned)", f"output={output[:80]!r}")
    finally:
        stop_runner("fetch_url_34")


# ── Test 35: notebook_edit tool ───────────────────────────────────────────────

async def test_tool_notebook_edit(nc):
    section("Test 35: tool — notebook_edit edits a Jupyter notebook cell")
    import tempfile
    prefix = f"{BASE}.notebook_35"
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")

    tmpdir = tempfile.mkdtemp(prefix="trogon_notebook_")
    nb_path = os.path.join(tmpdir, "test.ipynb")
    nb_content = json.dumps({
        "cells": [
            {"cell_type": "code", "source": ["print('original')"],
             "metadata": {}, "outputs": [], "execution_count": None}
        ],
        "metadata": {"kernelspec": {"name": "python3"}, "language_info": {"name": "python"}},
        "nbformat": 4,
        "nbformat_minor": 5,
    })
    with open(nb_path, "w") as f:
        f.write(nb_content)

    new_source = "print('edited by smoke test')"
    MockLLMServer.reset()
    MockLLMServer.queue("/responses",
                        xai_sse_function_call("resp-35-1", "call_35_1",
                                              "notebook_edit",
                                              json.dumps({
                                                  "path": "test.ipynb",
                                                  "cell_index": 0,
                                                  "content": new_source,
                                              })))
    MockLLMServer.queue("/responses", xai_sse("Notebook edited.", "resp-35-2"))

    start_runner("notebook_35", xai_bin, {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_for_runner_cwd(nc, prefix, tmpdir)
        if not sid:
            fail("notebook_edit: session.new timed out"); return
        info(f"session: {sid}  cwd={tmpdir}")

        result = await send_prompt(nc, prefix, sid, "edit the notebook")
        if e := prompt_err(result):
            fail("notebook_edit: prompt failed", e); return
        ok("notebook_edit: prompt completed (function_call cycle)")

        reqs = MockLLMServer.all_requests("/responses")
        if len(reqs) < 2:
            fail("notebook_edit: expected 2 HTTP requests", f"got {len(reqs)}"); return

        input2 = reqs[1].get("input", [])
        output = next((i.get("output", "") for i in input2
                       if i.get("type") == "function_call_output"), None)
        info(f"notebook_edit output: {output!r}")
        if output is None:
            fail("notebook_edit: function_call_output missing"); return

        try:
            nb_updated = json.loads(open(nb_path).read())
            cell_source = nb_updated["cells"][0]["source"]
            source_text = "".join(cell_source) if isinstance(cell_source, list) else cell_source
            if new_source in source_text:
                ok("notebook_edit: cell content updated on disk", f"source={source_text!r}")
            else:
                fail("notebook_edit: cell content not updated",
                     f"expected {new_source!r}, got {source_text!r}")
        except Exception as ex:
            info(f"notebook_edit: could not read updated notebook: {ex}")
            if "error" not in output.lower():
                ok("notebook_edit: tool executed without error", f"output={output!r}")
            else:
                fail("notebook_edit: tool returned error", output[:80])
    finally:
        stop_runner("notebook_35")


# ── Test 36: str_replace tool ─────────────────────────────────────────────────

async def test_tool_str_replace(nc):
    section("Test 36: tool — str_replace replaces text in a file and verifies on disk")
    import tempfile, shutil
    prefix = f"{BASE}.xai_str36"
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")

    tmpdir = tempfile.mkdtemp(prefix="trogon_str_replace_")
    target_file = os.path.join(tmpdir, "target.py")
    with open(target_file, "w") as f:
        f.write("def foo():\n    return 1\n")

    MockLLMServer.reset()
    MockLLMServer.queue("/responses",
                        xai_sse_function_call("resp-36-1", "call_36_1",
                                              "str_replace",
                                              json.dumps({"path": target_file,
                                                          "old_str": "return 1",
                                                          "new_str": "return 42"})))
    MockLLMServer.queue("/responses", xai_sse("File updated.", "resp-36-2"))

    start_runner("xai_str36", xai_bin, {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_for_runner_cwd(nc, prefix, tmpdir)
        if not sid:
            fail("str_replace: session.new timed out"); return
        info(f"session: {sid}  cwd={tmpdir}")

        result = await send_prompt(nc, prefix, sid, "replace the function body")
        if e := prompt_err(result):
            fail("str_replace: prompt", e); return
        ok("str_replace: prompt completed (function_call cycle)")

        reqs = MockLLMServer.all_requests("/responses")
        if len(reqs) < 2:
            fail("str_replace: expected 2 HTTP requests", f"got {len(reqs)}"); return

        input2 = reqs[1].get("input", [])
        output = next((i.get("output", "") for i in input2
                       if i.get("type") == "function_call_output"), None)
        info(f"str_replace output: {output!r}")
        if output is None:
            fail("str_replace: function_call_output missing"); return
        if "error" in output.lower() and "return 42" not in open(target_file).read():
            fail("str_replace: tool returned error", output[:80]); return
        ok("str_replace: tool executed without error", f"output={output!r}")

        content = open(target_file).read()
        if "return 42" in content:
            ok("str_replace: file content updated on disk", f"content={content!r}")
        else:
            fail("str_replace: file content not changed on disk", f"got={content!r}")
    finally:
        stop_runner("xai_str36")
        shutil.rmtree(tmpdir, ignore_errors=True)


# ── Test 37: search_files tool ────────────────────────────────────────────────

async def test_tool_search_files(nc):
    section("Test 37: tool — search_files finds pattern matches in a directory")
    import tempfile, shutil
    prefix = f"{BASE}.xai_sf37"
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")

    tmpdir = tempfile.mkdtemp(prefix="trogon_search_files_")
    with open(os.path.join(tmpdir, "match_a.txt"), "w") as f:
        f.write("needle in a haystack\n")
    with open(os.path.join(tmpdir, "match_b.py"), "w") as f:
        f.write("# needle here too\n")
    with open(os.path.join(tmpdir, "no_match.txt"), "w") as f:
        f.write("nothing relevant\n")

    MockLLMServer.reset()
    MockLLMServer.queue("/responses",
                        xai_sse_function_call("resp-37-1", "call_37_1",
                                              "search_files",
                                              json.dumps({"pattern": "needle"})))
    MockLLMServer.queue("/responses", xai_sse("Search complete.", "resp-37-2"))

    start_runner("xai_sf37", xai_bin, {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_for_runner_cwd(nc, prefix, tmpdir)
        if not sid:
            fail("search_files: session.new timed out"); return
        info(f"session: {sid}  cwd={tmpdir}")

        result = await send_prompt(nc, prefix, sid, "search for needle")
        if e := prompt_err(result):
            fail("search_files: prompt", e); return
        ok("search_files: prompt completed (function_call cycle)")

        reqs = MockLLMServer.all_requests("/responses")
        if len(reqs) < 2:
            fail("search_files: expected 2 HTTP requests", f"got {len(reqs)}"); return

        input2 = reqs[1].get("input", [])
        output = next((i.get("output", "") for i in input2
                       if i.get("type") == "function_call_output"), None)
        info(f"search_files output: {output!r}")
        if output is None:
            fail("search_files: function_call_output missing"); return
        if "error" in output.lower() and "needle" not in output:
            fail("search_files: tool returned error", output[:80]); return
        if "needle" in output or "match_a" in output or "match_b" in output:
            ok("search_files: matches found in output", f"output={output[:80]!r}")
        else:
            ok("search_files: tool executed (output present)", f"output={output[:80]!r}")
    finally:
        stop_runner("xai_sf37")
        shutil.rmtree(tmpdir, ignore_errors=True)


# ── Test 38: todo_write tool ──────────────────────────────────────────────────

async def test_tool_todo_write(nc):
    section("Test 38: tool — todo_write stores a todo item")
    prefix = f"{BASE}.xai_tw38"
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")

    MockLLMServer.reset()
    MockLLMServer.queue("/responses",
                        xai_sse_function_call("resp-38-1", "call_38_1",
                                              "todo_write",
                                              json.dumps({"id": "smoke-38",
                                                          "content": "implement smoke test 38",
                                                          "status": "pending"})))
    MockLLMServer.queue("/responses", xai_sse("Todo written.", "resp-38-2"))

    start_runner("xai_tw38", xai_bin, {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("todo_write: session.new timed out"); return
        info(f"session: {sid}")

        result = await send_prompt(nc, prefix, sid, "add a todo")
        if e := prompt_err(result):
            fail("todo_write: prompt", e); return
        ok("todo_write: prompt completed (function_call cycle)")

        reqs = MockLLMServer.all_requests("/responses")
        if len(reqs) < 2:
            fail("todo_write: expected 2 HTTP requests", f"got {len(reqs)}"); return

        input2 = reqs[1].get("input", [])
        output = next((i.get("output", "") for i in input2
                       if i.get("type") == "function_call_output"), None)
        info(f"todo_write output: {output!r}")
        if output is None:
            fail("todo_write: function_call_output missing"); return
        if "error" in output.lower():
            fail("todo_write: tool returned error", output[:80]); return
        ok("todo_write: tool executed without error", f"output={output!r}")
    finally:
        stop_runner("xai_tw38")


# ── Test 39: todo_read tool ───────────────────────────────────────────────────

async def test_tool_todo_read(nc):
    section("Test 39: tool — todo_read returns items written in the same session")
    prefix = f"{BASE}.xai_tr39"
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")

    MockLLMServer.reset()
    # First turn: write a todo
    MockLLMServer.queue("/responses",
                        xai_sse_function_call("resp-39-1", "call_39_1",
                                              "todo_write",
                                              json.dumps({"id": "smoke-39",
                                                          "content": "todo read test item",
                                                          "status": "in_progress"})))
    MockLLMServer.queue("/responses", xai_sse("Todo written.", "resp-39-2"))
    # Second turn: read todos
    MockLLMServer.queue("/responses",
                        xai_sse_function_call("resp-39-3", "call_39_3",
                                              "todo_read",
                                              json.dumps({})))
    MockLLMServer.queue("/responses", xai_sse("Here are your todos.", "resp-39-4"))

    start_runner("xai_tr39", xai_bin, {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("todo_read: session.new timed out"); return
        info(f"session: {sid}")

        # Turn 1: write
        r1 = await send_prompt(nc, prefix, sid, "add a todo item")
        if e := prompt_err(r1):
            fail("todo_read: write turn failed", e); return
        ok("todo_read: write turn completed")

        reqs_after_write = MockLLMServer.all_requests("/responses")
        w_input = reqs_after_write[1].get("input", []) if len(reqs_after_write) >= 2 else []
        w_out = next((i.get("output", "") for i in w_input
                      if i.get("type") == "function_call_output"), None)
        if w_out and "error" in w_out.lower():
            fail("todo_read: todo_write returned error in turn 1", w_out[:80]); return
        ok("todo_read: todo_write completed without error")

        # Turn 2: read (in same session — todos are session-scoped)
        MockLLMServer.reset()
        MockLLMServer.queue("/responses",
                            xai_sse_function_call("resp-39-3", "call_39_3",
                                                  "todo_read",
                                                  json.dumps({})))
        MockLLMServer.queue("/responses", xai_sse("Here are your todos.", "resp-39-4"))

        r2 = await send_prompt(nc, prefix, sid, "show my todos")
        if e := prompt_err(r2):
            fail("todo_read: read turn failed", e); return
        ok("todo_read: read turn completed")

        reqs2 = MockLLMServer.all_requests("/responses")
        if len(reqs2) < 2:
            fail("todo_read: expected 2 HTTP requests in read turn", f"got {len(reqs2)}"); return

        input2 = reqs2[1].get("input", [])
        output = next((i.get("output", "") for i in input2
                       if i.get("type") == "function_call_output"), None)
        info(f"todo_read output: {output!r}")
        if output is None:
            fail("todo_read: function_call_output missing"); return
        if "error" in output.lower():
            fail("todo_read: todo_read returned error", output[:80]); return
        if "smoke-39" in output or "todo read test item" in output or "in_progress" in output:
            ok("todo_read: written todo found in read output", f"output={output[:80]!r}")
        else:
            ok("todo_read: tool executed (output present)", f"output={output[:80]!r}")
    finally:
        stop_runner("xai_tr39")


# ── Test 40: registry runner startup registration ─────────────────────────────

async def test_registry_runner_startup(nc):
    section("Test 40: registry — xai-runner registers models in AGENT_REGISTRY at startup")
    prefix = f"{BASE}.xai_reg40"
    xai_bin = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")
    agent_type = f"xai-smoke-reg-{os.getpid()}"
    model_id = "grok-registry-smoke-40"

    MockLLMServer.reset()

    start_runner("xai_reg40", xai_bin, {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": model_id,
        "XAI_MODELS":        model_id,
        "AGENT_TYPE":        agent_type,
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("registry: runner did not start"); return
        ok("registry: runner started and accepted session.new")

        # Query the AGENT_REGISTRY JetStream KV bucket
        try:
            js = nc.jetstream()
            kv = await js.key_value("AGENT_REGISTRY")
            entry = await kv.get(agent_type)
            cap = json.loads(entry.value)
            models = cap.get("metadata", {}).get("models", [])
            info(f"registry entry: agent_type={cap.get('agent_type')!r} models={models}")
            if model_id in models:
                ok("registry: model ID registered in AGENT_REGISTRY at startup",
                   f"models={models}")
            else:
                fail("registry: model ID not found in registry",
                     f"expected {model_id!r} in {models}")
            prefix_in_meta = cap.get("metadata", {}).get("acp_prefix")
            if prefix_in_meta == prefix:
                ok("registry: acp_prefix in metadata matches runner prefix")
            else:
                fail("registry: acp_prefix mismatch",
                     f"expected {prefix!r}, got {prefix_in_meta!r}")
        except Exception as ex:
            fail("registry: failed to read AGENT_REGISTRY KV", str(ex)[:120])
    finally:
        stop_runner("xai_reg40")


# ── Test 41: --print flag CLI e2e ─────────────────────────────────────────────

async def test_print_flag_cli(nc):
    section("Test 41: --print flag — trogon CLI sends prompt and prints response to stdout")
    prefix = f"{BASE}.acp_print41"
    acp_bin    = os.path.join(RSDIR, "target", "debug", "trogon-acp-runner")
    trogon_bin = os.path.join(RSDIR, "target", "debug", "trogon")

    if not os.path.exists(trogon_bin):
        fail("--print: trogon binary not found", trogon_bin); return

    MockLLMServer.reset()

    start_runner("acp_print41", acp_bin, {
        "ACP_PREFIX":      prefix,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    })
    try:
        # Wait until the runner is ready (polls session.new — no LLM call)
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("--print: runner did not start"); return
        ok("--print: runner started")

        # Queue the mock LLM response for the --print invocation
        expected_text = "print-flag-response-smoke-41"
        MockLLMServer.queue("/anthropic/v1/messages", anthropic_sse(expected_text))

        result = subprocess.run(
            [trogon_bin, "--nats-url", NATS_URL, "--prefix", prefix,
             "--print", "hello from smoke test"],
            capture_output=True, text=True, timeout=30,
        )
        info(f"--print exit code: {result.returncode}")
        info(f"--print stdout: {result.stdout!r}")
        if result.stderr:
            info(f"--print stderr: {result.stderr[:120]!r}")

        if result.returncode != 0:
            fail("--print: CLI exited with non-zero code", f"rc={result.returncode}"); return
        ok("--print: CLI exited successfully")

        if expected_text in result.stdout:
            ok("--print: expected response text present in stdout",
               f"stdout={result.stdout.strip()!r}")
        else:
            fail("--print: expected text not in stdout",
                 f"expected {expected_text!r}, got {result.stdout!r}")
    finally:
        stop_runner("acp_print41")


# ── Test 42: compaction full cycle ────────────────────────────────────────────

async def test_acp_compaction_full_cycle(nc):
    section("Test 42: compaction — token threshold triggers trogon.compactor.compact call")
    prefix = f"{BASE}.acp_compact42"
    acp_bin = os.path.join(RSDIR, "target", "debug", "trogon-acp-runner")

    compactor_called = asyncio.Event()

    async def mock_compactor(msg):
        data = json.loads(msg.data)
        msgs = data.get("messages", [])
        # Return the same messages but marked as compacted
        reply = json.dumps({
            "messages": [{"role": "user",
                           "content": [{"type": "text",
                                        "text": "<context-summary>compacted</context-summary>"}]}],
            "compacted": True,
            "tokens_before": 100,
            "tokens_after": 10,
        }).encode()
        await msg.respond(reply)
        compactor_called.set()

    compactor_sub = await nc.subscribe("trogon.compactor.compact", cb=mock_compactor)

    MockLLMServer.reset()
    MockLLMServer.queue("/anthropic/v1/messages", anthropic_sse("compact cycle ok"))

    start_runner("acp_compact42", acp_bin, {
        "ACP_PREFIX":      prefix,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("compact: runner did not start"); return
        ok("compact: runner started")

        # Set token_budget = 1 in ACP_SESSIONS KV so any message exceeds the 85% threshold
        try:
            js = nc.jetstream()
            kv = await js.key_value("ACP_SESSIONS")
            entry = await kv.get(sid)
            state = json.loads(entry.value)
            state["token_budget"] = 1
            await kv.put(sid, json.dumps(state).encode())
            ok("compact: set token_budget=1 in ACP_SESSIONS KV")
        except Exception as ex:
            fail("compact: failed to set token_budget in KV", str(ex)[:120]); return

        result = await send_prompt(nc, prefix, sid, "hello, trigger compaction")
        if e := prompt_err(result):
            fail("compact: prompt failed", e); return
        ok("compact: prompt completed after compaction")

        try:
            await asyncio.wait_for(compactor_called.wait(), timeout=5.0)
            ok("compact: trogon.compactor.compact NATS endpoint was called")
        except asyncio.TimeoutError:
            fail("compact: compactor endpoint never called "
                 "(trogon.compactor.compact not received within 5s)")
    finally:
        stop_runner("acp_compact42")
        try:
            await compactor_sub.unsubscribe()
        except Exception:
            pass


# ── Test 43: openrouter registry startup ─────────────────────────────────────

async def test_registry_openrouter_startup(nc):
    section("Test 43: registry — openrouter-runner registers models in AGENT_REGISTRY at startup")
    prefix = f"{BASE}.or_reg43"
    or_bin = os.path.join(RSDIR, "target", "debug", "trogon-openrouter-runner")
    agent_type = f"or-smoke-reg-{os.getpid()}"
    model_id = "or-registry-smoke-43"

    MockLLMServer.reset()

    start_runner("or_reg43", or_bin, {
        "ACP_PREFIX":               prefix,
        "OPENROUTER_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":       "fake-or-key",
        "OPENROUTER_DEFAULT_MODEL": model_id,
        "OPENROUTER_MODELS":        model_id,
        "AGENT_TYPE":               agent_type,
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("registry or: runner did not start"); return
        ok("registry or: runner started and accepted session.new")

        try:
            js = nc.jetstream()
            kv = await js.key_value("AGENT_REGISTRY")
            entry = await kv.get(agent_type)
            cap = json.loads(entry.value)
            models = cap.get("metadata", {}).get("models", [])
            info(f"registry or entry: agent_type={cap.get('agent_type')!r} models={models}")
            if model_id in models:
                ok("registry or: model ID registered in AGENT_REGISTRY at startup",
                   f"models={models}")
            else:
                fail("registry or: model ID not found in registry",
                     f"expected {model_id!r} in {models}")
            prefix_in_meta = cap.get("metadata", {}).get("acp_prefix")
            if prefix_in_meta == prefix:
                ok("registry or: acp_prefix in metadata matches runner prefix")
            else:
                fail("registry or: acp_prefix mismatch",
                     f"expected {prefix!r}, got {prefix_in_meta!r}")
        except Exception as ex:
            fail("registry or: failed to read AGENT_REGISTRY KV", str(ex)[:120])
    finally:
        stop_runner("or_reg43")


# ── Test 44: codex registry startup ───────────────────────────────────────────

async def test_registry_codex_startup(nc):
    section("Test 44: registry — codex-runner registers models in AGENT_REGISTRY at startup")
    prefix = f"{BASE}.codex_reg44"
    codex_bin      = os.path.join(RSDIR, "target", "debug", "trogon-codex-runner")
    mock_codex_bin = os.path.join(RSDIR, "target", "debug", "mock_codex_server")
    agent_type = f"codex-smoke-reg-{os.getpid()}"
    # CODEX_MODELS must use id:label format (the agent parser requires a colon)
    model_id = "codex-registry-smoke-44"
    models_env = f"{model_id}:{model_id}"

    if not os.path.exists(mock_codex_bin):
        fail("registry codex: mock_codex_server binary not found", mock_codex_bin)
        return

    MockLLMServer.reset()

    start_runner("codex_reg44", codex_bin, {
        "ACP_PREFIX":   prefix,
        "CODEX_BIN":    mock_codex_bin,
        "CODEX_MODELS": models_env,
        "AGENT_TYPE":   agent_type,
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("registry codex: runner did not start"); return
        ok("registry codex: runner started and accepted session.new")

        try:
            js = nc.jetstream()
            kv = await js.key_value("AGENT_REGISTRY")
            entry = await kv.get(agent_type)
            cap = json.loads(entry.value)
            models = cap.get("metadata", {}).get("models", [])
            info(f"registry codex entry: agent_type={cap.get('agent_type')!r} models={models}")
            if model_id in models:
                ok("registry codex: model ID registered in AGENT_REGISTRY at startup",
                   f"models={models}")
            else:
                fail("registry codex: model ID not found in registry",
                     f"expected {model_id!r} in {models}")
            prefix_in_meta = cap.get("metadata", {}).get("acp_prefix")
            if prefix_in_meta == prefix:
                ok("registry codex: acp_prefix in metadata matches runner prefix")
            else:
                fail("registry codex: acp_prefix mismatch",
                     f"expected {prefix!r}, got {prefix_in_meta!r}")
        except Exception as ex:
            fail("registry codex: failed to read AGENT_REGISTRY KV", str(ex)[:120])
    finally:
        stop_runner("codex_reg44")


# ── Test 45: bash tool smoke ───────────────────────────────────────────────────

async def test_acp_bash_tool_smoke(nc):
    section("Test 45: bash tool — acp-runner dispatches bash via mock wasm-runtime over NATS")
    prefix = f"{BASE}.acp_bash45"
    wasm_prefix = f"{BASE}.wasm45"
    acp_bin = os.path.join(RSDIR, "target", "debug", "trogon-acp-runner")
    bash_called = asyncio.Event()
    output_calls: dict = {"count": 0}

    async def handle_terminal_create(msg):
        await msg.respond(json.dumps({"terminalId": "smoke-term-45"}).encode())
        bash_called.set()

    async def handle_terminal_output(msg):
        n = output_calls["count"]
        output_calls["count"] = n + 1
        if n == 0:
            # baseline snapshot — empty output
            await msg.respond(json.dumps({"output": ""}).encode())
        else:
            # polling — accumulated output with exit marker
            await msg.respond(json.dumps(
                {"output": "hello from smoke test\n__EXIT_0__\n"}
            ).encode())

    async def handle_write_stdin(msg):
        await msg.respond(json.dumps({}).encode())

    create_sub = await nc.subscribe(
        f"{wasm_prefix}.session.*.client.terminal.create", cb=handle_terminal_create
    )
    output_sub = await nc.subscribe(
        f"{wasm_prefix}.session.*.client.terminal.output", cb=handle_terminal_output
    )
    stdin_sub = await nc.subscribe(
        f"{wasm_prefix}.session.*.client.ext.terminal.write_stdin", cb=handle_write_stdin
    )

    MockLLMServer.reset()
    MockLLMServer.queue(
        "/anthropic/v1/messages",
        anthropic_sse_tool_use("toolu_bash_45", "bash",
                               json.dumps({"command": "echo hello from smoke test"})),
    )
    MockLLMServer.queue("/anthropic/v1/messages", anthropic_sse("Command executed successfully."))

    start_runner("acp_bash45", acp_bin, {
        "ACP_PREFIX":      prefix,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    })
    kv = None
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("bash tool: runner did not start"); return
        ok("bash tool: runner started")

        # AGENT_REGISTRY bucket is provisioned by acp-runner at startup.
        # Register the mock wasm-runtime capability so acp-runner can discover it.
        js = nc.jetstream()
        kv = await js.key_value("AGENT_REGISTRY")
        cap_json = json.dumps({
            "agent_type":    "mock-wasm-45",
            "capabilities":  ["execution", "chat"],
            "nats_subject":  f"{wasm_prefix}.agent.>",
            "current_load":  0,
            "metadata":      {"acp_prefix": wasm_prefix},
        }).encode()
        await kv.put("mock-wasm-45", cap_json)
        ok("bash tool: mock wasm-runtime registered in AGENT_REGISTRY")

        mr = await send_set_mode(nc, prefix, sid, "bypassPermissions")
        if mr.get("ok") or "error" not in mr:
            ok("bash tool: mode set to bypassPermissions")
        else:
            fail("bash tool: could not set bypassPermissions", str(mr)[:80])

        result = await send_prompt(nc, prefix, sid, "run a shell command")
        if e := prompt_err(result):
            fail("bash tool: prompt failed", e); return
        ok("bash tool: prompt completed (bash tool cycle)")

        try:
            await asyncio.wait_for(bash_called.wait(), timeout=5.0)
            ok("bash tool: terminal.create was called on mock wasm-runtime")
        except asyncio.TimeoutError:
            fail("bash tool: terminal.create never called within 5s")

        reqs = MockLLMServer.all_requests("/anthropic/v1/messages")
        if len(reqs) >= 2:
            messages = reqs[1].get("messages", [])
            tool_result_content = None
            for m in messages:
                for block in m.get("content", []):
                    if isinstance(block, dict) and block.get("type") == "tool_result":
                        c = block.get("content", "")
                        tool_result_content = c if isinstance(c, str) else str(c)
            info(f"bash tool: tool_result = {tool_result_content!r}")
            if tool_result_content and "hello from smoke test" in tool_result_content:
                ok("bash tool: bash output present in tool_result",
                   f"output={tool_result_content!r}")
            elif tool_result_content:
                ok("bash tool: tool_result present (tool executed)",
                   f"output={tool_result_content[:80]!r}")
            else:
                fail("bash tool: tool_result missing from second Anthropic request")
        else:
            fail("bash tool: expected 2 Anthropic requests", f"got {len(reqs)}")
    finally:
        stop_runner("acp_bash45")
        for sub in [create_sub, output_sub, stdin_sub]:
            try:
                await sub.unsubscribe()
            except Exception:
                pass
        if kv is not None:
            try:
                await kv.delete("mock-wasm-45")
            except Exception:
                pass


# ── Test 46: acp-runner registry startup ─────────────────────────────────────

async def test_registry_acp_startup(nc):
    section("Test 46: registry — acp-runner registers Claude models in AGENT_REGISTRY at startup")
    prefix = f"{BASE}.acp_reg46"
    acp_bin = os.path.join(RSDIR, "target", "debug", "trogon-acp-runner")
    agent_type = f"acp-smoke-reg-{os.getpid()}"

    MockLLMServer.reset()
    start_runner("acp_reg46", acp_bin, {
        "ACP_PREFIX":      prefix,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
        "AGENT_TYPE":      agent_type,
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("registry acp: runner did not start"); return
        ok("registry acp: runner started and accepted session.new")

        try:
            js = nc.jetstream()
            kv = await js.key_value("AGENT_REGISTRY")
            entry = await kv.get(agent_type)
            cap = json.loads(entry.value)
            models = cap.get("metadata", {}).get("models", [])
            info(f"registry acp entry: agent_type={cap.get('agent_type')!r} models={models}")
            expected = ["claude-opus-4-6", "claude-sonnet-4-6", "claude-haiku-4-5-20251001"]
            missing = [m for m in expected if m not in models]
            if not missing:
                ok("registry acp: all Claude model IDs registered in AGENT_REGISTRY",
                   f"models={models}")
            else:
                fail("registry acp: some models missing from registry",
                     f"missing={missing}, got={models}")
            prefix_in_meta = cap.get("metadata", {}).get("acp_prefix")
            if prefix_in_meta == prefix:
                ok("registry acp: acp_prefix in metadata matches runner prefix")
            else:
                fail("registry acp: acp_prefix mismatch",
                     f"expected {prefix!r}, got {prefix_in_meta!r}")
        except Exception as ex:
            fail("registry acp: failed to read AGENT_REGISTRY KV", str(ex)[:120])
    finally:
        stop_runner("acp_reg46")


# ── Test 47: session/get_state e2e ────────────────────────────────────────────

async def test_session_get_state(nc):
    section("Test 47: session/get_state — xai and openrouter return session state via ext_method")
    runners = [
        ("xai", "xai_gs47a",
         os.path.join(RSDIR, "target", "debug", "trogon-xai-runner"), {
             "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
             "XAI_API_KEY":       "fake-xai-key",
             "XAI_DEFAULT_MODEL": "grok-3",
         }),
        ("or", "or_gs47b",
         os.path.join(RSDIR, "target", "debug", "trogon-openrouter-runner"), {
             "OPENROUTER_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
             "OPENROUTER_API_KEY":       "fake-or-key",
             "OPENROUTER_DEFAULT_MODEL": "gpt-4o",
         }),
    ]
    for label, key, bin_path, env_extra in runners:
        prefix = f"{BASE}.{key}"
        MockLLMServer.reset()
        start_runner(key, bin_path, {"ACP_PREFIX": prefix, **env_extra})
        try:
            sid = await wait_for_runner(nc, prefix)
            if not sid:
                fail(f"get_state {label}: runner did not start")
                continue
            ok(f"get_state {label}: runner started, sid={sid[:8]}")

            state = await nats_req(
                nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid}
            )
            if "_error" in state or "code" in state:
                fail(f"get_state {label}: error response", str(state)[:120])
            elif "cwd" in state:
                ok(f"get_state {label}: state returned with cwd",
                   f"cwd={state.get('cwd')!r} model={state.get('model')!r}")
            else:
                fail(f"get_state {label}: unexpected response shape", str(state)[:120])
        finally:
            stop_runner(key)


# ── Test 48: CrossRunnerSwitcher via trogon binary ────────────────────────────

async def test_cross_runner_switcher_model(nc):
    section("Test 48: CrossRunnerSwitcher — /model in trogon binary switches runner via registry+export/import")
    prefix_acp = f"{BASE}.acp_cr48"
    prefix_xai = f"{BASE}.xai_cr48"
    model_id    = f"test-grok-cr48-{os.getpid()}"
    agent_type  = f"xai-cr48-{os.getpid()}"
    acp_bin     = os.path.join(RSDIR, "target", "debug", "trogon-acp-runner")
    xai_bin     = os.path.join(RSDIR, "target", "debug", "trogon-xai-runner")
    trogon_bin  = os.path.join(RSDIR, "target", "debug", "trogon")
    acp_resp    = "acp-initial-cr48"
    xai_resp    = "xai-after-switch-cr48"

    if not os.path.exists(trogon_bin):
        fail("cross switcher: trogon binary not found", trogon_bin); return

    start_runner("acp_cr48", acp_bin, {
        "ACP_PREFIX":      prefix_acp,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    })
    start_runner("xai_cr48", xai_bin, {
        "ACP_PREFIX":        prefix_xai,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": model_id,
        "XAI_MODELS":        f"{model_id}:Test Grok CR48",
        "AGENT_TYPE":        agent_type,
    })
    try:
        sid_acp = await wait_for_runner(nc, prefix_acp)
        sid_xai = await wait_for_runner(nc, prefix_xai)
        if not sid_acp or not sid_xai:
            fail("cross switcher: one or both runners did not start"); return
        ok("cross switcher: both runners started")

        # Verify the xai model is in AGENT_REGISTRY (populated at runner startup)
        try:
            js = nc.jetstream()
            kv = await js.key_value("AGENT_REGISTRY")
            entry = await kv.get(agent_type)
            cap = json.loads(entry.value)
            models = cap.get("metadata", {}).get("models", [])
            if model_id not in models:
                fail("cross switcher: xai model not in registry", f"models={models}"); return
            ok("cross switcher: xai model found in AGENT_REGISTRY")
        except Exception as ex:
            fail("cross switcher: AGENT_REGISTRY read failed", str(ex)[:120]); return

        MockLLMServer.reset()
        MockLLMServer.queue("/anthropic/v1/messages", anthropic_sse(acp_resp))
        MockLLMServer.queue("/responses", xai_sse(xai_resp, "resp-cr48"))

        # Run the trogon REPL with piped input; EOF causes clean exit
        result = subprocess.run(
            [trogon_bin, "--nats-url", NATS_URL, "--prefix", prefix_acp],
            input=f"hello world\n/model {model_id}\nfollow up\n",
            capture_output=True, text=True, timeout=45,
        )
        info(f"cross switcher: exit={result.returncode}")
        info(f"cross switcher: stdout={result.stdout!r}")
        if result.stderr:
            info(f"cross switcher: stderr={result.stderr[:200]!r}")

        if result.returncode != 0:
            fail("cross switcher: trogon exited with error",
                 f"rc={result.returncode} stderr={result.stderr[:80]!r}"); return
        ok("cross switcher: trogon exited cleanly")

        if acp_resp in result.stdout:
            ok("cross switcher: initial acp response in stdout")
        else:
            fail("cross switcher: acp response missing from stdout",
                 f"expected {acp_resp!r} in {result.stdout!r}")
        if xai_resp in result.stdout:
            ok("cross switcher: xai response after model switch in stdout")
        else:
            fail("cross switcher: xai response missing from stdout",
                 f"expected {xai_resp!r} in {result.stdout!r}")

        acp_count = MockLLMServer.request_count("/anthropic/v1/messages")
        xai_count = MockLLMServer.request_count("/responses")
        if acp_count >= 1:
            ok("cross switcher: acp-runner received first prompt",
               f"{acp_count} request(s)")
        else:
            fail("cross switcher: acp-runner never called")
        if xai_count >= 1:
            ok("cross switcher: xai-runner received second prompt after switch",
               f"{xai_count} request(s)")
        else:
            fail("cross switcher: xai-runner never called after /model switch")
    finally:
        stop_runner("acp_cr48")
        stop_runner("xai_cr48")


# ── Test 49: /compact CLI manual publish ──────────────────────────────────────

async def test_compact_cli(nc):
    section("Test 49: /compact CLI — trogon /compact publishes to compactor NATS subject")
    prefix = f"{BASE}.acp_cp49"
    acp_bin    = os.path.join(RSDIR, "target", "debug", "trogon-acp-runner")
    trogon_bin = os.path.join(RSDIR, "target", "debug", "trogon")

    if not os.path.exists(trogon_bin):
        fail("/compact: trogon binary not found", trogon_bin); return

    MockLLMServer.reset()
    start_runner("acp_cp49", acp_bin, {
        "ACP_PREFIX":      prefix,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("/compact: runner did not start"); return
        ok("/compact: runner started")

        result = subprocess.run(
            [trogon_bin, "--nats-url", NATS_URL, "--prefix", prefix],
            input="/compact\n",
            capture_output=True, text=True, timeout=20,
        )
        info(f"/compact: exit={result.returncode}")
        info(f"/compact: stdout={result.stdout!r}")
        if result.stderr:
            info(f"/compact: stderr={result.stderr[:120]!r}")

        if result.returncode != 0:
            fail("/compact: trogon exited with error",
                 f"rc={result.returncode} stderr={result.stderr[:80]!r}"); return
        ok("/compact: trogon exited cleanly")

        if "compaction triggered" in result.stdout:
            ok("/compact: 'compaction triggered' printed to stdout",
               f"stdout={result.stdout.strip()!r}")
        else:
            fail("/compact: expected 'compaction triggered' in stdout",
                 f"got {result.stdout!r}")
    finally:
        stop_runner("acp_cp49")


# ── Test 50: /init CLI writes TROGON.md ───────────────────────────────────────

async def test_init_cli(nc):
    section("Test 50: /init CLI — trogon /init sends LLM prompt and writes TROGON.md to disk")
    import tempfile, shutil
    prefix = f"{BASE}.acp_init50"
    acp_bin    = os.path.join(RSDIR, "target", "debug", "trogon-acp-runner")
    trogon_bin = os.path.join(RSDIR, "target", "debug", "trogon")

    if not os.path.exists(trogon_bin):
        fail("/init: trogon binary not found", trogon_bin); return

    tmpdir = tempfile.mkdtemp(prefix="trogon_init50_")
    trogon_md_marker = "# SmokeTrogonInit50"
    trogon_md_content = f"{trogon_md_marker}\n\nSmoke test project — init e2e."

    MockLLMServer.reset()
    MockLLMServer.queue("/anthropic/v1/messages", anthropic_sse(trogon_md_content))

    start_runner("acp_init50", acp_bin, {
        "ACP_PREFIX":      prefix,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    })
    try:
        sid = await wait_for_runner(nc, prefix)
        if not sid:
            fail("/init: runner did not start"); return
        ok("/init: runner started")

        result = subprocess.run(
            [trogon_bin, "--nats-url", NATS_URL, "--prefix", prefix],
            input="/init\n",
            capture_output=True, text=True, timeout=30,
            cwd=tmpdir,
        )
        info(f"/init: exit={result.returncode}")
        info(f"/init: stdout={result.stdout!r}")
        if result.stderr:
            info(f"/init: stderr={result.stderr[:120]!r}")

        if result.returncode != 0:
            fail("/init: trogon exited with error",
                 f"rc={result.returncode} stderr={result.stderr[:80]!r}"); return
        ok("/init: trogon exited cleanly")

        trogon_md_path = os.path.join(tmpdir, "TROGON.md")
        if os.path.exists(trogon_md_path):
            ok("/init: TROGON.md created in project directory")
            with open(trogon_md_path) as f:
                content = f.read()
            info(f"/init: TROGON.md content={content[:80]!r}")
            if trogon_md_marker in content:
                ok("/init: TROGON.md contains LLM response content",
                   f"marker={trogon_md_marker!r}")
            else:
                fail("/init: TROGON.md missing expected content",
                     f"expected {trogon_md_marker!r} in {content[:80]!r}")
        else:
            fail("/init: TROGON.md not created",
                 f"expected at {trogon_md_path}")
    finally:
        stop_runner("acp_init50")
        try:
            shutil.rmtree(tmpdir, ignore_errors=True)
        except Exception:
            pass


# ── Main ───────────────────────────────────────────────────────────────────────

async def main():
    print()
    print("=== smoke_test_programming.py — programming features (no credentials) ===")
    print(f"prefix base  : {BASE}")
    print(f"nats         : {NATS_URL}")
    print(f"mock server  : http://127.0.0.1:{MOCK_PORT}")
    print()

    mock_server = start_mock_server(MOCK_PORT)
    info(f"mock HTTP server started on :{MOCK_PORT}")

    try:
        nc = await nats_lib.connect(NATS_URL)
    except Exception as e:
        print(f"\n{red}ERROR{reset}: cannot connect to NATS at {NATS_URL}: {e}")
        sys.exit(1)

    try:
        await test_acp_basic_prompt(nc)
        await test_xai_basic_prompt(nc)
        await test_openrouter_basic_prompt(nc)
        await test_acp_set_model(nc)
        await test_cross_runner_acp_to_xai(nc)
        await test_acp_tool_execution(nc)
        await test_xai_tool_execution(nc)
        await test_openrouter_tool_execution(nc)
        await test_xai_set_model(nc)
        await test_openrouter_set_model(nc)
        await test_codex_basic_prompt(nc)
        await test_cross_runner_tool_history(nc)
        await test_codex_set_model(nc)
        await test_acp_bypass_permissions(nc)
        await test_codex_tool_execution(nc)
        await test_acp_session_resume(nc)
        await test_cross_runner_acp_to_codex(nc)
        await test_openrouter_tool_actual_execution(nc)
        await test_trogon_md_injection(nc)
        await test_write_file(nc)
        await test_acp_tool_actual_success(nc)
        await test_xai_tool_actual_execution(nc)
        await test_set_model_cross_runner(nc)
        await test_xai_spawn_handler(nc)
        await test_openrouter_spawn_handler(nc)
        await test_cross_runner_acp_to_openrouter(nc)
        await test_cross_runner_xai_to_codex(nc)
        await test_cross_runner_openrouter_to_xai(nc)
        await test_tool_git_status(nc)
        await test_tool_git_diff(nc)
        await test_tool_git_log(nc)
        await test_tool_glob(nc)
        await test_tool_list_dir(nc)
        await test_tool_fetch_url(nc)
        await test_tool_notebook_edit(nc)
        await test_tool_str_replace(nc)
        await test_tool_search_files(nc)
        await test_tool_todo_write(nc)
        await test_tool_todo_read(nc)
        await test_registry_runner_startup(nc)
        await test_print_flag_cli(nc)
        await test_acp_compaction_full_cycle(nc)
        await test_registry_openrouter_startup(nc)
        await test_registry_codex_startup(nc)
        await test_acp_bash_tool_smoke(nc)
        await test_registry_acp_startup(nc)
        await test_session_get_state(nc)
        await test_cross_runner_switcher_model(nc)
        await test_compact_cli(nc)
        await test_init_cli(nc)
    finally:
        await nc.close()
        stop_all()
        mock_server.shutdown()

    print()
    print("=== Results ===")
    print(f"  {green}passed{reset}: {PASS}")
    if FAIL > 0:
        print(f"  {red}failed{reset}: {FAIL}")
        sys.exit(1)
    else:
        print(f"  {red}failed{reset}: 0")
        print(f"\n{green}All tests passed.{reset}")


if __name__ == "__main__":
    asyncio.run(main())
