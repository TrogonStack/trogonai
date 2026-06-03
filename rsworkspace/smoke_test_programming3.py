#!/usr/bin/env python3
"""
smoke_test_programming3.py — Deeper programming workflow scenarios.

Probes behaviors that are hard to get right and likely to reveal bugs:

  S5.  xai stateful multi-turn   previous_response_id used across turns; cleared on set_model
  S6.  openrouter parallel tools 2 tool_calls in one LLM response → both executed
  S7.  acp write+str_replace     write a buggy file, then fix it with str_replace in next turn
  S8.  xai write_file disk       write_file creates real file with correct content
  S9.  or→xai cross-runner      openrouter analyzes → export → xai applies fix
  S10. set_model unknown rejects xai and openrouter reject unknown models; acp accepts any
  S11. acp text+tool block       text content block + tool_use in same response; tool executes
  S12. openrouter 5-turn history message count grows correctly across 5 turns
  S13. xai→codex cross-runner   codex receives imported xai history and responds
  S14. xai→acp cross-runner     acp receives xai context in its LLM call
  S15. git_status real repo     git_status tool returns actual git output from rsworkspace
  S16. openrouter TROGON.md     system prompt from file injected into messages[role=system]
  S17. acp two sessions isolated two simultaneous sessions do not bleed into each other
  S18. xai set_model then export after model switch, export/import to or works correctly
  S19. 3-runner chain           acp→or→xai sequential export/import; each stage adds context

Usage:
    NATS_URL=nats://localhost:4333 python3 smoke_test_programming3.py
"""

import asyncio, collections, json, os, shutil, subprocess, sys, tempfile, threading, time, uuid
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Optional

import nats as nats_lib

# ── Config ────────────────────────────────────────────────────────────────────
NATS_URL       = os.environ.get("NATS_URL", "nats://localhost:4222")
RSDIR          = os.path.dirname(os.path.abspath(__file__))
MOCK_PORT      = 19814
BASE           = f"prog3.{os.getpid()}"
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


def anthropic_sse_text_and_tool(text: str, tool_id: str, tool_name: str, input_json: str) -> bytes:
    """Anthropic SSE: text content block followed by tool_use in one response."""
    chunks = [
        "event: message_start\ndata: " + json.dumps({
            "type": "message_start",
            "message": {"id": "msg-mixed", "type": "message", "role": "assistant",
                        "content": [], "model": "mock", "stop_reason": None,
                        "stop_sequence": None, "usage": {"input_tokens": 10, "output_tokens": 0}},
        }) + "\n\n",
        # block 0: text
        "event: content_block_start\ndata: " + json.dumps({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "text", "text": ""},
        }) + "\n\n",
        "event: content_block_delta\ndata: " + json.dumps({
            "type": "content_block_delta", "index": 0,
            "delta": {"type": "text_delta", "text": text},
        }) + "\n\n",
        "event: content_block_stop\ndata: " + json.dumps({"type": "content_block_stop", "index": 0}) + "\n\n",
        # block 1: tool_use
        "event: content_block_start\ndata: " + json.dumps({
            "type": "content_block_start", "index": 1,
            "content_block": {"type": "tool_use", "id": tool_id, "name": tool_name, "input": {}},
        }) + "\n\n",
        "event: content_block_delta\ndata: " + json.dumps({
            "type": "content_block_delta", "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": input_json},
        }) + "\n\n",
        "event: content_block_stop\ndata: " + json.dumps({"type": "content_block_stop", "index": 1}) + "\n\n",
        "event: message_delta\ndata: " + json.dumps({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use", "stop_sequence": None},
            "usage": {"output_tokens": 8},
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


def or_sse_two_tools(cid1: str, n1: str, a1: str, cid2: str, n2: str, a2: str) -> bytes:
    """OpenRouter SSE: two tool calls in one response (parallel execution)."""
    chunks = [
        "data: " + json.dumps({"choices": [{"delta": {"tool_calls": [
            {"index": 0, "id": cid1, "type": "function", "function": {"name": n1, "arguments": ""}},
            {"index": 1, "id": cid2, "type": "function", "function": {"name": n2, "arguments": ""}},
        ]}, "finish_reason": None, "index": 0}]}) + "\n\n",
        "data: " + json.dumps({"choices": [{"delta": {"tool_calls": [
            {"index": 0, "function": {"arguments": a1}}
        ]}, "finish_reason": None, "index": 0}]}) + "\n\n",
        "data: " + json.dumps({"choices": [{"delta": {"tool_calls": [
            {"index": 1, "function": {"arguments": a2}}
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
        self.send_header("Content-Type", "text/plain")
        self.end_headers()
        try:
            self.wfile.write(b"ok")
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


BIN = {
    "acp":   os.path.join(RSDIR, "target/debug/trogon-acp-runner"),
    "xai":   os.path.join(RSDIR, "target/debug/trogon-xai-runner"),
    "or":    os.path.join(RSDIR, "target/debug/trogon-openrouter-runner"),
    "codex": os.path.join(RSDIR, "target/debug/trogon-codex-runner"),
    "mock_codex": os.path.join(RSDIR, "target/debug/mock_codex_server"),
}


# ═════════════════════════════════════════════════════════════════════════════
# S5 — xai stateful multi-turn: previous_response_id used between turns,
#       cleared by set_model so full history is re-sent
# ═════════════════════════════════════════════════════════════════════════════

async def s5_xai_stateful_multi_turn(nc):
    section("S5: xai stateful — previous_response_id across turns, cleared on set_model")
    pfx = f"{BASE}.s5"
    rs_file = f"/tmp/prog3_s5_{os.getpid()}.rs"
    with open(rs_file, "w") as f:
        f.write("fn add(a: i32, b: i32) -> i32 { todo!() }\n")

    MockLLM.reset()
    # Turn 1: fn_call (resp_id="r1") + text (resp_id="r2")
    MockLLM.queue("/responses", xai_sse_fn("r1", "c-s5-01", "read_file",
                                           json.dumps({"path": rs_file})))
    MockLLM.queue("/responses", xai_sse("I see a todo!() in add. It should return a + b.", "r2"))
    # Turn 2: text only, using previous_response_id="r2"
    MockLLM.queue("/responses", xai_sse("I'll implement it now.", "r3"))
    # Turn 3: after set_model, full history re-sent
    MockLLM.queue("/responses", xai_sse("Done! Implemented with grok-3-mini.", "r4"))

    start_runner("s5", BIN["xai"], {
        "ACP_PREFIX":        pfx,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake",
        "XAI_DEFAULT_MODEL": "grok-3",
        "XAI_MODELS":        "grok-3:Grok 3,grok-3-mini:Grok 3 Mini",
    })
    try:
        sid = await wait_runner(nc, pfx, cwd="/tmp")
        if not sid:
            fail("S5: session.new timed out"); return
        info(f"session: {sid[:8]}")

        # Turn 1: fn_call + text (2 API calls internally)
        r1 = await send_prompt(nc, pfx, sid, f"Read {rs_file} and explain the code")
        if prompt_err(r1):
            fail("S5: turn 1 failed", str(r1)); return
        ok("S5: turn 1 completed (grok-3 read+analyzed file)")

        reqs = MockLLM.requests("/responses")
        # reqs[0] = turn 1 call 1 (fn_call): no previous_response_id, input=[user msg]
        # reqs[1] = turn 1 call 2 (text after fn result): previous_response_id="r1", input=[fn_output]
        if len(reqs) >= 2:
            r0_prev = reqs[0].get("previous_response_id")
            r1_prev = reqs[1].get("previous_response_id")
            ok("S5: turn 1 call 1 has no previous_response_id") \
                if r0_prev is None else \
                fail("S5: turn 1 call 1 had previous_response_id unexpectedly", str(r0_prev))
            ok("S5: turn 1 call 2 uses previous_response_id='r1' (fn→text chain)",
               f"prev_id={r1_prev!r}") \
                if r1_prev == "r1" else \
                fail("S5: turn 1 call 2 wrong previous_response_id",
                     f"expected 'r1', got {r1_prev!r}")
        else:
            fail("S5: expected 2 API calls in turn 1", f"got {len(reqs)}")

        # Turn 2: no set_model — should use previous_response_id="r2"
        r2 = await send_prompt(nc, pfx, sid, "Implement the add function")
        if prompt_err(r2):
            fail("S5: turn 2 failed", str(r2)); return
        ok("S5: turn 2 completed (grok-3 second turn)")

        reqs = MockLLM.requests("/responses")
        # reqs[2] = turn 2 call 1: previous_response_id="r2", input=[user msg only] (1 item)
        if len(reqs) >= 3:
            t2_prev = reqs[2].get("previous_response_id")
            t2_input = reqs[2].get("input", [])
            ok(f"S5: turn 2 uses previous_response_id='r2' (stateful)",
               f"prev_id={t2_prev!r}") \
                if t2_prev == "r2" else \
                fail("S5: turn 2 did not use previous_response_id",
                     f"expected 'r2', got {t2_prev!r}")
            ok(f"S5: turn 2 sends only new user message in input ({len(t2_input)} items)") \
                if len(t2_input) == 1 else \
                info(f"S5: turn 2 input items: {len(t2_input)} (expected 1 for stateful mode)")
        else:
            fail("S5: expected 3 API calls total after turn 2", f"got {len(reqs)}")

        # set_model clears last_response_id
        sr = await send_set_model(nc, pfx, sid, "grok-3-mini")
        if prompt_err(sr):
            fail("S5: set_model failed", str(sr)); return
        ok("S5: switched to grok-3-mini")

        # Turn 3: full history must be re-sent (no previous_response_id)
        r3 = await send_prompt(nc, pfx, sid, "Confirm the implementation")
        if prompt_err(r3):
            fail("S5: turn 3 failed", str(r3)); return
        ok("S5: turn 3 completed (grok-3-mini after set_model)")

        reqs = MockLLM.requests("/responses")
        if len(reqs) >= 4:
            t3_prev = reqs[3].get("previous_response_id")
            t3_input = reqs[3].get("input", [])
            ok("S5: turn 3 has no previous_response_id (cleared by set_model)") \
                if t3_prev is None else \
                fail("S5: turn 3 still has previous_response_id after set_model",
                     f"got {t3_prev!r}")
            ok(f"S5: turn 3 sends full history ({len(t3_input)} items > 1)") \
                if len(t3_input) > 1 else \
                fail("S5: turn 3 input too short for full history",
                     f"got {len(t3_input)} items")
            t3_model = reqs[3].get("model")
            ok(f"S5: turn 3 uses grok-3-mini", f"model={t3_model}") \
                if t3_model == "grok-3-mini" else \
                fail("S5: turn 3 wrong model", f"expected grok-3-mini, got {t3_model}")
        else:
            fail("S5: expected 4 API calls total", f"got {len(reqs)}")
        info(f"S5: total xai API calls: {len(MockLLM.requests('/responses'))}")
    finally:
        stop_runner("s5")
        try: os.unlink(rs_file)
        except OSError: pass


# ═════════════════════════════════════════════════════════════════════════════
# S6 — openrouter: 2 parallel tool calls in one LLM response
# ═════════════════════════════════════════════════════════════════════════════

async def s6_openrouter_parallel_tools(nc):
    section("S6: openrouter parallel tools — 2 tool_calls in one response → both executed")
    pfx = f"{BASE}.s6"

    f1 = f"/tmp/prog3_s6a_{os.getpid()}.py"
    f2 = f"/tmp/prog3_s6b_{os.getpid()}.py"
    content1 = "# file 1: authentication module"
    content2 = "# file 2: database module"
    with open(f1, "w") as f: f.write(content1)
    with open(f2, "w") as f: f.write(content2)

    MockLLM.reset()
    # LLM call 1: 2 parallel read_file tool calls
    MockLLM.queue("/chat/completions",
        or_sse_two_tools("c-s6-01", "read_file", json.dumps({"path": f1}),
                         "c-s6-02", "read_file", json.dumps({"path": f2})))
    # LLM call 2: summary after reading both files
    MockLLM.queue("/chat/completions",
        or_sse("Both files read. auth.py handles authentication, db.py handles database access."))

    start_runner("s6", BIN["or"], {
        "ACP_PREFIX":          pfx,
        "OPENROUTER_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":  "fake",
    })
    try:
        sid = await wait_runner(nc, pfx, cwd="/tmp")
        if not sid:
            fail("S6: session.new timed out"); return
        info(f"session: {sid[:8]}")

        r = await send_prompt(nc, pfx, sid, "Read both source files and explain the architecture")
        if prompt_err(r):
            fail("S6: prompt failed", str(r)); return
        ok("S6: parallel tool execution completed")

        reqs = MockLLM.requests("/chat/completions")
        if len(reqs) != 2:
            fail("S6: expected 2 LLM calls", f"got {len(reqs)}"); return
        ok("S6: exactly 2 LLM API calls")

        # 2nd request must carry BOTH tool results
        msgs2 = reqs[1].get("messages", [])
        tool_msgs = [m for m in msgs2 if m.get("role") == "tool"]
        if len(tool_msgs) == 2:
            ok("S6: 2nd LLM call carries both tool results (parallel execution worked)",
               f"{len(tool_msgs)} role=tool messages")
        elif len(tool_msgs) == 1:
            fail("S6: only 1 tool result in 2nd call — runner lost the second parallel tool")
        else:
            fail("S6: unexpected tool result count", f"got {len(tool_msgs)}")

        # Both file contents must appear in tool results
        all_content = " ".join(m.get("content", "") for m in tool_msgs)
        if content1 in all_content and content2 in all_content:
            ok("S6: both file contents in tool results", repr(all_content[:60]))
        else:
            fail("S6: file contents missing from tool results", repr(all_content[:80]))
    finally:
        stop_runner("s6")
        for fn in (f1, f2):
            try: os.unlink(fn)
            except OSError: pass


# ═════════════════════════════════════════════════════════════════════════════
# S7 — acp: write_file then str_replace in next turn (file chain)
# ═════════════════════════════════════════════════════════════════════════════

async def s7_acp_write_then_fix(nc):
    section("S7: acp write+str_replace chain — create buggy file, fix in next turn")
    pfx = f"{BASE}.s7"
    chain_file = "prog3_chain.py"   # relative — runner cwd=/tmp

    MockLLM.reset()
    # Turn 1: write_file creates a buggy file (missing return)
    MockLLM.queue("/anthropic/v1/messages",
        anthropic_sse_tool("t-s7-write", "write_file", json.dumps({
            "path": chain_file,
            "content": "def multiply(a, b):\n    a * b  # missing return\n",
        })))
    MockLLM.queue("/anthropic/v1/messages",
        anthropic_sse("Created multiply.py. It has a missing return bug."))
    # Turn 2: str_replace fixes the bug
    MockLLM.queue("/anthropic/v1/messages",
        anthropic_sse_tool("t-s7-fix", "str_replace", json.dumps({
            "path": chain_file,
            "old_str": "    a * b  # missing return",
            "new_str": "    return a * b",
        })))
    MockLLM.queue("/anthropic/v1/messages",
        anthropic_sse("Fixed! Added the missing return statement."))

    start_runner("s7", BIN["acp"], {
        "ACP_PREFIX":      pfx,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake",
        "AGENT_MODEL":     "claude-opus-4-6",
    }, cwd="/tmp")
    try:
        sid = await wait_runner(nc, pfx, cwd="/tmp")
        if not sid:
            fail("S7: session.new timed out"); return
        info(f"session: {sid[:8]}")

        mr = await send_set_mode(nc, pfx, sid, "bypassPermissions")
        if prompt_err(mr):
            fail("S7: set_mode", str(mr)); return

        # Turn 1: write the file
        r1 = await send_prompt(nc, pfx, sid, f"Create {chain_file} with a multiply function")
        if prompt_err(r1):
            fail("S7: turn 1 (write_file)", str(r1)); return
        ok("S7: turn 1 completed — file written")

        full_path = f"/tmp/{chain_file}"
        if os.path.exists(full_path):
            ok("S7: write_file created real file on disk", full_path)
        else:
            fail("S7: write_file did NOT create file", full_path)

        # Turn 2: fix the file
        r2 = await send_prompt(nc, pfx, sid, f"Fix the return bug in {chain_file}")
        if prompt_err(r2):
            fail("S7: turn 2 (str_replace)", str(r2)); return
        ok("S7: turn 2 completed — fix applied")

        reqs = MockLLM.requests("/anthropic/v1/messages")
        ok(f"S7: 4 LLM calls total (2 turns × 2)", f"{len(reqs)} requests") \
            if len(reqs) == 4 else \
            fail(f"S7: expected 4 LLM calls", f"got {len(reqs)}")

        # 3rd LLM call must carry write_file result (turn 2, call 1 = fix)
        if len(reqs) >= 3:
            msgs3 = reqs[2].get("messages", [])
            has_history = len(msgs3) > 2
            ok(f"S7: turn 2 carries prior write_file history ({len(msgs3)} messages)") \
                if has_history else \
                fail("S7: turn 2 missing prior history", f"{len(msgs3)} messages")

        if os.path.exists(full_path):
            content = open(full_path).read()
            if "return a * b" in content:
                ok("S7: file fixed on disk by str_replace chain", repr(content.strip()))
            else:
                fail("S7: file not fixed on disk", repr(content))
    finally:
        stop_runner("s7")
        try: os.unlink(f"/tmp/{chain_file}")
        except OSError: pass


# ═════════════════════════════════════════════════════════════════════════════
# S8 — xai: write_file creates real file on disk
# ═════════════════════════════════════════════════════════════════════════════

async def s8_xai_write_file(nc):
    section("S8: xai write_file — creates real .rs file on disk")
    pfx = f"{BASE}.s8"
    rs_file = f"/tmp/prog3_s8_{os.getpid()}.rs"
    rs_code = 'pub fn greet(name: &str) -> String {\n    format!("Hello, {name}!")\n}\n'

    MockLLM.reset()
    MockLLM.queue("/responses",
        xai_sse_fn("r-s8-01", "c-s8-01", "write_file",
                   json.dumps({"path": rs_file, "content": rs_code})))
    MockLLM.queue("/responses",
        xai_sse("Created greet.rs with a greeting function.", "r-s8-02"))

    start_runner("s8", BIN["xai"], {
        "ACP_PREFIX":        pfx,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid = await wait_runner(nc, pfx, cwd="/tmp")
        if not sid:
            fail("S8: session.new timed out"); return
        info(f"session: {sid[:8]}")

        r = await send_prompt(nc, pfx, sid, f"Write a Rust greeting function to {rs_file}")
        if prompt_err(r):
            fail("S8: prompt failed", str(r)); return
        ok("S8: prompt completed (fn_call + text)")

        if os.path.exists(rs_file):
            content = open(rs_file).read()
            if 'format!("Hello, {name}!")' in content:
                ok("S8: file created with correct Rust code", repr(content.strip()))
            else:
                fail("S8: file created but content wrong", repr(content))
        else:
            fail("S8: write_file did NOT create file on disk", rs_file)

        reqs = MockLLM.requests("/responses")
        ok(f"S8: {len(reqs)} xai API calls (fn_call + text)") \
            if len(reqs) == 2 else \
            fail(f"S8: expected 2 API calls, got {len(reqs)}")
    finally:
        stop_runner("s8")
        try: os.unlink(rs_file)
        except OSError: pass


# ═════════════════════════════════════════════════════════════════════════════
# S9 — or→xai cross-runner: openrouter analyzes → xai fixes
# ═════════════════════════════════════════════════════════════════════════════

async def s9_or_to_xai_cross_runner(nc):
    section("S9: or→xai cross-runner — openrouter analyzes, xai applies fix")
    pfx_or  = f"{BASE}.s9o"
    pfx_xai = f"{BASE}.s9x"

    code_file = f"/tmp/prog3_s9_{os.getpid()}.py"
    with open(code_file, "w") as f:
        f.write("def square(x):\n    x * x  # missing return\n")

    MockLLM.reset()
    # openrouter: read file + analyze
    MockLLM.queue("/chat/completions",
        or_sse_tool("c-s9-01", "read_file", json.dumps({"path": code_file})))
    MockLLM.queue("/chat/completions",
        or_sse("Bug: `square` computes `x * x` but discards the result — needs `return`."))
    # xai: apply fix (has openrouter context from import)
    MockLLM.queue("/responses",
        xai_sse_fn("r-s9-01", "c-s9-01", "str_replace", json.dumps({
            "path": code_file,
            "old_str": "    x * x  # missing return",
            "new_str": "    return x * x",
        })))
    MockLLM.queue("/responses",
        xai_sse("Fixed! Added `return x * x`.", "r-s9-02"))

    start_runner("s9o", BIN["or"], {
        "ACP_PREFIX":          pfx_or,
        "OPENROUTER_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":  "fake",
    })
    start_runner("s9x", BIN["xai"], {
        "ACP_PREFIX":        pfx_xai,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid_or  = await wait_runner(nc, pfx_or,  cwd="/tmp")
        sid_xai = await wait_runner(nc, pfx_xai, cwd="/tmp")
        if not sid_or or not sid_xai:
            fail("S9: runner(s) timed out"); return
        info(f"or={sid_or[:8]}  xai={sid_xai[:8]}")

        # Step 1: openrouter analyzes
        r1 = await send_prompt(nc, pfx_or, sid_or, f"Analyze {code_file} and find bugs")
        if prompt_err(r1):
            fail("S9: openrouter analysis failed", str(r1)); return
        ok("S9: openrouter analyzed code (read_file + text)")

        or_reqs = MockLLM.requests("/chat/completions")
        ok(f"S9: openrouter made {len(or_reqs)} API calls") \
            if len(or_reqs) == 2 else \
            fail(f"S9: expected 2 openrouter calls, got {len(or_reqs)}")

        # Step 2: export openrouter → import into xai
        exported = await nats_req(nc, f"{pfx_or}.agent.ext.session/export",
                                  {"sessionId": sid_or})
        if not isinstance(exported, list) or not exported:
            fail("S9: or export failed", str(exported)[:100]); return
        ok(f"S9: or session exported ({len(exported)} messages)")

        ir = await nats_req(nc, f"{pfx_xai}.agent.ext.session/import",
                            {"sessionId": sid_xai, "messages": exported})
        if isinstance(ir, dict) and "code" in ir:
            fail("S9: xai import failed", str(ir)); return
        ok("S9: or analysis imported into xai session")

        # Step 3: xai applies fix using the imported analysis
        xai_before = len(MockLLM.requests("/responses"))
        r2 = await send_prompt(nc, pfx_xai, sid_xai, f"Apply the fix to {code_file}")
        if prompt_err(r2):
            fail("S9: xai fix failed", str(r2)); return
        ok("S9: xai applied fix (str_replace + text)")

        xai_reqs = MockLLM.requests("/responses")[xai_before:]
        ok(f"S9: xai made {len(xai_reqs)} API calls") \
            if len(xai_reqs) == 2 else \
            fail(f"S9: expected 2 xai calls, got {len(xai_reqs)}")

        # xai's first request must include imported openrouter history
        if xai_reqs:
            t1_input = xai_reqs[0].get("input", [])
            ok(f"S9: xai request has imported or context ({len(t1_input)} input items)") \
                if len(t1_input) > 1 else \
                fail("S9: xai input too short — import may not have worked",
                     f"{len(t1_input)} items")

        with open(code_file) as f:
            content = f.read()
        ok("S9: file fixed on disk by xai", repr(content.strip())) \
            if "return x * x" in content else \
            fail("S9: file not fixed on disk", repr(content))
    finally:
        stop_runner("s9o")
        stop_runner("s9x")
        try: os.unlink(code_file)
        except OSError: pass


# ═════════════════════════════════════════════════════════════════════════════
# S10 — set_model unknown model: xai/or reject; acp accepts any (no validation)
# ═════════════════════════════════════════════════════════════════════════════

async def s10_set_model_unknown(nc):
    section("S10: set_model unknown model — xai/or reject, acp accepts any")
    runners = [
        ("xai", BIN["xai"], {
            "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
            "XAI_API_KEY":       "fake",
            "XAI_DEFAULT_MODEL": "grok-3",
            "XAI_MODELS":        "grok-3:Grok 3",
        }, True),  # True = expects rejection
        ("or",  BIN["or"],  {
            "OPENROUTER_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
            "OPENROUTER_API_KEY":  "fake",
        }, True),
        ("acp", BIN["acp"], {
            "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
            "ANTHROPIC_TOKEN": "fake",
            "AGENT_MODEL":     "claude-opus-4-6",
        }, False),  # False = acp accepts any model string
    ]
    for label, binary, env_extra, expect_rejection in runners:
        pfx = f"{BASE}.s10_{label}"
        MockLLM.reset()
        start_runner(f"s10_{label}", binary,
                     {"ACP_PREFIX": pfx, **env_extra},
                     cwd="/tmp" if label == "acp" else None)
        try:
            sid = await wait_runner(nc, pfx, cwd="/tmp")
            if not sid:
                fail(f"S10 {label}: session.new timed out"); continue
            info(f"{label} session: {sid[:8]}")

            sr = await send_set_model(nc, pfx, sid, "completely-nonexistent-model-xyz")
            err = prompt_err(sr)
            if expect_rejection:
                if err and "unknown model" in err.lower():
                    ok(f"S10 {label}: unknown model correctly rejected", repr(err[:60]))
                elif err:
                    ok(f"S10 {label}: set_model rejected with error (different message)",
                       repr(err[:60]))
                else:
                    fail(f"S10 {label}: expected rejection but got success", str(sr))
            else:
                if err:
                    fail(f"S10 {label}: unexpected rejection (acp should accept any model)", err)
                else:
                    ok(f"S10 {label}: acp accepted unknown model (no client-side validation)")
                    # Verify model appears in next API call
                    MockLLM.queue("/anthropic/v1/messages", anthropic_sse("ok"))
                    rp = await send_prompt(nc, pfx, sid, "hello")
                    if not prompt_err(rp):
                        acp_reqs = MockLLM.requests("/anthropic/v1/messages")
                        if acp_reqs:
                            m = acp_reqs[-1].get("model")
                            ok(f"S10 acp: model passed to API", f"model={m!r}") \
                                if m == "completely-nonexistent-model-xyz" else \
                                info(f"S10 acp: model in API call: {m!r}")
        finally:
            stop_runner(f"s10_{label}")


# ═════════════════════════════════════════════════════════════════════════════
# S11 — acp: text content block + tool_use in same response
# ═════════════════════════════════════════════════════════════════════════════

async def s11_acp_text_and_tool(nc):
    section("S11: acp text+tool in one response — mixed content blocks")
    pfx = f"{BASE}.s11"
    s11_file = f"/tmp/prog3_s11_{os.getpid()}.py"
    with open(s11_file, "w") as f:
        f.write("def double(x):\n    x * 2  # missing return\n")

    MockLLM.reset()
    # LLM call 1: text "I'll fix this..." + tool_use(str_replace) in same response
    MockLLM.queue("/anthropic/v1/messages",
        anthropic_sse_text_and_tool(
            "I can see the bug — missing return statement. Fixing now...",
            "t-s11-01", "str_replace", json.dumps({
                "path": s11_file,
                "old_str": "    x * 2  # missing return",
                "new_str": "    return x * 2",
            })
        ))
    # LLM call 2: confirmation
    MockLLM.queue("/anthropic/v1/messages",
        anthropic_sse("Fixed! Added `return x * 2`."))

    start_runner("s11", BIN["acp"], {
        "ACP_PREFIX":      pfx,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake",
        "AGENT_MODEL":     "claude-opus-4-6",
    }, cwd="/tmp")
    try:
        sid = await wait_runner(nc, pfx, cwd="/tmp")
        if not sid:
            fail("S11: session.new timed out"); return
        info(f"session: {sid[:8]}")

        mr = await send_set_mode(nc, pfx, sid, "bypassPermissions")
        if prompt_err(mr):
            fail("S11: set_mode", str(mr)); return

        r = await send_prompt(nc, pfx, sid, f"Fix the bug in {s11_file}")
        if prompt_err(r):
            fail("S11: prompt failed", str(r)); return
        ok("S11: prompt completed (text+tool response processed)")

        reqs = MockLLM.requests("/anthropic/v1/messages")
        ok(f"S11: 2 LLM calls (mixed response + confirmation)") \
            if len(reqs) == 2 else \
            fail(f"S11: expected 2 LLM calls, got {len(reqs)}")

        # 2nd call must carry the str_replace tool_result
        if len(reqs) >= 2:
            msgs2 = reqs[1].get("messages", [])
            tool_results = [b for m in msgs2 if isinstance(m.get("content"), list)
                            for b in m["content"] if b.get("type") == "tool_result"]
            ok(f"S11: 2nd LLM call carries str_replace tool_result",
               f"{len(tool_results)} tool_result(s)") \
                if tool_results else \
                fail("S11: 2nd LLM call missing tool_result — text+tool block not processed")

        with open(s11_file) as f:
            content = f.read()
        ok("S11: str_replace from mixed block executed, file fixed on disk",
           repr(content.strip())) \
            if "return x * 2" in content else \
            fail("S11: file NOT fixed — tool in mixed block may not have executed",
                 repr(content))
    finally:
        stop_runner("s11")
        try: os.unlink(s11_file)
        except OSError: pass


# ═════════════════════════════════════════════════════════════════════════════
# S12 — openrouter: 5-turn history accumulates correctly
# ═════════════════════════════════════════════════════════════════════════════

async def s12_openrouter_history_accumulation(nc):
    section("S12: openrouter 5-turn history — message count grows correctly")
    pfx = f"{BASE}.s12"

    MockLLM.reset()
    for i in range(5):
        MockLLM.queue("/chat/completions",
            or_sse(f"Turn {i+1} response: understood your question about step {i+1}."))

    start_runner("s12", BIN["or"], {
        "ACP_PREFIX":          pfx,
        "OPENROUTER_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":  "fake",
    })
    try:
        sid = await wait_runner(nc, pfx, cwd="/tmp")
        if not sid:
            fail("S12: session.new timed out"); return
        info(f"session: {sid[:8]}")

        for i in range(5):
            r = await send_prompt(nc, pfx, sid, f"Programming question {i+1}: explain step {i+1}")
            if prompt_err(r):
                fail(f"S12: turn {i+1} failed", str(r)); return

        ok("S12: all 5 turns completed")
        reqs = MockLLM.requests("/chat/completions")
        ok(f"S12: 5 LLM calls total") if len(reqs) == 5 else \
            fail(f"S12: expected 5 calls, got {len(reqs)}")

        # Check message growth: turn N request should have (2*(N-1)+1) messages
        checks = {1: 1, 3: 5, 5: 9}  # turn → expected min message count
        for turn, expected_min in checks.items():
            if len(reqs) >= turn:
                msgs = reqs[turn - 1].get("messages", [])
                # filter out system message for count
                non_sys = [m for m in msgs if m.get("role") != "system"]
                ok(f"S12: turn {turn} has {len(non_sys)} messages (≥{expected_min})") \
                    if len(non_sys) >= expected_min else \
                    fail(f"S12: turn {turn} history too short",
                         f"expected ≥{expected_min}, got {len(non_sys)}")

        # Specifically: turn 5 must have 9 non-system messages (8 prior + 1 current)
        if len(reqs) >= 5:
            msgs5 = reqs[4].get("messages", [])
            non_sys5 = [m for m in msgs5 if m.get("role") != "system"]
            if len(non_sys5) >= 9:
                ok(f"S12: turn 5 has full history ({len(non_sys5)} messages)")
            else:
                fail(f"S12: turn 5 history incomplete — messages dropped?",
                     f"got {len(non_sys5)}, expected 9")
    finally:
        stop_runner("s12")


# ═════════════════════════════════════════════════════════════════════════════
# S13 — xai→codex cross-runner: codex receives imported history
# ═════════════════════════════════════════════════════════════════════════════

async def s13_xai_to_codex(nc):
    section("S13: xai→codex cross-runner — codex receives imported xai history")
    pfx_xai   = f"{BASE}.s13x"
    pfx_codex = f"{BASE}.s13c"

    if not os.path.exists(BIN["mock_codex"]):
        fail("S13: mock_codex_server not found", BIN["mock_codex"]); return

    MockLLM.reset()
    MockLLM.queue("/responses",
        xai_sse("I've analyzed the code structure and found 3 modules.", "r-s13-01"))

    start_runner("s13x", BIN["xai"], {
        "ACP_PREFIX":        pfx_xai,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    start_runner("s13c", BIN["codex"], {
        "ACP_PREFIX":              pfx_codex,
        "CODEX_BIN":               BIN["mock_codex"],
        "CODEX_SPAWN_TIMEOUT_SECS": "8",
        "MOCK_SEND_N_TEXT_EVENTS": "1",
    })
    try:
        sid_xai   = await wait_runner(nc, pfx_xai,   cwd="/tmp", timeout=RUNNER_READY)
        sid_codex = await wait_runner(nc, pfx_codex,  cwd="/tmp", timeout=18.0)
        if not sid_xai:
            fail("S13: xai session.new timed out"); return
        if not sid_codex:
            fail("S13: codex session.new timed out"); return
        info(f"xai={sid_xai[:8]}  codex={sid_codex[:8]}")

        # Step 1: xai does 1 turn
        r1 = await send_prompt(nc, pfx_xai, sid_xai, "Analyze the codebase structure")
        if prompt_err(r1):
            fail("S13: xai analysis failed", str(r1)); return
        ok("S13: xai completed analysis turn")

        xai_reqs = MockLLM.requests("/responses")
        ok(f"S13: xai made {len(xai_reqs)} API call(s)")

        # Step 2: export xai → import codex
        exported = await nats_req(nc, f"{pfx_xai}.agent.ext.session/export",
                                  {"sessionId": sid_xai})
        if not isinstance(exported, list) or not exported:
            fail("S13: xai export failed", str(exported)[:100]); return
        ok(f"S13: xai exported {len(exported)} messages")

        ir = await nats_req(nc, f"{pfx_codex}.agent.ext.session/import",
                            {"sessionId": sid_codex, "messages": exported})
        if isinstance(ir, dict) and "code" in ir:
            fail("S13: codex import failed", str(ir)); return
        ok("S13: xai history imported into codex session")

        # Step 3: codex prompt (mock_codex_server responds immediately)
        r2 = await send_prompt(nc, pfx_codex, sid_codex,
                               "Continue from the analysis and list the main files",
                               timeout=30.0)
        if prompt_err(r2):
            fail("S13: codex prompt failed", str(r2)); return
        ok("S13: codex responded after receiving xai history")
    finally:
        stop_runner("s13x")
        stop_runner("s13c")


# ═════════════════════════════════════════════════════════════════════════════
# S14 — xai→acp cross-runner: acp receives xai context in its LLM call
# ═════════════════════════════════════════════════════════════════════════════

async def s14_xai_to_acp(nc):
    section("S14: xai→acp cross-runner — acp continues with xai context")
    pfx_xai = f"{BASE}.s14x"
    pfx_acp = f"{BASE}.s14a"

    code_file = f"/tmp/prog3_s14_{os.getpid()}.py"
    with open(code_file, "w") as f:
        f.write("def divide(a, b):\n    a / b  # missing return\n")

    MockLLM.reset()
    # xai: read and analyze
    MockLLM.queue("/responses",
        xai_sse_fn("r-s14-01", "c-s14-01", "read_file",
                   json.dumps({"path": code_file})))
    MockLLM.queue("/responses",
        xai_sse("Found bug: `divide` missing return. The expression is computed but discarded.",
                "r-s14-02"))
    # acp: str_replace fix (with xai context from import)
    MockLLM.queue("/anthropic/v1/messages",
        anthropic_sse_tool("t-s14-01", "str_replace", json.dumps({
            "path": code_file,
            "old_str": "    a / b  # missing return",
            "new_str": "    return a / b",
        })))
    MockLLM.queue("/anthropic/v1/messages",
        anthropic_sse("Fixed! Added `return a / b`."))

    start_runner("s14x", BIN["xai"], {
        "ACP_PREFIX":        pfx_xai,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    start_runner("s14a", BIN["acp"], {
        "ACP_PREFIX":      pfx_acp,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake",
        "AGENT_MODEL":     "claude-opus-4-6",
    }, cwd="/tmp")
    try:
        sid_xai = await wait_runner(nc, pfx_xai, cwd="/tmp")
        sid_acp = await wait_runner(nc, pfx_acp, cwd="/tmp")
        if not sid_xai or not sid_acp:
            fail("S14: runner(s) timed out"); return
        info(f"xai={sid_xai[:8]}  acp={sid_acp[:8]}")

        mr = await send_set_mode(nc, pfx_acp, sid_acp, "bypassPermissions")
        if prompt_err(mr):
            fail("S14: acp set_mode", str(mr)); return

        r1 = await send_prompt(nc, pfx_xai, sid_xai, f"Read and analyze {code_file}")
        if prompt_err(r1):
            fail("S14: xai analysis failed", str(r1)); return
        ok("S14: xai analyzed code (read_file + text)")

        exported = await nats_req(nc, f"{pfx_xai}.agent.ext.session/export",
                                  {"sessionId": sid_xai})
        if not isinstance(exported, list) or not exported:
            fail("S14: xai export failed", str(exported)[:100]); return
        ok(f"S14: xai exported {len(exported)} messages")

        ir = await nats_req(nc, f"{pfx_acp}.agent.ext.session/import",
                            {"sessionId": sid_acp, "messages": exported})
        if isinstance(ir, dict) and "code" in ir:
            fail("S14: acp import failed", str(ir)); return
        ok("S14: xai context imported into acp session")

        acp_before = len(MockLLM.requests("/anthropic/v1/messages"))
        r2 = await send_prompt(nc, pfx_acp, sid_acp, f"Apply the fix to {code_file}")
        if prompt_err(r2):
            fail("S14: acp fix failed", str(r2)); return
        ok("S14: acp applied fix using imported xai context")

        acp_reqs = MockLLM.requests("/anthropic/v1/messages")[acp_before:]
        ok(f"S14: acp made {len(acp_reqs)} API calls") \
            if len(acp_reqs) == 2 else \
            fail(f"S14: expected 2 acp calls, got {len(acp_reqs)}")

        # acp first request must include xai history in messages
        if acp_reqs:
            msgs = acp_reqs[0].get("messages", [])
            ok(f"S14: acp request includes xai context ({len(msgs)} messages)") \
                if len(msgs) > 1 else \
                fail("S14: acp request missing imported xai context", f"{len(msgs)} messages")

        with open(code_file) as f:
            content = f.read()
        ok("S14: file fixed on disk by acp (xai context→acp fix chain)",
           repr(content.strip())) \
            if "return a / b" in content else \
            fail("S14: file not fixed", repr(content))
    finally:
        stop_runner("s14x")
        stop_runner("s14a")
        try: os.unlink(code_file)
        except OSError: pass


# ═════════════════════════════════════════════════════════════════════════════
# S15 — git_status real repo: xai tool returns actual git output
# ═════════════════════════════════════════════════════════════════════════════

async def s15_git_status_real(nc):
    section("S15: git_status real repo — tool returns actual git output from rsworkspace")
    pfx = f"{BASE}.s15"

    MockLLM.reset()
    MockLLM.queue("/responses",
        xai_sse_fn("r-s15-01", "c-s15-01", "git_status", "{}"))
    MockLLM.queue("/responses",
        xai_sse("The repository is on the programming-imple branch with some untracked files.",
                "r-s15-02"))

    start_runner("s15", BIN["xai"], {
        "ACP_PREFIX":        pfx,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        # Session cwd = RSDIR so git_status runs in the actual git repo
        sid = await wait_runner(nc, pfx, cwd=RSDIR)
        if not sid:
            fail("S15: session.new timed out"); return
        info(f"session: {sid[:8]}  cwd={RSDIR}")

        r = await send_prompt(nc, pfx, sid, "What is the current git status of this repo?")
        if prompt_err(r):
            fail("S15: prompt failed", str(r)); return
        ok("S15: prompt completed (git_status tool executed)")

        reqs = MockLLM.requests("/responses")
        ok(f"S15: {len(reqs)} xai API calls") if len(reqs) == 2 else \
            fail(f"S15: expected 2 calls, got {len(reqs)}")

        # The 2nd request must carry the real git status output
        if len(reqs) >= 2:
            inp2 = reqs[1].get("input", [])
            # Find function_call_output items
            fn_outputs = [i for i in inp2 if isinstance(i, dict)
                          and i.get("type") == "function_call_output"]
            if fn_outputs:
                git_output = fn_outputs[0].get("output", "")
                info(f"S15: git_status output = {git_output[:120]!r}")
                if any(kw in git_output for kw in ["branch", "Branch", "nothing", "modified",
                                                     "Untracked", "working tree"]):
                    ok("S15: real git output in function_call_output", git_output[:60])
                elif "Error" in git_output or "outside" in git_output:
                    fail("S15: git_status returned an error", git_output[:80])
                else:
                    ok("S15: git_status executed (output present)", git_output[:60])
            else:
                fail("S15: no function_call_output in 2nd request",
                     f"input items: {[i.get('type') for i in inp2]}")
    finally:
        stop_runner("s15")


# ═════════════════════════════════════════════════════════════════════════════
# S16 — openrouter TROGON.md: system prompt from file in messages[role=system]
# ═════════════════════════════════════════════════════════════════════════════

async def s16_openrouter_trogon_md(nc):
    section("S16: openrouter TROGON.md — system prompt injected into messages[role=system]")
    pfx = f"{BASE}.s16"
    marker = f"PROG3_TROGON_MARKER_{os.getpid()}"

    proj_dir = tempfile.mkdtemp(prefix="prog3_s16_")
    with open(os.path.join(proj_dir, "TROGON.md"), "w") as f:
        f.write(f"# Project Rules\n{marker}\nAlways add type hints to Python functions.")

    MockLLM.reset()
    MockLLM.queue("/chat/completions", or_sse("Understood the project rules."))

    start_runner("s16", BIN["or"], {
        "ACP_PREFIX":          pfx,
        "OPENROUTER_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":  "fake",
    })
    try:
        # Session cwd = proj_dir so TROGON.md is found
        sid = await wait_runner(nc, pfx, cwd=proj_dir)
        if not sid:
            fail("S16: session.new timed out"); return
        info(f"session: {sid[:8]}  cwd={proj_dir}")

        r = await send_prompt(nc, pfx, sid, "How should I write Python functions here?")
        if prompt_err(r):
            fail("S16: prompt failed", str(r)); return
        ok("S16: prompt completed")

        reqs = MockLLM.requests("/chat/completions")
        if not reqs:
            fail("S16: no API call recorded"); return

        body_str = json.dumps(reqs[0])
        if marker in body_str:
            msgs = reqs[0].get("messages", [])
            sys_msgs = [m for m in msgs if m.get("role") == "system"]
            ok(f"S16: TROGON.md marker in LLM request ({len(sys_msgs)} system message(s))")
        else:
            fail("S16: TROGON.md content NOT in LLM request — injection failed",
                 f"marker={marker!r}")
    finally:
        stop_runner("s16")
        shutil.rmtree(proj_dir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S17 — acp: two isolated sessions don't bleed history into each other
# ═════════════════════════════════════════════════════════════════════════════

async def s17_acp_session_isolation(nc):
    section("S17: acp session isolation — two sessions carry independent histories")
    pfx = f"{BASE}.s17"

    MockLLM.reset()
    # Session A: 1 turn
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse("Session A response."))
    # Session B: 1 turn
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse("Session B response."))
    # Session A: 2nd turn (must carry only A's history, not B's)
    MockLLM.queue("/anthropic/v1/messages", anthropic_sse("Session A second response."))

    start_runner("s17", BIN["acp"], {
        "ACP_PREFIX":      pfx,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake",
        "AGENT_MODEL":     "claude-opus-4-6",
    }, cwd="/tmp")
    try:
        # Create two separate sessions on the same runner
        sid_a = await wait_runner(nc, pfx, cwd="/tmp")
        resp_b = await nats_req(nc, f"{pfx}.agent.session.new",
                                {"cwd": "/tmp", "mcpServers": []})
        sid_b = resp_b.get("sessionId")
        if not sid_a or not sid_b:
            fail("S17: one or both sessions failed"); return
        info(f"session A={sid_a[:8]}  B={sid_b[:8]}")

        # Turn A1: unique prompt
        r_a1 = await send_prompt(nc, pfx, sid_a, "Session A question: how do I write a loop?")
        if prompt_err(r_a1):
            fail("S17: session A turn 1 failed", str(r_a1)); return
        ok("S17: session A turn 1 completed")

        # Turn B1: unique prompt
        r_b1 = await send_prompt(nc, pfx, sid_b, "Session B question: what is a closure?")
        if prompt_err(r_b1):
            fail("S17: session B turn 1 failed", str(r_b1)); return
        ok("S17: session B turn 1 completed")

        # Turn A2: session A's 2nd message — should have A's history only
        r_a2 = await send_prompt(nc, pfx, sid_a, "Follow-up on session A")
        if prompt_err(r_a2):
            fail("S17: session A turn 2 failed", str(r_a2)); return
        ok("S17: session A turn 2 completed")

        reqs = MockLLM.requests("/anthropic/v1/messages")
        ok(f"S17: 3 LLM calls total") if len(reqs) == 3 else \
            fail(f"S17: expected 3 calls, got {len(reqs)}")

        # A's 2nd turn (reqs[2]) should have 3 messages: A1-user, A1-asst, A2-user
        if len(reqs) >= 3:
            msgs_a2 = reqs[2].get("messages", [])
            # Find user messages — should only be A's questions
            user_msgs = [m for m in msgs_a2
                         if m.get("role") == "user"
                         and isinstance(m.get("content"), (str, list))]
            msg_texts = [
                (m["content"] if isinstance(m["content"], str)
                 else " ".join(b.get("text", "") for b in m["content"]
                               if isinstance(b, dict)))
                for m in user_msgs
            ]
            all_text = " ".join(msg_texts)
            if "closure" in all_text:
                fail("S17: session A's 2nd turn contains session B's message — ISOLATION BUG!",
                     repr(all_text[:80]))
            else:
                ok(f"S17: session A's history contains only A's messages ({len(msgs_a2)} total)")
    finally:
        stop_runner("s17")


# ═════════════════════════════════════════════════════════════════════════════
# S18 — xai set_model then cross-runner export→openrouter import
# ═════════════════════════════════════════════════════════════════════════════

async def s18_xai_model_switch_then_export(nc):
    section("S18: xai set_model → export → or import — context and model info preserved")
    pfx_xai = f"{BASE}.s18x"
    pfx_or  = f"{BASE}.s18o"

    code_file = f"/tmp/prog3_s18_{os.getpid()}.py"
    with open(code_file, "w") as f:
        f.write("def cube(x):\n    x ** 3  # missing return\n")

    MockLLM.reset()
    # xai grok-3: turn 1 — text analysis
    MockLLM.queue("/responses",
        xai_sse("I see cube() is missing a return statement.", "r-s18-01"))
    # after set_model to grok-3-mini: turn 2 — read file (uses full history)
    MockLLM.queue("/responses",
        xai_sse("Confirmed the missing return. Will hand off to openrouter.", "r-s18-02"))
    # openrouter: fix after import
    MockLLM.queue("/chat/completions",
        or_sse_tool("c-s18-01", "str_replace", json.dumps({
            "path": code_file,
            "old_str": "    x ** 3  # missing return",
            "new_str": "    return x ** 3",
        })))
    MockLLM.queue("/chat/completions",
        or_sse("Fixed! Added `return x ** 3`."))

    start_runner("s18x", BIN["xai"], {
        "ACP_PREFIX":        pfx_xai,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake",
        "XAI_DEFAULT_MODEL": "grok-3",
        "XAI_MODELS":        "grok-3:Grok 3,grok-3-mini:Grok 3 Mini",
    })
    start_runner("s18o", BIN["or"], {
        "ACP_PREFIX":          pfx_or,
        "OPENROUTER_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":  "fake",
    })
    try:
        sid_xai = await wait_runner(nc, pfx_xai, cwd="/tmp")
        sid_or  = await wait_runner(nc, pfx_or,  cwd="/tmp")
        if not sid_xai or not sid_or:
            fail("S18: runner(s) timed out"); return
        info(f"xai={sid_xai[:8]}  or={sid_or[:8]}")

        # Turn 1 with grok-3
        r1 = await send_prompt(nc, pfx_xai, sid_xai, f"Analyze {code_file}")
        if prompt_err(r1):
            fail("S18: xai turn 1 failed", str(r1)); return
        ok("S18: xai turn 1 (grok-3) completed")

        m1 = MockLLM.requests("/responses")[0].get("model")
        ok(f"S18: turn 1 used grok-3", f"model={m1}") \
            if m1 == "grok-3" else \
            fail(f"S18: turn 1 wrong model", f"got {m1}")

        # Switch model mid-session
        sr = await send_set_model(nc, pfx_xai, sid_xai, "grok-3-mini")
        if prompt_err(sr):
            fail("S18: set_model failed", str(sr)); return
        ok("S18: switched to grok-3-mini")

        # Turn 2 with grok-3-mini (last_response_id cleared → full history)
        r2 = await send_prompt(nc, pfx_xai, sid_xai, f"Confirm the issue in {code_file}")
        if prompt_err(r2):
            fail("S18: xai turn 2 failed", str(r2)); return
        ok("S18: xai turn 2 (grok-3-mini) completed")

        reqs_xai = MockLLM.requests("/responses")
        if len(reqs_xai) >= 2:
            m2 = reqs_xai[1].get("model")
            ok(f"S18: turn 2 used grok-3-mini", f"model={m2}") \
                if m2 == "grok-3-mini" else \
                fail(f"S18: turn 2 wrong model", f"got {m2}")

        # Export xai (2-turn session with model switch) → import or
        exported = await nats_req(nc, f"{pfx_xai}.agent.ext.session/export",
                                  {"sessionId": sid_xai})
        if not isinstance(exported, list) or len(exported) < 2:
            fail("S18: xai export too short", f"got {exported!r}"); return
        ok(f"S18: xai exported {len(exported)} messages from 2-turn session")

        # Verify portable messages are plain text (no xai-specific fields)
        roles = {m.get("role") for m in exported if isinstance(m, dict)}
        ok(f"S18: exported messages have standard roles", str(roles)) \
            if roles <= {"user", "assistant"} else \
            info(f"S18: exported roles: {roles}")

        ir = await nats_req(nc, f"{pfx_or}.agent.ext.session/import",
                            {"sessionId": sid_or, "messages": exported})
        if isinstance(ir, dict) and "code" in ir:
            fail("S18: or import failed", str(ir)); return
        ok("S18: xai 2-turn history imported into openrouter")

        or_before = len(MockLLM.requests("/chat/completions"))
        r3 = await send_prompt(nc, pfx_or, sid_or, f"Apply the fix to {code_file}")
        if prompt_err(r3):
            fail("S18: or fix failed", str(r3)); return
        ok("S18: openrouter applied fix with xai 2-turn context")

        or_reqs = MockLLM.requests("/chat/completions")[or_before:]
        if or_reqs:
            msgs = or_reqs[0].get("messages", [])
            ok(f"S18: or request has {len(msgs)} messages (includes xai history)") \
                if len(msgs) > 2 else \
                fail("S18: or request missing xai history", f"{len(msgs)} messages")

        with open(code_file) as f:
            content = f.read()
        ok("S18: file fixed by openrouter after xai model-switch export",
           repr(content.strip())) \
            if "return x ** 3" in content else \
            fail("S18: file not fixed", repr(content))
    finally:
        stop_runner("s18x")
        stop_runner("s18o")
        try: os.unlink(code_file)
        except OSError: pass


# ═════════════════════════════════════════════════════════════════════════════
# S19 — 3-runner chain: acp → or → xai (each stage adds a coding step)
# ═════════════════════════════════════════════════════════════════════════════

async def s19_three_runner_chain(nc):
    section("S19: 3-runner chain — acp→or→xai, each stage adds context")
    pfx_acp = f"{BASE}.s19a"
    pfx_or  = f"{BASE}.s19o"
    pfx_xai = f"{BASE}.s19x"

    code_file = f"/tmp/prog3_s19_{os.getpid()}.py"
    with open(code_file, "w") as f:
        f.write(
            "def add(a, b):\n    a + b  # bug 1\n\n"
            "def sub(a, b):\n    a - b  # bug 2\n\n"
            "def mul(a, b):\n    a * b  # bug 3\n"
        )

    MockLLM.reset()
    # Stage 1 (acp): identify all 3 bugs
    MockLLM.queue("/anthropic/v1/messages",
        anthropic_sse("Found 3 bugs: add, sub, and mul all missing return statements."))
    # Stage 2 (or): fix bug 1
    MockLLM.queue("/chat/completions",
        or_sse_tool("c-s19-01", "str_replace", json.dumps({
            "path": code_file, "old_str": "    a + b  # bug 1", "new_str": "    return a + b",
        })))
    MockLLM.queue("/chat/completions",
        or_sse("Fixed bug 1 (add). Context from acp: sub and mul still need fixes."))
    # Stage 3 (xai): fix bugs 2 and 3
    MockLLM.queue("/responses",
        xai_sse_fn("r-s19-01", "c-s19-01", "str_replace", json.dumps({
            "path": code_file, "old_str": "    a - b  # bug 2", "new_str": "    return a - b",
        })))
    MockLLM.queue("/responses",
        xai_sse_fn("r-s19-02", "c-s19-02", "str_replace", json.dumps({
            "path": code_file, "old_str": "    a * b  # bug 3", "new_str": "    return a * b",
        })))
    MockLLM.queue("/responses",
        xai_sse("All 3 bugs fixed across the 3-runner chain!", "r-s19-03"))

    start_runner("s19a", BIN["acp"], {
        "ACP_PREFIX":      pfx_acp,
        "PROXY_URL":       f"http://127.0.0.1:{MOCK_PORT}",
        "ANTHROPIC_TOKEN": "fake",
        "AGENT_MODEL":     "claude-opus-4-6",
    }, cwd="/tmp")
    start_runner("s19o", BIN["or"], {
        "ACP_PREFIX":          pfx_or,
        "OPENROUTER_BASE_URL": f"http://127.0.0.1:{MOCK_PORT}",
        "OPENROUTER_API_KEY":  "fake",
    })
    start_runner("s19x", BIN["xai"], {
        "ACP_PREFIX":        pfx_xai,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake",
        "XAI_DEFAULT_MODEL": "grok-3",
    })
    try:
        sid_acp = await wait_runner(nc, pfx_acp, cwd="/tmp")
        sid_or  = await wait_runner(nc, pfx_or,  cwd="/tmp")
        sid_xai = await wait_runner(nc, pfx_xai, cwd="/tmp")
        if not all([sid_acp, sid_or, sid_xai]):
            fail("S19: one or more runners timed out"); return
        info(f"acp={sid_acp[:8]}  or={sid_or[:8]}  xai={sid_xai[:8]}")

        mr = await send_set_mode(nc, pfx_acp, sid_acp, "bypassPermissions")
        if prompt_err(mr):
            fail("S19: acp set_mode", str(mr)); return

        # Stage 1: acp identifies bugs
        r1 = await send_prompt(nc, pfx_acp, sid_acp, f"Read {code_file} and list all bugs")
        if prompt_err(r1):
            fail("S19: acp stage 1 failed", str(r1)); return
        ok("S19: stage 1 (acp) — bugs identified")

        exp1 = await nats_req(nc, f"{pfx_acp}.agent.ext.session/export",
                              {"sessionId": sid_acp})
        if not isinstance(exp1, list):
            fail("S19: acp export failed", str(exp1)[:100]); return
        ok(f"S19: acp exported {len(exp1)} messages")

        ir1 = await nats_req(nc, f"{pfx_or}.agent.ext.session/import",
                             {"sessionId": sid_or, "messages": exp1})
        if isinstance(ir1, dict) and "code" in ir1:
            fail("S19: or import of acp context failed", str(ir1)); return
        ok("S19: acp context imported into openrouter")

        # Stage 2: or fixes bug 1
        or_before = len(MockLLM.requests("/chat/completions"))
        r2 = await send_prompt(nc, pfx_or, sid_or, f"Fix bug 1 (add function) in {code_file}")
        if prompt_err(r2):
            fail("S19: or stage 2 failed", str(r2)); return
        ok("S19: stage 2 (openrouter) — bug 1 fixed")

        or_reqs = MockLLM.requests("/chat/completions")[or_before:]
        if or_reqs:
            msgs2 = or_reqs[0].get("messages", [])
            ok(f"S19: or has acp context in request ({len(msgs2)} messages)") \
                if len(msgs2) > 1 else \
                info(f"S19: or request has {len(msgs2)} messages")

        exp2 = await nats_req(nc, f"{pfx_or}.agent.ext.session/export",
                              {"sessionId": sid_or})
        if not isinstance(exp2, list):
            fail("S19: or export failed", str(exp2)[:100]); return
        ok(f"S19: or exported {len(exp2)} messages (acp+or combined)")

        ir2 = await nats_req(nc, f"{pfx_xai}.agent.ext.session/import",
                             {"sessionId": sid_xai, "messages": exp2})
        if isinstance(ir2, dict) and "code" in ir2:
            fail("S19: xai import of or context failed", str(ir2)); return
        ok("S19: acp+or context imported into xai")

        # Stage 3: xai fixes bugs 2 and 3
        xai_before = len(MockLLM.requests("/responses"))
        r3 = await send_prompt(nc, pfx_xai, sid_xai,
                               f"Fix bugs 2 and 3 (sub and mul) in {code_file}")
        if prompt_err(r3):
            fail("S19: xai stage 3 failed", str(r3)); return
        ok("S19: stage 3 (xai) — bugs 2 and 3 fixed")

        xai_reqs = MockLLM.requests("/responses")[xai_before:]
        ok(f"S19: xai made {len(xai_reqs)} API calls (2 str_replace + 1 text)") \
            if len(xai_reqs) == 3 else \
            fail(f"S19: expected 3 xai calls, got {len(xai_reqs)}")

        # xai's first request must include the full acp+or history
        if xai_reqs:
            t1_input = xai_reqs[0].get("input", [])
            ok(f"S19: xai has full chain context ({len(t1_input)} input items)") \
                if len(t1_input) > 2 else \
                info(f"S19: xai input items: {len(t1_input)}")

        # All 3 bugs must be fixed on disk
        with open(code_file) as f:
            content = f.read()
        bugs_fixed = sum([
            "return a + b" in content,
            "return a - b" in content,
            "return a * b" in content,
        ])
        ok(f"S19: all 3 bugs fixed on disk by the 3-runner chain",
           f"{bugs_fixed}/3 fixed\n{content}") \
            if bugs_fixed == 3 else \
            fail(f"S19: only {bugs_fixed}/3 bugs fixed on disk", repr(content))
    finally:
        stop_runner("s19a")
        stop_runner("s19o")
        stop_runner("s19x")
        try: os.unlink(code_file)
        except OSError: pass


# ═════════════════════════════════════════════════════════════════════════════
# Main
# ═════════════════════════════════════════════════════════════════════════════

async def main():
    print(f"\n=== smoke_test_programming3.py — deeper programming scenarios ===")
    print(f"  prefix : {BASE}")
    print(f"  nats   : {NATS_URL}")
    print(f"  mock   : http://127.0.0.1:{MOCK_PORT}\n")

    missing = [k for k, v in BIN.items() if k != "mock_codex" and not os.path.exists(v)]
    if missing:
        print(f"  {red}ERROR{reset}: missing binaries: {missing}")
        sys.exit(1)

    server = start_mock(MOCK_PORT)
    info(f"mock server started on :{MOCK_PORT}")

    nc = await nats_lib.connect(NATS_URL)
    try:
        await s5_xai_stateful_multi_turn(nc)
        await s6_openrouter_parallel_tools(nc)
        await s7_acp_write_then_fix(nc)
        await s8_xai_write_file(nc)
        await s9_or_to_xai_cross_runner(nc)
        await s10_set_model_unknown(nc)
        await s11_acp_text_and_tool(nc)
        await s12_openrouter_history_accumulation(nc)
        await s13_xai_to_codex(nc)
        await s14_xai_to_acp(nc)
        await s15_git_status_real(nc)
        await s16_openrouter_trogon_md(nc)
        await s17_acp_session_isolation(nc)
        await s18_xai_model_switch_then_export(nc)
        await s19_three_runner_chain(nc)
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
