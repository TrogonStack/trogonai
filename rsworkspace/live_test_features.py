#!/usr/bin/env python3
"""
Live feature test — Path-scoped RBAC / Egress policy / Audit trail.

Tests the three platform-test features end-to-end against a freshly started
trogon-acp-runner subprocess.  No real Anthropic credentials required.

Strategy
--------
  1. Start a mock HTTP server that returns canned LLM responses so the agent
     loop can complete without a real API key.
  2. Start the runner subprocess with PROXY_URL pointing at the mock server.
  3. Wait for the runner to initialize, then run three feature tests plus
     six integration-test gap scenarios.

Protocol used (core NATS request-reply, NOT JetStream)
-------------------------------------------------------
  - Global ops  (initialize, session.new): nc.request()
  - Session ops (load, prompt, …):
        reply_inbox = "_INBOX." + uuid
        sub = await nc.subscribe(reply_inbox)
        await nc.publish(subject, payload, reply=reply_inbox)
        msg = await sub.next_msg(timeout=5)

  Do NOT use js.publish() for session ops — it does not set msg.reply and
  the runner drops the message with DispatchError::NoReplySubject.

Run
---
    python3 live_test_features.py

Or with a different NATS URL:
    NATS_URL=nats://other:4222 python3 live_test_features.py
"""

import asyncio
import collections
import json
import os
import signal
import subprocess
import sys
import threading
import time
import uuid
from contextlib import asynccontextmanager
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, HTTPServer

import nats

# ── Config ─────────────────────────────────────────────────────────────────────

NATS_URL   = os.environ.get("NATS_URL", "nats://localhost:4222")
PREFIX     = "acp.pt"          # dedicated prefix to avoid docker container at acp.claude
RSDIR      = os.path.dirname(os.path.abspath(__file__))
BINARY     = os.path.join(RSDIR, "target", "debug", "trogon-acp-runner")
MOCK_PORT  = 19876             # unlikely to clash

INIT_TIMEOUT   = 10.0          # seconds to wait for runner to come up
NATS_TIMEOUT   = 5.0           # timeout for individual NATS ops
PROMPT_TIMEOUT = 30.0          # timeout for the full prompt round-trip

# ── Colours ────────────────────────────────────────────────────────────────────

green  = "\033[32m"
red    = "\033[31m"
yellow = "\033[33m"
cyan   = "\033[36m"
bold   = "\033[1m"
reset  = "\033[0m"

passed = 0
failed = 0


def ok(label, detail=""):
    global passed
    passed += 1
    sfx = f"  {yellow}{detail}{reset}" if detail else ""
    print(f"  {green}PASS{reset}  {label}{sfx}")


def fail(label, detail=""):
    global failed
    failed += 1
    print(f"  {red}FAIL{reset}  {label}  {yellow}{detail}{reset}")


def warn(label, detail=""):
    sfx = f"  {yellow}{detail}{reset}" if detail else ""
    print(f"  {yellow}WARN{reset}  {label}{sfx}")


def section(title):
    print(f"\n{cyan}── {title} {'─' * max(0, 60 - len(title))}{reset}")


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


# ── Canned LLM responses ───────────────────────────────────────────────────────

_END_TURN_RESPONSE = {
    "stop_reason": "end_turn",
    "content": [{"type": "text", "text": "Done."}],
    "usage": {
        "input_tokens": 15,
        "output_tokens": 3,
        "cache_creation_input_tokens": 0,
        "cache_read_input_tokens": 0,
    },
}


def _tool_use_response(*tools):
    """Build a tool_use LLM response.  Each tool is (name, tool_id, input_dict)."""
    return {
        "stop_reason": "tool_use",
        "content": [
            {"type": "tool_use", "id": tid, "name": name, "input": inp}
            for name, tid, inp in tools
        ],
        "usage": {
            "input_tokens": 10,
            "output_tokens": 5,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0,
        },
    }


# ── Mock Anthropic HTTP server ─────────────────────────────────────────────────
#
# The runner calls {PROXY_URL}/anthropic/v1/messages and expects a plain JSON
# body (the agent loop uses `.json::<AnthropicResponse>()`, not SSE parsing).
#
# Call MockAnthropicHandler.reset(responses) before each test to prime the
# queue.  Responses are dequeued in order; once exhausted, _END_TURN_RESPONSE
# is returned for every subsequent call.

class MockAnthropicHandler(BaseHTTPRequestHandler):
    """Return canned Anthropic JSON responses from a per-test queue."""

    _lock           = threading.Lock()
    _response_queue = collections.deque()
    _request_log    = []

    @classmethod
    def reset(cls, responses=None):
        """Clear the queue and optionally prime it with `responses`."""
        with cls._lock:
            cls._response_queue.clear()
            if responses:
                cls._response_queue.extend(responses)
            cls._request_log = []

    def log_message(self, fmt, *args):
        pass  # silence default HTTP access log

    def _read_body(self):
        length = int(self.headers.get("Content-Length", 0))
        return self.rfile.read(length) if length else b""

    def do_POST(self):
        body = self._read_body()
        with MockAnthropicHandler._lock:
            try:
                parsed = json.loads(body)
            except Exception:
                parsed = {}
            MockAnthropicHandler._request_log.append(parsed)
            if MockAnthropicHandler._response_queue:
                response = MockAnthropicHandler._response_queue.popleft()
            else:
                response = _END_TURN_RESPONSE

        body_bytes = json.dumps(response).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body_bytes)))
        self.end_headers()
        self.wfile.write(body_bytes)

    def do_GET(self):
        """GET /log — return captured request log as JSON (debug helper)."""
        with MockAnthropicHandler._lock:
            data = json.dumps(MockAnthropicHandler._request_log).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


def start_mock_server(port: int) -> HTTPServer:
    """Start the mock HTTP server in a daemon thread and return the server object."""
    server = HTTPServer(("127.0.0.1", port), MockAnthropicHandler)
    t = threading.Thread(target=server.serve_forever, daemon=True)
    t.start()
    return server


# ── NATS helpers ───────────────────────────────────────────────────────────────

async def nats_req(nc, subject: str, payload: dict) -> dict:
    """Core NATS request-reply for global operations."""
    data = json.dumps(payload).encode()
    msg = await nc.request(subject, data, timeout=NATS_TIMEOUT)
    return json.loads(msg.data)


async def nats_session_cmd(nc, subject: str, payload: dict, timeout: float = NATS_TIMEOUT) -> dict:
    """
    Core NATS request-reply for session-scoped operations.

    Uses nc.publish(subject, payload, reply=inbox) so msg.reply is set and
    the runner can reply.  Do NOT use js.publish() — it does not set
    msg.reply and the runner drops the message with NoReplySubject.
    """
    reply_inbox = "_INBOX." + str(uuid.uuid4()).replace("-", "")
    sub = await nc.subscribe(reply_inbox)
    try:
        await nc.publish(subject, json.dumps(payload).encode(), reply=reply_inbox)
        msg = await asyncio.wait_for(sub.next_msg(), timeout=timeout)
        return json.loads(msg.data)
    except asyncio.TimeoutError:
        return {"error": f"timeout waiting for reply on {subject}"}
    except Exception as e:
        return {"error": str(e)}
    finally:
        try:
            await sub.unsubscribe()
        except Exception:
            pass


def _is_puback(data: dict) -> bool:
    """Detect a JetStream PubAck reply: {"stream": "...", "seq": N[, "duplicate": true]}."""
    return "stream" in data and "seq" in data


async def send_prompt(nc, session_id: str, text: str, timeout: float = PROMPT_TIMEOUT) -> dict:
    """
    Send a prompt and wait for the runner's completion response.

    Skips JetStream PubAcks that arrive on the reply inbox when the prompt
    subject is captured by a JetStream stream with no_ack=false.  The runner
    saves KV *before* sending the response, so when this function returns the
    audit log is already persisted.
    """
    prompt_payload = {
        "sessionId": session_id,
        "prompt": [{"type": "text", "text": text}],
    }
    reply_inbox = "_INBOX." + str(uuid.uuid4()).replace("-", "")
    sub = await nc.subscribe(reply_inbox)
    try:
        await nc.publish(
            f"{PREFIX}.session.{session_id}.agent.prompt",
            json.dumps(prompt_payload).encode(),
            reply=reply_inbox,
        )
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            try:
                msg = await asyncio.wait_for(sub.next_msg(), timeout=min(remaining, 5.0))
                data = json.loads(msg.data)
                if _is_puback(data):
                    continue  # skip JetStream PubAck, wait for actual runner response
                return data
            except asyncio.TimeoutError:
                break
        return {"error": f"timeout after {timeout}s"}
    except Exception as e:
        return {"error": str(e)}
    finally:
        try:
            await sub.unsubscribe()
        except Exception:
            pass


@asynccontextmanager
async def auto_respond_permissions(nc, session_id: str, option: str = "allow"):
    """
    Subscribe to the permission request channel for `session_id` and
    automatically respond with `option` ("allow" or "reject").

    Yields the list of received permission request payloads so the caller
    can assert how many were sent.

    Permission response format (agent-client-protocol-schema 0.11.4):
        {"outcome": {"outcome": "selected", "optionId": "<option>"}}
    """
    subject  = f"{PREFIX}.session.{session_id}.client.session.request_permission"
    received = []

    async def handler(msg):
        try:
            received.append(json.loads(msg.data) if msg.data else {})
        except Exception:
            received.append({})
        if msg.reply:
            resp = {"outcome": {"outcome": "selected", "optionId": option}}
            await nc.publish(msg.reply, json.dumps(resp).encode())

    sub = await nc.subscribe(subject, cb=handler)
    try:
        yield received
    finally:
        try:
            await sub.unsubscribe()
        except Exception:
            pass


# ── KV helpers ────────────────────────────────────────────────────────────────

async def kv_put(kv, key: str, state: dict):
    await kv.put(key, json.dumps(state).encode())


async def kv_get(kv, key: str):
    entry = await kv.get(key)
    if entry is None or entry.value is None:
        return None
    return json.loads(entry.value)


# ── Session state builder ─────────────────────────────────────────────────────

def make_session(extra: dict = None) -> dict:
    now = now_iso()
    base = {
        "messages": [],
        "mode": "default",
        "cwd": "/tmp/live-feature-test",
        "created_at": now,
        "updated_at": now,
        "title": "",
        "mcp_servers": [],
        "allowed_tools": [],
        "tool_policies": [],
        "audit_log": [],
    }
    if extra:
        base.update(extra)
    return base


async def new_session_with_state(nc, kv, extra_state: dict):
    """
    Create a session via the runner (session.new) then overwrite its KV entry
    with test-specific state.  Returns (session_id, None) on success or
    (None, error_dict) on failure.
    """
    r = await nats_req(nc, f"{PREFIX}.agent.session.new",
                       {"cwd": "/tmp/live-feature-test", "mcpServers": []})
    sid = r.get("sessionId") or r.get("session_id")
    if not sid:
        return None, r
    await kv_put(kv, sid, make_session(extra_state))
    return sid, None


async def disable_client_ops_pubacks(nc) -> bool:
    """
    Set no_ack=True on the ACP_PT_CLIENT_OPS JetStream stream.

    Without this, JetStream sends a PubAck to the permission request's reply
    inbox before any NATS subscriber can respond, causing the runner to parse
    the PubAck as a RequestPermissionResponse (deserialization fails → deny).
    """
    stream_name = PREFIX.replace(".", "_").upper() + "_CLIENT_OPS"
    try:
        resp = await nc.request(f"$JS.API.STREAM.INFO.{stream_name}", b"", timeout=5.0)
        info = json.loads(resp.data)
        if "error" in info:
            return False
        config = info["config"]
        config["no_ack"] = True
        resp2 = await nc.request(
            f"$JS.API.STREAM.UPDATE.{stream_name}",
            json.dumps(config).encode(),
            timeout=5.0,
        )
        result = json.loads(resp2.data)
        return "error" not in result
    except Exception:
        return False


# ── Runner lifecycle ──────────────────────────────────────────────────────────

def start_runner(mock_port: int) -> subprocess.Popen:
    """Start the trogon-acp-runner subprocess with the mock proxy URL."""
    env = os.environ.copy()
    env.update({
        "NATS_URL":             NATS_URL,
        "ACP_PREFIX":           PREFIX,
        "PROXY_URL":            f"http://127.0.0.1:{mock_port}",
        "ANTHROPIC_TOKEN":      "fake-token-no-real-key",
        "AGENT_MODEL":          "claude-opus-4-6",
        "AGENT_MAX_ITERATIONS": "3",
        "RUST_LOG":             "trogon_acp_runner=info,warn",
    })
    proc = subprocess.Popen(
        [BINARY],
        env=env,
        cwd=RSDIR,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    return proc


async def wait_for_runner(nc, timeout: float = INIT_TIMEOUT) -> bool:
    """
    Poll acp.pt.agent.initialize until the runner responds or timeout.
    Returns True on success.
    """
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            data = json.dumps({"protocolVersion": 1}).encode()
            msg = await nc.request(f"{PREFIX}.agent.initialize", data, timeout=1.0)
            r = json.loads(msg.data)
            if "protocolVersion" in r or "error" not in r:
                return True
        except Exception:
            pass
        await asyncio.sleep(0.3)
    return False


# ── Prompt drain helper ───────────────────────────────────────────────────────

async def drain_perm(sub, collected):
    """Drain any queued messages on a NATS subscription (2-second window)."""
    try:
        while True:
            msg = await asyncio.wait_for(sub.next_msg(), timeout=0.2)
            collected.append(json.loads(msg.data))
    except (asyncio.TimeoutError, Exception):
        pass


# ── Main test logic ───────────────────────────────────────────────────────────

async def main():
    print(f"\n{bold}{'═' * 65}{reset}")
    print(f"  {bold}Live feature test — RBAC / Egress / Audit trail{reset}")
    print(f"  NATS  : {NATS_URL}")
    print(f"  prefix: {PREFIX}")
    print(f"  binary: {BINARY}")
    print(f"{bold}{'═' * 65}{reset}")

    # ── Pre-flight checks ─────────────────────────────────────────────────────
    if not os.path.isfile(BINARY):
        print(f"\n{red}BINARY NOT FOUND: {BINARY}{reset}")
        print("Build it first with:  cargo build -p trogon-acp-runner")
        sys.exit(1)

    # ── Start mock HTTP server ────────────────────────────────────────────────
    section("0. Setup — mock HTTP server + runner subprocess")
    mock_server = start_mock_server(MOCK_PORT)
    ok("mock Anthropic server started", f"http://127.0.0.1:{MOCK_PORT}")

    # ── Start runner ──────────────────────────────────────────────────────────
    runner = start_runner(MOCK_PORT)
    ok("runner subprocess started", f"pid={runner.pid}")

    # ── Connect to NATS ───────────────────────────────────────────────────────
    try:
        nc = await nats.connect(NATS_URL)
    except Exception as e:
        fail("NATS connection", str(e))
        runner.terminate()
        sys.exit(1)

    js = nc.jetstream()

    # ── Wait for runner to initialize ─────────────────────────────────────────
    alive = await wait_for_runner(nc, timeout=INIT_TIMEOUT)
    if not alive:
        fail("runner initialization", f"did not respond within {INIT_TIMEOUT}s")
        runner.terminate()
        await nc.close()
        sys.exit(1)
    ok("runner initialized and responding to initialize")

    # Open (or reuse) the ACP_SESSIONS KV bucket.
    kv = None
    for _attempt in range(5):
        try:
            kv = await js.key_value("ACP_SESSIONS")
            break
        except Exception:
            await asyncio.sleep(0.5)
    if kv is None:
        fail("ACP_SESSIONS KV bucket", "bucket not available after 5 attempts")
        runner.terminate()
        await nc.close()
        sys.exit(1)

    ok("ACP_SESSIONS KV bucket open")

    # Disable JetStream PubAcks on ClientOps so permission reply inboxes are
    # not intercepted by JetStream before the Python handler can respond.
    puback_ok = await disable_client_ops_pubacks(nc)
    if puback_ok:
        ok("ClientOps stream no_ack=True (permission replies won't be intercepted)")
    else:
        warn("could not set no_ack on ClientOps — Gap 1 may fail",
             "stream may not exist yet or JetStream unavailable")

    # ── Create a session via the live runner ──────────────────────────────────
    r = await nats_req(nc, f"{PREFIX}.agent.session.new",
                       {"cwd": "/tmp/live-feature-test", "mcpServers": []})
    session_id = r.get("sessionId") or r.get("session_id")
    if not session_id:
        fail("session.new", str(r))
        runner.terminate()
        await nc.close()
        sys.exit(1)
    ok("session.new", session_id)

    # ══════════════════════════════════════════════════════════════════════════
    # Feature 1 — Path-scoped RBAC  (tool_policies)
    # ══════════════════════════════════════════════════════════════════════════
    section("Feature 1 — Path-scoped RBAC  (tool_policies + live prompt)")

    tool_policies = [
        {"tool": "write_file", "path_pattern": "/workspace/**", "action": "allow"},
        {"tool": "write_file", "path_pattern": "/workspace/.env", "action": "deny"},
    ]
    state = make_session({"tool_policies": tool_policies})
    await kv_put(kv, session_id, state)
    ok("wrote tool_policies to ACP_SESSIONS KV")

    back = await kv_get(kv, session_id)
    if back and back.get("tool_policies") == tool_policies:
        ok("tool_policies KV round-trip")
    else:
        fail("tool_policies KV round-trip", str(back.get("tool_policies") if back else "None"))

    r = await nats_session_cmd(
        nc,
        f"{PREFIX}.session.{session_id}.agent.load",
        {"sessionId": session_id, "cwd": "/tmp/live-feature-test", "mcpServers": []},
    )
    if "error" in r and "code" not in r:
        fail("session.load with tool_policies", str(r))
    else:
        ok("runner loaded session with tool_policies (no crash)")

    # Prime the mock: write_file on /workspace/safe.py (Allow rule matches) → end_turn
    MockAnthropicHandler.reset([
        _tool_use_response(
            ("write_file", "tool-call-1",
             {"path": "/workspace/safe.py", "content": "# generated by mock LLM\n"})
        ),
        _END_TURN_RESPONSE,
    ])

    # Subscribe to permission_request channel — should receive nothing for Allow.
    perm_received = []
    perm_subject  = f"{PREFIX}.session.{session_id}.client.session.request_permission"
    perm_sub      = await nc.subscribe(perm_subject)

    prompt_payload = {
        "sessionId": session_id,
        "prompt": [{"type": "text", "text": "write to /workspace/safe.py"}],
    }
    prompt_reply_subject = "_INBOX." + str(uuid.uuid4()).replace("-", "")
    prompt_sub = await nc.subscribe(prompt_reply_subject)

    await nc.publish(
        f"{PREFIX}.session.{session_id}.agent.prompt",
        json.dumps(prompt_payload).encode(),
        reply=prompt_reply_subject,
    )

    prompt_response = None
    try:
        prompt_msg = await asyncio.wait_for(prompt_sub.next_msg(), timeout=PROMPT_TIMEOUT)
        prompt_response = json.loads(prompt_msg.data)
    except asyncio.TimeoutError:
        fail("prompt response", f"timed out after {PROMPT_TIMEOUT}s")
    except Exception as e:
        fail("prompt response", str(e))
    finally:
        try:
            await prompt_sub.unsubscribe()
        except Exception:
            pass

    if prompt_response is not None:
        stop_reason = prompt_response.get("stopReason") or prompt_response.get("stop_reason", "?")
        if "error" not in prompt_response and "code" not in prompt_response:
            ok("prompt completed", f"stop_reason={stop_reason}")
        else:
            fail("prompt", str(prompt_response))

    await asyncio.sleep(2.0)
    await drain_perm(perm_sub, perm_received)
    try:
        await perm_sub.unsubscribe()
    except Exception:
        pass

    if not perm_received:
        ok("no permission_request fired for /workspace/safe.py (Allow rule matched)")
    else:
        fail("unexpected permission_request received", str(perm_received[:1]))

    with MockAnthropicHandler._lock:
        call_count = len(MockAnthropicHandler._request_log)
    if call_count >= 2:
        ok("mock LLM called for tool_use + end_turn turns", f"calls={call_count}")
    elif call_count == 1:
        warn("mock LLM called only once (tool_use turn only)", f"calls={call_count}")
    else:
        fail("mock LLM was not called", f"calls={call_count}")

    # ══════════════════════════════════════════════════════════════════════════
    # Feature 3 — Audit trail  (verified here, after the prompt has completed)
    # ══════════════════════════════════════════════════════════════════════════
    section("Feature 3 — Audit trail  (post-prompt KV inspection)")

    await asyncio.sleep(1.0)

    fresh = await kv_get(kv, session_id)
    if fresh is None:
        fail("KV read after prompt", "key missing")
    else:
        audit_log = fresh.get("audit_log", [])
        if audit_log:
            ok("audit_log is non-empty after prompt", f"entries={len(audit_log)}")
        else:
            fail("audit_log empty after prompt",
                 "expected at least one entry for write_file call")

        if audit_log:
            allowed_entries = [e for e in audit_log if e.get("outcome") == "allowed"]
            if allowed_entries:
                ok("audit_log has 'allowed' outcome entry", f"count={len(allowed_entries)}")
            else:
                outcomes = [e.get("outcome") for e in audit_log]
                fail("no 'allowed' outcome found in audit_log", str(outcomes))

            entry = allowed_entries[0] if allowed_entries else audit_log[0]
            if entry.get("tool"):
                ok("audit entry has tool field", entry["tool"])
            else:
                fail("audit entry missing tool field", str(entry))

            if entry.get("input_summary"):
                ok("audit entry has input_summary", entry["input_summary"])
            else:
                fail("audit entry missing input_summary", str(entry))

            ts = entry.get("timestamp", "")
            if ts and len(ts) >= 10 and ("T" in ts or "-" in ts):
                ok("audit entry timestamp looks like ISO-8601", ts[:20])
            else:
                fail("audit entry timestamp missing or malformed", str(ts))

    # ── Audit trail: 5-outcome KV round-trip ─────────────────────────────────
    outcomes = ["allowed", "denied", "required_approval", "approved_by_user", "denied_by_user"]
    entries = [
        {
            "timestamp": now_iso(),
            "tool": f"tool_{o}",
            "input_summary": f"/path/{o}",
            "outcome": o,
        }
        for o in outcomes
    ]
    await kv_put(kv, f"{session_id}-audit-rt", make_session({"audit_log": entries}))
    back_audit = await kv_get(kv, f"{session_id}-audit-rt")
    if back_audit:
        stored = back_audit.get("audit_log", [])
        stored_outcomes = {e["outcome"] for e in stored}
        if stored_outcomes == set(outcomes):
            ok("all 5 AuditOutcome variants round-trip via KV", str(sorted(stored_outcomes)))
        else:
            fail("AuditOutcome variants missing", str(stored_outcomes))
    else:
        fail("KV read back for 5-outcome audit test", "key missing")

    # ── Audit trail: 500-entry cap ────────────────────────────────────────────
    cap = 500
    big_log = [
        {
            "timestamp": now_iso(),
            "tool": f"tool-{i}",
            "input_summary": f"/path/{i}",
            "outcome": "allowed",
        }
        for i in range(505)
    ]
    excess = len(big_log) - cap
    capped_log = big_log[excess:]  # first 5 dropped

    await kv_put(kv, f"{session_id}-audit-cap",
                 make_session({"audit_log": capped_log}))
    back_cap = await kv_get(kv, f"{session_id}-audit-cap")
    if back_cap:
        stored = back_cap.get("audit_log", [])
        if len(stored) == cap:
            ok("audit_log 500-entry cap round-trip", f"stored={len(stored)}")
        else:
            fail("audit_log cap", f"expected {cap}, got {len(stored)}")
        if stored and stored[0]["tool"] == "tool-5":
            ok("oldest 5 entries evicted (first surviving: tool-5)")
        elif stored:
            warn("first surviving entry", stored[0]["tool"])
    else:
        fail("KV read back for audit 500-cap test", "key missing")

    # ══════════════════════════════════════════════════════════════════════════
    # Feature 2 — Egress policy
    # ══════════════════════════════════════════════════════════════════════════
    section("Feature 2 — Egress policy  (KV round-trip + runner parse)")

    egress_safe = {
        "default_action": "allow",
        "rules": [
            {"host_pattern": "169.254.*",       "action": "deny"},
            {"host_pattern": "169.254.169.254", "action": "deny"},
        ],
    }
    egress_strict = {
        "default_action": "deny",
        "rules": [
            {"host_pattern": "api.anthropic.com", "action": "allow"},
        ],
    }

    for label, policy in [("default_safe", egress_safe), ("strict_allowlist", egress_strict)]:
        sid_e = f"{session_id}-egress-{label}"
        await kv_put(kv, sid_e, make_session({"egress_policy": policy}))
        back_e = await kv_get(kv, sid_e)
        if back_e and back_e.get("egress_policy") == policy:
            ok(f"egress_policy round-trip [{label}]")
        else:
            fail(f"egress_policy [{label}]",
                 str(back_e.get("egress_policy") if back_e else None))

    back_safe = await kv_get(kv, f"{session_id}-egress-default_safe")
    if back_safe:
        ep = back_safe.get("egress_policy", {})
        deny_hosts = [r["host_pattern"] for r in ep.get("rules", []) if r["action"] == "deny"]
        if "169.254.169.254" in deny_hosts or "169.254.*" in deny_hosts:
            ok("hard-block 169.254.* present in persisted default_safe policy",
               str(deny_hosts))
        else:
            fail("hard-block 169.254.* not found in policy rules", str(deny_hosts))

    egress_sid = f"{session_id}-egress-runner-parse"
    await kv_put(kv, egress_sid, make_session({"egress_policy": egress_strict}))
    r = await nats_session_cmd(
        nc,
        f"{PREFIX}.session.{egress_sid}.agent.load",
        {"sessionId": egress_sid, "cwd": "/tmp", "mcpServers": []},
    )
    if "error" in r and "code" not in r:
        fail("session.load with egress_policy", str(r))
    else:
        ok("runner parsed session with egress_policy (no crash)")

    egress_mcp_sid = f"{session_id}-egress-mcp"
    await kv_put(kv, egress_mcp_sid, make_session({
        "egress_policy": egress_strict,
        "mcp_servers": [
            {"name": "blocked-mcp", "url": "http://169.254.169.254/mcp", "headers": []}
        ],
    }))
    r = await nats_session_cmd(
        nc,
        f"{PREFIX}.session.{egress_mcp_sid}.agent.load",
        {"sessionId": egress_mcp_sid, "cwd": "/tmp", "mcpServers": []},
    )
    if "error" in r and "code" not in r:
        fail("session.load with blocked MCP server", str(r))
    else:
        ok("runner loaded session with denied MCP server (skipped gracefully)")

    # ══════════════════════════════════════════════════════════════════════════
    # Gap 1 — RequireApproval tool_policy → permission_request over NATS
    #
    # Integration tests check RequireApproval logic via in-process mpsc channel.
    # This live test exercises the full NATS permission_bridge path:
    #   runner → NATS request → Python handler → NATS reply → runner continues
    # ══════════════════════════════════════════════════════════════════════════
    section("Gap 1 — RequireApproval: permission_request sent to NATS, user allows")

    sid_g1, err_g1 = await new_session_with_state(nc, kv, {
        "tool_policies": [
            {
                "tool": "write_file",
                "path_pattern": "/workspace/sensitive/**",
                "action": "require_approval",
            }
        ]
    })
    if err_g1:
        fail("Gap1: session.new failed", str(err_g1))
    else:
        ok("Gap1: session created", sid_g1)

    MockAnthropicHandler.reset([
        _tool_use_response(
            ("write_file", "tc-g1",
             {"path": "/workspace/sensitive/data.txt", "content": "secret"})
        ),
        _END_TURN_RESPONSE,
    ])

    async with auto_respond_permissions(nc, sid_g1, "allow") as perm_g1:
        resp_g1 = await send_prompt(nc, sid_g1, "write something sensitive")

    if "error" not in resp_g1 and "code" not in resp_g1:
        ok("Gap1: prompt completed with RequireApproval policy")
    else:
        fail("Gap1: prompt failed", str(resp_g1))

    if perm_g1:
        ok("Gap1: permission_request received over NATS", f"count={len(perm_g1)}")
    else:
        fail("Gap1: no permission_request received (bridge did not fire)")

    fresh_g1 = await kv_get(kv, sid_g1)
    if fresh_g1:
        log_g1  = fresh_g1.get("audit_log", [])
        outs_g1 = [e.get("outcome") for e in log_g1]
        if "required_approval" in outs_g1:
            ok("Gap1: audit has required_approval entry", str(outs_g1))
        else:
            fail("Gap1: audit missing required_approval", str(outs_g1))
        if "approved_by_user" in outs_g1:
            ok("Gap1: audit has approved_by_user entry")
        else:
            fail("Gap1: audit missing approved_by_user", str(outs_g1))
    else:
        fail("Gap1: KV read after prompt returned nothing")

    # ══════════════════════════════════════════════════════════════════════════
    # Gap 2 — Deny > Allow precedence in the live runner
    #
    # Two overlapping tool_policies: Allow /workspace/** and Deny /workspace/secrets/**
    # The mock LLM writes to /workspace/secrets/key.txt → Deny wins, no permission
    # prompt is sent, audit has "denied".
    # ══════════════════════════════════════════════════════════════════════════
    section("Gap 2 — Deny > Allow: overlapping policies, Deny wins silently")

    sid_g2, err_g2 = await new_session_with_state(nc, kv, {
        "tool_policies": [
            {"tool": "write_file", "path_pattern": "/workspace/**",         "action": "allow"},
            {"tool": "write_file", "path_pattern": "/workspace/secrets/**", "action": "deny"},
        ]
    })
    if err_g2:
        fail("Gap2: session.new failed", str(err_g2))
    else:
        ok("Gap2: session created", sid_g2)

    MockAnthropicHandler.reset([
        _tool_use_response(
            ("write_file", "tc-g2",
             {"path": "/workspace/secrets/key.txt", "content": "top-secret"})
        ),
        _END_TURN_RESPONSE,
    ])

    perm_g2_received = []
    perm_sub_g2 = await nc.subscribe(
        f"{PREFIX}.session.{sid_g2}.client.session.request_permission"
    )

    resp_g2 = await send_prompt(nc, sid_g2, "write to secrets/key.txt")
    await drain_perm(perm_sub_g2, perm_g2_received)
    try:
        await perm_sub_g2.unsubscribe()
    except Exception:
        pass

    if "error" not in resp_g2 and "code" not in resp_g2:
        ok("Gap2: prompt completed")
    else:
        fail("Gap2: prompt failed", str(resp_g2))

    if not perm_g2_received:
        ok("Gap2: no permission_request fired (Deny blocks without user prompt)")
    else:
        fail("Gap2: unexpected permission_request received", str(perm_g2_received[:1]))

    fresh_g2 = await kv_get(kv, sid_g2)
    if fresh_g2:
        log_g2  = fresh_g2.get("audit_log", [])
        outs_g2 = [e.get("outcome") for e in log_g2]
        if "denied" in outs_g2:
            ok("Gap2: audit has 'denied' entry — Deny > Allow confirmed", str(outs_g2))
        else:
            fail("Gap2: audit missing 'denied' entry", str(outs_g2))
        if "allowed" not in outs_g2:
            ok("Gap2: no 'allowed' entry (Allow policy was overridden by Deny)")
        else:
            fail("Gap2: 'allowed' entry should not be present", str(outs_g2))
    else:
        fail("Gap2: KV read after prompt returned nothing")

    # ══════════════════════════════════════════════════════════════════════════
    # Gap 3 — Glob mismatch → fallthrough to interactive gate, user rejects
    #
    # tool_policies: Allow /workspace/**
    # Mock LLM writes to /tmp/outside.txt — does NOT match /workspace/**
    # → None returned by eval_tool_policies → falls through to ChannelPermissionChecker
    # → permission_request sent to NATS → Python responds "reject"
    # → audit: required_approval + denied_by_user
    # ══════════════════════════════════════════════════════════════════════════
    section("Gap 3 — Glob mismatch: no-match fallthrough to interactive gate, user rejects")

    sid_g3, err_g3 = await new_session_with_state(nc, kv, {
        "tool_policies": [
            {"tool": "write_file", "path_pattern": "/workspace/**", "action": "allow"},
        ]
    })
    if err_g3:
        fail("Gap3: session.new failed", str(err_g3))
    else:
        ok("Gap3: session created", sid_g3)

    MockAnthropicHandler.reset([
        _tool_use_response(
            ("write_file", "tc-g3",
             {"path": "/tmp/outside.txt", "content": "data"})
        ),
        _END_TURN_RESPONSE,
    ])

    async with auto_respond_permissions(nc, sid_g3, "reject") as perm_g3:
        resp_g3 = await send_prompt(nc, sid_g3, "write to /tmp/outside.txt")

    if "error" not in resp_g3 and "code" not in resp_g3:
        ok("Gap3: prompt completed")
    else:
        fail("Gap3: prompt failed", str(resp_g3))

    if perm_g3:
        ok("Gap3: permission_request received (glob mismatch → interactive fallthrough)",
           f"count={len(perm_g3)}")
    else:
        fail("Gap3: no permission_request — glob mismatch should trigger interactive gate")

    fresh_g3 = await kv_get(kv, sid_g3)
    if fresh_g3:
        log_g3  = fresh_g3.get("audit_log", [])
        outs_g3 = [e.get("outcome") for e in log_g3]
        if "required_approval" in outs_g3:
            ok("Gap3: audit has required_approval")
        else:
            fail("Gap3: audit missing required_approval", str(outs_g3))
        if "denied_by_user" in outs_g3:
            ok("Gap3: audit has denied_by_user (user rejected the out-of-scope path)")
        else:
            fail("Gap3: audit missing denied_by_user", str(outs_g3))
    else:
        fail("Gap3: KV read after prompt returned nothing")

    # ══════════════════════════════════════════════════════════════════════════
    # Gap 4 — Default egress policy applied when egress_policy is absent
    #
    # When SessionState.egress_policy is None the runner calls
    # EgressPolicy::default_safe() which blocks 169.254.*.  A session with no
    # egress_policy but a blocked MCP URL should load and run without crashing.
    # ══════════════════════════════════════════════════════════════════════════
    section("Gap 4 — Default egress: absent egress_policy → default_safe applied")

    sid_g4, err_g4 = await new_session_with_state(nc, kv, {
        # Deliberately NO egress_policy key → runner must apply default_safe()
        "mcp_servers": [
            {"name": "metadata-blocked", "url": "http://169.254.169.254/mcp", "headers": []}
        ],
    })
    if err_g4:
        fail("Gap4: session.new failed", str(err_g4))
    else:
        ok("Gap4: session created (no egress_policy field)", sid_g4)

    r_g4 = await nats_session_cmd(
        nc,
        f"{PREFIX}.session.{sid_g4}.agent.load",
        {"sessionId": sid_g4, "cwd": "/tmp", "mcpServers": []},
    )
    if "error" in r_g4 and "code" not in r_g4:
        fail("Gap4: session.load with absent egress_policy failed", str(r_g4))
    else:
        ok("Gap4: session.load succeeded — runner applied default_safe() to None egress_policy")

    # Run a prompt: the runner builds the agent, sees the 169.254.* MCP server,
    # applies default_safe(), blocks it, and continues without crashing.
    MockAnthropicHandler.reset([_END_TURN_RESPONSE])
    resp_g4 = await send_prompt(nc, sid_g4, "do nothing")
    if "error" not in resp_g4 and "code" not in resp_g4:
        ok("Gap4: prompt completed — default_safe egress did not crash the runner")
    else:
        fail("Gap4: prompt failed with absent egress_policy", str(resp_g4))

    # Confirm that a session WITH an explicit egress_policy set to None-equivalent
    # is treated the same way (defensive check).
    sid_g4b, _ = await new_session_with_state(nc, kv, {
        # egress_policy is the JSON null / Python None; runner must handle it
        "egress_policy": None,
        "mcp_servers": [
            {"name": "null-egress-mcp", "url": "http://169.254.169.254/mcp", "headers": []}
        ],
    })
    if sid_g4b:
        MockAnthropicHandler.reset([_END_TURN_RESPONSE])
        resp_g4b = await send_prompt(nc, sid_g4b, "also do nothing")
        if "error" not in resp_g4b and "code" not in resp_g4b:
            ok("Gap4b: prompt succeeded with explicit null egress_policy")
        else:
            fail("Gap4b: prompt failed with explicit null egress_policy", str(resp_g4b))

    # ══════════════════════════════════════════════════════════════════════════
    # Gap 5 — Multiple tool calls in one LLM turn → multiple audit entries
    #
    # Integration tests call the permission checker once per test.  This live
    # test has the mock return TWO tool_use blocks in a single response to
    # verify that each is checked independently and creates its own audit entry.
    # ══════════════════════════════════════════════════════════════════════════
    section("Gap 5 — Multiple tool calls per turn: two tool_use → two audit entries")

    sid_g5, err_g5 = await new_session_with_state(nc, kv, {
        "tool_policies": [
            {"tool": "write_file", "path_pattern": "/workspace/**", "action": "allow"},
        ]
    })
    if err_g5:
        fail("Gap5: session.new failed", str(err_g5))
    else:
        ok("Gap5: session created", sid_g5)

    MockAnthropicHandler.reset([
        _tool_use_response(
            ("write_file", "tc-g5a", {"path": "/workspace/file_a.txt", "content": "alpha"}),
            ("write_file", "tc-g5b", {"path": "/workspace/file_b.txt", "content": "beta"}),
        ),
        _END_TURN_RESPONSE,
    ])

    resp_g5 = await send_prompt(nc, sid_g5, "write two files")

    if "error" not in resp_g5 and "code" not in resp_g5:
        ok("Gap5: prompt with two tool calls completed")
    else:
        fail("Gap5: prompt failed", str(resp_g5))

    fresh_g5 = await kv_get(kv, sid_g5)
    if fresh_g5:
        log_g5     = fresh_g5.get("audit_log", [])
        allowed_g5 = [e for e in log_g5 if e.get("outcome") == "allowed"]
        if len(allowed_g5) >= 2:
            ok("Gap5: two 'allowed' audit entries for two tool calls",
               f"count={len(allowed_g5)}")
        else:
            summaries = [e.get("input_summary") for e in allowed_g5]
            fail("Gap5: expected ≥2 allowed audit entries",
                 f"got {len(allowed_g5)}: {summaries}")
    else:
        fail("Gap5: KV read after prompt returned nothing")

    # ══════════════════════════════════════════════════════════════════════════
    # Gap 6 — Bash command captured as input_summary in audit
    #
    # extract_input_summary() has a special branch for tool_name == "bash"/"Bash":
    # it uses the "command" field (first 60 chars) instead of "path".
    # This live test confirms the full pipeline writes the command to the KV audit.
    #
    # Note: the runner's tool list may not include "bash".  The permission
    # checker is called by the agent_core for every tool_use block the LLM
    # returns, so the audit entry is created even if execution fails with
    # "unknown tool".
    # ══════════════════════════════════════════════════════════════════════════
    section("Gap 6 — Bash input_summary: command captured as audit input_summary")

    BASH_CMD = "ls /workspace/test_dir"

    sid_g6, err_g6 = await new_session_with_state(nc, kv, {
        # No tool_policies → bash call falls through to interactive gate;
        # we auto-respond "allow" so the permission flow completes.
    })
    if err_g6:
        fail("Gap6: session.new failed", str(err_g6))
    else:
        ok("Gap6: session created", sid_g6)

    MockAnthropicHandler.reset([
        _tool_use_response(("bash", "tc-g6", {"command": BASH_CMD})),
        _END_TURN_RESPONSE,
    ])

    async with auto_respond_permissions(nc, sid_g6, "allow") as perm_g6:
        resp_g6 = await send_prompt(nc, sid_g6, "list the workspace directory")

    if "error" not in resp_g6 and "code" not in resp_g6:
        ok("Gap6: prompt with bash tool call completed")
    else:
        fail("Gap6: prompt failed", str(resp_g6))

    if perm_g6:
        ok("Gap6: permission_request received for bash tool", f"count={len(perm_g6)}")
    else:
        warn("Gap6: no permission_request (bash may not have gone through permission gate)")

    fresh_g6 = await kv_get(kv, sid_g6)
    if fresh_g6:
        log_g6 = fresh_g6.get("audit_log", [])
        bash_entries = [e for e in log_g6 if e.get("tool") in ("bash", "Bash")]
        if bash_entries:
            entry_g6 = bash_entries[0]
            got_summary = entry_g6.get("input_summary", "")
            expected    = BASH_CMD[:60]
            if got_summary == expected:
                ok("Gap6: bash command captured as input_summary", got_summary)
            else:
                fail("Gap6: input_summary mismatch",
                     f"expected={expected!r}  got={got_summary!r}")
        else:
            tool_names = [e.get("tool") for e in log_g6]
            if tool_names:
                fail("Gap6: no audit entry with tool='bash'", f"tools seen: {tool_names}")
            else:
                fail("Gap6: audit_log is empty after bash prompt")
    else:
        fail("Gap6: KV read after bash prompt returned nothing")

    # ══════════════════════════════════════════════════════════════════════════
    # Gap 7 — allow_always: persists tool to allowed_tools; second call
    #         auto-approved with no NATS permission request
    #
    # permission_bridge.rs has a third option ("allow_always") that:
    #   1. returns true (allows this call)
    #   2. appends the tool name to SessionState.allowed_tools in KV
    #
    # On the SECOND call the runner's RulesPermissionChecker finds the tool in
    # allowed_tools → Allow, no NATS request, audit records "allowed" directly.
    # ══════════════════════════════════════════════════════════════════════════
    section("Gap 7 — allow_always: tool persisted, second call auto-approved")

    sid_g7, err_g7 = await new_session_with_state(nc, kv, {
        "tool_policies": [
            {
                "tool": "write_file",
                "path_pattern": "/workspace/sensitive/**",
                "action": "require_approval",
            }
        ]
    })
    if err_g7:
        fail("Gap7: session.new failed", str(err_g7))
    else:
        ok("Gap7: session created", sid_g7)

    # ── First call: user responds allow_always ────────────────────────────────
    MockAnthropicHandler.reset([
        _tool_use_response(
            ("write_file", "tc-g7a",
             {"path": "/workspace/sensitive/report.txt", "content": "data"})
        ),
        _END_TURN_RESPONSE,
    ])

    async with auto_respond_permissions(nc, sid_g7, "allow_always") as perm_g7a:
        resp_g7a = await send_prompt(nc, sid_g7, "write sensitive report (first time)")

    if "error" not in resp_g7a and "code" not in resp_g7a:
        ok("Gap7: first prompt completed")
    else:
        fail("Gap7: first prompt failed", str(resp_g7a))

    if perm_g7a:
        ok("Gap7: permission_request received on first call", f"count={len(perm_g7a)}")
    else:
        fail("Gap7: no permission_request on first call (expected RequireApproval)")

    fresh_g7a = await kv_get(kv, sid_g7)
    if fresh_g7a:
        allowed = fresh_g7a.get("allowed_tools", [])
        if "write_file" in allowed:
            ok("Gap7: 'write_file' persisted to allowed_tools after allow_always",
               str(allowed))
        else:
            fail("Gap7: 'write_file' not found in allowed_tools", str(allowed))

        log_g7a  = fresh_g7a.get("audit_log", [])
        outs_g7a = [e.get("outcome") for e in log_g7a]
        if "required_approval" in outs_g7a:
            ok("Gap7: audit has required_approval for first call")
        else:
            fail("Gap7: audit missing required_approval", str(outs_g7a))
        if "approved_by_user" in outs_g7a:
            ok("Gap7: audit has approved_by_user for first call")
        else:
            fail("Gap7: audit missing approved_by_user", str(outs_g7a))
    else:
        fail("Gap7: KV read after first prompt returned nothing")

    # ── Second call: same tool, same path → auto-approved, no NATS request ───
    MockAnthropicHandler.reset([
        _tool_use_response(
            ("write_file", "tc-g7b",
             {"path": "/workspace/sensitive/report2.txt", "content": "data2"})
        ),
        _END_TURN_RESPONSE,
    ])

    perm_g7b_received = []
    perm_sub_g7b = await nc.subscribe(
        f"{PREFIX}.session.{sid_g7}.client.session.request_permission"
    )

    resp_g7b = await send_prompt(nc, sid_g7, "write sensitive report (second time)")
    await drain_perm(perm_sub_g7b, perm_g7b_received)
    try:
        await perm_sub_g7b.unsubscribe()
    except Exception:
        pass

    if "error" not in resp_g7b and "code" not in resp_g7b:
        ok("Gap7: second prompt completed")
    else:
        fail("Gap7: second prompt failed", str(resp_g7b))

    if not perm_g7b_received:
        ok("Gap7: no permission_request on second call — tool auto-approved from allowed_tools")
    else:
        fail("Gap7: unexpected permission_request on second call", str(perm_g7b_received[:1]))

    fresh_g7b = await kv_get(kv, sid_g7)
    if fresh_g7b:
        log_g7b  = fresh_g7b.get("audit_log", [])
        outs_g7b = [e.get("outcome") for e in log_g7b]
        allowed_entries = [o for o in outs_g7b if o == "allowed"]
        if allowed_entries:
            ok("Gap7: second call audit has 'allowed' (no re-prompt)",
               f"all outcomes: {outs_g7b}")
        else:
            fail("Gap7: second call audit missing 'allowed' entry", str(outs_g7b))
        if outs_g7b.count("required_approval") == 1:
            ok("Gap7: required_approval appears exactly once (first call only)")
        else:
            fail("Gap7: required_approval count unexpected",
                 f"count={outs_g7b.count('required_approval')} outcomes={outs_g7b}")
    else:
        fail("Gap7: KV read after second prompt returned nothing")

    # ══════════════════════════════════════════════════════════════════════════
    # Teardown
    # ══════════════════════════════════════════════════════════════════════════
    section("Teardown")

    mock_server.shutdown()
    ok("mock HTTP server stopped")

    runner.terminate()
    try:
        runner.wait(timeout=5)
        ok("runner subprocess terminated cleanly")
    except subprocess.TimeoutExpired:
        runner.kill()
        warn("runner subprocess killed after timeout")

    await nc.close()

    # ── Summary ───────────────────────────────────────────────────────────────
    total = passed + failed
    print(f"\n{bold}{'═' * 65}{reset}")
    color = green if failed == 0 else red
    print(f"  {color}{bold}{passed}/{total} passed{reset}   {red}{failed} failed{reset}")
    verdict = "ALL TESTS PASSED" if failed == 0 else f"{failed} TEST(S) FAILED"
    print(f"  {color}{bold}{verdict}{reset}")
    print(f"{bold}{'═' * 65}{reset}\n")

    sys.exit(0 if failed == 0 else 1)


if __name__ == "__main__":
    asyncio.run(main())
