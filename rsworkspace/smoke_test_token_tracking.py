#!/usr/bin/env python3
"""
Token tracking smoke test — runs against the live xai-runner binary.

Covers what can be proven without an API key:
  1. Zero-token session: token fields absent from KV JSON (skip_serializing_if)
  2. Session list: zero tokens → no _meta token fields
  3. list_sessions exposes totalInputTokens / totalOutputTokens /
     totalCacheReadTokens from a KV snapshot with real token values
  4. Fork doesn't inherit parent's token totals (KV entry has no token fields)
  5. Fork's list_sessions entry has no token _meta fields

Requires:
  - NATS server with JetStream at NATS_URL (default nats://localhost:14222)
  - trogon-xai-runner binary at BINARY
"""
import asyncio
import json
import os
import subprocess
import sys
import time
import uuid

import nats
from nats.js import JetStreamContext

NATS_URL = os.environ.get("NATS_URL", "nats://localhost:14222")
BINARY   = os.environ.get(
    "BINARY",
    "./target/debug/trogon-xai-runner",
)

PREFIX = f"smoke-tok-{os.getpid()}"
BUCKET = "SESSIONS"

PASS = 0
FAIL = 0
_runner_proc = None


# ── helpers ───────────────────────────────────────────────────────────────────

def ok(msg):
    global PASS
    PASS += 1
    print(f"  \033[32mPASS\033[0m  {msg}")


def fail(msg):
    global FAIL
    FAIL += 1
    print(f"  \033[31mFAIL\033[0m  {msg}")


def info(msg):
    print(f"  \033[33mINFO\033[0m  {msg}")


def start_runner():
    global _runner_proc
    env = {
        **os.environ,
        "NATS_URL":   NATS_URL,
        "ACP_PREFIX": PREFIX,
        "XAI_API_KEY": "",
        "RUST_LOG":   "warn",
    }
    _runner_proc = subprocess.Popen(
        [BINARY], env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    time.sleep(0.7)
    info(f"runner started  PID={_runner_proc.pid}")


def stop_runner():
    global _runner_proc
    if _runner_proc:
        _runner_proc.terminate()
        try:
            _runner_proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            _runner_proc.kill()
        _runner_proc = None
        info("runner stopped")


async def req(nc, subject, payload, timeout=5):
    """Plain NATS request-reply (for global methods: session.new, session.list)."""
    msg = await nc.request(subject, json.dumps(payload).encode(), timeout=timeout)
    return json.loads(msg.data)


async def js_session_req(nc, session_id, method, payload, timeout=5):
    """
    Send a session-scoped JetStream command and wait for the runner's response.

    Session commands (load, fork, close, prompt, …) are captured by the
    COMMANDS JetStream stream with no_ack=false.  nc.request() would receive
    the JetStream PubAck, not the runner's response.  The runner sends its
    actual reply to {PREFIX}.session.{session_id}.agent.response.{req_id},
    keyed by the X-Req-Id header we provide.
    """
    req_id = str(uuid.uuid4())
    response_subject = f"{PREFIX}.session.{session_id}.agent.response.{req_id}"

    sub = await nc.subscribe(response_subject)
    try:
        await nc.publish(
            f"{PREFIX}.session.{session_id}.agent.{method}",
            json.dumps(payload).encode(),
            headers={"X-Req-Id": req_id},
        )
        msg = await sub.next_msg(timeout=timeout)
        return json.loads(msg.data)
    finally:
        await sub.unsubscribe()


# ── KV helpers ────────────────────────────────────────────────────────────────

async def kv_get(js: JetStreamContext, key: str):
    """Return parsed JSON from SESSIONS KV, or None if missing."""
    try:
        kv = await js.key_value(BUCKET)
        entry = await kv.get(key)
        return json.loads(entry.value)
    except Exception:
        return None


async def kv_put(js: JetStreamContext, key: str, value: dict):
    """Write a JSON value to SESSIONS KV, creating the bucket if needed."""
    try:
        kv = await js.key_value(BUCKET)
    except Exception:
        kv = await js.create_key_value(nats.js.api.KeyValueConfig(bucket=BUCKET))
    await kv.put(key, json.dumps(value).encode())


# ── main ──────────────────────────────────────────────────────────────────────

async def main():
    os.chdir(os.path.dirname(os.path.abspath(__file__)))

    print()
    print("=== token tracking smoke test (live binary) ===")
    print(f"binary : {BINARY}")
    print(f"prefix : {PREFIX}")
    print(f"nats   : {NATS_URL}")
    print()

    # ── Phase 1: start runner and create a session ────────────────────────────
    print("── Phase 1: create session, verify zero tokens absent from KV ──")
    start_runner()

    nc = await nats.connect(NATS_URL)
    js = nc.jetstream()

    # session.new (global method — plain NATS request-reply)
    resp = await req(nc, f"{PREFIX}.agent.session.new", {"cwd": "/tmp", "mcpServers": []})
    session_id = resp.get("sessionId")
    if session_id:
        ok(f"session.new → {session_id}")
    else:
        fail(f"session.new returned no sessionId: {resp}")
        await nc.close()
        stop_runner()
        return

    # ── Test 1: zero tokens absent from KV JSON ───────────────────────────────
    print()
    print("[1] zero tokens absent from KV JSON")
    # close writes the final snapshot to KV; use JetStream protocol for session commands
    await js_session_req(nc, session_id, "close", {"sessionId": session_id})
    kv_key = f"default.{session_id}"
    snapshot = await kv_get(js, kv_key)
    if snapshot is None:
        fail("KV entry not found after session.close")
    else:
        has_input  = "total_input_tokens"  in snapshot
        has_output = "total_output_tokens" in snapshot
        has_cache  = "total_cache_read_tokens" in snapshot
        if not has_input and not has_output and not has_cache:
            ok("zero-token fields absent from KV JSON (skip_serializing_if works)")
        else:
            fail(
                f"zero-token fields present in KV — "
                f"input={has_input} output={has_output} cache={has_cache}"
            )

    # ── Test 2: list_sessions — no token _meta when tokens are zero ───────────
    print()
    print("[2] list_sessions — no token _meta for zero-token session")
    # Re-create session (the one we closed is gone from memory)
    resp = await req(nc, f"{PREFIX}.agent.session.new", {"cwd": "/tmp", "mcpServers": []})
    session_id2 = resp.get("sessionId")
    if not session_id2:
        fail(f"second session.new failed: {resp}")
        await nc.close()
        stop_runner()
        return
    ok(f"second session.new → {session_id2}")

    list_resp = await req(nc, f"{PREFIX}.agent.session.list", {})
    found = next((s for s in list_resp.get("sessions", []) if s["sessionId"] == session_id2), None)
    if found is None:
        fail(f"session {session_id2} not in list")
    else:
        meta = found.get("_meta") or found.get("meta") or {}
        has_tok = any(k in meta for k in ("totalInputTokens", "totalOutputTokens", "totalCacheReadTokens"))
        if not has_tok:
            ok("list_sessions _meta has no token fields when tokens are zero")
        else:
            fail(f"list_sessions _meta unexpectedly has token fields: {meta}")

    # ── Phase 2: inject tokens into KV, restart, verify list_sessions ─────────
    print()
    print("── Phase 2: inject tokens into KV, restart, verify list_sessions ──")
    stop_runner()

    # Write KV snapshot with token data for session_id2
    kv_key2 = f"default.{session_id2}"
    existing = await kv_get(js, kv_key2)
    if existing is None:
        # Build minimal snapshot if not yet persisted
        existing = {
            "id":         session_id2,
            "tenant_id":  "default",
            "name":       "new conversation",
            "messages":   [],
            "created_at": "2026-01-01T00:00:00.000Z",
            "updated_at": "2026-01-01T00:00:00.000Z",
        }
    # Inject token data
    existing["total_input_tokens"]      = 100
    existing["total_output_tokens"]     = 40
    existing["total_cache_read_tokens"] = 25
    await kv_put(js, kv_key2, existing)
    info(f"injected tokens into KV for {session_id2}: input=100 output=40 cache_read=25")

    # Restart runner — it will load from KV on demand
    start_runner()

    # ── Test 3: list_sessions exposes token _meta from KV snapshot ────────────
    print()
    print("[3] list_sessions exposes token _meta from loaded KV snapshot")
    # load_session triggers KV read into memory; use JetStream protocol
    load_resp = await js_session_req(
        nc,
        session_id2,
        "load",
        {"sessionId": session_id2, "cwd": "/tmp", "mcpServers": []},
    )
    info(f"load response: {load_resp}")

    list_resp = await req(nc, f"{PREFIX}.agent.session.list", {})
    found = next((s for s in list_resp.get("sessions", []) if s["sessionId"] == session_id2), None)
    if found is None:
        fail(f"session {session_id2} not in list after load")
    else:
        meta = found.get("_meta") or {}
        got_input  = meta.get("totalInputTokens")
        got_output = meta.get("totalOutputTokens")
        got_cache  = meta.get("totalCacheReadTokens")

        if got_input == 100:
            ok("list_sessions _meta.totalInputTokens = 100")
        else:
            fail(f"expected totalInputTokens=100, got {got_input}  (full meta: {meta})")

        if got_output == 40:
            ok("list_sessions _meta.totalOutputTokens = 40")
        else:
            fail(f"expected totalOutputTokens=40, got {got_output}")

        if got_cache == 25:
            ok("list_sessions _meta.totalCacheReadTokens = 25")
        else:
            fail(f"expected totalCacheReadTokens=25, got {got_cache}")

    # ── Phase 3: fork session → tokens must not be inherited ──────────────────
    print()
    print("── Phase 3: fork session — tokens must not be inherited ──")

    fork_resp = await js_session_req(
        nc,
        session_id2,
        "fork",
        {"sessionId": session_id2, "cwd": "/tmp"},
    )
    fork_id = fork_resp.get("sessionId")
    if fork_id and fork_id != session_id2:
        ok(f"session.fork → {fork_id}")
    else:
        fail(f"fork returned unexpected response: {fork_resp}")
        await nc.close()
        stop_runner()
        return

    # ── Test 4: fork KV entry has no token fields ─────────────────────────────
    print()
    print("[4] fork KV entry has no token fields")
    fork_kv_key = f"default.{fork_id}"
    fork_snapshot = await kv_get(js, fork_kv_key)
    if fork_snapshot is None:
        fail("fork KV entry not found")
    else:
        has_input  = "total_input_tokens"  in fork_snapshot
        has_output = "total_output_tokens" in fork_snapshot
        has_cache  = "total_cache_read_tokens" in fork_snapshot
        if not has_input and not has_output and not has_cache:
            ok("fork KV entry has no token fields (fork does not inherit parent tokens)")
        else:
            fail(
                f"fork KV entry has token fields — "
                f"input={fork_snapshot.get('total_input_tokens')} "
                f"output={fork_snapshot.get('total_output_tokens')} "
                f"cache={fork_snapshot.get('total_cache_read_tokens')}"
            )

    # ── Test 5: list_sessions — fork has no token _meta ───────────────────────
    print()
    print("[5] list_sessions — fork has no token _meta fields")
    list_resp = await req(nc, f"{PREFIX}.agent.session.list", {})
    fork_info = next((s for s in list_resp.get("sessions", []) if s["sessionId"] == fork_id), None)
    if fork_info is None:
        fail(f"fork {fork_id} not found in session list")
    else:
        meta = fork_info.get("_meta") or {}
        has_tok = any(k in meta for k in ("totalInputTokens", "totalOutputTokens", "totalCacheReadTokens"))
        if not has_tok:
            ok("list_sessions: fork has no token _meta fields")
        else:
            fail(f"fork's list_sessions _meta unexpectedly has token fields: {meta}")

    # ── Done ──────────────────────────────────────────────────────────────────
    await nc.close()
    stop_runner()

    print()
    print("=== Results ===")
    print(f"  \033[32mpassed\033[0m: {PASS}")
    if FAIL > 0:
        print(f"  \033[31mfailed\033[0m: {FAIL}")
        sys.exit(1)
    else:
        print(f"  \033[31mfailed\033[0m: {FAIL}")
        print(f"\n\033[32mAll {PASS} token tracking smoke tests passed.\033[0m")


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        stop_runner()
