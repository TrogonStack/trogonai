#!/usr/bin/env python3
"""
smoke_test_programming9.py — Deep codex-runner behavioral verification.

The codex-runner wraps the Codex CLI (codex app-server) via JSON-RPC over
stdin/stdout. For integration tests we substitute CODEX_BIN with a
mock_codex_server binary that responds to all JSON-RPC commands and emits
configurable events.

  S83. codex get_state fresh      thread_id set at session creation; history=[], model=null
  S84. codex basic prompt         text "event 0" received; 2 history entries
  S85. codex 3-turn accumulation  6 history entries after 3 prompts (3 user + 3 assistant)
  S86. codex set_model            state.model reflects change; unknown model → -32602 (InvalidParams)
  S87. codex list_children        root session → {"children": []}
  S88. codex export format        PortableMessages with role/text/blocks
  S89. codex tool event           4 history entries: user + asst_tool_call + user_tool_result + asst_text
  S90. codex 3 text events        MOCK_SEND_N_TEXT_EVENTS=3 → "event 0event 1event 2" accumulated
  S91. codex import + next prompt  pending_history used to prepend context; history grows correctly
  S92. codex error during turn    MOCK_TURN_SENDS_ERROR → prompt ends; runner survives
  S93. codex 2 sessions isolated  different thread_ids, different histories, no cross-contamination
  S94. codex unknown ext_method   -32601 error; runner functional afterwards

Usage:
    NATS_URL=nats://localhost:4333 python3 smoke_test_programming9.py
"""

import asyncio
import json
import os
import subprocess
import sys
import time
import uuid
from typing import Optional

import nats as nats_lib

# ── Config ────────────────────────────────────────────────────────────────────
NATS_URL       = os.environ.get("NATS_URL", "nats://localhost:4222")
RSDIR          = os.path.dirname(os.path.abspath(__file__))
BASE           = f"prog9.{os.getpid()}"
PROMPT_TIMEOUT = 45.0
NATS_TIMEOUT   = 12.0
RUNNER_READY   = 14.0

green, red, yellow, cyan, reset = "\033[32m", "\033[31m", "\033[33m", "\033[36m", "\033[0m"
PASS = 0
FAIL = 0
procs: dict = {}

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


# ── Runner helpers ────────────────────────────────────────────────────────────

def codex_runner_env(prefix: str, model: str = "o4-mini", **extra) -> dict:
    env = {
        "ACP_PREFIX":           prefix,
        "CODEX_BIN":            BIN_MOCK,
        "CODEX_DEFAULT_MODEL":  model,
        "CODEX_MODELS":         f"{model}:{model},o3:o3,o4-mini:o4-mini,gpt-4o:GPT-4o",
        "OPENAI_API_KEY":       "fake-openai-key",
    }
    env.update(extra)
    return env


def start_runner(name: str, env_extra: dict):
    env = {**os.environ, "NATS_URL": NATS_URL, "RUST_LOG": "warn", **env_extra}
    procs[name] = subprocess.Popen(
        [BIN_CODEX], env=env,
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


# ═════════════════════════════════════════════════════════════════════════════
# S83 — codex get_state fresh: thread_id set, history=[], model=null
# ═════════════════════════════════════════════════════════════════════════════

async def s83_codex_get_state_fresh(nc):
    section("S83: codex get_state fresh — thread_id set, history=[], model=null")
    prefix = f"{BASE}.cx_s83"
    start_runner("s83", codex_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S83: session.new timed out"); return
        info(f"S83: session={sid[:8]}")

        state = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid})
        if "_error" in state or "code" in state:
            fail("S83: get_state returned error", str(state)[:80]); return
        ok("S83: get_state succeeded")

        thread_id = state.get("thread_id", "")
        if thread_id and thread_id.startswith("mock-thread-"):
            ok(f"S83: thread_id set at session creation ({thread_id})")
        else:
            fail("S83: thread_id should be set by mock at session creation", str(thread_id)[:60])

        history = state.get("history", "MISSING")
        if isinstance(history, list) and len(history) == 0:
            ok("S83: history=[] for fresh session")
        else:
            fail("S83: expected history=[]", str(history)[:60])

        model = state.get("model", "MISSING")
        if model is None:
            ok("S83: model=null (uses runner default)")
        else:
            fail("S83: expected model=null", str(model)[:40])

        cwd = state.get("cwd", "")
        if cwd == "/tmp":
            ok("S83: cwd='/tmp' in get_state")
        else:
            fail("S83: expected cwd='/tmp'", str(cwd))

    finally:
        stop_runner("s83")


# ═════════════════════════════════════════════════════════════════════════════
# S84 — codex basic prompt: text "event 0" received; 2 history entries
# ═════════════════════════════════════════════════════════════════════════════

async def s84_codex_basic_prompt(nc):
    section("S84: codex basic prompt — mock emits text 'event 0'; 2 history entries")
    prefix = f"{BASE}.cx_s84"
    start_runner("s84", codex_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S84: session.new timed out"); return

        resp = await send_prompt(nc, prefix, sid, "tell me something")
        if prompt_err(resp):
            fail("S84: prompt failed", str(resp)[:80]); return
        ok("S84: prompt completed")

        state = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid})
        history = state.get("history", [])
        info(f"S84: history length = {len(history)}")
        if len(history) == 2:
            ok("S84: 2 history entries (user + assistant)")
        else:
            fail("S84: expected 2 history entries", f"got {len(history)}")
            return

        user_entry = history[0]
        if user_entry.get("role") == "user" and "tell me something" in user_entry.get("text", ""):
            ok("S84: user message preserved in history")
        else:
            fail("S84: user entry wrong", str(user_entry)[:80])

        asst_entry = history[1]
        if asst_entry.get("role") == "assistant" and "event 0" in asst_entry.get("text", ""):
            ok("S84: assistant text 'event 0' in history (from mock)")
        else:
            fail("S84: assistant entry wrong", str(asst_entry)[:80])

    finally:
        stop_runner("s84")


# ═════════════════════════════════════════════════════════════════════════════
# S85 — codex 3-turn accumulation: 6 history entries
# ═════════════════════════════════════════════════════════════════════════════

async def s85_codex_multi_turn_history(nc):
    section("S85: codex 3-turn accumulation — 6 history entries (3 user + 3 assistant)")
    prefix = f"{BASE}.cx_s85"
    start_runner("s85", codex_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S85: session.new timed out"); return

        for i in range(1, 4):
            r = await send_prompt(nc, prefix, sid, f"question {i} s85")
            if prompt_err(r):
                fail(f"S85: prompt {i} failed", str(r)[:60]); return
        ok("S85: 3 prompts completed")

        state = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid})
        history = state.get("history", [])
        info(f"S85: history length = {len(history)}")
        if len(history) == 6:
            ok("S85: 6 history entries (3 user + 3 assistant)")
        else:
            fail("S85: expected 6 history entries", f"got {len(history)}")
            return

        roles = [m.get("role") for m in history]
        expected = ["user", "assistant"] * 3
        if roles == expected:
            ok("S85: history roles alternate user/assistant correctly")
        else:
            fail("S85: unexpected history role pattern", str(roles))

    finally:
        stop_runner("s85")


# ═════════════════════════════════════════════════════════════════════════════
# S86 — codex set_model: state.model reflects change; unknown model → error
# ═════════════════════════════════════════════════════════════════════════════

async def s86_codex_set_model(nc):
    section("S86: codex set_model — state.model changes; unknown model → -32602 error")
    prefix = f"{BASE}.cx_s86"
    start_runner("s86", codex_runner_env(prefix, model="o4-mini"))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S86: session.new timed out"); return

        # Initially model=null (uses runner default)
        state = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid})
        if state.get("model") is None:
            ok("S86: initial model=null")
        else:
            fail("S86: expected initial model=null", str(state.get("model")))

        # Set to valid model
        r = await send_set_model(nc, prefix, sid, "o3")
        if "code" in r or "_error" in r:
            fail("S86: set_model to o3 failed", str(r)[:80]); return
        ok("S86: set_model to o3 succeeded")

        state = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid})
        if state.get("model") == "o3":
            ok("S86: get_state.model='o3' after set_model")
        else:
            fail("S86: get_state.model wrong after set_model", str(state.get("model")))

        # Set to unknown model → should fail with -32602 (InvalidParams)
        r2 = await send_set_model(nc, prefix, sid, "completely-unknown-codex-model-s86")
        info(f"S86: unknown model response = {str(r2)[:100]!r}")
        if r2.get("code") == -32602:
            ok("S86: error code -32602 (InvalidParams) for unknown model")
        else:
            fail("S86: expected -32602 for unknown model", str(r2.get("code")))

        if "unknown model" in r2.get("message", "").lower():
            ok("S86: error message mentions 'unknown model'")
        else:
            fail("S86: error message should mention 'unknown model'", r2.get("message", "")[:80])

        # Model should still be o3 after failed set_model
        state = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid})
        if state.get("model") == "o3":
            ok("S86: model remains 'o3' after failed set_model")
        else:
            fail("S86: model changed unexpectedly", str(state.get("model")))

    finally:
        stop_runner("s86")


# ═════════════════════════════════════════════════════════════════════════════
# S87 — codex list_children: root session → {"children": []}
# ═════════════════════════════════════════════════════════════════════════════

async def s87_codex_list_children(nc):
    section("S87: codex list_children — root session → {'children': []}")
    prefix = f"{BASE}.cx_s87"
    start_runner("s87", codex_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S87: session.new timed out"); return

        resp = await nats_req(nc, f"{prefix}.agent.ext.session/list_children",
                              {"sessionId": sid})
        info(f"S87: list_children = {str(resp)[:120]!r}")

        if "_error" in resp or "code" in resp:
            fail("S87: list_children returned error", str(resp)[:80]); return

        children = resp.get("children", "MISSING")
        if isinstance(children, list) and len(children) == 0:
            ok("S87: list_children returned {'children': []} for root session")
        else:
            fail("S87: expected {'children': []}", str(resp)[:80])

    finally:
        stop_runner("s87")


# ═════════════════════════════════════════════════════════════════════════════
# S88 — codex export format: PortableMessages with role/text/blocks
# ═════════════════════════════════════════════════════════════════════════════

async def s88_codex_export_format(nc):
    section("S88: codex export format — PortableMessages with role/text/blocks")
    prefix = f"{BASE}.cx_s88"
    start_runner("s88", codex_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S88: session.new timed out"); return

        resp = await send_prompt(nc, prefix, sid, "export test s88")
        if prompt_err(resp):
            fail("S88: prompt failed", str(resp)[:80]); return

        exp = await nats_req(nc, f"{prefix}.agent.ext.session/export", {"sessionId": sid})
        if "_error" in exp or "code" in exp:
            fail("S88: export failed", str(exp)[:80]); return

        # Result should be a list of PortableMessages
        portable = exp
        if isinstance(portable, str):
            portable = json.loads(portable)
        if "result" in portable and isinstance(portable["result"], str):
            portable = json.loads(portable["result"])

        info(f"S88: exported {len(portable) if isinstance(portable, list) else '?'} messages")
        if not isinstance(portable, list) or len(portable) < 2:
            fail("S88: expected list with ≥2 PortableMessages", str(portable)[:80]); return
        ok(f"S88: exported {len(portable)} PortableMessages")

        # Check user message
        user_msg = portable[0]
        if user_msg.get("role") == "user":
            ok("S88: first exported message role='user'")
        else:
            fail("S88: first message should be user", str(user_msg)[:60])

        # codex uses text_only() → blocks=[] is omitted from JSON (skip_serializing_if=Vec::is_empty)
        if "text" in user_msg:
            ok("S88: user message has 'text' field")
        else:
            fail("S88: user message missing 'text'", str(user_msg)[:80])

        # Check assistant message
        asst_msg = portable[1]
        if asst_msg.get("role") == "assistant":
            ok("S88: second exported message role='assistant'")
        else:
            fail("S88: second message should be assistant", str(asst_msg)[:60])

        if "event 0" in asst_msg.get("text", ""):
            ok("S88: assistant text field contains 'event 0' (mock text)")
        else:
            fail("S88: assistant text missing 'event 0'", str(asst_msg.get("text"))[:60])

        # codex text_only() → blocks=[] omitted; if present must be type=text; absence is also valid
        asst_blocks = asst_msg.get("blocks", [])
        if not asst_blocks or asst_blocks[0].get("type") == "text":
            ok("S88: assistant blocks absent (empty, skip_serializing_if) or type='text' — correct")
        else:
            fail("S88: assistant blocks should be absent or type=text", str(asst_blocks)[:80])

    finally:
        stop_runner("s88")


# ═════════════════════════════════════════════════════════════════════════════
# S89 — codex tool event: 4 history entries: user+asst_tool_call+user_tool_result+asst_text
# ═════════════════════════════════════════════════════════════════════════════

async def s89_codex_tool_event(nc):
    section("S89: codex tool event — MOCK_SEND_TOOL_EVENT → 4 history entries")
    prefix = f"{BASE}.cx_s89"
    start_runner("s89", codex_runner_env(prefix, MOCK_SEND_TOOL_EVENT="1"))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S89: session.new timed out"); return

        resp = await send_prompt(nc, prefix, sid, "run a tool s89")
        if prompt_err(resp):
            fail("S89: prompt failed", str(resp)[:80]); return
        ok("S89: prompt completed with tool event")

        state = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid})
        history = state.get("history", [])
        info(f"S89: history length = {len(history)}")
        info(f"S89: history roles = {[m.get('role') for m in history]}")
        if len(history) == 4:
            ok("S89: 4 history entries (user + asst_tool_call + user_tool_result + asst_text)")
        else:
            fail("S89: expected 4 history entries", f"got {len(history)}")
            return

        # Entry 0: user
        if history[0].get("role") == "user":
            ok("S89: history[0] is user message")
        else:
            fail("S89: history[0] should be user", str(history[0].get("role")))

        # Entry 1: assistant with tool_call blocks
        asst_tool = history[1]
        if asst_tool.get("role") == "assistant" and asst_tool.get("text") == "[tool call]":
            ok("S89: history[1] is assistant '[tool call]'")
        else:
            fail("S89: history[1] should be assistant '[tool call]'", str(asst_tool)[:80])

        tool_blocks = asst_tool.get("blocks", [])
        tc_block = next((b for b in tool_blocks if b.get("type") == "tool_call"), None)
        if tc_block and tc_block.get("name") == "bash":
            ok("S89: tool_call block has name='bash' (from mock)")
        else:
            fail("S89: tool_call block wrong", str(tool_blocks)[:80])

        # Entry 2: user with tool_result blocks
        user_result = history[2]
        if user_result.get("role") == "user":
            ok("S89: history[2] is user (tool result message)")
        else:
            fail("S89: history[2] should be user", str(user_result.get("role")))

        result_blocks = user_result.get("blocks", [])
        tr_block = next((b for b in result_blocks if b.get("type") == "tool_result"), None)
        if tr_block and tr_block.get("tool_call_id") == "mock-tool-1":
            ok("S89: tool_result block has correct tool_call_id 'mock-tool-1'")
        else:
            fail("S89: tool_result block wrong", str(result_blocks)[:80])

        # Entry 3: assistant with accumulated text
        asst_text = history[3]
        if asst_text.get("role") == "assistant" and "event 0" in asst_text.get("text", ""):
            ok("S89: history[3] is assistant text from mock 'event 0'")
        else:
            fail("S89: history[3] should be assistant with 'event 0'", str(asst_text)[:80])

    finally:
        stop_runner("s89")


# ═════════════════════════════════════════════════════════════════════════════
# S90 — codex 3 text events: MOCK_SEND_N_TEXT_EVENTS=3 → text accumulated
# ═════════════════════════════════════════════════════════════════════════════

async def s90_codex_multiple_text_events(nc):
    section("S90: codex 3 text events — MOCK_SEND_N_TEXT_EVENTS=3 → text accumulated")
    prefix = f"{BASE}.cx_s90"
    start_runner("s90", codex_runner_env(prefix, MOCK_SEND_N_TEXT_EVENTS="3"))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S90: session.new timed out"); return

        resp = await send_prompt(nc, prefix, sid, "multiple events s90")
        if prompt_err(resp):
            fail("S90: prompt failed", str(resp)[:80]); return
        ok("S90: prompt completed")

        state = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid})
        history = state.get("history", [])
        if len(history) < 2:
            fail("S90: expected at least 2 history entries", f"got {len(history)}"); return

        asst_text = history[1].get("text", "")
        info(f"S90: assistant text = {asst_text!r}")
        # Mock emits "event 0", "event 1", "event 2" as separate TextDelta chunks
        # accumulated: "event 0event 1event 2"
        expected = "event 0event 1event 2"
        if asst_text == expected:
            ok(f"S90: text accumulated correctly: {expected!r}")
        else:
            fail("S90: unexpected accumulated text", f"got {asst_text!r}")

    finally:
        stop_runner("s90")


# ═════════════════════════════════════════════════════════════════════════════
# S91 — codex import + next prompt: pending_history used; history grows correctly
# ═════════════════════════════════════════════════════════════════════════════

async def s91_codex_import_pending_history(nc):
    section("S91: codex import + next prompt — imported history + new entries = 4 total")
    prefix = f"{BASE}.cx_s91"
    start_runner("s91", codex_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S91: session.new timed out"); return

        # Import 2 portable messages (user + assistant) into the session
        portable_messages = [
            {"role": "user", "text": "imported user s91",
             "blocks": [{"type": "text", "text": "imported user s91"}]},
            {"role": "assistant", "text": "imported assistant s91",
             "blocks": [{"type": "text", "text": "imported assistant s91"}]},
        ]
        import_resp = await nats_req(
            nc, f"{prefix}.agent.ext.session/import",
            {"sessionId": sid, "messages": portable_messages},
        )
        if "_error" in import_resp or "code" in import_resp:
            fail("S91: import failed", str(import_resp)[:80]); return
        ok("S91: import succeeded")

        # Verify history after import (should have 2 entries from import)
        state_after_import = await nats_req(
            nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid}
        )
        history_after_import = state_after_import.get("history", [])
        if len(history_after_import) == 2:
            ok("S91: history has 2 entries immediately after import")
        else:
            fail("S91: expected 2 history entries after import",
                 f"got {len(history_after_import)}")

        # Send a prompt — the pending_history gets used and then cleared
        resp = await send_prompt(nc, prefix, sid, "new question s91")
        if prompt_err(resp):
            fail("S91: prompt failed", str(resp)[:80]); return
        ok("S91: prompt completed after import")

        # History should now have: [imported_user, imported_asst, new_user, new_asst]
        state = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid})
        history = state.get("history", [])
        info(f"S91: history length after prompt = {len(history)}")
        info(f"S91: history roles = {[m.get('role') for m in history]}")

        if len(history) == 4:
            ok("S91: history has 4 entries: imported(2) + new turn(2)")
        else:
            fail("S91: expected 4 history entries after import + prompt",
                 f"got {len(history)}")
            return

        # First 2 entries are the imported messages
        if "imported user s91" in history[0].get("text", ""):
            ok("S91: history[0] is the imported user message")
        else:
            fail("S91: history[0] should be imported user", str(history[0])[:80])

        if "imported assistant s91" in history[1].get("text", ""):
            ok("S91: history[1] is the imported assistant message")
        else:
            fail("S91: history[1] should be imported assistant", str(history[1])[:80])

        # Entry 2 is the new user prompt
        if history[2].get("role") == "user" and "new question s91" in history[2].get("text", ""):
            ok("S91: history[2] is the new user prompt")
        else:
            fail("S91: history[2] should be new user prompt", str(history[2])[:80])

        # Entry 3 is the new assistant response
        if history[3].get("role") == "assistant" and "event 0" in history[3].get("text", ""):
            ok("S91: history[3] is the new assistant response")
        else:
            fail("S91: history[3] should be new assistant", str(history[3])[:80])

    finally:
        stop_runner("s91")


# ═════════════════════════════════════════════════════════════════════════════
# S92 — codex error during turn: runner survives and remains functional
# ═════════════════════════════════════════════════════════════════════════════

async def s92_codex_error_during_turn(nc):
    section("S92: codex error during turn — MOCK_TURN_SENDS_ERROR → prompt ends; runner survives")
    prefix = f"{BASE}.cx_s92"
    start_runner("s92", codex_runner_env(prefix, MOCK_TURN_SENDS_ERROR="1"))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S92: session.new timed out"); return

        # Prompt with turn error — should not crash, should return (possibly with error)
        resp = await send_prompt(nc, prefix, sid, "this turn will error s92")
        # The prompt may succeed (EndTurn) or return an error, but must NOT crash the runner
        if "_error" in resp and "timed out" in resp.get("_error", ""):
            fail("S92: prompt timed out (runner hung on error)"); return
        ok("S92: prompt returned (no hang/timeout) despite turn error")

        # Runner must remain functional: create a new session
        # But with MOCK_TURN_SENDS_ERROR, all turns error. We just verify session creation works.
        sid2 = await new_session(nc, prefix, cwd="/tmp")
        if sid2:
            ok("S92: runner functional after turn error (new session created)")
        else:
            fail("S92: runner crashed after turn error (cannot create new session)")

    finally:
        stop_runner("s92")


# ═════════════════════════════════════════════════════════════════════════════
# S93 — codex 2 sessions isolated: different thread_ids, different histories
# ═════════════════════════════════════════════════════════════════════════════

async def s93_codex_session_isolation(nc):
    section("S93: codex 2 sessions isolated — different thread_ids, different histories")
    prefix = f"{BASE}.cx_s93"
    start_runner("s93", codex_runner_env(prefix))
    try:
        sid_a = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid_a:
            fail("S93: session A creation timed out"); return
        sid_b = await new_session(nc, prefix, cwd="/tmp")
        if not sid_b:
            fail("S93: session B creation failed"); return

        info(f"S93: A={sid_a[:8]}  B={sid_b[:8]}")

        # Get thread_ids — they should be different
        state_a = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid_a})
        state_b = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid_b})

        tid_a = state_a.get("thread_id", "")
        tid_b = state_b.get("thread_id", "")
        info(f"S93: thread_id A={tid_a}, B={tid_b}")

        if tid_a and tid_b and tid_a != tid_b:
            ok(f"S93: sessions have different thread_ids ({tid_a} vs {tid_b})")
        else:
            fail("S93: sessions should have different thread_ids",
                 f"A={tid_a}, B={tid_b}")

        # Send one prompt to each session
        ra = await send_prompt(nc, prefix, sid_a, "session A question s93")
        rb = await send_prompt(nc, prefix, sid_b, "session B question s93")

        if prompt_err(ra):
            fail("S93: session A prompt failed", str(ra)[:60]); return
        if prompt_err(rb):
            fail("S93: session B prompt failed", str(rb)[:60]); return
        ok("S93: both prompts completed")

        # Each session should have 2 entries in its own history
        state_a = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid_a})
        state_b = await nats_req(nc, f"{prefix}.agent.ext.session/get_state", {"sessionId": sid_b})

        hist_a = state_a.get("history", [])
        hist_b = state_b.get("history", [])
        info(f"S93: history A length={len(hist_a)}, history B length={len(hist_b)}")

        if len(hist_a) == 2 and len(hist_b) == 2:
            ok("S93: each session has 2 history entries (isolated)")
        else:
            fail("S93: expected 2 entries per session",
                 f"A={len(hist_a)}, B={len(hist_b)}")

        # Verify history A does NOT contain session B's prompt
        hist_a_str = json.dumps(hist_a)
        hist_b_str = json.dumps(hist_b)
        if "session A question s93" in hist_a_str and "session B question s93" not in hist_a_str:
            ok("S93: session A history contains only A's prompt (isolated)")
        else:
            fail("S93: session A history contaminated", hist_a_str[:120])

        if "session B question s93" in hist_b_str and "session A question s93" not in hist_b_str:
            ok("S93: session B history contains only B's prompt (isolated)")
        else:
            fail("S93: session B history contaminated", hist_b_str[:120])

    finally:
        stop_runner("s93")


# ═════════════════════════════════════════════════════════════════════════════
# S94 — codex unknown ext_method: -32601 error; runner functional afterwards
# ═════════════════════════════════════════════════════════════════════════════

async def s94_codex_unknown_ext_method(nc):
    section("S94: codex unknown ext_method — -32601 error; runner stays functional")
    prefix = f"{BASE}.cx_s94"
    start_runner("s94", codex_runner_env(prefix))
    try:
        sid = await wait_runner(nc, prefix, cwd="/tmp")
        if not sid:
            fail("S94: session.new timed out"); return

        resp = await nats_req(nc, f"{prefix}.agent.ext.session/nonexistent_method_s94",
                              {"sessionId": sid})
        info(f"S94: unknown ext_method response = {str(resp)[:120]!r}")

        if "_error" in resp or "code" in resp:
            code = resp.get("code", resp.get("_error", ""))
            msg = resp.get("message", str(resp))
            if resp.get("code") == -32601 or "unknown ext method" in msg.lower():
                ok(f"S94: correct error returned for unknown ext_method (code={code})")
            else:
                fail("S94: unexpected error type", f"code={code}, msg={msg[:80]}")
        else:
            fail("S94: expected error for unknown ext_method", str(resp)[:80])

        # Runner should remain functional after unknown method call
        resp2 = await send_prompt(nc, prefix, sid, "alive check s94")
        if prompt_err(resp2) and "timed out" in str(resp2.get("_error", "")):
            fail("S94: runner timed out after unknown ext_method")
        else:
            ok("S94: runner functional after unknown ext_method (prompt succeeded)")

    finally:
        stop_runner("s94")


# ═════════════════════════════════════════════════════════════════════════════
# Main
# ═════════════════════════════════════════════════════════════════════════════

async def main():
    print("=== smoke_test_programming9.py — deep codex-runner behavioral verification ===")
    print(f"  prefix : {BASE}")
    print(f"  nats   : {NATS_URL}")
    print(f"  mock   : {BIN_MOCK}")

    if not os.path.exists(BIN_CODEX):
        print(f"{red}ERROR: {BIN_CODEX} not found — run 'cargo build' first{reset}")
        sys.exit(1)
    if not os.path.exists(BIN_MOCK):
        print(f"{red}ERROR: {BIN_MOCK} not found — run 'cargo build' first{reset}")
        sys.exit(1)

    nc = await nats_lib.connect(NATS_URL)
    try:
        await s83_codex_get_state_fresh(nc)
        await s84_codex_basic_prompt(nc)
        await s85_codex_multi_turn_history(nc)
        await s86_codex_set_model(nc)
        await s87_codex_list_children(nc)
        await s88_codex_export_format(nc)
        await s89_codex_tool_event(nc)
        await s90_codex_multiple_text_events(nc)
        await s91_codex_import_pending_history(nc)
        await s92_codex_error_during_turn(nc)
        await s93_codex_session_isolation(nc)
        await s94_codex_unknown_ext_method(nc)
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
