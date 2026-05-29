#!/usr/bin/env python3
"""
PR 7 live smoke test: session/export and session/import via running xai-runner binary.

Protocol notes:
  - ExtRequest is #[serde(transparent)] over params — body is just the raw params JSON
  - ExtRequest.method is #[serde(skip)] — method is encoded in the NATS subject
  - Subject format: {PREFIX}.agent.ext.{method}  (global, not session-scoped)
  - session.new is global → works via plain NATS request-reply
  - session.load is JetStream → requires X-Req-Id header + response subject subscription
    (not tested here; would require full JetStream client implementation)
"""
import asyncio
import json
import os
import subprocess
import sys
import time

NATS_URL = os.environ.get("NATS_URL", "nats://localhost:4222")
BINARY = os.environ.get("BINARY", "./target/debug/trogon-xai-runner")
PREFIX = f"smoke-ext-{os.getpid()}"
KV_BUCKET = f"SMOKE_EXT_{os.getpid()}"

PASS = 0
FAIL = 0
runner_proc = None


def ok(msg):
    global PASS
    PASS += 1
    print(f"  \033[32mPASS\033[0m {msg}")


def fail(msg):
    global FAIL
    FAIL += 1
    print(f"  \033[31mFAIL\033[0m {msg}")


def info(msg):
    print(f"  \033[33mINFO\033[0m {msg}")


def start_runner():
    global runner_proc
    env = {
        **os.environ,
        "ACP_PREFIX": PREFIX,
        "XAI_SESSION_BUCKET": KV_BUCKET,
        "XAI_API_KEY": "dummy",
        "NATS_URL": NATS_URL,
        "RUST_LOG": "warn",
    }
    runner_proc = subprocess.Popen([BINARY], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(0.5)


def stop_runner():
    global runner_proc
    if runner_proc:
        runner_proc.terminate()
        runner_proc.wait(timeout=3)
        runner_proc = None


async def nats_req(nc, subject, params, timeout=5.0):
    """Send request. params is the body dict (for ext_method: just the params JSON)."""
    msg = await nc.request(subject, json.dumps(params).encode(), timeout=timeout)
    return json.loads(msg.data)


async def main():
    import nats

    print()
    print("=== PR 7 ext_method live smoke test (xai-runner) ===")
    print(f"binary : {BINARY}")
    print(f"prefix : {PREFIX}")
    print(f"nats   : {NATS_URL}")
    print()

    start_runner()
    info(f"runner PID={runner_proc.pid}")

    nc = await nats.connect(NATS_URL)
    try:
        export_subject = f"{PREFIX}.agent.ext.session/export"
        import_subject = f"{PREFIX}.agent.ext.session/import"

        # ── 1. Create session ─────────────────────────────────────────────
        print("[1] session.new")
        resp = await nats_req(nc, f"{PREFIX}.agent.session.new", {"cwd": "/tmp", "mcpServers": []})
        session_id = resp.get("sessionId")
        if session_id:
            ok(f"session created: {session_id}")
        else:
            fail(f"session.new failed: {resp}")
            return

        # ── 2. Export empty session ────────────────────────────────────────
        print()
        print("[2] session/export (empty session)")
        resp = await nats_req(nc, export_subject, {"sessionId": session_id})
        messages = resp if isinstance(resp, list) else None
        if messages == []:
            ok("export of empty session returns []")
        else:
            fail(f"expected empty list, got: {resp}")

        # ── 3. Import messages ─────────────────────────────────────────────
        print()
        print("[3] session/import (4 messages)")
        import_messages = [
            {"role": "user", "text": "What is 2+2?"},
            {"role": "assistant", "text": "The answer is 4."},
            {"role": "user", "text": "And 3+3?"},
            {"role": "assistant", "text": "The answer is 6."},
        ]
        resp = await nats_req(nc, import_subject, {"sessionId": session_id, "messages": import_messages})
        if isinstance(resp, dict) and "code" not in resp:
            ok("import succeeded")
        else:
            fail(f"import failed: {resp}")

        # ── 4. Export and verify exact messages ────────────────────────────
        print()
        print("[4] session/export (after import) — exact match")
        resp = await nats_req(nc, export_subject, {"sessionId": session_id})
        messages = resp if isinstance(resp, list) else None
        if messages == import_messages:
            ok("export after import returns exact 4 messages (roles and text match)")
        else:
            fail(f"mismatch — got: {json.dumps(resp)}")

        # ── 5. Unknown session returns error ───────────────────────────────
        print()
        print("[5] export unknown session → JSON-RPC error")
        resp = await nats_req(nc, export_subject, {"sessionId": "no-such-session-999"})
        if isinstance(resp, dict) and "code" in resp:
            ok("unknown session returns JSON-RPC error (code)")
        else:
            fail(f"expected error for unknown session, got: {resp}")

        # ── 6. Import unknown session → error ─────────────────────────────
        print()
        print("[6] import into unknown session → JSON-RPC error")
        resp = await nats_req(nc, import_subject,
                              {"sessionId": "no-such-session-999",
                               "messages": [{"role": "user", "text": "hi"}]})
        if isinstance(resp, dict) and "code" in resp:
            ok("import into unknown session returns JSON-RPC error")
        else:
            fail(f"expected error, got: {resp}")

        # ── 7. Import replaces existing history ────────────────────────────
        print()
        print("[7] import replaces existing history (idempotent)")
        new_messages = [
            {"role": "user", "text": "New conversation start"},
            {"role": "assistant", "text": "Hello from new history"},
        ]
        resp = await nats_req(nc, import_subject, {"sessionId": session_id, "messages": new_messages})
        if isinstance(resp, dict) and "code" not in resp:
            resp2 = await nats_req(nc, export_subject, {"sessionId": session_id})
            if resp2 == new_messages:
                ok("second import replaces history correctly (2 messages)")
            else:
                fail(f"expected {new_messages}, got {resp2}")
        else:
            fail(f"second import failed: {resp}")

        # ── 8. Large import round-trip ─────────────────────────────────────
        print()
        print("[8] import 10 messages → export all 10")
        big_history = [{"role": "user" if i % 2 == 0 else "assistant", "text": f"turn {i}"}
                       for i in range(10)]
        resp = await nats_req(nc, import_subject, {"sessionId": session_id, "messages": big_history})
        if isinstance(resp, dict) and "code" not in resp:
            resp2 = await nats_req(nc, export_subject, {"sessionId": session_id})
            if resp2 == big_history:
                ok("10-message import/export round-trip correct")
            else:
                fail(f"expected 10 messages, got {resp2}")
        else:
            fail(f"big import failed: {resp}")

        # ── 9. Empty import (clear history) ───────────────────────────────
        print()
        print("[9] import [] → export returns []")
        resp = await nats_req(nc, import_subject, {"sessionId": session_id, "messages": []})
        if isinstance(resp, dict) and "code" not in resp:
            resp2 = await nats_req(nc, export_subject, {"sessionId": session_id})
            if resp2 == []:
                ok("empty import clears history")
            else:
                fail(f"expected [], got {resp2}")
        else:
            fail(f"empty import failed: {resp}")

        # ── 10. Multiple sessions independent ─────────────────────────────
        print()
        print("[10] two sessions have independent history")
        resp2 = await nats_req(nc, f"{PREFIX}.agent.session.new", {"cwd": "/tmp", "mcpServers": []})
        session_id2 = resp2.get("sessionId")
        if session_id2:
            msgs_s1 = [{"role": "user", "text": "session 1"}]
            msgs_s2 = [{"role": "user", "text": "session 2"}]
            await nats_req(nc, import_subject, {"sessionId": session_id, "messages": msgs_s1})
            await nats_req(nc, import_subject, {"sessionId": session_id2, "messages": msgs_s2})
            e1 = await nats_req(nc, export_subject, {"sessionId": session_id})
            e2 = await nats_req(nc, export_subject, {"sessionId": session_id2})
            if e1 == msgs_s1 and e2 == msgs_s2:
                ok("two sessions maintain independent history")
            else:
                fail(f"session isolation failed: s1={e1} s2={e2}")
        else:
            fail(f"second session.new failed: {resp2}")

    finally:
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
        print(f"\n\033[32mAll ext_method smoke tests passed.\033[0m")


if __name__ == "__main__":
    asyncio.run(main())
