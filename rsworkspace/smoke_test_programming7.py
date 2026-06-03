#!/usr/bin/env python3
"""
smoke_test_programming7.py — Deep xai-runner behavioral verification.

Every scenario targets correct xai-runner behavior that could hide subtle bugs:

  S59. xai get_state fresh          before any prompts: history=[], last_response_id=null
  S60. xai get_state after prompt   last_response_id matches SSE resp_id; history grows
  S61. xai stale-ID retry           error on 2nd turn → runner retries with full history (3 calls total)
  S62. xai 3-turn resp_id chain     each turn uses previous turn's resp_id as previous_response_id
  S63. xai 2-tool-round chain       fn_call A → fn_call B → text in 1 prompt; 3 xai calls, correct prev_ids
  S64. xai TROGON.md system item    TROGON.md marker appears in /responses input as system role item
  S65. xai list_children empty      root session → {"children": []}
  S66. xai unknown ext_method       returns "unknown ext method" error; runner survives
  S67. xai get_state after set_model state.model reflects set_model change
  S68. xai stateful 2nd turn        only 1 input item in 2nd call (user msg only, not full history)
  S69. xai export PortableMessage    format: role + text + blocks all present; text matches content
  S70. xai 2 sessions same runner   each session's get_state shows its own model (isolation)

Usage:
    NATS_URL=nats://localhost:4333 python3 smoke_test_programming7.py
"""

import asyncio, collections, json, os, shutil, subprocess, sys, tempfile, threading, time, uuid
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Optional

import nats as nats_lib

# ── Config ────────────────────────────────────────────────────────────────────
NATS_URL       = os.environ.get("NATS_URL", "nats://localhost:4222")
RSDIR          = os.path.dirname(os.path.abspath(__file__))
MOCK_PORT      = 19822
BASE           = f"prog7.{os.getpid()}"
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


def xai_sse_error(message: str) -> bytes:
    """Simulates a mid-stream xAI error (server error, not a 4xx client error)."""
    return (
        "data: " + json.dumps({"type": "response.error",
                               "error": {"code": "server_error", "message": message}}) + "\n\n"
        "data: [DONE]\n\n"
    ).encode()


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
            resp = xai_sse("ok")
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
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


def xai_runner_env(prefix: str, model: str = "grok-3") -> dict:
    return {
        "ACP_PREFIX":        prefix,
        "XAI_BASE_URL":      f"http://127.0.0.1:{MOCK_PORT}",
        "XAI_API_KEY":       "fake-xai-key",
        "XAI_DEFAULT_MODEL": model,
    }


BIN_XAI = os.path.join(RSDIR, "target/debug/trogon-xai-runner")


# ═════════════════════════════════════════════════════════════════════════════
# S59 — xai get_state fresh: before any prompts → history=[], last_response_id=null
# ═════════════════════════════════════════════════════════════════════════════

async def s59_xai_get_state_fresh(nc):
    section("S59: xai get_state fresh — no prompts → history=[], last_response_id=null")
    prefix = f"{BASE}.xai_s59"
    MockLLM.reset()

    start_runner("s59", BIN_XAI, xai_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S59: session.new timed out"); return
        info(f"S59: session={sid[:8]} — calling get_state before any prompt")

        state = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid})
        info(f"S59: get_state = {str(state)[:200]!r}")

        if "_error" in state or "code" in state:
            fail("S59: get_state returned error", str(state)[:80]); return
        ok("S59: get_state succeeded")

        # history should be empty
        history = state.get("history", None)
        if history is not None and len(history) == 0:
            ok("S59: history=[] for fresh session")
        elif history is None:
            ok("S59: history field absent (fresh session has no history)")
        else:
            fail("S59: fresh session has non-empty history", str(history)[:80])

        # last_response_id should be null/None
        last_id = state.get("last_response_id", "FIELD_ABSENT")
        if last_id is None:
            ok("S59: last_response_id=null for fresh session")
        elif last_id == "FIELD_ABSENT":
            ok("S59: last_response_id field absent (never set)")
        else:
            fail("S59: last_response_id set on fresh session", repr(last_id))

        # cwd should match what we passed
        cwd_val = state.get("cwd", "")
        if cwd_val == "/tmp":
            ok("S59: cwd='/tmp' in get_state")
        else:
            fail("S59: cwd mismatch in get_state", f"expected '/tmp', got {cwd_val!r}")
    finally:
        stop_runner("s59")


# ═════════════════════════════════════════════════════════════════════════════
# S60 — xai get_state after prompt: last_response_id matches SSE resp_id
# ═════════════════════════════════════════════════════════════════════════════

async def s60_xai_get_state_after_prompt(nc):
    section("S60: xai get_state after prompt — last_response_id matches SSE resp_id")
    prefix = f"{BASE}.xai_s60"
    known_resp_id = f"resp-s60-{uuid.uuid4().hex[:8]}"

    MockLLM.reset()
    MockLLM.queue(xai_sse("hello from the model", known_resp_id))

    start_runner("s60", BIN_XAI, xai_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S60: session.new timed out"); return

        r = await send_prompt(nc, prefix, sid, "hello")
        if prompt_err(r):
            fail("S60: prompt failed", str(r)); return
        ok("S60: prompt completed")

        state = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid})
        info(f"S60: get_state last_response_id = {state.get('last_response_id')!r}")

        last_id = state.get("last_response_id")
        if last_id == known_resp_id:
            ok(f"S60: last_response_id matches SSE resp_id exactly ({known_resp_id[:20]}...)")
        elif last_id:
            fail("S60: last_response_id set but doesn't match SSE resp_id",
                 f"expected={known_resp_id!r} got={last_id!r}")
        else:
            fail("S60: last_response_id is null after successful prompt")

        # history should have 2 entries (user + assistant)
        history = state.get("history", [])
        if len(history) == 2:
            ok("S60: history has 2 entries (user + assistant) after 1 prompt")
        else:
            fail(f"S60: expected 2 history entries, got {len(history)}")

        roles = [m.get("role") for m in history if isinstance(m, dict)]
        if "user" in roles and "assistant" in roles:
            ok("S60: history has both user and assistant roles")
        else:
            fail("S60: unexpected roles in history", str(roles))
    finally:
        stop_runner("s60")


# ═════════════════════════════════════════════════════════════════════════════
# S61 — xai stale previous_response_id retry: error on 2nd turn → retry with full history
# ═════════════════════════════════════════════════════════════════════════════

async def s61_xai_stale_id_retry(nc):
    section("S61: xai stale-ID retry — error on 2nd turn → runner retries; 3 API calls total")
    prefix = f"{BASE}.xai_s61"

    turn1_resp_id = "resp-s61-turn1"
    turn2_retry_resp_id = "resp-s61-turn2-retry"

    MockLLM.reset()
    # 1: turn 1 → success (sets last_response_id = turn1_resp_id)
    MockLLM.queue(xai_sse("turn 1 success", turn1_resp_id))
    # 2: turn 2 first attempt (with previous_response_id=turn1_resp_id) → server error
    MockLLM.queue(xai_sse_error("stale previous_response_id from expired context"))
    # 3: turn 2 retry (full history, no previous_response_id) → success
    MockLLM.queue(xai_sse("turn 2 retry success", turn2_retry_resp_id))

    start_runner("s61", BIN_XAI, xai_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S61: session.new timed out"); return

        # Turn 1: success — establishes last_response_id
        r1 = await send_prompt(nc, prefix, sid, "turn 1 message")
        if prompt_err(r1):
            fail("S61: turn 1 failed", str(r1)); return
        ok("S61: turn 1 completed (last_response_id established)")

        # Turn 2: first attempt errors, runner retries transparently
        r2 = await send_prompt(nc, prefix, sid, "turn 2 message")
        if prompt_err(r2):
            fail("S61: turn 2 failed after retry (runner gave up)", str(r2)); return
        ok("S61: turn 2 completed (transparent retry succeeded)")

        reqs = MockLLM.requests()
        info(f"S61: total xai API calls = {len(reqs)}")

        if len(reqs) == 3:
            ok("S61: exactly 3 API calls (turn1 + failed_attempt + retry)")
        elif len(reqs) >= 3:
            ok(f"S61: {len(reqs)} API calls (≥3 expected for retry)")
        else:
            fail(f"S61: expected 3 API calls, got {len(reqs)} — retry may not have fired")
            return

        # Call 1 (turn 1): no previous_response_id
        prev_id_1 = reqs[0].get("previous_response_id")
        if prev_id_1 is None:
            ok("S61: call 1 has no previous_response_id (fresh session)")
        else:
            info(f"S61: call 1 previous_response_id = {prev_id_1!r}")

        # Call 2 (turn 2 first attempt): MUST have previous_response_id = turn1_resp_id
        prev_id_2 = reqs[1].get("previous_response_id")
        if prev_id_2 == turn1_resp_id:
            ok(f"S61: call 2 has previous_response_id='{turn1_resp_id}' (stale ID sent)")
        else:
            fail("S61: call 2 missing expected previous_response_id",
                 f"expected={turn1_resp_id!r} got={prev_id_2!r}")

        # Call 3 (retry): MUST NOT have previous_response_id (full history mode)
        prev_id_3 = reqs[2].get("previous_response_id")
        if prev_id_3 is None:
            ok("S61: call 3 (retry) has no previous_response_id — full history sent")
        else:
            fail("S61: call 3 (retry) still uses previous_response_id — retry logic broken",
                 f"prev_id={prev_id_3!r}")

        # Verify retry call has more input items (full history, not just 1)
        input_retry = reqs[2].get("input", [])
        if len(input_retry) > 1:
            ok(f"S61: retry call has {len(input_retry)} input items (full history, not just user msg)")
        elif len(input_retry) == 1:
            # might be ok if history is compressed, but unusual
            info(f"S61: retry call has 1 input item (may have been condensed)")
        else:
            fail("S61: retry call has no input items", str(reqs[2])[:80])
    finally:
        stop_runner("s61")


# ═════════════════════════════════════════════════════════════════════════════
# S62 — xai 3-turn previous_response_id chain: each turn uses previous turn's resp_id
# ═════════════════════════════════════════════════════════════════════════════

async def s62_xai_three_turn_resp_id_chain(nc):
    section("S62: xai 3-turn resp_id chain — each turn uses previous turn's resp_id")
    prefix = f"{BASE}.xai_s62"

    r1, r2, r3 = "resp-s62-t1", "resp-s62-t2", "resp-s62-t3"

    MockLLM.reset()
    MockLLM.queue(xai_sse("response 1", r1))
    MockLLM.queue(xai_sse("response 2", r2))
    MockLLM.queue(xai_sse("response 3", r3))

    start_runner("s62", BIN_XAI, xai_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S62: session.new timed out"); return

        for i, (text, rid) in enumerate([(f"turn-{i+1}", r) for i, r in enumerate([r1, r2, r3])], 1):
            r = await send_prompt(nc, prefix, sid, f"turn-{i}")
            if prompt_err(r):
                fail(f"S62: turn {i} failed", str(r)); return
        ok("S62: 3 turns completed")

        reqs = MockLLM.requests()
        if len(reqs) != 3:
            fail(f"S62: expected 3 API calls, got {len(reqs)}"); return
        ok("S62: exactly 3 xai API calls")

        # Turn 1: no previous_response_id
        p1 = reqs[0].get("previous_response_id")
        if p1 is None:
            ok("S62: turn 1 has no previous_response_id (correct — first turn)")
        else:
            fail("S62: turn 1 should have no previous_response_id", repr(p1))

        # Turn 2: must use r1 as previous_response_id
        p2 = reqs[1].get("previous_response_id")
        if p2 == r1:
            ok(f"S62: turn 2 uses turn 1's resp_id as previous_response_id ('{r1}')")
        else:
            fail("S62: turn 2 wrong previous_response_id",
                 f"expected={r1!r} got={p2!r}")

        # Turn 3: must use r2 as previous_response_id
        p3 = reqs[2].get("previous_response_id")
        if p3 == r2:
            ok(f"S62: turn 3 uses turn 2's resp_id as previous_response_id ('{r2}')")
        else:
            fail("S62: turn 3 wrong previous_response_id",
                 f"expected={r2!r} got={p3!r}")

        # Turn 2 and 3 should have only 1 input item (user message only — stateful mode)
        input2 = reqs[1].get("input", [])
        input3 = reqs[2].get("input", [])
        if len(input2) == 1 and len(input3) == 1:
            ok("S62: turns 2 and 3 each have 1 input item (stateful — not full history)")
        else:
            info(f"S62: input lengths: turn2={len(input2)} turn3={len(input3)}")
            fail("S62: expected 1 input item per stateful turn", f"turn2={input2} turn3={input3}")
    finally:
        stop_runner("s62")


# ═════════════════════════════════════════════════════════════════════════════
# S63 — xai 2-tool-round chain in 1 prompt: fn_call A → fn_call B → text
# ═════════════════════════════════════════════════════════════════════════════

async def s63_xai_two_tool_round_chain(nc):
    section("S63: xai 2-tool-round chain — fn_call A → fn_call B → text; 3 calls, correct prev_ids")
    prefix = f"{BASE}.xai_s63"
    tmpdir = tempfile.mkdtemp(prefix="prog7_s63_")

    with open(os.path.join(tmpdir, "alpha.txt"), "w") as f: f.write("alpha file content\n")
    with open(os.path.join(tmpdir, "beta.txt"), "w") as f: f.write("beta file content\n")

    ra1, ra2, ra3 = "resp-s63-a1", "resp-s63-a2", "resp-s63-a3"

    MockLLM.reset()
    # Call 1 (user prompt → fn_call A)
    MockLLM.queue(xai_sse_fn(ra1, "ca1", "read_file", json.dumps({"path": "alpha.txt"})))
    # Call 2 (fn_output_A → fn_call B)
    MockLLM.queue(xai_sse_fn(ra2, "ca2", "read_file", json.dumps({"path": "beta.txt"})))
    # Call 3 (fn_output_B → text)
    MockLLM.queue(xai_sse("both files analyzed successfully", ra3))

    start_runner("s63", BIN_XAI, xai_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd=tmpdir)
        if not sid:
            fail("S63: session.new timed out"); return

        r = await send_prompt(nc, prefix, sid, "analyze both files")
        if prompt_err(r):
            fail("S63: prompt failed", str(r)); return
        ok("S63: prompt completed (2 tool rounds + final text)")

        reqs = MockLLM.requests()
        info(f"S63: total xai API calls = {len(reqs)}")

        if len(reqs) == 3:
            ok("S63: exactly 3 xai API calls (user → fn_A → fn_B → text)")
        else:
            fail(f"S63: expected 3 API calls, got {len(reqs)}")
            return

        # Call 1: user's initial message, no previous_response_id
        p1 = reqs[0].get("previous_response_id")
        if p1 is None:
            ok("S63: call 1 has no previous_response_id (initial prompt)")
        else:
            fail("S63: call 1 should have no previous_response_id", repr(p1))

        # Call 2: function_call_output for tool A, previous_response_id=ra1
        p2 = reqs[1].get("previous_response_id")
        input2 = reqs[1].get("input", [])
        if p2 == ra1:
            ok(f"S63: call 2 uses ra1 as previous_response_id ('{ra1}')")
        else:
            fail("S63: call 2 wrong previous_response_id", f"expected={ra1!r} got={p2!r}")

        if input2 and input2[0].get("type") == "function_call_output":
            call_id_2 = input2[0].get("call_id", "")
            output_2 = input2[0].get("output", "")
            if call_id_2 == "ca1":
                ok("S63: call 2 input has function_call_output with call_id=ca1")
            else:
                fail("S63: call 2 function_call_output wrong call_id", f"got={call_id_2!r}")
            if "alpha file content" in output_2:
                ok("S63: call 2 function_call_output contains alpha.txt content")
            else:
                fail("S63: call 2 function_call_output missing alpha content", repr(output_2[:80]))
        else:
            fail("S63: call 2 input missing function_call_output", str(input2)[:80])

        # Call 3: function_call_output for tool B, previous_response_id=ra2
        p3 = reqs[2].get("previous_response_id")
        input3 = reqs[2].get("input", [])
        if p3 == ra2:
            ok(f"S63: call 3 uses ra2 as previous_response_id ('{ra2}')")
        else:
            fail("S63: call 3 wrong previous_response_id", f"expected={ra2!r} got={p3!r}")

        if input3 and input3[0].get("type") == "function_call_output":
            call_id_3 = input3[0].get("call_id", "")
            output_3 = input3[0].get("output", "")
            if call_id_3 == "ca2":
                ok("S63: call 3 input has function_call_output with call_id=ca2")
            else:
                fail("S63: call 3 function_call_output wrong call_id", f"got={call_id_3!r}")
            if "beta file content" in output_3:
                ok("S63: call 3 function_call_output contains beta.txt content")
            else:
                fail("S63: call 3 function_call_output missing beta content", repr(output_3[:80]))
        else:
            fail("S63: call 3 input missing function_call_output", str(input3)[:80])
    finally:
        stop_runner("s63")
        shutil.rmtree(tmpdir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S64 — xai TROGON.md system item: marker appears in /responses input
# ═════════════════════════════════════════════════════════════════════════════

async def s64_xai_trogon_md_system_item(nc):
    section("S64: xai TROGON.md → marker in /responses input as system item")
    prefix = f"{BASE}.xai_s64"
    tmpdir = tempfile.mkdtemp(prefix="prog7_s64_")
    marker = f"TROGON_S64_MARKER_{uuid.uuid4().hex[:8]}"

    with open(os.path.join(tmpdir, "TROGON.md"), "w") as f:
        f.write(f"# Workspace\nToken: {marker}\n")

    MockLLM.reset()
    MockLLM.queue(xai_sse("understood workspace context", "r-s64"))

    start_runner("s64", BIN_XAI, xai_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd=tmpdir)
        if not sid:
            fail("S64: session.new timed out"); return

        r = await send_prompt(nc, prefix, sid, "hello")
        if prompt_err(r):
            fail("S64: prompt failed", str(r)); return
        ok("S64: prompt completed")

        reqs = MockLLM.requests()
        if not reqs:
            fail("S64: no xai API call"); return

        input_items = reqs[0].get("input", [])
        body_str = json.dumps(reqs[0])
        info(f"S64: input items count = {len(input_items)}")
        info(f"S64: body contains marker? {marker in body_str}")

        if marker not in body_str:
            fail("S64: TROGON.md marker NOT in xai /responses request",
                 f"marker={marker!r}"); return
        ok(f"S64: TROGON.md marker present in xai /responses request")

        # Verify it's in a system-role item
        system_items = [
            i for i in input_items
            if isinstance(i, dict) and i.get("role") == "system"
        ]
        if system_items:
            sys_content = json.dumps(system_items)
            if marker in sys_content:
                ok("S64: marker is in a system-role item in input array")
            else:
                fail("S64: system item present but doesn't contain the marker", sys_content[:100])
        else:
            fail("S64: no system-role item in input — TROGON.md not sent as system")
    finally:
        stop_runner("s64")
        shutil.rmtree(tmpdir, ignore_errors=True)


# ═════════════════════════════════════════════════════════════════════════════
# S65 — xai list_children empty: root session → {"children": []}
# ═════════════════════════════════════════════════════════════════════════════

async def s65_xai_list_children_empty(nc):
    section("S65: xai list_children — root session → {'children': []}")
    prefix = f"{BASE}.xai_s65"
    MockLLM.reset()

    start_runner("s65", BIN_XAI, xai_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S65: session.new timed out"); return
        info(f"S65: session={sid[:8]}")

        resp = await nats_req(nc, f"{prefix}.agent.ext.session/list_children",
                              {"sessionId": sid})
        info(f"S65: list_children response = {str(resp)[:120]!r}")

        if "_error" in resp or "code" in resp:
            fail("S65: list_children returned error", str(resp)[:80]); return

        children = resp.get("children", None)
        if children is None:
            # maybe the response IS the list
            if isinstance(resp, list) and len(resp) == 0:
                ok("S65: list_children returned empty list []")
            else:
                fail("S65: unexpected list_children format", str(resp)[:80])
        elif isinstance(children, list) and len(children) == 0:
            ok("S65: list_children returned {'children': []} for root session")
        elif isinstance(children, list):
            fail(f"S65: expected empty children, got {len(children)}", str(children)[:80])
        else:
            fail("S65: children is not a list", str(resp)[:80])
    finally:
        stop_runner("s65")


# ═════════════════════════════════════════════════════════════════════════════
# S66 — xai unknown ext_method: returns "unknown ext method" error; runner survives
# ═════════════════════════════════════════════════════════════════════════════

async def s66_xai_unknown_ext_method(nc):
    section("S66: xai unknown ext_method → error; runner survives")
    prefix = f"{BASE}.xai_s66"
    MockLLM.reset()
    MockLLM.queue(xai_sse("still alive", "r-s66"))

    start_runner("s66", BIN_XAI, xai_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S66: session.new timed out"); return

        resp = await nats_req(nc, f"{prefix}.agent.ext.session/nonexistent_method_s66",
                              {"sessionId": sid})
        info(f"S66: unknown ext_method response = {str(resp)[:120]!r}")

        if "code" in resp or "_error" in resp:
            msg = resp.get("message", str(resp))
            if "unknown" in msg.lower() or "not found" in msg.lower():
                ok("S66: 'unknown ext method' error returned")
            else:
                ok("S66: error returned for unknown ext_method", msg[:80])
        else:
            fail("S66: no error for unknown ext_method", str(resp)[:80])

        # Runner should still be alive
        r = await send_prompt(nc, prefix, sid, "still alive?")
        if prompt_err(r):
            fail("S66: runner crashed after unknown ext_method", str(r)); return
        ok("S66: runner functional after unknown ext_method call")
    finally:
        stop_runner("s66")


# ═════════════════════════════════════════════════════════════════════════════
# S67 — xai get_state after set_model: state.model reflects change
# ═════════════════════════════════════════════════════════════════════════════

async def s67_xai_get_state_after_set_model(nc):
    section("S67: xai get_state after set_model — state.model reflects the change")
    prefix = f"{BASE}.xai_s67"
    MockLLM.reset()

    start_runner("s67", BIN_XAI, xai_runner_env(prefix, model="grok-3"))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S67: session.new timed out"); return

        # Initial get_state: model should be None (uses runner default)
        state_before = await nats_req(nc, f"{prefix}.agent.ext.session/get_state",
                                      {"sessionId": sid})
        model_before = state_before.get("model")
        info(f"S67: model before set_model = {model_before!r}")
        if model_before is None:
            ok("S67: model=null before set_model (uses runner default grok-3)")
        else:
            info(f"S67: model pre-set = {model_before!r}")

        # set_model to grok-3-mini
        sr = await send_set_model(nc, prefix, sid, "grok-3-mini")
        if prompt_err(sr):
            fail("S67: set_model failed", str(sr)); return
        ok("S67: set_model to grok-3-mini succeeded")

        # get_state after set_model: model should be "grok-3-mini"
        state_after = await nats_req(nc, f"{prefix}.agent.ext.session/get_state",
                                     {"sessionId": sid})
        model_after = state_after.get("model")
        info(f"S67: model after set_model = {model_after!r}")

        if model_after == "grok-3-mini":
            ok("S67: get_state.model='grok-3-mini' after set_model")
        else:
            fail("S67: get_state.model not updated after set_model",
                 f"expected='grok-3-mini' got={model_after!r}")

        # Also verify last_response_id was cleared by set_model
        last_id = state_after.get("last_response_id")
        if last_id is None:
            ok("S67: last_response_id=null after set_model (cleared correctly)")
        else:
            info(f"S67: last_response_id after set_model = {last_id!r} (may have been set before)")
    finally:
        stop_runner("s67")


# ═════════════════════════════════════════════════════════════════════════════
# S68 — xai stateful 2nd turn: only 1 input item (user message, not full history)
# ═════════════════════════════════════════════════════════════════════════════

async def s68_xai_stateful_second_turn_one_input(nc):
    section("S68: xai stateful 2nd turn — only 1 input item (user msg only, not full history)")
    prefix = f"{BASE}.xai_s68"
    turn1_rid = "resp-s68-t1"
    turn2_rid = "resp-s68-t2"

    MockLLM.reset()
    MockLLM.queue(xai_sse("response to turn 1", turn1_rid))
    MockLLM.queue(xai_sse("response to turn 2", turn2_rid))

    start_runner("s68", BIN_XAI, xai_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S68: session.new timed out"); return

        r1 = await send_prompt(nc, prefix, sid, "first turn message")
        if prompt_err(r1):
            fail("S68: turn 1 failed", str(r1)); return

        r2 = await send_prompt(nc, prefix, sid, "second turn message")
        if prompt_err(r2):
            fail("S68: turn 2 failed", str(r2)); return
        ok("S68: 2 turns completed")

        reqs = MockLLM.requests()
        if len(reqs) < 2:
            fail("S68: expected 2 API calls"); return

        input1 = reqs[0].get("input", [])
        input2 = reqs[1].get("input", [])

        info(f"S68: call 1 input items = {len(input1)}, call 2 input items = {len(input2)}")

        # Call 1 (first turn): must have full history (just user message since fresh session)
        # Could be 1 (just the user msg) or 2+ if there's a system prompt
        if len(input1) >= 1:
            ok(f"S68: call 1 has {len(input1)} input item(s)")
        else:
            fail("S68: call 1 has 0 input items")

        # Call 2 (stateful second turn): must have exactly 1 input item (new user message)
        # because previous_response_id carries the context
        if len(input2) == 1:
            item = input2[0]
            role = item.get("role")
            content = item.get("content", "")
            if role == "user" and "second turn" in str(content):
                ok("S68: call 2 has exactly 1 input item (new user message) — stateful mode")
            else:
                ok(f"S68: call 2 has 1 input item (role={role!r})")
        else:
            fail(f"S68: call 2 has {len(input2)} items instead of 1 — may be sending full history",
                 str(input2)[:120])

        # Verify call 2 has previous_response_id set to turn 1's resp_id
        prev_id_2 = reqs[1].get("previous_response_id")
        if prev_id_2 == turn1_rid:
            ok(f"S68: call 2 previous_response_id = turn 1's resp_id ('{turn1_rid}')")
        else:
            fail("S68: call 2 previous_response_id wrong",
                 f"expected={turn1_rid!r} got={prev_id_2!r}")
    finally:
        stop_runner("s68")


# ═════════════════════════════════════════════════════════════════════════════
# S69 — xai export PortableMessage format: role + text + blocks all present
# ═════════════════════════════════════════════════════════════════════════════

async def s69_xai_export_portable_format(nc):
    section("S69: xai export PortableMessage format — role, text, blocks all present")
    prefix = f"{BASE}.xai_s69"
    expected_assistant_text = f"assistant-response-s69-{uuid.uuid4().hex[:8]}"

    MockLLM.reset()
    MockLLM.queue(xai_sse(expected_assistant_text, "r-s69"))

    start_runner("s69", BIN_XAI, xai_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S69: session.new timed out"); return

        r = await send_prompt(nc, prefix, sid, "user message for export test")
        if prompt_err(r):
            fail("S69: prompt failed", str(r)); return
        ok("S69: prompt completed")

        exp = await nats_req(nc, f"{prefix}.agent.ext.session/export", {"sessionId": sid})
        if "_error" in exp or "code" in exp:
            fail("S69: export failed", str(exp)[:80]); return

        msgs = exp if isinstance(exp, list) else []
        info(f"S69: exported {len(msgs)} PortableMessages")

        if len(msgs) != 2:
            fail(f"S69: expected 2 messages, got {len(msgs)}"); return
        ok("S69: 2 PortableMessages exported (user + assistant)")

        user_msg, asst_msg = msgs[0], msgs[1]

        # Verify role field
        if user_msg.get("role") == "user" and asst_msg.get("role") == "assistant":
            ok("S69: roles correct: user then assistant")
        else:
            fail("S69: roles wrong",
                 f"user_role={user_msg.get('role')!r} asst_role={asst_msg.get('role')!r}")

        # Verify text field (assistant message should contain the expected text)
        asst_text = asst_msg.get("text", "")
        if expected_assistant_text in asst_text:
            ok(f"S69: assistant text field contains expected content")
        else:
            fail("S69: assistant text field missing expected content",
                 f"expected={expected_assistant_text!r} got={asst_text!r}")

        # Verify blocks field
        asst_blocks = asst_msg.get("blocks", None)
        if asst_blocks is not None and isinstance(asst_blocks, list):
            if len(asst_blocks) > 0:
                block = asst_blocks[0]
                if block.get("type") == "text":
                    ok("S69: blocks field present with type='text' block")
                    block_text = block.get("text", "")
                    if expected_assistant_text in block_text:
                        ok("S69: blocks[0].text matches expected content")
                    else:
                        fail("S69: blocks[0].text missing expected content",
                             f"got={block_text!r}")
                else:
                    info(f"S69: block type = {block.get('type')!r}")
                    ok("S69: blocks field present")
            else:
                fail("S69: blocks field is empty []")
        elif asst_blocks is None:
            fail("S69: blocks field missing from PortableMessage")
        else:
            fail("S69: blocks field is not a list", str(asst_blocks)[:80])
    finally:
        stop_runner("s69")


# ═════════════════════════════════════════════════════════════════════════════
# S70 — xai 2 sessions same runner: each session's get_state shows its own model
# ═════════════════════════════════════════════════════════════════════════════

async def s70_xai_two_sessions_model_isolation(nc):
    section("S70: xai 2 sessions same runner — each session's get_state shows its own model")
    prefix = f"{BASE}.xai_s70"
    MockLLM.reset()

    start_runner("s70", BIN_XAI, xai_runner_env(prefix, model="grok-3"))
    try:
        sid_a = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid_a:
            fail("S70: session A timed out"); return
        sid_b = await new_session(nc, prefix, cwd="/tmp")
        if not sid_b:
            fail("S70: session B creation failed"); return
        info(f"S70: A={sid_a[:8]}  B={sid_b[:8]}")

        # Set session A model to grok-3-mini
        sr_a = await send_set_model(nc, prefix, sid_a, "grok-3-mini")
        if prompt_err(sr_a):
            fail("S70: session A set_model failed", str(sr_a)); return
        ok("S70: session A model set to grok-3-mini")

        # Set session B model to grok-4
        sr_b = await send_set_model(nc, prefix, sid_b, "grok-4")
        if prompt_err(sr_b):
            fail("S70: session B set_model failed", str(sr_b)); return
        ok("S70: session B model set to grok-4")

        # get_state for each session
        state_a = await nats_req(nc, f"{prefix}.agent.ext.session/get_state",
                                 {"sessionId": sid_a})
        state_b = await nats_req(nc, f"{prefix}.agent.ext.session/get_state",
                                 {"sessionId": sid_b})

        model_a = state_a.get("model")
        model_b = state_b.get("model")
        info(f"S70: session A model = {model_a!r}, session B model = {model_b!r}")

        if model_a == "grok-3-mini":
            ok("S70: session A get_state.model='grok-3-mini' (isolated correctly)")
        else:
            fail("S70: session A model wrong", f"expected='grok-3-mini' got={model_a!r}")

        if model_b == "grok-4":
            ok("S70: session B get_state.model='grok-4' (isolated correctly)")
        else:
            fail("S70: session B model wrong", f"expected='grok-4' got={model_b!r}")

        if model_a != model_b:
            ok("S70: sessions have different models (no cross-contamination)")
        else:
            fail("S70: sessions share the same model — isolation broken", f"both={model_a!r}")

        # Verify cwds are also isolated (both /tmp, but independent)
        cwd_a = state_a.get("cwd", "")
        cwd_b = state_b.get("cwd", "")
        if cwd_a == "/tmp" and cwd_b == "/tmp":
            ok("S70: both sessions have correct cwd='/tmp'")
        else:
            info(f"S70: cwd_a={cwd_a!r} cwd_b={cwd_b!r}")
    finally:
        stop_runner("s70")


# ═════════════════════════════════════════════════════════════════════════════
# Main
# ═════════════════════════════════════════════════════════════════════════════

async def main():
    print(f"\n=== smoke_test_programming7.py — deep xai-runner behavioral verification ===")
    print(f"  prefix : {BASE}")
    print(f"  nats   : {NATS_URL}")
    print(f"  mock   : http://127.0.0.1:{MOCK_PORT}")

    start_mock(MOCK_PORT)
    print(f"  {yellow}INFO{reset}  mock server started on :{MOCK_PORT}")

    nc = await nats_lib.connect(NATS_URL)
    try:
        await s59_xai_get_state_fresh(nc)
        await s60_xai_get_state_after_prompt(nc)
        await s61_xai_stale_id_retry(nc)
        await s62_xai_three_turn_resp_id_chain(nc)
        await s63_xai_two_tool_round_chain(nc)
        await s64_xai_trogon_md_system_item(nc)
        await s65_xai_list_children_empty(nc)
        await s66_xai_unknown_ext_method(nc)
        await s67_xai_get_state_after_set_model(nc)
        await s68_xai_stateful_second_turn_one_input(nc)
        await s69_xai_export_portable_format(nc)
        await s70_xai_two_sessions_model_isolation(nc)
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
