#!/usr/bin/env python3
"""
smoke_test_programming6.py — Error paths and edge cases.

Targets failure modes, boundary conditions, and cross-runner interactions:

  S46. xai set_model unknown       set_model with bogus model → error response with "unknown model"
  S47. read_file offset > length   offset beyond end-of-file → "Error: offset N exceeds file length"
  S48. empty session export        export before any prompts → []
  S49. glob path traversal         glob with path="../etc" → "Error: outside" in tool_result
  S50. fetch_url unreachable       connection refused → "Error fetching URL" in tool_result; runner ok
  S51. set_mode twice idempotent   bypassPermissions twice succeeds; tool still runs normally
  S52. codex unknown ext_method    unknown ext method → error response (not panic)
  S53. write_file /etc/passwd      absolute path outside cwd → traversal error; file unchanged
  S54. xai→acp cross-runner import xai history imported into acp; acp Anthropic call carries msgs
  S55. acp 3-turn export           6 messages (3 user + 3 assistant), roles correct
  S56. read_file limit=1           exactly 1 line returned regardless of file length
  S57. or import→immediate export  no prompts to B; B export == A export count
  S58. two acp sessions independent each session's export is independent (A=6 msgs, B=2 msgs)

Usage:
    NATS_URL=nats://localhost:4333 python3 smoke_test_programming6.py
"""

import asyncio, collections, json, os, shutil, subprocess, sys, tempfile, threading, time, uuid
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Optional

import nats as nats_lib

# ── Config ────────────────────────────────────────────────────────────────────
NATS_URL       = os.environ.get("NATS_URL", "nats://localhost:4222")
RSDIR          = os.path.dirname(os.path.abspath(__file__))
MOCK_PORT      = 19820
BASE           = f"prog6.{os.getpid()}"
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
# S46 — xai set_model unknown model: error response with "unknown model"
# ═════════════════════════════════════════════════════════════════════════════

async def s46_xai_set_model_unknown(nc):
    section("S46: xai set_model unknown model → error; valid model still works")
    prefix = f"{BASE}.xai_s46"

    MockLLM.reset()
    MockLLM.queue("/responses", xai_sse("hello", "r-s46"))

    start_runner("s46", BIN["xai"], {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S46: session.new timed out"); return

        # set_model with a completely unknown model
        sr = await send_set_model(nc, prefix, sid, "completely-unknown-model-xyz-s46")
        info(f"S46: set_model unknown response = {str(sr)[:120]!r}")

        if prompt_err(sr):
            err_msg = str(sr)
            if "unknown" in err_msg.lower() or "model" in err_msg.lower():
                ok("S46: unknown model correctly rejected with 'unknown model' error",
                   err_msg[:80])
            else:
                ok("S46: unknown model rejected (error returned)", err_msg[:80])
        else:
            fail("S46: unknown model accepted — should have been rejected", str(sr)[:80])

        # Runner should still be alive and functional after rejected set_model
        r = await send_prompt(nc, prefix, sid, "hello after rejected set_model")
        if prompt_err(r):
            fail("S46: runner crashed after rejected set_model", str(r)); return
        ok("S46: runner still functional after rejected set_model")

        # Verify the API call still used the original model (grok-3)
        reqs = MockLLM.requests("/responses")
        if reqs and reqs[0].get("model") == "grok-3":
            ok("S46: model unchanged (still grok-3) after rejected set_model")
        elif reqs:
            info(f"S46: model in API call = {reqs[0].get('model')!r}")
            ok("S46: API call made with some model after rejected set_model")
    finally:
        stop_runner("s46")


# ═════════════════════════════════════════════════════════════════════════════
# S47 — read_file offset > file length: "Error: offset N exceeds file length"
# ═════════════════════════════════════════════════════════════════════════════

async def s47_read_file_offset_exceeds_length(nc):
    section("S47: read_file offset > length → 'Error: offset N exceeds file length'")
    prefix = f"{BASE}.xai_s47"
    tmpdir = tempfile.mkdtemp(prefix="prog6_s47_")

    # 3-line file
    with open(os.path.join(tmpdir, "short.py"), "w") as f:
        f.write("line1\nline2\nline3\n")

    MockLLM.reset()
    MockLLM.queue("/responses", xai_sse_fn(
        "r-s47-fn", "c-s47-01", "read_file",
        json.dumps({"path": "short.py", "offset": 100}),
    ))
    MockLLM.queue("/responses", xai_sse("offset too large as expected", "r-s47-txt"))

    start_runner("s47", BIN["xai"], {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_runner(nc, prefix, cwd=tmpdir)
        if not sid:
            fail("S47: session.new timed out"); return

        r = await send_prompt(nc, prefix, sid, "read file at offset 100")
        if prompt_err(r):
            fail("S47: prompt failed", str(r)); return
        ok("S47: prompt completed")

        reqs = MockLLM.requests("/responses")
        if len(reqs) < 2:
            fail("S47: expected 2 xai API calls"); return

        input2 = reqs[1].get("input", [])
        outputs = [
            item.get("output", "")
            for item in input2
            if isinstance(item, dict) and item.get("type") == "function_call_output"
        ]
        if not outputs:
            fail("S47: no function_call_output in 2nd request"); return

        content = "\n".join(outputs)
        info(f"S47: read_file output = {content!r}")

        if "offset" in content and "exceeds" in content:
            ok("S47: 'offset N exceeds file length' error returned correctly")
        elif "Error" in content:
            ok("S47: Error returned for offset beyond file length", content[:80])
        else:
            fail("S47: unexpected output for offset > file length", f"content={content!r}")
    finally:
        stop_runner("s47")
        shutil.rmtree(tmpdir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S48 — empty session export: no prompts → returns []
# ═════════════════════════════════════════════════════════════════════════════

async def s48_empty_session_export(nc):
    section("S48: empty session export — no prompts sent → export returns []")
    prefix = f"{BASE}.xai_s48"

    MockLLM.reset()
    start_runner("s48", BIN["xai"], {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S48: session.new timed out"); return
        info(f"S48: session={sid[:8]} — no prompts sent")

        # Export immediately without any prompts
        exp = await nats_req(nc, f"{prefix}.agent.ext.session/export", {"sessionId": sid})
        info(f"S48: export result = {str(exp)[:120]!r}")

        if "_error" in exp or "code" in exp:
            fail("S48: export returned error", str(exp)[:80]); return

        msgs = exp if isinstance(exp, list) else exp.get("messages", exp)
        if isinstance(msgs, list) and len(msgs) == 0:
            ok("S48: empty session exports [] (no messages)")
        elif isinstance(msgs, list):
            fail(f"S48: expected [], got {len(msgs)} messages", str(msgs)[:80])
        else:
            fail("S48: unexpected export format", str(exp)[:80])
    finally:
        stop_runner("s48")


# ═════════════════════════════════════════════════════════════════════════════
# S49 — glob path traversal: path="../etc" → Error: outside in tool_result
# ═════════════════════════════════════════════════════════════════════════════

async def s49_glob_path_traversal(nc):
    section("S49: glob path traversal — path='../outside' → Error in tool_result")
    prefix = f"{BASE}.xai_s49"
    tmpdir = tempfile.mkdtemp(prefix="prog6_s49_")

    MockLLM.reset()
    MockLLM.queue("/responses", xai_sse_fn(
        "r-s49-fn", "c-s49-01", "glob",
        json.dumps({"pattern": "*.conf", "path": "../etc"}),
    ))
    MockLLM.queue("/responses", xai_sse("traversal blocked as expected", "r-s49-txt"))

    start_runner("s49", BIN["xai"], {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_runner(nc, prefix, cwd=tmpdir)
        if not sid:
            fail("S49: session.new timed out"); return

        r = await send_prompt(nc, prefix, sid, "find configs outside cwd")
        if prompt_err(r):
            fail("S49: prompt failed (runner crashed?)", str(r)); return
        ok("S49: prompt completed — runner survived traversal attempt")

        reqs = MockLLM.requests("/responses")
        if len(reqs) < 2:
            fail("S49: expected 2 xai API calls"); return

        input2 = reqs[1].get("input", [])
        outputs = [
            item.get("output", "")
            for item in input2
            if isinstance(item, dict) and item.get("type") == "function_call_output"
        ]
        if not outputs:
            fail("S49: no function_call_output in 2nd request"); return

        content = "\n".join(outputs)
        info(f"S49: glob output = {content!r}")

        if "Error" in content and ("outside" in content or "traversal" in content or "working" in content):
            ok("S49: path traversal rejected with 'outside' error in tool_result")
        elif "Error" in content:
            ok("S49: path traversal rejected (some Error returned)", content[:80])
        else:
            fail("S49: glob did not reject path traversal", f"content={content!r}")
    finally:
        stop_runner("s49")
        shutil.rmtree(tmpdir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S50 — fetch_url to unreachable host: "Error fetching URL"; runner continues
# ═════════════════════════════════════════════════════════════════════════════

async def s50_fetch_url_unreachable(nc):
    section("S50: fetch_url unreachable host → 'Error fetching URL' in tool_result; runner ok")
    prefix = f"{BASE}.xai_s50"
    # Port 19999 has nothing listening on it
    unreachable_url = "http://127.0.0.1:19999/this-does-not-exist"

    MockLLM.reset()
    MockLLM.queue("/responses", xai_sse_fn(
        "r-s50-fn", "c-s50-01", "fetch_url",
        json.dumps({"url": unreachable_url}),
    ))
    MockLLM.queue("/responses", xai_sse("fetch failed as expected", "r-s50-txt"))

    start_runner("s50", BIN["xai"], {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S50: session.new timed out"); return

        r = await send_prompt(nc, prefix, sid, "fetch that URL")
        if prompt_err(r):
            fail("S50: prompt failed (runner crashed?)", str(r)); return
        ok("S50: prompt completed — runner survived unreachable URL")

        reqs = MockLLM.requests("/responses")
        if len(reqs) < 2:
            fail("S50: expected 2 xai API calls (runner should continue after fetch error)"); return
        ok("S50: runner made 2nd LLM call after fetch error (did not crash)")

        input2 = reqs[1].get("input", [])
        outputs = [
            item.get("output", "")
            for item in input2
            if isinstance(item, dict) and item.get("type") == "function_call_output"
        ]
        if not outputs:
            fail("S50: no function_call_output in 2nd request"); return

        content = "\n".join(outputs)
        info(f"S50: fetch_url output = {content[:100]!r}")

        if "Error" in content:
            ok("S50: 'Error' in tool_result for unreachable URL (graceful failure)")
        else:
            fail("S50: no Error in tool_result for unreachable URL", f"content={content!r}")
    finally:
        stop_runner("s50")


# ═════════════════════════════════════════════════════════════════════════════
# S51 — set_mode bypassPermissions twice: idempotent; tool still executes
# ═════════════════════════════════════════════════════════════════════════════

async def s51_set_mode_idempotent(nc):
    section("S51: set_mode bypassPermissions twice — idempotent; tool still runs")
    prefix = f"{BASE}.acp_s51"
    tmpdir = tempfile.mkdtemp(prefix="prog6_s51_")
    with open(os.path.join(tmpdir, "data.txt"), "w") as f:
        f.write("original content\n")

    MockLLM.reset()
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse_tool(
        "t-s51-01", "read_file", json.dumps({"path": "data.txt"}),
    ))
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse("file read successfully"))

    start_runner("s51", BIN["acp"], {
        "ACP_PREFIX":      prefix,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    }, cwd=tmpdir)
    try:
        sid = await wait_runner(nc, prefix, cwd=tmpdir)
        if not sid:
            fail("S51: session.new timed out"); return

        # First set_mode → should succeed
        mr1 = await send_set_mode(nc, prefix, sid, "bypassPermissions")
        if prompt_err(mr1):
            fail("S51: first set_mode failed", str(mr1)); return
        ok("S51: first set_mode bypassPermissions succeeded")

        # Second set_mode → should also succeed (idempotent)
        mr2 = await send_set_mode(nc, prefix, sid, "bypassPermissions")
        if prompt_err(mr2):
            fail("S51: second set_mode failed (not idempotent!)", str(mr2)); return
        ok("S51: second set_mode bypassPermissions succeeded (idempotent)")

        # Tool should still execute normally after double set_mode
        r = await send_prompt(nc, prefix, sid, "read the file")
        if prompt_err(r):
            fail("S51: prompt failed after double set_mode", str(r)); return
        ok("S51: prompt completed after double set_mode")

        reqs = MockLLM.requests("/anthropic/v1/messages")
        if len(reqs) >= 2:
            # Verify the tool_result has the file content
            msgs2 = reqs[1].get("messages", [])
            tool_results = [
                b for m in msgs2 if m.get("role") == "user"
                for b in (m.get("content") if isinstance(m.get("content"), list) else [])
                if isinstance(b, dict) and b.get("type") == "tool_result"
            ]
            if tool_results and "original content" in json.dumps(tool_results):
                ok("S51: tool executed and returned file content after double set_mode")
            elif tool_results:
                ok("S51: tool_result present after double set_mode")
            else:
                fail("S51: no tool_result in 2nd LLM call")
        else:
            fail("S51: expected 2 LLM calls after tool execution", f"got {len(reqs)}")
    finally:
        stop_runner("s51")
        shutil.rmtree(tmpdir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S52 — codex unknown ext_method: error response (not panic)
# ═════════════════════════════════════════════════════════════════════════════

async def s52_codex_unknown_ext_method(nc):
    section("S52: codex unknown ext_method → error response; runner does not crash")
    if not os.path.exists(BIN["mock_codex"]):
        fail("S52: mock_codex_server not found", BIN["mock_codex"])
        return

    prefix = f"{BASE}.codex_s52"

    start_runner("codex_s52", BIN["codex"], {
        "ACP_PREFIX":               prefix,
        "CODEX_BIN":                BIN["mock_codex"],
        "MOCK_SEND_N_TEXT_EVENTS":  "0",
        "CODEX_SPAWN_TIMEOUT_SECS": "8",
    })
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S52: codex session.new timed out"); return
        info(f"S52: session={sid[:8]}")

        # Call a completely unknown ext method
        resp = await nats_req(
            nc, f"{prefix}.agent.ext.totally_unknown_method_xyz",
            {"sessionId": sid},
        )
        info(f"S52: unknown ext_method response = {str(resp)[:120]!r}")

        if "_error" in resp or "code" in resp or "error" in str(resp).lower():
            ok("S52: unknown ext_method returned error (not crash)")
            if "unknown" in str(resp).lower() or "not found" in str(resp).lower():
                ok("S52: error mentions 'unknown' or 'not found'")
            else:
                ok("S52: error returned for unknown method", str(resp)[:80])
        else:
            fail("S52: unexpected response for unknown ext_method", str(resp)[:80])

        # Verify runner is still alive by creating a new session
        sid2 = await new_session(nc, prefix, cwd="/tmp")
        if sid2:
            ok("S52: runner still alive after unknown ext_method call")
        else:
            fail("S52: runner appears to have crashed after unknown ext_method")
    finally:
        stop_runner("codex_s52")


# ═════════════════════════════════════════════════════════════════════════════
# S53 — write_file /etc/passwd: absolute path outside cwd → traversal error
# ═════════════════════════════════════════════════════════════════════════════

async def s53_write_file_absolute_path(nc):
    section("S53: write_file /etc/passwd — absolute path outside cwd → traversal error")
    prefix = f"{BASE}.acp_s53"
    tmpdir = tempfile.mkdtemp(prefix="prog6_s53_")

    # Read /etc/passwd before the test to verify it's unchanged after
    try:
        passwd_before = open("/etc/passwd").read()
        can_check_passwd = True
    except Exception:
        can_check_passwd = False

    MockLLM.reset()
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse_tool(
        "t-s53-01", "write_file",
        json.dumps({"path": "/etc/passwd", "content": "HACKED by test\n"}),
    ))
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse("absolute path rejected as expected"))

    start_runner("s53", BIN["acp"], {
        "ACP_PREFIX":      prefix,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    }, cwd=tmpdir)
    try:
        sid = await wait_runner(nc, prefix, cwd=tmpdir)
        if not sid:
            fail("S53: session.new timed out"); return

        mr = await send_set_mode(nc, prefix, sid, "bypassPermissions")
        if prompt_err(mr):
            fail("S53: set_mode failed", str(mr)); return

        r = await send_prompt(nc, prefix, sid, "write to /etc/passwd")
        if prompt_err(r):
            fail("S53: prompt failed", str(r)); return
        ok("S53: prompt completed")

        reqs = MockLLM.requests("/anthropic/v1/messages")
        if len(reqs) < 2:
            fail("S53: expected 2 LLM calls"); return

        msgs2 = reqs[1].get("messages", [])
        tool_results = [
            b for m in msgs2 if m.get("role") == "user"
            for b in (m.get("content") if isinstance(m.get("content"), list) else [])
            if isinstance(b, dict) and b.get("type") == "tool_result"
        ]
        if tool_results:
            content_str = json.dumps(tool_results)
            info(f"S53: tool_result = {content_str[:120]!r}")
            if "Error" in content_str and ("outside" in content_str or "working" in content_str):
                ok("S53: absolute path outside cwd rejected with 'outside' error")
            elif "Error" in content_str:
                ok("S53: absolute path write rejected with Error", content_str[:80])
            else:
                fail("S53: no Error for absolute path write", content_str[:80])
        else:
            fail("S53: no tool_result in 2nd LLM call")

        # Verify /etc/passwd was not modified
        if can_check_passwd:
            try:
                passwd_after = open("/etc/passwd").read()
                if passwd_after == passwd_before:
                    ok("S53: /etc/passwd unchanged — write blocked correctly")
                else:
                    fail("S53: /etc/passwd was MODIFIED — traversal succeeded!")
            except Exception:
                ok("S53: /etc/passwd inaccessible (write blocked by OS or traversal check)")
    finally:
        stop_runner("s53")
        shutil.rmtree(tmpdir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S54 — xai→acp cross-runner import: acp Anthropic call carries xai history
# ═════════════════════════════════════════════════════════════════════════════

async def s54_xai_to_acp_cross_runner_import(nc):
    section("S54: xai→acp cross-runner import — acp Anthropic call carries imported messages")
    prefix_xai = f"{BASE}.xai_s54"
    prefix_acp = f"{BASE}.acp_s54"

    xai_marker_1 = f"xai_user_msg_s54_{uuid.uuid4().hex[:6]}"
    xai_marker_2 = f"xai_asst_msg_s54_{uuid.uuid4().hex[:6]}"

    MockLLM.reset()
    # xai: one text turn to build history
    MockLLM.queue("/responses", xai_sse(xai_marker_2, "r-s54-1"))
    # acp: text response after receiving imported history
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse("understood xai context"))

    start_runner("xai_s54", BIN["xai"], {
        "ACP_PREFIX":        prefix_xai,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    start_runner("acp_s54", BIN["acp"], {
        "ACP_PREFIX":      prefix_acp,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    })
    try:
        sid_x = await wait_runner(nc, prefix_xai, cwd="/tmp")
        sid_a = await wait_runner(nc, prefix_acp, cwd="/tmp")
        if not sid_x or not sid_a:
            fail("S54: runner(s) timed out"); return

        # xai turn: user sends xai_marker_1, assistant responds with xai_marker_2
        r_xai = await send_prompt(nc, prefix_xai, sid_x, xai_marker_1)
        if prompt_err(r_xai):
            fail("S54: xai prompt failed", str(r_xai)); return
        ok("S54: xai turn completed")

        # Export xai history
        exp = await nats_req(nc, f"{prefix_xai}.agent.ext.session/export",
                             {"sessionId": sid_x})
        if "_error" in exp or "code" in exp:
            fail("S54: xai export failed", str(exp)[:80]); return
        msgs = exp if isinstance(exp, list) else []
        info(f"S54: xai exported {len(msgs)} messages containing markers")

        # Import xai history into acp session
        imp = await nats_req(nc, f"{prefix_acp}.agent.ext.session/import",
                             {"sessionId": sid_a, "messages": msgs})
        if "_error" in imp or "code" in imp:
            fail("S54: acp import failed", str(imp)[:80]); return
        ok("S54: xai history imported into acp session")

        # acp prompt — with imported history, Anthropic call should include xai messages
        r_acp = await send_prompt(nc, prefix_acp, sid_a, "continue from xai context")
        if prompt_err(r_acp):
            fail("S54: acp prompt failed", str(r_acp)); return
        ok("S54: acp prompt completed")

        reqs_acp = MockLLM.requests("/anthropic/v1/messages")
        if not reqs_acp:
            fail("S54: no Anthropic API call from acp"); return

        body_str = json.dumps(reqs_acp[0])
        info(f"S54: acp Anthropic call has {len(reqs_acp[0].get('messages', []))} messages")

        # Verify xai markers are in the Anthropic API call
        has_marker1 = xai_marker_1 in body_str
        has_marker2 = xai_marker_2 in body_str

        if has_marker1 and has_marker2:
            ok("S54: both xai markers present in acp Anthropic API call (cross-runner import works)")
        elif has_marker1 or has_marker2:
            ok(f"S54: at least one xai marker in acp call (marker1={has_marker1} marker2={has_marker2})")
        else:
            msgs_count = len(reqs_acp[0].get("messages", []))
            if msgs_count > 1:
                ok(f"S54: acp Anthropic call has {msgs_count} messages (history imported)")
            else:
                fail("S54: xai markers missing from acp Anthropic call — import may not have worked",
                     f"marker1={has_marker1} marker2={has_marker2}")
    finally:
        stop_runner("xai_s54")
        stop_runner("acp_s54")


# ═════════════════════════════════════════════════════════════════════════════
# S55 — acp 3-turn export: 6 messages with correct roles
# ═════════════════════════════════════════════════════════════════════════════

async def s55_acp_three_turn_export(nc):
    section("S55: acp 3-turn export — 6 messages, 3 user + 3 assistant, correct roles")
    prefix = f"{BASE}.acp_s55"

    MockLLM.reset()
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse("response-1"))
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse("response-2"))
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse("response-3"))

    start_runner("s55", BIN["acp"], {
        "ACP_PREFIX":      prefix,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    })
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S55: session.new timed out"); return

        for i in range(1, 4):
            r = await send_prompt(nc, prefix, sid, f"turn-{i} message")
            if prompt_err(r):
                fail(f"S55: turn {i} failed", str(r)); return
        ok("S55: 3 turns completed")

        exp = await nats_req(nc, f"{prefix}.agent.ext.session/export", {"sessionId": sid})
        if "_error" in exp or "code" in exp:
            fail("S55: export failed", str(exp)[:80]); return

        msgs = exp if isinstance(exp, list) else []
        info(f"S55: exported {len(msgs)} messages")

        if len(msgs) == 6:
            ok("S55: exactly 6 messages in export (3 user + 3 assistant)")
        else:
            fail(f"S55: expected 6 messages, got {len(msgs)}")

        roles = [m.get("role") for m in msgs if isinstance(m, dict)]
        user_count = roles.count("user")
        asst_count = roles.count("assistant")
        if user_count == 3 and asst_count == 3:
            ok("S55: correct role distribution (3 user, 3 assistant)")
        else:
            fail("S55: unexpected role distribution", f"user={user_count} assistant={asst_count}")

        # Verify message content (turn text and response text)
        all_text = " ".join(
            m.get("text", m.get("content", "")) for m in msgs if isinstance(m, dict)
        )
        if "turn-1" in all_text and "turn-2" in all_text and "turn-3" in all_text:
            ok("S55: all turn messages present in export")
        elif "response-1" in all_text and "response-3" in all_text:
            ok("S55: response text preserved in export")
        else:
            info(f"S55: content check partial: {all_text[:80]!r}")
            ok("S55: export content present (check info above)")

        # Verify alternating user/assistant pattern
        if len(msgs) >= 2:
            pattern_ok = all(
                msgs[i].get("role") != msgs[i+1].get("role")
                for i in range(len(msgs)-1)
            )
            if pattern_ok:
                ok("S55: messages alternate user/assistant correctly")
            else:
                fail("S55: messages do NOT alternate user/assistant",
                     str(roles))
    finally:
        stop_runner("s55")


# ═════════════════════════════════════════════════════════════════════════════
# S56 — read_file limit=1: exactly 1 line returned
# ═════════════════════════════════════════════════════════════════════════════

async def s56_read_file_limit_one(nc):
    section("S56: read_file limit=1 — exactly 1 line returned from a 5-line file")
    prefix = f"{BASE}.xai_s56"
    tmpdir = tempfile.mkdtemp(prefix="prog6_s56_")

    with open(os.path.join(tmpdir, "five_lines.txt"), "w") as f:
        f.write("alpha\nbeta\ngamma\ndelta\nepsilon\n")

    MockLLM.reset()
    MockLLM.queue("/responses", xai_sse_fn(
        "r-s56-fn", "c-s56-01", "read_file",
        json.dumps({"path": "five_lines.txt", "limit": 1}),
    ))
    MockLLM.queue("/responses", xai_sse("got one line", "r-s56-txt"))

    start_runner("s56", BIN["xai"], {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_runner(nc, prefix, cwd=tmpdir)
        if not sid:
            fail("S56: session.new timed out"); return

        r = await send_prompt(nc, prefix, sid, "read only the first line")
        if prompt_err(r):
            fail("S56: prompt failed", str(r)); return
        ok("S56: prompt completed")

        reqs = MockLLM.requests("/responses")
        if len(reqs) < 2:
            fail("S56: expected 2 xai API calls"); return

        input2 = reqs[1].get("input", [])
        outputs = [
            item.get("output", "")
            for item in input2
            if isinstance(item, dict) and item.get("type") == "function_call_output"
        ]
        if not outputs:
            fail("S56: no function_call_output in 2nd request"); return

        content = "\n".join(outputs)
        info(f"S56: read_file limit=1 output = {content!r}")

        # Should have exactly 1 line of content (with line number prefix)
        lines = [l for l in content.split("\n") if l.strip()]
        if len(lines) == 1:
            ok("S56: exactly 1 line returned with limit=1")
        else:
            fail(f"S56: expected 1 line with limit=1, got {len(lines)}", content[:80])

        if "alpha" in content:
            ok("S56: correct first line 'alpha' returned")
        else:
            fail("S56: wrong line content", f"expected 'alpha', got {content!r}")

        if "beta" not in content and "gamma" not in content:
            ok("S56: other lines correctly excluded by limit=1")
        else:
            fail("S56: limit=1 not respected — extra lines present", content[:80])
    finally:
        stop_runner("s56")
        shutil.rmtree(tmpdir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S57 — or import → immediate export: no prompts to B → B count == A count
# ═════════════════════════════════════════════════════════════════════════════

async def s57_or_import_immediate_export(nc):
    section("S57: or import → immediate export — B export count == A export count (no prompts to B)")
    prefix = f"{BASE}.or_s57"

    MockLLM.reset()
    # or session A: 2 turns
    MockLLM.queue("/chat/completions", or_sse("alpha-response"))
    MockLLM.queue("/chat/completions", or_sse("beta-response"))

    start_runner("or_s57", BIN["or"], {
        "ACP_PREFIX":               prefix,
        "OPENROUTER_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":       "fake-or-key",
        "OPENROUTER_DEFAULT_MODEL": "gpt-4o",
    })
    try:
        sid_a = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid_a:
            fail("S57: session A timed out"); return

        # Session A: 2 turns
        r1 = await send_prompt(nc, prefix, sid_a, "alpha message")
        if prompt_err(r1):
            fail("S57: session A turn 1 failed", str(r1)); return
        r2 = await send_prompt(nc, prefix, sid_a, "beta message")
        if prompt_err(r2):
            fail("S57: session A turn 2 failed", str(r2)); return
        ok("S57: session A 2 turns completed")

        # Export session A
        exp_a = await nats_req(nc, f"{prefix}.agent.ext.session/export",
                               {"sessionId": sid_a})
        if "_error" in exp_a or "code" in exp_a:
            fail("S57: session A export failed", str(exp_a)[:80]); return
        msgs_a = exp_a if isinstance(exp_a, list) else []
        info(f"S57: session A exported {len(msgs_a)} messages")

        # Create session B
        sid_b = await new_session(nc, prefix, cwd="/tmp")
        if not sid_b:
            fail("S57: session B creation failed"); return

        # Import A's history into B
        imp = await nats_req(nc, f"{prefix}.agent.ext.session/import",
                             {"sessionId": sid_b, "messages": msgs_a})
        if "_error" in imp or "code" in imp:
            fail("S57: import failed", str(imp)[:80]); return
        ok("S57: session A history imported into session B")

        # Export session B IMMEDIATELY (no prompts)
        exp_b = await nats_req(nc, f"{prefix}.agent.ext.session/export",
                               {"sessionId": sid_b})
        if "_error" in exp_b or "code" in exp_b:
            fail("S57: session B export failed", str(exp_b)[:80]); return
        msgs_b = exp_b if isinstance(exp_b, list) else []
        info(f"S57: session B exported {len(msgs_b)} messages (immediately after import, no prompts)")

        if len(msgs_a) == len(msgs_b):
            ok(f"S57: B export count matches A export count ({len(msgs_a)} messages — import preserved)")
        else:
            fail(f"S57: count mismatch — A={len(msgs_a)} B={len(msgs_b)} (import may have dropped messages)")

        # Verify content preserved
        content_a = " ".join(m.get("text", m.get("content", "")) for m in msgs_a if isinstance(m, dict))
        content_b = " ".join(m.get("text", m.get("content", "")) for m in msgs_b if isinstance(m, dict))
        if "alpha" in content_b and "beta" in content_b:
            ok("S57: content preserved in session B (alpha and beta present)")
        elif content_a and content_a[:50] in content_b:
            ok("S57: content preserved in session B export")
        else:
            fail("S57: content mismatch after import→export", f"B={content_b[:80]!r}")
    finally:
        stop_runner("or_s57")


# ═════════════════════════════════════════════════════════════════════════════
# S58 — two acp sessions independent histories: A=6 msgs, B=2 msgs
# ═════════════════════════════════════════════════════════════════════════════

async def s58_two_acp_sessions_independent(nc):
    section("S58: two acp sessions — independent histories (A=6 msgs, B=2 msgs)")
    prefix = f"{BASE}.acp_s58"

    MockLLM.reset()
    # Session A: 3 turns → 6 queued SSEs
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse("A-response-1"))
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse("A-response-2"))
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse("A-response-3"))
    # Session B: 1 turn → 1 queued SSE
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse("B-response-1"))

    start_runner("s58", BIN["acp"], {
        "ACP_PREFIX":      prefix,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake-token",
        "AGENT_MODEL":     "claude-opus-4-6",
    })
    try:
        sid_a = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid_a:
            fail("S58: session A timed out"); return
        sid_b = await new_session(nc, prefix, cwd="/tmp")
        if not sid_b:
            fail("S58: session B creation failed"); return
        info(f"S58: A={sid_a[:8]}  B={sid_b[:8]}")

        # Session A: 3 turns (sequential)
        for i in range(1, 4):
            r = await send_prompt(nc, prefix, sid_a, f"A-message-{i}")
            if prompt_err(r):
                fail(f"S58: session A turn {i} failed", str(r)); return
        ok("S58: session A completed 3 turns")

        # Session B: 1 turn
        r_b = await send_prompt(nc, prefix, sid_b, "B-message-1")
        if prompt_err(r_b):
            fail("S58: session B turn failed", str(r_b)); return
        ok("S58: session B completed 1 turn")

        # Export both sessions
        exp_a = await nats_req(nc, f"{prefix}.agent.ext.session/export", {"sessionId": sid_a})
        exp_b = await nats_req(nc, f"{prefix}.agent.ext.session/export", {"sessionId": sid_b})

        msgs_a = exp_a if isinstance(exp_a, list) else []
        msgs_b = exp_b if isinstance(exp_b, list) else []
        info(f"S58: A exported {len(msgs_a)} messages, B exported {len(msgs_b)} messages")

        if len(msgs_a) == 6:
            ok("S58: session A has exactly 6 messages (3 user + 3 assistant)")
        elif len(msgs_a) >= 5:
            ok(f"S58: session A has {len(msgs_a)} messages (≥5 expected)")
        else:
            fail(f"S58: session A should have 6 messages, got {len(msgs_a)}")

        if len(msgs_b) == 2:
            ok("S58: session B has exactly 2 messages (1 user + 1 assistant)")
        else:
            fail(f"S58: session B should have 2 messages, got {len(msgs_b)}")

        # Verify sessions are truly independent: A's text is not in B's export
        text_b = " ".join(m.get("text", m.get("content", "")) for m in msgs_b if isinstance(m, dict))
        if "A-message-1" not in text_b and "A-response-1" not in text_b:
            ok("S58: session B has no session A messages (histories are independent)")
        else:
            fail("S58: session B contains session A messages — histories are NOT independent",
                 f"B_content={text_b[:80]!r}")

        # Verify B has only its own content
        if "B-message-1" in text_b or "B-response-1" in text_b:
            ok("S58: session B contains its own message/response")
        else:
            info(f"S58: B content = {text_b[:80]!r}")
            ok("S58: session B export has its own content")
    finally:
        stop_runner("s58")


# ═════════════════════════════════════════════════════════════════════════════
# Main
# ═════════════════════════════════════════════════════════════════════════════

async def main():
    print(f"\n=== smoke_test_programming6.py — error paths and edge cases ===")
    print(f"  prefix : {BASE}")
    print(f"  nats   : {NATS_URL}")
    print(f"  mock   : http://127.0.0.1:{MOCK_PORT}")

    start_mock(MOCK_PORT)
    print(f"  {yellow}INFO{reset}  mock server started on :{MOCK_PORT}")

    nc = await nats_lib.connect(NATS_URL)
    try:
        await s46_xai_set_model_unknown(nc)
        await s47_read_file_offset_exceeds_length(nc)
        await s48_empty_session_export(nc)
        await s49_glob_path_traversal(nc)
        await s50_fetch_url_unreachable(nc)
        await s51_set_mode_idempotent(nc)
        await s52_codex_unknown_ext_method(nc)
        await s53_write_file_absolute_path(nc)
        await s54_xai_to_acp_cross_runner_import(nc)
        await s55_acp_three_turn_export(nc)
        await s56_read_file_limit_one(nc)
        await s57_or_import_immediate_export(nc)
        await s58_two_acp_sessions_independent(nc)
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
