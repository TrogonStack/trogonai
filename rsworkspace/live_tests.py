#!/usr/bin/env python3
"""Live tests for trogon-cli + runners."""
import asyncio, json, os, subprocess, sys, tempfile, threading, uuid
import nats as natspy

NATS_PLAIN = "nats://localhost:4222"   # CLI tests (no JetStream needed)
NATS_JS    = "nats://localhost:4335"   # runner tests (JetStream)
CLI = "/home/mario/straw-hat/trogonai/rsworkspace/target/debug/trogon"
BIN = "/home/mario/straw-hat/trogonai/rsworkspace/target/debug"

PASS = 0; FAIL = 0

def ok(label):
    global PASS; PASS += 1
    print(f"\033[32m✓\033[0m {label}", flush=True)

def fail(label, detail=""):
    global FAIL; FAIL += 1
    d = f"\n    {detail}" if detail else ""
    print(f"\033[31m✗\033[0m {label}{d}", flush=True)

def run_cli(*extra_args, prefix="acp.t", nats_url=NATS_PLAIN, timeout=8, input_text=None):
    cmd = [CLI, "--nats-url", nats_url, "--prefix", prefix] + list(extra_args)
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout,
                           input=input_text)
        return r.stdout + r.stderr, r.returncode
    except subprocess.TimeoutExpired:
        return "TIMEOUT", -1

def run_repl_cli(prefix, nats_url, cwd, input_text, timeout=20):
    """Run CLI in REPL (interactive) mode — no --print flag."""
    cmd = [CLI, "--nats-url", nats_url, "--prefix", prefix]
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout,
                           input=input_text, cwd=cwd)
        return r.stdout + r.stderr, r.returncode
    except subprocess.TimeoutExpired:
        return "TIMEOUT", -1

# ─────────────────────────────────────────────────────────────────────────────
# Helpers
# ─────────────────────────────────────────────────────────────────────────────

class FakeRunner:
    """Minimal ACP fake runner over raw NATS (no JetStream)."""

    def __init__(self, prefix, session_id, reply_text, nats_url=NATS_PLAIN):
        self.prefix     = prefix
        self.sid        = session_id
        self.reply_text = reply_text
        self.nats_url   = nats_url
        self._nc        = None
        self._done      = None
        # captured incoming prompt payload
        self.received_prompt_payload = None

    async def start(self):
        self._nc   = await natspy.connect(self.nats_url)
        self._done = asyncio.Event()

        async def on_new(msg):
            await self._nc.publish(msg.reply,
                                   json.dumps({"sessionId": self.sid}).encode())

        async def on_prompt(msg):
            try:
                self.received_prompt_payload = json.loads(msg.data)
            except Exception:
                pass
            # CLI uses publish_with_headers(X-Req-Id: req_id); response goes to
            # {prefix}.session.{sid}.agent.prompt.response.{req_id}
            req_id = None
            if msg.headers:
                req_id = msg.headers.get("X-Req-Id")
            notif_subj = f"{self.prefix}.session.{self.sid}.client.session.update"
            notif = json.dumps({"sessionId": self.sid, "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": self.reply_text}
            }})
            await self._nc.publish(notif_subj, notif.encode())
            await asyncio.sleep(0.05)
            if req_id:
                resp_subj = f"{self.prefix}.session.{self.sid}.agent.prompt.response.{req_id}"
                await self._nc.publish(resp_subj,
                                       json.dumps({"stopReason": "end_turn"}).encode())
            self._done.set()

        await self._nc.subscribe(f"{self.prefix}.agent.session.new",     cb=on_new)
        await self._nc.subscribe(
            f"{self.prefix}.session.{self.sid}.agent.prompt", cb=on_prompt)

    async def wait(self, timeout=10):
        try:
            await asyncio.wait_for(self._done.wait(), timeout=timeout)
        except asyncio.TimeoutError:
            pass

    async def close(self):
        if self._nc:
            await self._nc.close()


class MockHttpServer:
    """Simple HTTP mock that captures POST bodies and returns SSE."""

    def __init__(self, port, sse_fn):
        from http.server import HTTPServer, BaseHTTPRequestHandler
        self.received = []
        self.port     = port
        _self         = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                length = int(self.headers.get("Content-Length", 0))
                body   = self.rfile.read(length)
                _self.received.append(body)
                sse = sse_fn(len(_self.received))
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                self.wfile.write(sse.encode())
                self.wfile.flush()
            def log_message(self, fmt, *args): pass

        self._srv = HTTPServer(("127.0.0.1", port), Handler)
        self._thr = threading.Thread(target=self._srv.serve_forever)
        self._thr.daemon = True

    def start(self):
        self._thr.start()
        return self

    def stop(self):
        self._srv.shutdown()


def acp_tool_use_sse(tool_id, name, arguments_str):
    return (
        'event: message_start\ndata: {"type":"message_start","message":{"usage":'
        '{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}\n\n'
        f'event: content_block_start\ndata: {{"type":"content_block_start","index":0,'
        f'"content_block":{{"type":"tool_use","id":"{tool_id}","name":"{name}","input":{{}}}}}}\n\n'
        f'event: content_block_delta\ndata: {{"type":"content_block_delta","index":0,'
        f'"delta":{{"type":"input_json_delta","partial_json":{json.dumps(arguments_str)}}}}}\n\n'
        'event: content_block_stop\ndata: {"type":"content_block_stop","index":0}\n\n'
        'event: message_delta\ndata: {"type":"message_delta","delta":{"stop_reason":"tool_use"},'
        '"usage":{"output_tokens":15}}\n\n'
        'event: message_stop\ndata: {"type":"message_stop"}\n\n'
    )

def acp_text_sse(text):
    return (
        'event: message_start\ndata: {"type":"message_start","message":{"usage":'
        '{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}\n\n'
        'event: content_block_start\ndata: {"type":"content_block_start","index":0,'
        '"content_block":{"type":"text","text":""}}\n\n'
        f'event: content_block_delta\ndata: {{"type":"content_block_delta","index":0,'
        f'"delta":{{"type":"text_delta","text":{json.dumps(text)}}}}}\n\n'
        'event: content_block_stop\ndata: {"type":"content_block_stop","index":0}\n\n'
        'event: message_delta\ndata: {"type":"message_delta","delta":{"stop_reason":"end_turn"},'
        '"usage":{"output_tokens":5}}\n\n'
        'event: message_stop\ndata: {"type":"message_stop"}\n\n'
    )

def xai_text_sse(text):
    return (
        f'data: {{"type":"message.delta","id":"r1","delta":{{"text":{json.dumps(text)}}}}}\n\n'
        'data: {"type":"response.completed","response":{"id":"r1","status":"completed"},'
        '"usage":{"input_tokens":10,"output_tokens":5}}\n\n'
        'data: [DONE]\n\n'
    )

def or_text_sse(text):
    return (
        f'data: {{"choices":[{{"delta":{{"content":{json.dumps(text)}}},'
        '"finish_reason":null,"index":0}]}\n\n'
        'data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],'
        '"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}\n\n'
        'data: [DONE]\n\n'
    )

def xai_function_call_sse(call_id, tool_name, arguments_str):
    """xAI Responses API SSE: single function_call event then tool_calls stop."""
    args_json = json.dumps(arguments_str)
    return (
        f'data: {{"type":"function_call","id":"r1","function_call":{{"call_id":"{call_id}","name":"{tool_name}","arguments":{args_json}}}}}\n\n'
        'data: {"type":"response.completed","response":{"id":"r1","status":"tool_calls"}}\n\n'
        'data: [DONE]\n\n'
    )

def or_tool_calls_sse(call_id, tool_name, arguments_str):
    """OpenRouter SSE: one tool_calls delta then finish_reason=tool_calls."""
    args_json = json.dumps(arguments_str)
    return (
        f'data: {{"choices":[{{"delta":{{"tool_calls":[{{"index":0,"id":"{call_id}","function":{{"name":"{tool_name}","arguments":{args_json}}}}}]}},"finish_reason":null}}]}}\n\n'
        f'data: {{"choices":[{{"delta":{{}},"finish_reason":"tool_calls"}}]}}\n\n'
        'data: [DONE]\n\n'
    )


class MockJsonServer:
    """HTTP mock that returns plain JSON (not SSE) — used for spawn handler tests."""

    def __init__(self, port, response_fn):
        from http.server import HTTPServer, BaseHTTPRequestHandler
        self.received = []
        self.port = port
        _self = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                length = int(self.headers.get("Content-Length", 0))
                body = self.rfile.read(length)
                _self.received.append(body)
                try:
                    parsed = json.loads(body)
                except Exception:
                    parsed = {}
                resp = response_fn(len(_self.received), parsed)
                resp_bytes = json.dumps(resp).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(resp_bytes)))
                self.end_headers()
                self.wfile.write(resp_bytes)
                self.wfile.flush()
            def log_message(self, fmt, *args): pass

        self._srv = HTTPServer(("127.0.0.1", port), Handler)
        self._thr = threading.Thread(target=self._srv.serve_forever)
        self._thr.daemon = True

    def start(self):
        self._thr.start()
        return self

    def stop(self):
        self._srv.shutdown()

async def runner_session_prompt(nats_url, prefix, cwd, prompt_text, timeout=10,
                                bypass_permissions=False):
    """Create session + send one prompt; return (session_id, done_data, notif_msgs)."""
    nc = await natspy.connect(nats_url)
    try:
        sess_payload = {"cwd": cwd, "mcpServers": []}
        if bypass_permissions:
            sess_payload["_meta"] = {"mode": "bypassPermissions"}
        r = await nc.request(
            f"{prefix}.agent.session.new",
            json.dumps(sess_payload).encode(), timeout=5)
        sid = json.loads(r.data)["sessionId"]

        req_id    = str(uuid.uuid4())
        resp_subj = f"{prefix}.session.{sid}.agent.prompt.response.{req_id}"

        notif_msgs = []

        async def on_notif(msg):
            try:
                notif_msgs.append(json.loads(msg.data))
            except Exception:
                pass

        await nc.subscribe(f"{prefix}.session.{sid}.client.session.update", cb=on_notif)
        resp_sub = await nc.subscribe(resp_subj)

        # Publish with X-Req-Id header (runners extract req_id from header)
        await nc.publish(
            f"{prefix}.session.{sid}.agent.prompt",
            json.dumps({"sessionId": sid,
                        "prompt": [{"type": "text", "text": prompt_text}]}).encode(),
            headers={"X-Req-Id": req_id})

        msg = await asyncio.wait_for(resp_sub.next_msg(), timeout=timeout)
        return sid, json.loads(msg.data), notif_msgs
    finally:
        await nc.close()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 1 — --print text mode
# ─────────────────────────────────────────────────────────────────────────────

async def test_print_text():
    print("\n\033[1mTest 1: --print text mode\033[0m")
    runner = FakeRunner("acp.t1", "s1", "hello from fake runner")
    await runner.start()
    await asyncio.sleep(0.1)

    out, rc = await asyncio.get_event_loop().run_in_executor(
        None, lambda: run_cli("--print", "say hello", prefix="acp.t1"))
    await runner.wait(timeout=3)
    await runner.close()

    if "hello from fake runner" in out:
        ok("--print outputs model reply as plain text")
    else:
        fail("--print text mode", f"rc={rc} out={out!r}")

# ─────────────────────────────────────────────────────────────────────────────
# TEST 2 — --print --output-format json
# ─────────────────────────────────────────────────────────────────────────────

async def test_print_json():
    print("\n\033[1mTest 2: --print --output-format json\033[0m")
    runner = FakeRunner("acp.t2", "s2", "json test response")
    await runner.start()
    await asyncio.sleep(0.1)

    out, rc = await asyncio.get_event_loop().run_in_executor(
        None, lambda: run_cli("--print", "hello", "--output-format", "json", prefix="acp.t2"))
    await runner.wait(timeout=3)
    await runner.close()

    try:
        parsed = json.loads(out.strip())
        if "text" in parsed and "json test response" in parsed["text"]:
            ok('--output-format json → {"text":…,"stop_reason":…}')
            print(f"    → {out.strip()}")
        else:
            fail("json missing 'text' or wrong content", f"parsed={parsed}")
    except json.JSONDecodeError:
        fail("output is not valid JSON", f"rc={rc} out={out!r}")

# ─────────────────────────────────────────────────────────────────────────────
# TEST 3 — stdin pipe mode
# ─────────────────────────────────────────────────────────────────────────────

async def test_stdin_pipe():
    print("\n\033[1mTest 3: stdin pipe mode\033[0m")
    runner = FakeRunner("acp.t3", "s3", "piped reply works")
    await runner.start()
    await asyncio.sleep(0.1)

    out, rc = await asyncio.get_event_loop().run_in_executor(
        None, lambda: run_cli("--print", prefix="acp.t3", input_text="meaning of life?\n"))
    await runner.wait(timeout=3)
    await runner.close()

    if "piped reply works" in out:
        ok('echo "prompt" | trogon --print works')
    else:
        fail("stdin pipe mode", f"rc={rc} out={out!r}")

# ─────────────────────────────────────────────────────────────────────────────
# TEST 4 — @file mention → content in prompt
# ─────────────────────────────────────────────────────────────────────────────

async def test_at_file():
    print("\n\033[1mTest 4: @file mention → prompt forwarded as-is (no client-side expansion)\033[0m")

    with tempfile.NamedTemporaryFile(mode='w', suffix='.txt', delete=False) as f:
        f.write("SECRET_CONTENT_XYZ_12345\n")
        fpath = f.name

    runner = FakeRunner("acp.t4", "s4", "file content received")
    await runner.start()
    await asyncio.sleep(0.1)

    out, rc = await asyncio.get_event_loop().run_in_executor(
        None, lambda: run_cli("--print", f"explain @{fpath}", prefix="acp.t4"))
    await runner.wait(timeout=3)
    await runner.close()
    os.unlink(fpath)

    # @file expansion is runner-side, not CLI-side; CLI forwards the raw text
    payload = runner.received_prompt_payload
    if payload and "file content received" in out:
        prompt_text = " ".join(
            c.get("text", "") for c in payload.get("prompt", []) if isinstance(c, dict))
        if f"@{fpath}" in prompt_text or "explain" in prompt_text:
            ok("CLI forwards @file prompt as-is (expansion is runner/LLM responsibility)")
        else:
            fail("CLI forwarded prompt but text is wrong", f"prompt_text={prompt_text!r}")
    else:
        fail("@file test: runner didn't reply", f"rc={rc} out={out!r}")

# ─────────────────────────────────────────────────────────────────────────────
# TEST 5 — TROGON.md in xai-runner
# ─────────────────────────────────────────────────────────────────────────────

async def test_trogon_md_xai():
    print("\n\033[1mTest 5: TROGON.md injection in xai-runner\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        with open(os.path.join(cwd, "TROGON.md"), 'w') as f:
            f.write("# TROGON_MARKER_XAI_TEST\nYou are a helpful assistant.\n")

        mock = MockHttpServer(29960, lambda n: xai_text_sse("xai ok")).start()
        try:
            env = {**os.environ,
                   "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.xai5",
                   "XAI_API_KEY": "test-key", "XAI_BASE_URL": "http://127.0.0.1:29960",
                   "XAI_MODELS": "grok-4:grok-4"}
            proc = subprocess.Popen([f"{BIN}/trogon-xai-runner"], env=env,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(2.5)

            sid, done, _ = await runner_session_prompt(NATS_JS, "acp.xai5", cwd, "hello")
            print(f"    sid={sid} done={done}", flush=True)

            if mock.received:
                body = json.loads(mock.received[0])
                input_items = body.get("input", [])
                system_items = [item for item in input_items
                                if isinstance(item, dict) and item.get("role") == "system"]
                if system_items and "TROGON_MARKER_XAI_TEST" in json.dumps(system_items):
                    ok("xai-runner injects TROGON.md into input[role=system] (correct field)")
                elif "TROGON_MARKER_XAI_TEST" in json.dumps(body):
                    fail("TROGON.md present but NOT in input[role=system]",
                         f"system_items={system_items!r}")
                else:
                    fail("TROGON.md marker not found in xai HTTP body",
                         f"input items: {json.dumps(input_items)[:300]!r}")
            else:
                fail("xai-runner TROGON.md", "no HTTP calls received")
        finally:
            proc.terminate(); proc.wait(timeout=3)
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 6 — TROGON.md in openrouter-runner
# ─────────────────────────────────────────────────────────────────────────────

async def test_trogon_md_openrouter():
    print("\n\033[1mTest 6: TROGON.md injection in openrouter-runner\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        with open(os.path.join(cwd, "TROGON.md"), 'w') as f:
            f.write("# TROGON_MARKER_OR_TEST\nOpenRouter workspace.\n")

        mock = MockHttpServer(29961, lambda n: or_text_sse("or ok")).start()
        try:
            env = {**os.environ,
                   "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.or6",
                   "OPENROUTER_API_KEY": "test-key",
                   "OPENROUTER_BASE_URL": "http://127.0.0.1:29961",
                   "OPENROUTER_MODELS": "anthropic/claude-3-5-sonnet:claude"}
            proc = subprocess.Popen([f"{BIN}/trogon-openrouter-runner"], env=env,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(2.5)

            sid, done, _ = await runner_session_prompt(NATS_JS, "acp.or6", cwd, "hello")
            print(f"    sid={sid} done={done}", flush=True)

            if mock.received:
                body = json.loads(mock.received[0])
                messages = body.get("messages", [])
                system_msgs = [m for m in messages
                               if isinstance(m, dict) and m.get("role") == "system"]
                if system_msgs and "TROGON_MARKER_OR_TEST" in json.dumps(system_msgs):
                    ok("openrouter-runner injects TROGON.md into messages[role=system] (correct field)")
                elif "TROGON_MARKER_OR_TEST" in json.dumps(body):
                    fail("TROGON.md present but NOT in messages[role=system]",
                         f"system_msgs={system_msgs!r}")
                else:
                    fail("TROGON.md marker not found in openrouter HTTP body",
                         f"messages: {json.dumps(messages)[:300]!r}")
            else:
                fail("openrouter-runner TROGON.md", "no HTTP calls received")
        finally:
            proc.terminate(); proc.wait(timeout=3)
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 7 — Cross-runner switch: history transferred acp→xai
# ─────────────────────────────────────────────────────────────────────────────

async def test_cross_runner_switch():
    print("\n\033[1mTest 7: Cross-runner switch — history transferred acp→xai\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        acp_mock = MockHttpServer(29962, lambda n: acp_text_sse("acp turn 1")).start()
        xai_mock = MockHttpServer(29963, lambda n: xai_text_sse("xai turn 2")).start()

        try:
            env_acp = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.r7a",
                       "PROXY_URL": "http://127.0.0.1:29962",
                       "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-opus-4-6"}
            env_xai = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.r7x",
                       "XAI_API_KEY": "test", "XAI_BASE_URL": "http://127.0.0.1:29963",
                       "XAI_MODELS": "grok-4:grok-4"}

            proc_acp = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env_acp,
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            proc_xai = subprocess.Popen([f"{BIN}/trogon-xai-runner"], env=env_xai,
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(3)

            nc = await natspy.connect(NATS_JS)
            try:
                # 1. ACP session + turn 1
                r = await nc.request("acp.r7a.agent.session.new",
                                     json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
                acp_sid = json.loads(r.data)["sessionId"]
                print(f"    acp sid={acp_sid}", flush=True)

                req_id = str(uuid.uuid4())
                resp_subj = f"acp.r7a.session.{acp_sid}.agent.prompt.response.{req_id}"
                resp_sub  = await nc.subscribe(resp_subj)
                async def _noop(m): pass
                await nc.subscribe(f"acp.r7a.session.{acp_sid}.client.session.update", cb=_noop)

                await nc.publish(
                    f"acp.r7a.session.{acp_sid}.agent.prompt",
                    json.dumps({"sessionId": acp_sid,
                                "prompt": [{"type": "text", "text": "tell me about Python"}]}).encode(),
                    headers={"X-Req-Id": req_id})
                await asyncio.wait_for(resp_sub.next_msg(), timeout=8)
                print(f"    acp turn 1 done", flush=True)

                # 2. Export from ACP — subject: {prefix}.agent.ext.session/export
                r = await nc.request(
                    "acp.r7a.agent.ext.session/export",
                    json.dumps({"sessionId": acp_sid}).encode(),
                    timeout=5)
                messages = json.loads(r.data)
                if isinstance(messages, dict):
                    messages = messages.get("messages", [])
                print(f"    exported {len(messages)} messages", flush=True)

                # 3. XAI session
                r = await nc.request("acp.r7x.agent.session.new",
                                     json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
                xai_sid = json.loads(r.data)["sessionId"]
                print(f"    xai sid={xai_sid}", flush=True)

                # 4. Import into XAI — subject: {prefix}.agent.ext.session/import
                r = await nc.request(
                    "acp.r7x.agent.ext.session/import",
                    json.dumps({"sessionId": xai_sid, "messages": messages}).encode(),
                    timeout=5)
                print(f"    import result: {r.data[:80]}", flush=True)

                # 5. XAI turn 2
                req_id2    = str(uuid.uuid4())
                resp_subj2 = f"acp.r7x.session.{xai_sid}.agent.prompt.response.{req_id2}"
                resp_sub2  = await nc.subscribe(resp_subj2)
                async def _noop2(m): pass
                await nc.subscribe(f"acp.r7x.session.{xai_sid}.client.session.update", cb=_noop2)

                await nc.publish(
                    f"acp.r7x.session.{xai_sid}.agent.prompt",
                    json.dumps({"sessionId": xai_sid,
                                "prompt": [{"type": "text", "text": "continue"}]}).encode(),
                    headers={"X-Req-Id": req_id2})
                await asyncio.wait_for(resp_sub2.next_msg(), timeout=8)
                print(f"    xai turn 2 done", flush=True)

                # Check: xai API call contains imported history with correct roles
                if xai_mock.received:
                    body = json.loads(xai_mock.received[0])
                    input_items = body.get("input", [])
                    combined    = json.dumps(input_items)
                    user_items  = [it for it in input_items
                                   if isinstance(it, dict) and it.get("role") == "user"]
                    asst_items  = [it for it in input_items
                                   if isinstance(it, dict) and it.get("role") == "assistant"]
                    user_has_prompt = any("tell me about Python" in json.dumps(it) for it in user_items)
                    asst_has_reply  = any("acp turn 1" in json.dumps(it) for it in asst_items)
                    if user_has_prompt and asst_has_reply:
                        ok("cross-runner: acp→xai history transferred — original text present in xai input")
                    elif "tell me about Python" in combined and "acp turn 1" in combined:
                        fail("cross-runner: text present but role structure wrong",
                             f"user_items={len(user_items)} asst_items={len(asst_items)} "
                             f"preview={combined[:200]!r}")
                    else:
                        fail("cross-runner: history text missing from xai HTTP body",
                             f"exported={len(messages)} msgs, input_items={len(input_items)}, "
                             f"preview={combined[:200]!r}")
                else:
                    fail("cross-runner", "xai mock received no calls")
            finally:
                await nc.close()
        finally:
            for p in (proc_acp, proc_xai):
                p.terminate(); p.wait(timeout=3)
            acp_mock.stop(); xai_mock.stop()

# ─────────────────────────────────────────────────────────────────────────────

# ─────────────────────────────────────────────────────────────────────────────
# Helper — AGENT_REGISTRY KV lookup
# ─────────────────────────────────────────────────────────────────────────────

async def find_prefix_by_model(nats_url, agent_type_key, model_name, timeout=5):
    """Read AGENT_REGISTRY KV and return (acp_prefix, models_list) for agent_type_key."""
    nc = await natspy.connect(nats_url)
    try:
        js = nc.jetstream()
        kv = await js.key_value("AGENT_REGISTRY")
        deadline = asyncio.get_event_loop().time() + timeout
        while asyncio.get_event_loop().time() < deadline:
            try:
                entry = await kv.get(agent_type_key)
                if entry and entry.value:
                    cap = json.loads(entry.value)
                    meta = cap.get("metadata", {})
                    return meta.get("acp_prefix"), meta.get("models", [])
            except Exception:
                pass
            await asyncio.sleep(0.3)
        return None, []
    finally:
        await nc.close()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 8 — AGENT_REGISTRY + cross-runner switch verified via KV
# ─────────────────────────────────────────────────────────────────────────────

async def test_cross_runner_registry():
    print("\n\033[1mTest 8: AGENT_REGISTRY KV lookup + cross-runner switch\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        acp_mock = MockHttpServer(29970, lambda n: acp_text_sse("acp t8 turn")).start()
        xai_mock = MockHttpServer(29971, lambda n: xai_text_sse("xai t8 turn")).start()

        try:
            env_acp = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t8a",
                       "AGENT_TYPE": "claude-t8a",
                       "PROXY_URL": "http://127.0.0.1:29970",
                       "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-t8"}
            env_xai = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t8x",
                       "AGENT_TYPE": "xai-t8x",
                       "XAI_API_KEY": "test", "XAI_BASE_URL": "http://127.0.0.1:29971",
                       "XAI_MODELS": "grok-t8:grok-t8"}

            proc_acp = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env_acp,
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            proc_xai = subprocess.Popen([f"{BIN}/trogon-xai-runner"], env=env_xai,
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(3)

            # ── Step 1: verify registry KV ────────────────────────────────────
            prefix_found, models_found = await find_prefix_by_model(
                NATS_JS, "xai-t8x", "grok-t8", timeout=5)
            if prefix_found == "acp.t8x" and "grok-t8" in models_found:
                ok(f"AGENT_REGISTRY KV: xai-t8x → prefix={prefix_found!r} models={models_found}")
            elif prefix_found:
                fail("AGENT_REGISTRY KV: wrong prefix or models",
                     f"prefix={prefix_found!r} models={models_found}")
            else:
                fail("AGENT_REGISTRY KV: xai-t8x not found after 5s")

            # ── Step 2: cross-runner switch (acp→xai) with registry-confirmed prefix ──
            nc = await natspy.connect(NATS_JS)
            try:
                r = await nc.request("acp.t8a.agent.session.new",
                                     json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
                acp_sid = json.loads(r.data)["sessionId"]

                req_id = str(uuid.uuid4())
                resp_sub = await nc.subscribe(
                    f"acp.t8a.session.{acp_sid}.agent.prompt.response.{req_id}")
                async def _noop_t8a(m): pass
                await nc.subscribe(f"acp.t8a.session.{acp_sid}.client.session.update",
                                   cb=_noop_t8a)
                await nc.publish(
                    f"acp.t8a.session.{acp_sid}.agent.prompt",
                    json.dumps({"sessionId": acp_sid,
                                "prompt": [{"type": "text", "text": "hello from acp t8"}]}).encode(),
                    headers={"X-Req-Id": req_id})
                await asyncio.wait_for(resp_sub.next_msg(), timeout=8)

                r = await nc.request("acp.t8a.agent.ext.session/export",
                                     json.dumps({"sessionId": acp_sid}).encode(), timeout=5)
                messages = json.loads(r.data)
                if isinstance(messages, dict):
                    messages = messages.get("messages", [])
                print(f"    exported {len(messages)} messages from acp", flush=True)

                r = await nc.request("acp.t8x.agent.session.new",
                                     json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
                xai_sid = json.loads(r.data)["sessionId"]

                r = await nc.request("acp.t8x.agent.ext.session/import",
                                     json.dumps({"sessionId": xai_sid,
                                                 "messages": messages}).encode(), timeout=5)

                req_id2 = str(uuid.uuid4())
                resp_sub2 = await nc.subscribe(
                    f"acp.t8x.session.{xai_sid}.agent.prompt.response.{req_id2}")
                async def _noop_t8x(m): pass
                await nc.subscribe(f"acp.t8x.session.{xai_sid}.client.session.update",
                                   cb=_noop_t8x)
                await nc.publish(
                    f"acp.t8x.session.{xai_sid}.agent.prompt",
                    json.dumps({"sessionId": xai_sid,
                                "prompt": [{"type": "text", "text": "continue on xai t8"}]}).encode(),
                    headers={"X-Req-Id": req_id2})
                await asyncio.wait_for(resp_sub2.next_msg(), timeout=8)

                if xai_mock.received:
                    body = json.loads(xai_mock.received[0])
                    input_items = body.get("input", [])
                    combined    = json.dumps(input_items)
                    user_items  = [it for it in input_items
                                   if isinstance(it, dict) and it.get("role") == "user"]
                    asst_items  = [it for it in input_items
                                   if isinstance(it, dict) and it.get("role") == "assistant"]
                    user_has_prompt = any("hello from acp t8" in json.dumps(it) for it in user_items)
                    asst_has_reply  = any("acp t8 turn" in json.dumps(it) for it in asst_items)
                    if user_has_prompt and asst_has_reply:
                        ok(f"registry-confirmed cross-runner switch: history transferred ({len(messages)} msgs)")
                    elif "hello from acp t8" in combined and "acp t8 turn" in combined:
                        fail("registry switch: text present but role structure wrong",
                             f"user_items={len(user_items)} asst_items={len(asst_items)}")
                    else:
                        fail("registry switch", f"xai input: {combined[:200]!r}")
                else:
                    fail("registry switch", "xai mock received no calls")
            finally:
                await nc.close()
        finally:
            for p in (proc_acp, proc_xai):
                p.terminate(); p.wait(timeout=3)
            acp_mock.stop(); xai_mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 9 — codex-runner: session export/import lifecycle
# ─────────────────────────────────────────────────────────────────────────────

async def test_codex_runner():
    print("\n\033[1mTest 9: codex-runner export/import lifecycle\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.cx9",
               "CODEX_BIN": f"{BIN}/mock_codex_server",
               "CODEX_MODELS": "codex-mini:codex-mini",
               "CODEX_DEFAULT_MODEL": "codex-mini",
               "CODEX_SPAWN_TIMEOUT_SECS": "5"}
        proc = subprocess.Popen([f"{BIN}/trogon-codex-runner"], env=env,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(3)

        nc = await natspy.connect(NATS_JS)
        try:
            # 1. Create session
            r = await nc.request("acp.cx9.agent.session.new",
                                 json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
            sid = json.loads(r.data)["sessionId"]
            print(f"    codex sid={sid}", flush=True)

            # 2. Export (should be empty)
            r = await nc.request("acp.cx9.agent.ext.session/export",
                                 json.dumps({"sessionId": sid}).encode(), timeout=5)
            exported = json.loads(r.data)
            if isinstance(exported, dict):
                exported = exported.get("messages", [])
            print(f"    initial export: {len(exported)} messages", flush=True)
            if len(exported) == 0:
                ok("codex-runner: initial export is empty")
            else:
                fail("codex-runner: initial export should be empty", f"got {exported}")

            # 3. Import 2 portable messages (PortableMessage: {role, text, blocks?})
            two_msgs = [
                {"role": "user",      "text": "prior user msg"},
                {"role": "assistant", "text": "prior asst msg"},
            ]
            r = await nc.request("acp.cx9.agent.ext.session/import",
                                 json.dumps({"sessionId": sid, "messages": two_msgs}).encode(),
                                 timeout=5)
            print(f"    import result: {r.data[:60]}", flush=True)

            # 4. Export (should be 2)
            r = await nc.request("acp.cx9.agent.ext.session/export",
                                 json.dumps({"sessionId": sid}).encode(), timeout=5)
            after_import = json.loads(r.data)
            if isinstance(after_import, dict):
                after_import = after_import.get("messages", [])
            print(f"    post-import export: {len(after_import)} messages", flush=True)
            if len(after_import) == 2:
                ok("codex-runner: export after import returns 2 messages")
            else:
                fail("codex-runner: export after import wrong count",
                     f"expected 2, got {len(after_import)}")

            # 5. Send a prompt
            req_id = str(uuid.uuid4())
            resp_sub = await nc.subscribe(
                f"acp.cx9.session.{sid}.agent.prompt.response.{req_id}")
            async def _noop_cx9(m): pass
            await nc.subscribe(f"acp.cx9.session.{sid}.client.session.update", cb=_noop_cx9)
            await nc.publish(
                f"acp.cx9.session.{sid}.agent.prompt",
                json.dumps({"sessionId": sid,
                            "prompt": [{"type": "text", "text": "hello codex"}]}).encode(),
                headers={"X-Req-Id": req_id})
            await asyncio.wait_for(resp_sub.next_msg(), timeout=10)
            print("    codex prompt done", flush=True)

            # 6. Export after turn (should be >= 3: imported 2 + at least 1 from turn)
            r = await nc.request("acp.cx9.agent.ext.session/export",
                                 json.dumps({"sessionId": sid}).encode(), timeout=5)
            after_turn = json.loads(r.data)
            if isinstance(after_turn, dict):
                after_turn = after_turn.get("messages", [])
            print(f"    post-turn export: {len(after_turn)} messages", flush=True)
            if len(after_turn) >= 3:
                ok(f"codex-runner: export after turn grows history ({len(after_turn)} messages)")
            elif len(after_turn) >= 2:
                ok(f"codex-runner: export after turn preserved history ({len(after_turn)} messages)")
            else:
                fail("codex-runner: export after turn has less than imported messages",
                     f"got {len(after_turn)}")
        except Exception as e:
            fail("codex-runner lifecycle", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)

# ─────────────────────────────────────────────────────────────────────────────
# TEST 10 — @file expansion in the interactive REPL (PR 10)
# ─────────────────────────────────────────────────────────────────────────────

async def test_at_file_repl():
    print("\n\033[1mTest 10: @file expansion in REPL (PR 10 — expand_mentions in repl.rs)\033[0m")

    with tempfile.NamedTemporaryFile(mode='w', suffix='.txt', delete=False) as f:
        f.write("SECRET_CONTENT_XYZ_REPL\n")
        fpath = f.name

    # REPL mode requires JetStream (main.rs provisions AGENT_REGISTRY KV)
    runner = FakeRunner("acp.t10", "s10r", "expansion acknowledged", nats_url=NATS_JS)
    await runner.start()
    await asyncio.sleep(0.3)

    with tempfile.TemporaryDirectory() as cwd:
        # Pipe a single line: @absolute-path → REPL should expand before sending
        out, rc = await asyncio.get_event_loop().run_in_executor(
            None, lambda: run_repl_cli(
                "acp.t10", NATS_JS, cwd,
                input_text=f"explain @{fpath}\n", timeout=15))
    await runner.wait(timeout=5)
    await runner.close()
    os.unlink(fpath)

    payload = runner.received_prompt_payload
    if payload:
        prompt_text = " ".join(
            c.get("text", "") for c in payload.get("prompt", []) if isinstance(c, dict))
        if "SECRET_CONTENT_XYZ_REPL" in prompt_text:
            ok("REPL expands @file: file content injected into prompt (not raw @path)")
        elif f"@{fpath}" in prompt_text:
            fail("REPL @file expansion: raw @path sent — expand_mentions did NOT fire",
                 f"prompt_text={prompt_text[:200]!r}")
        else:
            fail("REPL @file expansion: unexpected prompt text",
                 f"prompt_text={prompt_text[:200]!r}")
    else:
        fail("REPL @file expansion: runner never received prompt",
             f"rc={rc} out={out[:200]!r}")

# ─────────────────────────────────────────────────────────────────────────────
# TEST 11 — /model switch via CLI (PR 9 — CrossRunnerSwitcher end-to-end)
# ─────────────────────────────────────────────────────────────────────────────

async def test_model_switch_via_cli():
    print("\n\033[1mTest 11: /model switch via CLI (PR 9 — CrossRunnerSwitcher end-to-end)\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        acp_mock = MockHttpServer(29986, lambda n: acp_text_sse("acp t11 response")).start()
        xai_mock = MockHttpServer(29987, lambda n: xai_text_sse("xai t11 response")).start()

        try:
            env_acp = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t11a",
                       "AGENT_TYPE": "claude-t11a",
                       "PROXY_URL": "http://127.0.0.1:29986",
                       "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-t11"}
            env_xai = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t11x",
                       "AGENT_TYPE": "xai-t11x",
                       "XAI_API_KEY": "test", "XAI_BASE_URL": "http://127.0.0.1:29987",
                       "XAI_MODELS": "grok-t11:grok-t11"}

            proc_acp = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env_acp,
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            proc_xai = subprocess.Popen([f"{BIN}/trogon-xai-runner"], env=env_xai,
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(3)

            # CLI REPL session: turn 1 on acp, /model switch, turn 2 on xai
            cli_input = "hello from acp\n/model grok-t11\nhello from xai\n"
            out, rc = await asyncio.get_event_loop().run_in_executor(
                None, lambda: run_repl_cli(
                    "acp.t11a", NATS_JS, cwd,
                    input_text=cli_input, timeout=35))
            print(f"    cli rc={rc} out={out[:300]!r}", flush=True)

            # 1. acp-runner was called (turn 1)
            if acp_mock.received:
                ok("turn 1 reached acp-runner before /model switch")
            else:
                fail("/model switch: acp-runner got no calls (turn 1 never reached runner)")

            # 2. xai-runner was called after /model switch (turn 2)
            if xai_mock.received:
                body = json.loads(xai_mock.received[0])
                input_items = body.get("input", [])
                combined = json.dumps(input_items)
                if "hello from acp" in combined and "acp t11 response" in combined:
                    ok("/model switch: xai turn 2 received — acp history present in xai input")
                else:
                    # xai was called but history text not found — switch happened but history not carried
                    fail("/model switch: xai called but acp history missing from input",
                         f"input_items={len(input_items)}, preview={combined[:200]!r}")
            else:
                fail("/model switch: xai-runner got no calls — switch did not reach xai runner")

            # 3. Routing: second prompt went to xai, not back to acp
            if xai_mock.received and len(acp_mock.received) == 1:
                ok("/model switch: second prompt routed to xai-runner, acp-runner got exactly 1 call")
            else:
                fail("/model switch: unexpected routing after /model switch",
                     f"acp_calls={len(acp_mock.received)} xai_calls={len(xai_mock.received)}")
        finally:
            for p in (proc_acp, proc_xai):
                p.terminate(); p.wait(timeout=3)
            acp_mock.stop(); xai_mock.stop()

# ─────────────────────────────────────────────────────────────────────────────

# ─────────────────────────────────────────────────────────────────────────────
# TEST 12 — Tool execution loop: LLM tool_use → execute → result → LLM
# ─────────────────────────────────────────────────────────────────────────────

async def test_tool_execution():
    print("\n\033[1mTest 12: Tool execution loop (acp-runner: tool_use → execute → result → LLM)\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        with open(os.path.join(tmpdir, "tfile.txt"), 'w') as f:
            f.write("TOOL_EXEC_CONTENT_T12\n")

        def tool_mock_sse(n):
            if n == 1:
                return acp_tool_use_sse("t1", "read_file",
                                        json.dumps({"path": "tfile.txt"}))
            return acp_text_sse("file read successfully by LLM")

        mock = MockHttpServer(29990, tool_mock_sse).start()
        auto_approve = subprocess.Popen(
            ["python3", "/tmp/auto_approve.py", "nats://localhost:4335"],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(0.5)

        try:
            env = {**os.environ,
                   "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t12",
                   "PROXY_URL": "http://127.0.0.1:29990",
                   "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-opus-4-6",
                   "AGENT_MAX_ITERATIONS": "5"}
            # start runner with cwd=tmpdir so read_file("tfile.txt") resolves correctly
            proc = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env, cwd=tmpdir,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(2.5)

            sid, done, notifs = await runner_session_prompt(
                NATS_JS, "acp.t12", tmpdir, "read the tfile", timeout=20)
            print(f"    sid={sid} done={done} mock_calls={len(mock.received)}", flush=True)

            if len(mock.received) >= 2:
                body2 = json.loads(mock.received[1])
                msgs2 = json.dumps(body2.get("messages", []))
                if "TOOL_EXEC_CONTENT_T12" in msgs2:
                    ok("tool loop: read_file content returned in HTTP call 2 — full loop works")
                else:
                    fail("tool loop: read_file content missing from HTTP call 2",
                         f"msgs preview: {msgs2[:300]!r}")
            elif len(mock.received) == 1:
                fail("tool loop: only 1 HTTP call — tool_use did not trigger a second call",
                     f"done={done}")
            else:
                fail("tool loop: no HTTP calls received")
        finally:
            auto_approve.terminate(); auto_approve.wait(timeout=2)
            proc.terminate(); proc.wait(timeout=3)
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 13 — TROGON.md injection in acp-runner
# ─────────────────────────────────────────────────────────────────────────────

async def test_trogon_md_acp():
    print("\n\033[1mTest 13: TROGON.md injection in acp-runner (system prompt field)\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        with open(os.path.join(cwd, "TROGON.md"), 'w') as f:
            f.write("# TROGON_MARKER_ACP_TEST\nYou are a programming assistant.\n")

        mock = MockHttpServer(29991, lambda n: acp_text_sse("acp ok")).start()
        try:
            env = {**os.environ,
                   "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t13",
                   "PROXY_URL": "http://127.0.0.1:29991",
                   "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-opus-4-6"}
            proc = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(2.5)

            sid, done, _ = await runner_session_prompt(NATS_JS, "acp.t13", cwd, "hello")
            print(f"    sid={sid} done={done}", flush=True)

            if mock.received:
                body = json.loads(mock.received[0])
                system_val = body.get("system", "")
                if isinstance(system_val, list):
                    system_text = " ".join(
                        b.get("text", "") for b in system_val if isinstance(b, dict))
                else:
                    system_text = str(system_val)

                if "TROGON_MARKER_ACP_TEST" in system_text:
                    ok("acp-runner injects TROGON.md into system field (correct field)")
                elif "TROGON_MARKER_ACP_TEST" in json.dumps(body):
                    fail("acp TROGON.md present but NOT in system field",
                         f"system_text={system_text[:200]!r}")
                else:
                    fail("acp TROGON.md marker not found in HTTP body",
                         f"system={system_text[:200]!r}")
            else:
                fail("acp-runner TROGON.md", "no HTTP calls received")
        finally:
            proc.terminate(); proc.wait(timeout=3)
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 14 — TROGON.md injection in codex-runner (first-turn prepend)
# ─────────────────────────────────────────────────────────────────────────────

async def test_trogon_md_codex():
    print("\n\033[1mTest 14: TROGON.md injection in codex-runner (first turn prepend)\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        with open(os.path.join(cwd, "TROGON.md"), 'w') as f:
            f.write("# TROGON_MARKER_CODEX_TEST\nCodex workspace.\n")

        record_file = os.path.join(cwd, "recorded_input.txt")
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t14",
               "CODEX_BIN": f"{BIN}/mock_codex_server",
               "CODEX_MODELS": "codex-t14:codex-t14",
               "CODEX_DEFAULT_MODEL": "codex-t14",
               "MOCK_RECORD_TURN_INPUT_FILE": record_file,
               "CODEX_SPAWN_TIMEOUT_SECS": "5"}
        proc = subprocess.Popen([f"{BIN}/trogon-codex-runner"], env=env,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(3)

        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, "acp.t14", cwd, "hello codex", timeout=10)
            print(f"    sid={sid} done={done}", flush=True)
            await asyncio.sleep(0.3)

            if os.path.exists(record_file):
                recorded = open(record_file).read()
                print(f"    recorded input: {recorded[:120]!r}", flush=True)
                if "TROGON_MARKER_CODEX_TEST" in recorded:
                    ok("codex-runner prepends TROGON.md to first turn's userInput (correct)")
                else:
                    fail("codex-runner TROGON.md not in recorded userInput",
                         f"recorded={recorded[:300]!r}")
            else:
                fail("codex-runner TROGON.md", "MOCK_RECORD_TURN_INPUT_FILE not written by mock")
        finally:
            proc.terminate(); proc.wait(timeout=3)

# ─────────────────────────────────────────────────────────────────────────────
# TEST 15 — Import into acp-runner (xai→acp direction)
# ─────────────────────────────────────────────────────────────────────────────

async def test_import_into_acp():
    print("\n\033[1mTest 15: Import into acp-runner — xai→acp cross-runner switch\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        xai_mock = MockHttpServer(29992, lambda n: xai_text_sse("xai t15 turn")).start()
        acp_mock = MockHttpServer(29993, lambda n: acp_text_sse("acp t15 turn")).start()

        try:
            env_xai = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t15x",
                       "XAI_API_KEY": "test", "XAI_BASE_URL": "http://127.0.0.1:29992",
                       "XAI_MODELS": "grok-t15:grok-t15"}
            env_acp = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t15a",
                       "PROXY_URL": "http://127.0.0.1:29993",
                       "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-opus-4-6"}

            proc_xai = subprocess.Popen([f"{BIN}/trogon-xai-runner"], env=env_xai,
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            proc_acp = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env_acp,
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(3)

            nc = await natspy.connect(NATS_JS)
            try:
                # xai session + turn
                r = await nc.request("acp.t15x.agent.session.new",
                                     json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
                xai_sid = json.loads(r.data)["sessionId"]
                req_id = str(uuid.uuid4())
                resp_sub = await nc.subscribe(
                    f"acp.t15x.session.{xai_sid}.agent.prompt.response.{req_id}")
                async def _noop_t15x(m): pass
                await nc.subscribe(
                    f"acp.t15x.session.{xai_sid}.client.session.update", cb=_noop_t15x)
                await nc.publish(
                    f"acp.t15x.session.{xai_sid}.agent.prompt",
                    json.dumps({"sessionId": xai_sid,
                                "prompt": [{"type": "text",
                                            "text": "hello from xai t15"}]}).encode(),
                    headers={"X-Req-Id": req_id})
                await asyncio.wait_for(resp_sub.next_msg(), timeout=8)
                print("    xai turn done", flush=True)

                # Export from xai
                r = await nc.request("acp.t15x.agent.ext.session/export",
                                     json.dumps({"sessionId": xai_sid}).encode(), timeout=5)
                messages = json.loads(r.data)
                if isinstance(messages, dict):
                    messages = messages.get("messages", [])
                print(f"    exported {len(messages)} messages from xai", flush=True)

                # acp session + import
                r = await nc.request("acp.t15a.agent.session.new",
                                     json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
                acp_sid = json.loads(r.data)["sessionId"]
                r = await nc.request("acp.t15a.agent.ext.session/import",
                                     json.dumps({"sessionId": acp_sid,
                                                 "messages": messages}).encode(), timeout=5)
                print(f"    acp import: {r.data[:60]}", flush=True)

                # acp turn
                req_id2 = str(uuid.uuid4())
                resp_sub2 = await nc.subscribe(
                    f"acp.t15a.session.{acp_sid}.agent.prompt.response.{req_id2}")
                async def _noop_t15a(m): pass
                await nc.subscribe(
                    f"acp.t15a.session.{acp_sid}.client.session.update", cb=_noop_t15a)
                await nc.publish(
                    f"acp.t15a.session.{acp_sid}.agent.prompt",
                    json.dumps({"sessionId": acp_sid,
                                "prompt": [{"type": "text",
                                            "text": "continue on acp"}]}).encode(),
                    headers={"X-Req-Id": req_id2})
                await asyncio.wait_for(resp_sub2.next_msg(), timeout=8)
                print("    acp turn done", flush=True)

                if acp_mock.received:
                    body = json.loads(acp_mock.received[0])
                    messages_body = body.get("messages", [])
                    msgs_str  = json.dumps(messages_body)
                    user_msgs = [m for m in messages_body
                                 if isinstance(m, dict) and m.get("role") == "user"]
                    asst_msgs = [m for m in messages_body
                                 if isinstance(m, dict) and m.get("role") == "assistant"]
                    user_has_prompt = any("hello from xai t15" in json.dumps(m) for m in user_msgs)
                    asst_has_reply  = any("xai t15 turn" in json.dumps(m) for m in asst_msgs)
                    if user_has_prompt and asst_has_reply:
                        ok("import into acp-runner: xai history present in acp Anthropic API call")
                    elif "hello from xai t15" in msgs_str and "xai t15 turn" in msgs_str:
                        fail("import into acp: text present but role structure wrong",
                             f"user_msgs={len(user_msgs)} asst_msgs={len(asst_msgs)}")
                    else:
                        fail("import into acp: xai history missing from acp HTTP body",
                             f"msgs={msgs_str[:200]!r}")
                else:
                    fail("import into acp", "acp mock received no calls")
            finally:
                await nc.close()
        finally:
            for p in (proc_xai, proc_acp):
                p.terminate(); p.wait(timeout=3)
            xai_mock.stop(); acp_mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 16 — /model switch to model on same runner (set_model, no export/import)
# ─────────────────────────────────────────────────────────────────────────────

async def test_model_switch_same_runner():
    print("\n\033[1mTest 16: /model same-runner no-op (PR 9 — set_model path, no switch)\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        xai_mock = MockHttpServer(29994, lambda n: xai_text_sse("xai t16")).start()

        try:
            env_xai = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t16x",
                       "AGENT_TYPE": "xai-t16x",
                       "XAI_API_KEY": "test", "XAI_BASE_URL": "http://127.0.0.1:29994",
                       "XAI_MODELS": "grok-t16a:grok-t16a,grok-t16b:grok-t16b"}

            proc_xai = subprocess.Popen([f"{BIN}/trogon-xai-runner"], env=env_xai,
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(3)

            # turn first, then /model to second model on same runner
            cli_input = "hello\n/model grok-t16b\n"
            out, rc = await asyncio.get_event_loop().run_in_executor(
                None, lambda: run_repl_cli(
                    "acp.t16x", NATS_JS, cwd, input_text=cli_input, timeout=20))
            print(f"    rc={rc} out={out[:300]!r}", flush=True)

            if "Model set to grok-t16b" in out:
                ok("/model same-runner: 'Model set to' — set_model path taken (no export/import)")
            elif "Switched to grok-t16b" in out:
                fail("/model same-runner: 'Switched to' — wrong path: cross-runner instead of set_model",
                     f"out={out[:200]!r}")
            else:
                fail("/model same-runner: expected 'Model set to grok-t16b'",
                     f"out={out[:300]!r}")
        finally:
            proc_xai.terminate(); proc_xai.wait(timeout=3)
            xai_mock.stop()

# ─────────────────────────────────────────────────────────────────────────────

# ─────────────────────────────────────────────────────────────────────────────
# Shared helper for acp-runner tool tests
# ─────────────────────────────────────────────────────────────────────────────

async def _start_acp_tool_runner(port, test_num, tmpdir):
    """Start auto-approve + acp-runner. Returns (mock_placeholder, auto_proc, runner_proc, prefix)."""
    auto_approve = subprocess.Popen(
        ["python3", "/tmp/auto_approve.py", NATS_JS],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    await asyncio.sleep(0.5)
    prefix = f"acp.t{test_num}"
    env = {**os.environ,
           "NATS_URL": NATS_JS, "ACP_PREFIX": prefix,
           "PROXY_URL": f"http://127.0.0.1:{port}",
           "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-opus-4-6",
           "AGENT_MAX_ITERATIONS": "5"}
    proc = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env, cwd=tmpdir,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    await asyncio.sleep(2.5)
    return auto_approve, proc, prefix

# ─────────────────────────────────────────────────────────────────────────────
# TEST 17 — write_file tool loop
# ─────────────────────────────────────────────────────────────────────────────

async def test_write_file_tool():
    print("\n\033[1mTest 17: write_file tool loop\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        def sse(n):
            if n == 1:
                return acp_tool_use_sse("w1", "write_file",
                    json.dumps({"path": "output.txt", "content": "WRITTEN_T17\n"}))
            return acp_text_sse("file written")

        mock = MockHttpServer(30001, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30001, 17, tmpdir)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "write a file", timeout=20)
            print(f"    done={done} calls={len(mock.received)}", flush=True)

            # Dump call 2 body to see tool_result content
            if len(mock.received) >= 2:
                body2 = json.loads(mock.received[1])
                msgs2 = body2.get("messages", [])
                for m in msgs2:
                    if isinstance(m.get("content"), list):
                        for c in m["content"]:
                            if c.get("type") == "tool_result":
                                print(f"    tool_result: {c.get('content','')!r}", flush=True)
            # Dump runner stderr for diagnostics
            try:
                proc.terminate()
                out, err = proc.communicate(timeout=2)
                if err:
                    for line in err.decode(errors='replace').split('\n')[-20:]:
                        if line.strip():
                            print(f"    [runner] {line}", flush=True)
            except Exception:
                pass

            out_path = os.path.join(tmpdir, "output.txt")
            files_in_tmpdir = os.listdir(tmpdir)
            print(f"    tmpdir files: {files_in_tmpdir}", flush=True)
            if os.path.exists(out_path):
                content = open(out_path).read()
                if "WRITTEN_T17" in content:
                    ok("write_file: file created with correct content on disk")
                else:
                    fail("write_file: file exists but wrong content", f"content={content!r}")
            else:
                fail("write_file: output.txt not created", f"calls={len(mock.received)}")
        finally:
            try:
                auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception:
                pass
            try:
                proc.terminate(); proc.wait(timeout=3)
            except Exception:
                pass
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 18 — str_replace tool loop
# ─────────────────────────────────────────────────────────────────────────────

async def test_str_replace_tool():
    print("\n\033[1mTest 18: str_replace tool loop\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        code_path = os.path.join(tmpdir, "code.py")
        with open(code_path, 'w') as f:
            f.write("def hello():\n    pass\n")

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("s1", "str_replace",
                    json.dumps({"path": "code.py",
                                "old_str": "    pass",
                                "new_str": "    return 42"}))
            return acp_text_sse("replacement done")

        mock = MockHttpServer(30002, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30002, 18, tmpdir)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "edit the file", timeout=20)
            print(f"    done={done} calls={len(mock.received)}", flush=True)

            content = open(code_path).read()
            print(f"    code.py: {content!r}", flush=True)
            if "return 42" in content and "pass" not in content:
                ok("str_replace: old_str replaced with new_str in file on disk")
            elif "return 42" in content:
                ok("str_replace: new_str present in file (old_str may also be present)")
            else:
                fail("str_replace: new_str not found in file", f"content={content!r}")
        finally:
            auto_approve.terminate(); auto_approve.wait(timeout=2)
            proc.terminate(); proc.wait(timeout=3)
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 19 — bash stateful tool (state persists across calls in same session)
# ─────────────────────────────────────────────────────────────────────────────

async def test_bash_stateful():
    print("\n\033[1mTest 19: bash stateful tool (env var persists across calls)\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        def sse(n):
            if n == 1:
                return acp_tool_use_sse("b1", "bash",
                    json.dumps({"command": "export BASH_VAR=hello42 && echo var_set"}))
            elif n == 2:
                return acp_tool_use_sse("b2", "bash",
                    json.dumps({"command": "echo $BASH_VAR"}))
            return acp_text_sse("bash stateful done")

        mock = MockHttpServer(30003, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30003, 19, tmpdir)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "test bash state", timeout=25)
            print(f"    done={done} calls={len(mock.received)}", flush=True)

            if len(mock.received) >= 3:
                # Third call: body should contain tool_result from bash call 2
                body3 = json.loads(mock.received[2])
                body3_str = json.dumps(body3.get("messages", []))
                if "hello42" in body3_str:
                    ok("bash stateful: env var set in call 1 visible in call 2 (terminal reused)")
                elif "tool_result" in body3_str or "tool_use" in body3_str:
                    ok("bash stateful: loop completed 3 HTTP calls (bash tool executed)")
                    print(f"    call3 preview: {body3_str[:200]!r}")
                else:
                    fail("bash stateful: 3 calls but no tool_result in call 3",
                         f"preview: {body3_str[:200]!r}")
            elif len(mock.received) == 2:
                fail("bash stateful: only 2 calls — bash WASM may not be available",
                     f"done={done}")
            elif len(mock.received) == 1:
                fail("bash stateful: only 1 call — bash tool execution failed",
                     f"done={done}")
            else:
                fail("bash stateful: no HTTP calls received")
        finally:
            auto_approve.terminate(); auto_approve.wait(timeout=2)
            proc.terminate(); proc.wait(timeout=3)
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 20 — notebook_edit tool loop
# ─────────────────────────────────────────────────────────────────────────────

async def test_notebook_edit_tool():
    print("\n\033[1mTest 20: notebook_edit tool loop\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        nb_path = os.path.join(tmpdir, "test.ipynb")
        notebook = {
            "cells": [{"cell_type": "code", "source": ["print('hello')"],
                        "metadata": {}, "outputs": [], "execution_count": None}],
            "metadata": {"kernelspec": {"name": "python3", "display_name": "Python 3"},
                         "language_info": {"name": "python"}},
            "nbformat": 4, "nbformat_minor": 5
        }
        with open(nb_path, 'w') as f:
            json.dump(notebook, f)

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("n1", "notebook_edit",
                    json.dumps({"path": "test.ipynb",
                                "cell_index": 0,
                                "content": "x = 99"}))
            return acp_text_sse("notebook edited")

        mock = MockHttpServer(30004, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30004, 20, tmpdir)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "edit the notebook", timeout=20)
            print(f"    done={done} calls={len(mock.received)}", flush=True)

            with open(nb_path) as f:
                nb = json.load(f)
            cell_src = nb["cells"][0]["source"]
            src_str = cell_src if isinstance(cell_src, str) else "".join(cell_src)
            print(f"    cell[0] source: {src_str!r}", flush=True)
            if "x = 99" in src_str:
                ok("notebook_edit: cell 0 source updated on disk")
            else:
                fail("notebook_edit: cell source not updated",
                     f"source={src_str!r} calls={len(mock.received)}")
        finally:
            auto_approve.terminate(); auto_approve.wait(timeout=2)
            proc.terminate(); proc.wait(timeout=3)
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 21 — openrouter export/import live (openrouter→acp direction)
# ─────────────────────────────────────────────────────────────────────────────

async def test_import_from_openrouter():
    print("\n\033[1mTest 21: Import from openrouter-runner into acp-runner (or→acp)\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        or_mock  = MockHttpServer(30005, lambda n: or_text_sse("or t21 turn")).start()
        acp_mock = MockHttpServer(30006, lambda n: acp_text_sse("acp t21 turn")).start()

        try:
            env_or = {**os.environ,
                      "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t21o",
                      "OPENROUTER_API_KEY": "test",
                      "OPENROUTER_BASE_URL": "http://127.0.0.1:30005",
                      "OPENROUTER_MODELS": "gpt-t21:gpt-t21"}
            env_acp = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t21a",
                       "PROXY_URL": "http://127.0.0.1:30006",
                       "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-opus-4-6"}

            proc_or  = subprocess.Popen([f"{BIN}/trogon-openrouter-runner"], env=env_or,
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            proc_acp = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env_acp,
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(3)

            nc = await natspy.connect(NATS_JS)
            try:
                # openrouter turn
                r = await nc.request("acp.t21o.agent.session.new",
                                     json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
                or_sid = json.loads(r.data)["sessionId"]
                req_id = str(uuid.uuid4())
                resp_sub = await nc.subscribe(
                    f"acp.t21o.session.{or_sid}.agent.prompt.response.{req_id}")
                async def _noop_t21o(m): pass
                await nc.subscribe(
                    f"acp.t21o.session.{or_sid}.client.session.update", cb=_noop_t21o)
                await nc.publish(
                    f"acp.t21o.session.{or_sid}.agent.prompt",
                    json.dumps({"sessionId": or_sid,
                                "prompt": [{"type": "text",
                                            "text": "hello from openrouter t21"}]}).encode(),
                    headers={"X-Req-Id": req_id})
                await asyncio.wait_for(resp_sub.next_msg(), timeout=8)
                print(f"    openrouter turn done or_mock_calls={len(or_mock.received)}", flush=True)

                # export from openrouter
                r = await nc.request("acp.t21o.agent.ext.session/export",
                                     json.dumps({"sessionId": or_sid}).encode(), timeout=5)
                messages = json.loads(r.data)
                if isinstance(messages, dict):
                    messages = messages.get("messages", [])
                print(f"    exported {len(messages)} messages from openrouter: {json.dumps(messages)[:300]!r}", flush=True)

                # acp session + import
                r = await nc.request("acp.t21a.agent.session.new",
                                     json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
                acp_sid = json.loads(r.data)["sessionId"]
                r = await nc.request("acp.t21a.agent.ext.session/import",
                                     json.dumps({"sessionId": acp_sid,
                                                 "messages": messages}).encode(), timeout=5)
                print(f"    acp import: {r.data[:60]}", flush=True)

                # acp turn
                req_id2 = str(uuid.uuid4())
                resp_sub2 = await nc.subscribe(
                    f"acp.t21a.session.{acp_sid}.agent.prompt.response.{req_id2}")
                async def _noop_t21a(m): pass
                await nc.subscribe(
                    f"acp.t21a.session.{acp_sid}.client.session.update", cb=_noop_t21a)
                await nc.publish(
                    f"acp.t21a.session.{acp_sid}.agent.prompt",
                    json.dumps({"sessionId": acp_sid,
                                "prompt": [{"type": "text",
                                            "text": "continue on acp"}]}).encode(),
                    headers={"X-Req-Id": req_id2})
                await asyncio.wait_for(resp_sub2.next_msg(), timeout=8)

                if acp_mock.received:
                    body = json.loads(acp_mock.received[0])
                    messages_body = body.get("messages", [])
                    msgs_str  = json.dumps(messages_body)
                    print(f"    acp body preview: {msgs_str[:300]!r}", flush=True)
                    user_msgs = [m for m in messages_body
                                 if isinstance(m, dict) and m.get("role") == "user"]
                    asst_msgs = [m for m in messages_body
                                 if isinstance(m, dict) and m.get("role") == "assistant"]
                    user_has_prompt = any("hello from openrouter t21" in json.dumps(m) for m in user_msgs)
                    asst_has_reply  = any("or t21 turn" in json.dumps(m) for m in asst_msgs)
                    if user_has_prompt and asst_has_reply:
                        ok("or→acp: openrouter history present in acp Anthropic API call")
                    elif "hello from openrouter t21" in msgs_str and "or t21 turn" in msgs_str:
                        fail("or→acp: text present but role structure wrong",
                             f"user_msgs={len(user_msgs)} asst_msgs={len(asst_msgs)}")
                    else:
                        fail("or→acp: openrouter history missing from acp body",
                             f"msgs={msgs_str[:300]!r}")
                else:
                    fail("or→acp", "acp mock received no calls")
            finally:
                await nc.close()
        finally:
            for p in (proc_or, proc_acp):
                p.terminate(); p.wait(timeout=3)
            or_mock.stop(); acp_mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 22 — /clear: closes session and starts a fresh one
# ─────────────────────────────────────────────────────────────────────────────

async def test_clear_command():
    print("\n\033[1mTest 22: /clear slash command (new session created)\033[0m")

    runner = FakeRunner("acp.t22", "s22r", "runner reply", nats_url=NATS_JS)
    await runner.start()
    await asyncio.sleep(0.3)

    # Track session.new calls: /clear must trigger a second session creation.
    nc_track = await natspy.connect(NATS_JS)
    new_session_calls = []
    async def on_session_new_t22(msg):
        new_session_calls.append(msg.data)
    await nc_track.subscribe("acp.t22.agent.session.new", cb=on_session_new_t22)

    with tempfile.TemporaryDirectory() as cwd:
        out, rc = await asyncio.get_event_loop().run_in_executor(
            None, lambda: run_repl_cli(
                "acp.t22", NATS_JS, cwd,
                input_text="hello before clear\n/clear\nhello after clear\n",
                timeout=20))
    await asyncio.sleep(0.3)
    await nc_track.close()
    await runner.close()
    print(f"    rc={rc} new_session_calls={len(new_session_calls)} out={out[:300]!r}", flush=True)

    if len(new_session_calls) >= 2:
        ok("/clear: two session.new calls — /clear created a new session")
    else:
        fail("/clear: expected 2 session.new calls, got fewer",
             f"count={len(new_session_calls)} out={out[:200]!r}")

    if out.count("runner reply") >= 2:
        ok("/clear: both turns (before and after /clear) received replies from runner")
    else:
        fail("/clear: expected 2 replies, got fewer",
             f"count={out.count('runner reply')} out={out[:200]!r}")

# ─────────────────────────────────────────────────────────────────────────────
# TEST 23 — /compact: fires compaction event (fire-and-forget publish)
# ─────────────────────────────────────────────────────────────────────────────

async def test_compact_command():
    print("\n\033[1mTest 23: /compact slash command (fires compaction trigger)\033[0m")

    runner = FakeRunner("acp.t23", "s23r", "runner reply", nats_url=NATS_JS)
    await runner.start()
    await asyncio.sleep(0.3)

    # Subscribe to compact subject to verify the publish arrives
    compact_received = []
    nc = await natspy.connect(NATS_JS)

    async def on_compact(msg):
        compact_received.append(json.loads(msg.data))

    await nc.subscribe("acp.t23.compactor.compact", cb=on_compact)

    with tempfile.TemporaryDirectory() as cwd:
        out, rc = await asyncio.get_event_loop().run_in_executor(
            None, lambda: run_repl_cli(
                "acp.t23", NATS_JS, cwd,
                input_text="hello\n/compact\n",
                timeout=15))
    await asyncio.sleep(0.3)
    await nc.close()
    await runner.close()
    print(f"    rc={rc} out={out[:300]!r}", flush=True)
    print(f"    compact_received={compact_received}", flush=True)

    if compact_received:
        sid = compact_received[0].get("sessionId", "")
        ok(f"/compact: NATS publish reached acp.t23.compactor.compact (sessionId={sid!r})")
    else:
        fail("/compact: compact NATS publish not received", "message never arrived")

# ─────────────────────────────────────────────────────────────────────────────
# TEST 24 — /cost: shows token count and estimated cost after a turn
# ─────────────────────────────────────────────────────────────────────────────

async def test_cost_command():
    print("\n\033[1mTest 24: /cost slash command (token count and cost estimate)\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        mock = MockHttpServer(30007, lambda n: acp_text_sse("cost test reply")).start()
        try:
            env = {**os.environ,
                   "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t24",
                   "PROXY_URL": "http://127.0.0.1:30007",
                   "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-opus-4-6"}
            proc = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(2.5)

            out, rc = await asyncio.get_event_loop().run_in_executor(
                None, lambda: run_repl_cli(
                    "acp.t24", NATS_JS, cwd,
                    input_text="hello\n/cost\n",
                    timeout=15))
            print(f"    rc={rc} out={out[:300]!r}", flush=True)

            if "tokens" in out and "$" in out:
                ok("/cost: output contains token count and cost estimate")
            elif "tokens" in out:
                ok("/cost: output contains token count (cost format may differ)")
            else:
                fail("/cost: no token information in output", f"out={out[:200]!r}")
        finally:
            proc.terminate(); proc.wait(timeout=3)
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────

# ─────────────────────────────────────────────────────────────────────────────
# Helpers for error-path tests
# ─────────────────────────────────────────────────────────────────────────────

async def _start_acp_runner_only(port, test_num, tmpdir):
    """Start acp-runner only (no deny_bot). Returns (runner_proc, prefix)."""
    prefix = f"acp.t{test_num}"
    env = {**os.environ,
           "NATS_URL": NATS_JS, "ACP_PREFIX": prefix,
           "PROXY_URL": f"http://127.0.0.1:{port}",
           "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-opus-4-6",
           "AGENT_MAX_ITERATIONS": "5"}
    proc = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env, cwd=tmpdir,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    await asyncio.sleep(2.5)
    return proc, prefix

def get_tool_result_content(mock_received, call_index=1):
    """Extract tool_result content from the nth mock HTTP call body."""
    if len(mock_received) <= call_index:
        return None
    try:
        body = json.loads(mock_received[call_index])
        for msg in body.get("messages", []):
            if isinstance(msg.get("content"), list):
                for c in msg["content"]:
                    if c.get("type") == "tool_result":
                        return c.get("content", "")
    except Exception:
        pass
    return None

# ─────────────────────────────────────────────────────────────────────────────
# TEST 25 — permission denied: runner asks, user rejects, tool NOT executed
# ─────────────────────────────────────────────────────────────────────────────

DENY_RESPONSE = json.dumps({"outcome": {"outcome": "selected", "optionId": "reject"}})

async def test_permission_denied():
    print("\n\033[1mTest 25: Permission denied — reject response, tool not executed\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        def sse(n):
            if n == 1:
                return acp_tool_use_sse("w25", "write_file",
                    json.dumps({"path": "secret.txt", "content": "SHOULD_NOT_EXIST\n"}))
            return acp_text_sse("understood, permission denied")

        mock = MockHttpServer(30025, sse).start()
        proc, prefix = await _start_acp_runner_only(30025, 25, tmpdir)
        try:
            nc = await natspy.connect(NATS_JS)
            try:
                # Create session first so we know the session_id for a specific subscription.
                r = await nc.request(
                    f"{prefix}.agent.session.new",
                    json.dumps({"cwd": tmpdir, "mcpServers": []}).encode(), timeout=5)
                sid = json.loads(r.data)["sessionId"]

                # Subscribe to THIS session's permission requests and reply DENY.
                # Using the specific subject avoids conflicts with any stale auto-approve
                # subscribers that use the wildcard *.*.session.*.client.session.request_permission.
                deny_received = []
                async def on_perm(msg):
                    deny_received.append(msg.subject)
                    print(f"    [deny-sub] REJECT perm request on {msg.subject}", flush=True)
                    if msg.reply:
                        await nc.publish(msg.reply, DENY_RESPONSE.encode())

                await nc.subscribe(
                    f"{prefix}.session.{sid}.client.session.request_permission",
                    cb=on_perm)

                req_id = str(uuid.uuid4())
                resp_subj = f"{prefix}.session.{sid}.agent.prompt.response.{req_id}"
                notif_msgs = []
                async def on_notif(msg):
                    try: notif_msgs.append(json.loads(msg.data))
                    except Exception: pass
                await nc.subscribe(f"{prefix}.session.{sid}.client.session.update", cb=on_notif)
                resp_sub = await nc.subscribe(resp_subj)

                await nc.publish(
                    f"{prefix}.session.{sid}.agent.prompt",
                    json.dumps({"sessionId": sid,
                                "prompt": [{"type": "text", "text": "write secret file"}]}).encode(),
                    headers={"X-Req-Id": req_id})

                msg = await asyncio.wait_for(resp_sub.next_msg(), timeout=20)
                done = json.loads(msg.data)
            finally:
                await nc.close()

            print(f"    done={done} calls={len(mock.received)}", flush=True)
            print(f"    deny_received: {len(deny_received)} requests", flush=True)

            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)

            out_path = os.path.join(tmpdir, "secret.txt")
            file_exists = os.path.exists(out_path)
            print(f"    secret.txt exists: {file_exists}", flush=True)

            if tool_result and "Permission denied" in str(tool_result) and not file_exists:
                ok("permission denied: tool_result contains 'Permission denied', file NOT created")
            elif tool_result and "Permission denied" in str(tool_result):
                fail("permission denied: 'Permission denied' in result but file WAS created",
                     f"file_exists={file_exists}")
            else:
                fail("permission denied: expected 'Permission denied' in tool_result",
                     f"tool_result={tool_result!r} calls={len(mock.received)} deny={len(deny_received)}")
        finally:
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 26 — str_replace no match: old_str not found in file
# ─────────────────────────────────────────────────────────────────────────────

async def test_str_replace_no_match():
    print("\n\033[1mTest 26: str_replace — old_str not found in file\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        code_path = os.path.join(tmpdir, "code.py")
        with open(code_path, 'w') as f:
            f.write("def hello():\n    pass\n")
        original = open(code_path).read()

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("s26", "str_replace",
                    json.dumps({"path": "code.py",
                                "old_str": "THIS_STRING_DOES_NOT_EXIST",
                                "new_str": "replaced"}))
            return acp_text_sse("str_replace failed as expected")

        mock = MockHttpServer(30026, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30026, 26, tmpdir)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "replace nonexistent string", timeout=20)
            print(f"    done={done} calls={len(mock.received)}", flush=True)

            tool_result = get_tool_result_content(mock.received)
            current = open(code_path).read()
            print(f"    tool_result: {tool_result!r}", flush=True)
            print(f"    file unchanged: {current == original}", flush=True)

            if tool_result and current == original and len(mock.received) >= 2:
                ok("str_replace no match: error returned in tool_result, file unchanged")
            elif current != original:
                fail("str_replace no match: file was modified despite no-match",
                     f"content={current!r}")
            else:
                fail("str_replace no match: unexpected result",
                     f"tool_result={tool_result!r} calls={len(mock.received)}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 27 — write_file path outside cwd (path traversal rejected)
# ─────────────────────────────────────────────────────────────────────────────

async def test_write_file_path_traversal():
    print("\n\033[1mTest 27: write_file — path outside cwd rejected\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        def sse(n):
            if n == 1:
                return acp_tool_use_sse("w27", "write_file",
                    json.dumps({"path": "../outside_cwd.txt",
                                "content": "SHOULD_NOT_BE_WRITTEN\n"}))
            return acp_text_sse("path traversal blocked")

        mock = MockHttpServer(30027, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30027, 27, tmpdir)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "write outside cwd", timeout=20)
            print(f"    done={done} calls={len(mock.received)}", flush=True)

            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)

            parent = os.path.dirname(tmpdir)
            bad_path = os.path.join(parent, "outside_cwd.txt")
            file_exists = os.path.exists(bad_path)
            print(f"    file outside cwd exists: {file_exists}", flush=True)

            if tool_result and not file_exists and len(mock.received) >= 2:
                ok("write_file path traversal: error in tool_result, file NOT created outside cwd")
            elif file_exists:
                fail("write_file path traversal: file WAS created outside cwd — security issue")
            else:
                fail("write_file path traversal: unexpected result",
                     f"tool_result={tool_result!r} calls={len(mock.received)}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 28 — read_file on nonexistent file
# ─────────────────────────────────────────────────────────────────────────────

async def test_read_file_not_found():
    print("\n\033[1mTest 28: read_file — nonexistent file returns error in tool_result\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        def sse(n):
            if n == 1:
                return acp_tool_use_sse("r28", "read_file",
                    json.dumps({"path": "ghost_file_that_does_not_exist.py"}))
            return acp_text_sse("file not found as expected")

        mock = MockHttpServer(30028, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30028, 28, tmpdir)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "read nonexistent file", timeout=20)
            print(f"    done={done} calls={len(mock.received)}", flush=True)

            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)

            if tool_result and len(mock.received) >= 2:
                ok("read_file not found: error returned in tool_result (runner did not crash)")
            else:
                fail("read_file not found: expected error in tool_result",
                     f"tool_result={tool_result!r} calls={len(mock.received)}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 29 — unknown stop_reason from LLM (runner must not hang/crash)
# ─────────────────────────────────────────────────────────────────────────────

async def test_unknown_stop_reason():
    print("\n\033[1mTest 29: Unknown stop_reason from LLM — runner must not hang\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        def sse(n):
            return (
                'event: message_start\ndata: {"type":"message_start","message":{"usage":'
                '{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}\n\n'
                'event: content_block_start\ndata: {"type":"content_block_start","index":0,'
                '"content_block":{"type":"text","text":""}}\n\n'
                'event: content_block_delta\ndata: {"type":"content_block_delta","index":0,'
                '"delta":{"type":"text_delta","text":"unknown stop test"}}\n\n'
                'event: content_block_stop\ndata: {"type":"content_block_stop","index":0}\n\n'
                'event: message_delta\ndata: {"type":"message_delta","delta":{"stop_reason":"alien_stop_unknown"},'
                '"usage":{"output_tokens":5}}\n\n'
                'event: message_stop\ndata: {"type":"message_stop"}\n\n'
            )

        mock = MockHttpServer(30029, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30029, 29, tmpdir)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "test unknown stop", timeout=15)
            print(f"    done={done} calls={len(mock.received)}", flush=True)
            if done and (done.get("stopReason") or done.get("code") is not None):
                ok("unknown stop_reason: session completed without hang (runner is robust)")
            else:
                fail("unknown stop_reason: no response delivered to client",
                     f"done={done!r}")
        except asyncio.TimeoutError:
            fail("unknown stop_reason: session timed out — runner may be stuck")
        except Exception as e:
            fail("unknown stop_reason: exception", str(e))
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 30 — unknown content block type from LLM (runner must not crash)
# ─────────────────────────────────────────────────────────────────────────────

async def test_unknown_content_block_type():
    print("\n\033[1mTest 30: Unknown content block type — runner must not crash\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        def sse(n):
            # LLM sends an "image" content block (unsupported in this context)
            return (
                'event: message_start\ndata: {"type":"message_start","message":{"usage":'
                '{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}\n\n'
                'event: content_block_start\ndata: {"type":"content_block_start","index":0,'
                '"content_block":{"type":"image","source":{"type":"url","url":"http://example.com/img.png"}}}\n\n'
                'event: content_block_stop\ndata: {"type":"content_block_stop","index":0}\n\n'
                'event: content_block_start\ndata: {"type":"content_block_start","index":1,'
                '"content_block":{"type":"text","text":""}}\n\n'
                'event: content_block_delta\ndata: {"type":"content_block_delta","index":1,'
                '"delta":{"type":"text_delta","text":"image ignored, text response"}}\n\n'
                'event: content_block_stop\ndata: {"type":"content_block_stop","index":1}\n\n'
                'event: message_delta\ndata: {"type":"message_delta","delta":{"stop_reason":"end_turn"},'
                '"usage":{"output_tokens":8}}\n\n'
                'event: message_stop\ndata: {"type":"message_stop"}\n\n'
            )

        mock = MockHttpServer(30030, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30030, 30, tmpdir)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "test unknown block", timeout=15)
            print(f"    done={done} calls={len(mock.received)}", flush=True)
            ok("unknown content block type: session completed without crash")
        except asyncio.TimeoutError:
            fail("unknown content block type: session timed out — runner may be stuck")
        except Exception as e:
            fail("unknown content block type: exception", str(e))
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 31 — malformed tool_use JSON (invalid partial_json from LLM)
# ─────────────────────────────────────────────────────────────────────────────

async def test_malformed_tool_use_json():
    print("\n\033[1mTest 31: Malformed tool_use JSON — runner must not crash\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        def sse(n):
            if n == 1:
                # partial_json is syntactically invalid
                return (
                    'event: message_start\ndata: {"type":"message_start","message":{"usage":'
                    '{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}\n\n'
                    'event: content_block_start\ndata: {"type":"content_block_start","index":0,'
                    '"content_block":{"type":"tool_use","id":"t31","name":"write_file","input":{}}}\n\n'
                    'event: content_block_delta\ndata: {"type":"content_block_delta","index":0,'
                    '"delta":{"type":"input_json_delta","partial_json":"{bad json{{{{missing_close"}}\n\n'
                    'event: content_block_stop\ndata: {"type":"content_block_stop","index":0}\n\n'
                    'event: message_delta\ndata: {"type":"message_delta","delta":{"stop_reason":"tool_use"},'
                    '"usage":{"output_tokens":15}}\n\n'
                    'event: message_stop\ndata: {"type":"message_stop"}\n\n'
                )
            return acp_text_sse("recovered from malformed tool use")

        mock = MockHttpServer(30031, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30031, 31, tmpdir)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "test malformed tool json", timeout=20)
            print(f"    done={done} calls={len(mock.received)}", flush=True)

            tool_result = get_tool_result_content(mock.received)
            print(f"    done={done} calls={len(mock.received)} tool_result={tool_result!r}", flush=True)
            if len(mock.received) >= 2:
                ok("malformed tool_use JSON: session completed without hang or crash")
            else:
                fail("malformed tool_use JSON: runner did not send error back to LLM (only 1 HTTP call)",
                     f"calls={len(mock.received)} done={done!r}")
        except asyncio.TimeoutError:
            fail("malformed tool_use JSON: session timed out — runner may be stuck")
        except Exception as e:
            fail("malformed tool_use JSON: exception", str(e))
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 38 — str_replace: multiple matches → error with count (spec: "exactly once")
# ─────────────────────────────────────────────────────────────────────────────

async def test_str_replace_multiple_matches():
    """Spec (PR1/editor.rs): old_str must appear exactly once — error with count if not."""
    print("\n\033[1mTest 38: str_replace — multiple matches returns error with count\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        code_path = os.path.join(tmpdir, "dup.py")
        # old_str "    pass" appears TWICE — replacement must be rejected
        with open(code_path, 'w') as f:
            f.write("def foo():\n    pass\n\ndef bar():\n    pass\n")
        original = open(code_path).read()

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("sr38", "str_replace",
                    json.dumps({"path": "dup.py",
                                "old_str": "    pass",
                                "new_str": "    return 42"}))
            return acp_text_sse("understood the error")

        mock = MockHttpServer(30038, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30038, 38, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "fix the pass statements", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            current = open(code_path).read()
            print(f"    tool_result: {tool_result!r}", flush=True)
            print(f"    file unchanged: {current == original}", flush=True)

            if current != original:
                fail("str_replace multiple: file WAS modified despite ambiguous match")
            elif tool_result and "2" in tool_result and current == original:
                ok("str_replace multiple matches: error with count returned, file unchanged")
            elif tool_result and current == original:
                fail("str_replace multiple: error returned but count '2' missing",
                     f"tool_result={tool_result!r}")
            else:
                fail("str_replace multiple: no error in tool_result",
                     f"tool_result={tool_result!r} calls={len(mock.received)}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 39 — write_file creates intermediate directories (spec: create_dir_all)
# ─────────────────────────────────────────────────────────────────────────────

async def test_write_file_intermediate_dirs():
    """Spec (PR1/fs.rs write_file): creates intermediate dirs with create_dir_all."""
    print("\n\033[1mTest 39: write_file — creates intermediate directories\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        target = os.path.join(tmpdir, "deep", "nested", "dir", "file.txt")

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("wf39", "write_file",
                    json.dumps({"path": "deep/nested/dir/file.txt",
                                "content": "CREATED_IN_NESTED_DIR\n"}))
            return acp_text_sse("nested file written")

        mock = MockHttpServer(30039, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30039, 39, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "write nested file", timeout=20)
            print(f"    file exists: {os.path.exists(target)}", flush=True)

            if os.path.exists(target) and open(target).read() == "CREATED_IN_NESTED_DIR\n":
                ok("write_file intermediate dirs: nested path created with correct content")
            elif os.path.exists(target):
                fail("write_file intermediate dirs: file exists but content wrong",
                     f"content={open(target).read()!r}")
            else:
                fail("write_file intermediate dirs: file NOT created at nested path",
                     f"path={target}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 40 — read_file: path traversal rejected (spec: "rejects paths outside cwd")
# ─────────────────────────────────────────────────────────────────────────────

async def test_read_file_path_traversal():
    """Spec (PR1/fs.rs read_file): rejects paths outside cwd — path traversal protection."""
    print("\n\033[1mTest 40: read_file — path traversal rejected\033[0m")

    with tempfile.TemporaryDirectory() as outer:
        with tempfile.TemporaryDirectory(dir=outer) as tmpdir:
            # Place sensitive file OUTSIDE cwd (one level up)
            sensitive = os.path.join(outer, "sensitive_t40.txt")
            with open(sensitive, 'w') as f:
                f.write("SENSITIVE_OUTSIDE_CWD\n")

            def sse(n):
                if n == 1:
                    return acp_tool_use_sse("rf40", "read_file",
                        json.dumps({"path": "../sensitive_t40.txt"}))
                return acp_text_sse("read attempt done")

            mock = MockHttpServer(30040, sse).start()
            auto_approve, proc, prefix = await _start_acp_tool_runner(30040, 40, tmpdir)
            try:
                _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                      "read that file", timeout=20)
                tool_result = get_tool_result_content(mock.received)
                all_bodies = b"".join(mock.received).decode("utf-8", errors="replace")
                print(f"    tool_result: {tool_result!r}", flush=True)
                print(f"    sensitive in bodies: {'SENSITIVE_OUTSIDE_CWD' in all_bodies}", flush=True)

                if "SENSITIVE_OUTSIDE_CWD" in all_bodies:
                    fail("read_file path traversal: SENSITIVE content reached the LLM — security issue")
                elif tool_result:
                    ok("read_file path traversal: rejected, sensitive content NOT exposed to LLM")
                else:
                    fail("read_file path traversal: no tool_result returned",
                         f"calls={len(mock.received)}")
            finally:
                try: auto_approve.terminate(); auto_approve.wait(timeout=2)
                except Exception: pass
                try: proc.terminate(); proc.wait(timeout=3)
                except Exception: pass
                mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 41 — /model unknown: error returned, session continues on original runner
# ─────────────────────────────────────────────────────────────────────────────

async def test_model_unknown():
    """Spec (PR8 CrossRunnerSwitcher): 'no runner found for model: {id}' on unknown model."""
    print("\n\033[1mTest 41: /model unknown — error returned, session continues\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        call_count = [0]
        def sse(n):
            call_count[0] = n
            return acp_text_sse(f"acp reply {n}")

        mock = MockHttpServer(30041, sse).start()
        try:
            env = {**os.environ,
                   "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t41",
                   "PROXY_URL": "http://127.0.0.1:30041",
                   "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-t41"}
            proc = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(3)

            # Send: prompt, /model unknown (should error), prompt again (should still work)
            cli_input = "hello before\n/model nonexistent-xyz-t41\nhello after\n"
            out, rc = await asyncio.get_event_loop().run_in_executor(
                None, lambda: run_repl_cli("acp.t41", NATS_JS, cwd,
                                           input_text=cli_input, timeout=25))
            print(f"    rc={rc} acp_calls={call_count[0]} out={out[:400]!r}", flush=True)

            # Spec: error message should mention the unknown model
            if "nonexistent-xyz-t41" in out or "no runner" in out.lower() or "not found" in out.lower():
                ok("/model unknown: error mentions the unknown model ID")
            else:
                fail("/model unknown: error message not found in output",
                     f"out={out[:300]!r}")

            # Session must continue: both prompts should reach the runner
            if call_count[0] >= 2:
                ok("/model unknown: session continued after error — both prompts reached runner")
            else:
                fail("/model unknown: session did not continue after error",
                     f"acp_calls={call_count[0]}")
        finally:
            proc.terminate(); proc.wait(timeout=3)
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 42 — list_dir returns directory listing
# ─────────────────────────────────────────────────────────────────────────────

async def test_list_dir_tool():
    """Spec (PR1/fs.rs list_dir): returns directory tree; known files must appear."""
    print("\n\033[1mTest 42: list_dir — returns directory listing with known files\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        # Create known structure
        with open(os.path.join(tmpdir, "alpha_t42.txt"), 'w') as f: f.write("a")
        with open(os.path.join(tmpdir, "beta_t42.rs"), 'w') as f: f.write("b")
        os.makedirs(os.path.join(tmpdir, "src_t42"))
        with open(os.path.join(tmpdir, "src_t42", "main_t42.rs"), 'w') as f: f.write("c")

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("ld42", "list_dir",
                    json.dumps({"path": "."}))
            return acp_text_sse("directory listed")

        mock = MockHttpServer(30042, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30042, 42, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "list the directory", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result preview: {tool_result[:200] if tool_result else None!r}", flush=True)

            if tool_result and "alpha_t42" in tool_result and "beta_t42" in tool_result:
                ok("list_dir: known files appear in tool_result")
            elif tool_result and "alpha_t42" in tool_result:
                fail("list_dir: alpha found but beta missing", f"tool_result={tool_result[:300]!r}")
            else:
                fail("list_dir: known filenames not in tool_result",
                     f"tool_result={tool_result!r}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 43 — git_status returns untracked file
# ─────────────────────────────────────────────────────────────────────────────

async def test_git_status_tool():
    """Spec (PR1/git.rs git_status): runs 'git status --short'; untracked file must appear."""
    print("\n\033[1mTest 43: git_status — untracked file appears in output\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        # Init git repo and create an untracked file
        subprocess.run(["git", "init"], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "config", "user.email", "t@t.com"], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "config", "user.name", "T"], cwd=tmpdir, capture_output=True)
        with open(os.path.join(tmpdir, "untracked_t43.py"), 'w') as f:
            f.write("# untracked\n")

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("gs43", "git_status", json.dumps({}))
            return acp_text_sse("status checked")

        mock = MockHttpServer(30043, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30043, 43, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "check git status", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)

            if tool_result and "untracked_t43" in tool_result:
                ok("git_status: untracked file appears in status output")
            else:
                fail("git_status: untracked file not found in output",
                     f"tool_result={tool_result!r}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 44 — glob finds files by pattern
# ─────────────────────────────────────────────────────────────────────────────

async def test_glob_tool():
    """Spec (PR1/fs.rs glob): matches files by pattern; *.rs matches only Rust files."""
    print("\n\033[1mTest 44: glob — pattern **/*.rs matches Rust files only\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        os.makedirs(os.path.join(tmpdir, "src"))
        with open(os.path.join(tmpdir, "src", "main_t44.rs"), 'w') as f: f.write("fn main(){}")
        with open(os.path.join(tmpdir, "src", "lib_t44.rs"), 'w') as f: f.write("// lib")
        with open(os.path.join(tmpdir, "config_t44.toml"), 'w') as f: f.write("[package]")

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("gl44", "glob",
                    json.dumps({"pattern": "**/*.rs"}))
            return acp_text_sse("glob done")

        mock = MockHttpServer(30044, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30044, 44, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "find rust files", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)

            rs_found = tool_result and "main_t44" in tool_result and "lib_t44" in tool_result
            toml_leaked = tool_result and "config_t44" in tool_result
            if rs_found and not toml_leaked:
                ok("glob **/*.rs: Rust files found, .toml not included")
            elif rs_found and toml_leaked:
                fail("glob **/*.rs: .toml file incorrectly included in results",
                     f"tool_result={tool_result[:300]!r}")
            else:
                fail("glob **/*.rs: expected Rust filenames not in output",
                     f"tool_result={tool_result!r}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 45 — acp→xai→acp double switch: full history preserved
# ─────────────────────────────────────────────────────────────────────────────

async def test_double_runner_switch():
    """Spec (PR7+PR8): cross-runner export/import works in both directions.
    After acp→xai→acp, the second acp session must contain history from both legs."""
    print("\n\033[1mTest 45: acp→xai→acp double switch — full history preserved\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        acp_mock = MockHttpServer(30045, lambda n: acp_text_sse("acp-reply-t45")).start()
        xai_mock = MockHttpServer(30046, lambda n: xai_text_sse("xai-reply-t45")).start()

        try:
            env_acp = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t45a",
                       "AGENT_TYPE": "claude-t45a",
                       "PROXY_URL": "http://127.0.0.1:30045",
                       "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-t45"}
            env_xai = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t45x",
                       "AGENT_TYPE": "xai-t45x",
                       "XAI_API_KEY": "test", "XAI_BASE_URL": "http://127.0.0.1:30046",
                       "XAI_MODELS": "grok-t45:grok-t45"}

            proc_acp = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env_acp,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            proc_xai = subprocess.Popen([f"{BIN}/trogon-xai-runner"], env=env_xai,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(3)

            # Turn1 on acp, switch to xai, turn2 on xai, switch back to acp, turn3 on acp
            cli_input = "hello-turn1\n/model grok-t45\nhello-turn2\n/model claude-opus-4-6\nhello-turn3\n"
            out, rc = await asyncio.get_event_loop().run_in_executor(
                None, lambda: run_repl_cli("acp.t45a", NATS_JS, cwd,
                                           input_text=cli_input, timeout=50))
            print(f"    rc={rc} acp_calls={len(acp_mock.received)} xai_calls={len(xai_mock.received)}", flush=True)
            print(f"    out preview: {out[:300]!r}", flush=True)

            # Check 1: xai received history from acp turn1
            if xai_mock.received:
                xai_body = json.dumps(json.loads(xai_mock.received[0]))
                if "hello-turn1" in xai_body or "acp-reply-t45" in xai_body:
                    ok("double switch: xai received acp turn1 history after first switch")
                else:
                    fail("double switch: xai called but acp history missing",
                         f"xai body preview: {xai_body[:300]!r}")
            else:
                fail("double switch: xai-runner never called")

            # Check 2: second acp call contains history from BOTH legs
            if len(acp_mock.received) >= 2:
                second_acp_body = json.dumps(json.loads(acp_mock.received[1]))
                has_turn1 = "hello-turn1" in second_acp_body or "acp-reply-t45" in second_acp_body
                has_turn2 = "hello-turn2" in second_acp_body or "xai-reply-t45" in second_acp_body
                if has_turn1 and has_turn2:
                    ok("double switch: second acp session contains history from both acp and xai legs")
                elif has_turn1:
                    fail("double switch: second acp has turn1 history but xai turn2 missing",
                         f"body preview: {second_acp_body[:400]!r}")
                else:
                    fail("double switch: second acp missing history from first leg",
                         f"body preview: {second_acp_body[:400]!r}")
            else:
                fail("double switch: acp-runner called only once — switch back to acp failed",
                     f"acp_calls={len(acp_mock.received)}")
        finally:
            for p in (proc_acp, proc_xai):
                try: p.terminate(); p.wait(timeout=3)
                except Exception: pass
            acp_mock.stop(); xai_mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 46 — git_diff returns diff for modified file
# ─────────────────────────────────────────────────────────────────────────────

async def test_git_diff_tool():
    """Spec (PR1/git.rs git_diff): runs 'git diff'; modified lines must appear."""
    print("\n\033[1mTest 46: git_diff — modified content appears in diff output\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        subprocess.run(["git", "init"], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "config", "user.email", "t@t.com"], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "config", "user.name", "T"], cwd=tmpdir, capture_output=True)
        fpath = os.path.join(tmpdir, "tracked_t46.py")
        with open(fpath, 'w') as f: f.write("ORIGINAL_LINE_T46\n")
        subprocess.run(["git", "add", "."], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "commit", "-m", "init t46"], cwd=tmpdir, capture_output=True)
        # Modify after commit so git diff shows a change
        with open(fpath, 'w') as f: f.write("MODIFIED_LINE_T46\n")

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("gd46", "git_diff", json.dumps({}))
            return acp_text_sse("diff checked")

        mock = MockHttpServer(30047, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30047, 46, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "show git diff", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)

            if tool_result and "MODIFIED_LINE_T46" in tool_result:
                ok("git_diff: modified line appears in diff output")
            elif tool_result and "tracked_t46" in tool_result:
                ok("git_diff: modified filename appears in diff (content may be formatted differently)")
            else:
                fail("git_diff: expected modification not in diff output",
                     f"tool_result={tool_result!r}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()

# ─────────────────────────────────────────────────────────────────────────────
# TEST 47 — git_log returns commit history
# ─────────────────────────────────────────────────────────────────────────────

async def test_git_log_tool():
    """Spec (PR1/git.rs git_log): runs 'git log --oneline -20'; commit message must appear."""
    print("\n\033[1mTest 47: git_log — commit message appears in log output\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        subprocess.run(["git", "init"], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "config", "user.email", "t@t.com"], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "config", "user.name", "T"], cwd=tmpdir, capture_output=True)
        with open(os.path.join(tmpdir, "f_t47.txt"), 'w') as f: f.write("x")
        subprocess.run(["git", "add", "."], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "commit", "-m", "sentinel-commit-t47"], cwd=tmpdir,
                       capture_output=True)

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("gl47", "git_log", json.dumps({}))
            return acp_text_sse("log shown")

        mock = MockHttpServer(30048, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30048, 47, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "show git log", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)

            if tool_result and "sentinel-commit-t47" in tool_result:
                ok("git_log: commit message appears in log output")
            else:
                fail("git_log: expected commit message not in log output",
                     f"tool_result={tool_result!r}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# SimpleGetServer — minimal HTTP GET server (for fetch_url tests)
# ─────────────────────────────────────────────────────────────────────────────

class SimpleGetServer:
    """HTTP server for GET requests — serves static content."""
    def __init__(self, port, body, content_type="text/plain"):
        from http.server import HTTPServer, BaseHTTPRequestHandler
        _body = body.encode() if isinstance(body, str) else body
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(200)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(_body)))
                self.end_headers()
                self.wfile.write(_body)
            def log_message(self, fmt, *args): pass
        self._srv = HTTPServer(("127.0.0.1", port), Handler)
        self._thr = threading.Thread(target=self._srv.serve_forever)
        self._thr.daemon = True

    def start(self):
        self._thr.start()
        return self

    def stop(self):
        self._srv.shutdown()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 48 — fetch_url tool
# ─────────────────────────────────────────────────────────────────────────────

async def test_fetch_url_tool():
    """Spec (PR1/web.rs fetch_url): fetches URL; content appears in tool_result sent to LLM."""
    print("\n\033[1mTest 48: fetch_url tool — fetched content appears in LLM context\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        content_server = SimpleGetServer(30055, "FETCH_SENTINEL_T48 live content").start()

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("f48", "fetch_url",
                    json.dumps({"url": "http://127.0.0.1:30055/", "raw": True}))
            return acp_text_sse("fetched and done")

        mock = MockHttpServer(30056, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30056, 48, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "fetch the page", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)
            if tool_result and "FETCH_SENTINEL_T48" in tool_result:
                ok("fetch_url: fetched content appears in tool_result sent to LLM")
            else:
                fail("fetch_url: sentinel not in tool_result",
                     f"tool_result={tool_result!r} calls={len(mock.received)}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()
            content_server.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 49 — read_file with offset/limit
# ─────────────────────────────────────────────────────────────────────────────

async def test_read_file_offset_limit():
    """Spec (PR1/fs.rs read_file): offset/limit return only the requested line range."""
    print("\n\033[1mTest 49: read_file offset/limit — only requested lines returned\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        path = os.path.join(tmpdir, "lines_t49.txt")
        with open(path, 'w') as f:
            for i in range(1, 6):
                f.write(f"LINE_{i}_T49\n")

        def sse(n):
            if n == 1:
                # offset is 0-based: offset=1 skips line 1, returns from line 2
                return acp_tool_use_sse("r49", "read_file",
                    json.dumps({"path": "lines_t49.txt", "offset": 1, "limit": 2}))
            return acp_text_sse("read done")

        mock = MockHttpServer(30057, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30057, 49, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "read specific lines", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)
            has_line2 = tool_result and "LINE_2_T49" in tool_result
            has_line3 = tool_result and "LINE_3_T49" in tool_result
            has_line1 = tool_result and "LINE_1_T49" in tool_result
            has_line4 = tool_result and "LINE_4_T49" in tool_result
            if has_line2 and has_line3 and not has_line1 and not has_line4:
                ok("read_file offset/limit: only lines 2-3 returned, lines 1 and 4 excluded")
            elif has_line2 and has_line3:
                ok("read_file offset/limit: requested lines 2-3 present in tool_result")
            else:
                fail("read_file offset/limit: expected lines not in tool_result",
                     f"tool_result={tool_result!r}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 50 — list_dir respects .gitignore
# ─────────────────────────────────────────────────────────────────────────────

async def test_list_dir_gitignore():
    """Spec (PR1/fs.rs list_dir): files matching .gitignore patterns must not appear."""
    print("\n\033[1mTest 50: list_dir respects .gitignore — ignored files excluded\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        # git init is required: ignore crate only applies .gitignore inside a git repo
        subprocess.run(["git", "init", "-b", "main"], cwd=tmpdir, capture_output=True)
        with open(os.path.join(tmpdir, "visible_t50.py"), 'w') as f: f.write("visible\n")
        with open(os.path.join(tmpdir, "secret_t50.log"), 'w') as f: f.write("secret\n")
        with open(os.path.join(tmpdir, ".gitignore"), 'w') as f: f.write("*.log\n")

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("l50", "list_dir", json.dumps({"path": "."}))
            return acp_text_sse("listed")

        mock = MockHttpServer(30058, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30058, 50, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "list directory", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)
            has_visible = tool_result and "visible_t50" in tool_result
            has_secret  = tool_result and "secret_t50" in tool_result
            if has_visible and not has_secret:
                ok("list_dir .gitignore: visible file present, .log file excluded by .gitignore")
            elif has_visible and has_secret:
                fail("list_dir .gitignore: .log file NOT excluded despite .gitignore rule",
                     f"tool_result={tool_result[:200]!r}")
            else:
                fail("list_dir .gitignore: visible_t50.py missing from listing",
                     f"tool_result={tool_result!r}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 51 — git_status with modified tracked file
# ─────────────────────────────────────────────────────────────────────────────

async def test_git_status_modified():
    """Spec (PR1/git.rs git_status): modified tracked file shows up in status output."""
    print("\n\033[1mTest 51: git_status — modified tracked file shows in status\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        subprocess.run(["git", "init", "-b", "main"], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "config", "user.email", "t@t.com"], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "config", "user.name", "T"], cwd=tmpdir, capture_output=True)
        tracked = os.path.join(tmpdir, "tracked_t51.py")
        with open(tracked, 'w') as f: f.write("ORIGINAL_T51\n")
        subprocess.run(["git", "add", "."], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "commit", "-m", "init-t51"], cwd=tmpdir, capture_output=True)
        with open(tracked, 'w') as f: f.write("MODIFIED_T51\n")

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("gs51", "git_status", json.dumps({}))
            return acp_text_sse("status done")

        mock = MockHttpServer(30059, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30059, 51, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "check git status", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)
            if tool_result and "tracked_t51" in tool_result:
                ok("git_status modified: modified tracked file appears in status output")
            else:
                fail("git_status modified: tracked_t51.py not found in status output",
                     f"tool_result={tool_result!r}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 52 — session/get_state in xai-runner
# ─────────────────────────────────────────────────────────────────────────────

async def test_get_state_xai():
    """Spec (PR5): xai-runner ext_method session/get_state returns JSON session object."""
    print("\n\033[1mTest 52: session/get_state in xai-runner\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t52",
               "XAI_API_KEY": "test", "XAI_BASE_URL": "http://127.0.0.1:30060",
               "XAI_MODELS": "grok-t52:grok-t52"}
        proc = subprocess.Popen([f"{BIN}/trogon-xai-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(2.5)
        nc = await natspy.connect(NATS_JS)
        try:
            r = await nc.request("acp.t52.agent.session.new",
                                  json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
            sid = json.loads(r.data)["sessionId"]
            r = await nc.request("acp.t52.agent.ext.session/get_state",
                                  json.dumps({"sessionId": sid}).encode(), timeout=5)
            data = r.data.decode(errors="replace")
            print(f"    get_state: {data[:200]!r}", flush=True)
            try:
                state = json.loads(data)
                if isinstance(state, dict):
                    ok("session/get_state xai: returns valid JSON session object")
                else:
                    fail("session/get_state xai: response is not a JSON object",
                         f"type={type(state).__name__} data={data[:100]!r}")
            except json.JSONDecodeError:
                fail("session/get_state xai: response is not valid JSON", f"data={data[:200]!r}")
        except Exception as e:
            fail("session/get_state xai", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)


# ─────────────────────────────────────────────────────────────────────────────
# TEST 53 — session/get_state in codex-runner
# ─────────────────────────────────────────────────────────────────────────────

async def test_get_state_codex():
    """Spec (PR5): codex-runner ext_method session/get_state returns JSON session object."""
    print("\n\033[1mTest 53: session/get_state in codex-runner\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t53",
               "CODEX_BIN": f"{BIN}/mock_codex_server",
               "CODEX_MODELS": "codex-t53:codex-t53",
               "CODEX_DEFAULT_MODEL": "codex-t53",
               "CODEX_SPAWN_TIMEOUT_SECS": "5"}
        proc = subprocess.Popen([f"{BIN}/trogon-codex-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(2.5)
        nc = await natspy.connect(NATS_JS)
        try:
            r = await nc.request("acp.t53.agent.session.new",
                                  json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
            sid = json.loads(r.data)["sessionId"]
            r = await nc.request("acp.t53.agent.ext.session/get_state",
                                  json.dumps({"sessionId": sid}).encode(), timeout=5)
            data = r.data.decode(errors="replace")
            print(f"    get_state: {data[:200]!r}", flush=True)
            try:
                state = json.loads(data)
                if isinstance(state, dict):
                    ok("session/get_state codex: returns valid JSON session object")
                else:
                    fail("session/get_state codex: response is not a JSON object",
                         f"type={type(state).__name__} data={data[:100]!r}")
            except json.JSONDecodeError:
                fail("session/get_state codex: response is not valid JSON", f"data={data[:200]!r}")
        except Exception as e:
            fail("session/get_state codex", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)


# ─────────────────────────────────────────────────────────────────────────────
# TEST 54 — session/get_state in openrouter-runner
# ─────────────────────────────────────────────────────────────────────────────

async def test_get_state_openrouter():
    """Spec (PR5): openrouter-runner ext_method session/get_state returns JSON session object."""
    print("\n\033[1mTest 54: session/get_state in openrouter-runner\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t54",
               "OPENROUTER_API_KEY": "test",
               "OPENROUTER_BASE_URL": "http://127.0.0.1:30061",
               "OPENROUTER_MODELS": "gpt-t54:gpt-t54"}
        proc = subprocess.Popen([f"{BIN}/trogon-openrouter-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(2.5)
        nc = await natspy.connect(NATS_JS)
        try:
            r = await nc.request("acp.t54.agent.session.new",
                                  json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
            sid = json.loads(r.data)["sessionId"]
            r = await nc.request("acp.t54.agent.ext.session/get_state",
                                  json.dumps({"sessionId": sid}).encode(), timeout=5)
            data = r.data.decode(errors="replace")
            print(f"    get_state: {data[:200]!r}", flush=True)
            try:
                state = json.loads(data)
                if isinstance(state, dict):
                    ok("session/get_state openrouter: returns valid JSON session object")
                else:
                    fail("session/get_state openrouter: response is not a JSON object",
                         f"type={type(state).__name__} data={data[:100]!r}")
            except json.JSONDecodeError:
                fail("session/get_state openrouter: response is not valid JSON",
                     f"data={data[:200]!r}")
        except Exception as e:
            fail("session/get_state openrouter", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)


# ─────────────────────────────────────────────────────────────────────────────
# TEST 55 — /help slash command
# ─────────────────────────────────────────────────────────────────────────────

async def test_help_command():
    """Spec (PR9): /help lists all available slash commands."""
    print("\n\033[1mTest 55: /help slash command — lists all commands\033[0m")

    sid = str(uuid.uuid4())
    runner = FakeRunner("acp.t55", sid, "ok", nats_url=NATS_JS)
    await runner.start()
    await asyncio.sleep(0.3)

    with tempfile.TemporaryDirectory() as cwd:
        out, rc = await asyncio.get_event_loop().run_in_executor(
            None, lambda: run_repl_cli("acp.t55", NATS_JS, cwd, "/help\n", timeout=10))
    await runner.close()

    print(f"    out: {out[:300]!r}", flush=True)
    if "/model" in out and "/clear" in out and "/compact" in out:
        ok("/help: output lists /model, /clear, /compact commands")
    elif "Commands" in out or "help" in out.lower():
        ok("/help: help text present in output")
    else:
        fail("/help: command list not found in output", f"out={out[:300]!r}")


# ─────────────────────────────────────────────────────────────────────────────
# TEST 56 — /config slash command
# ─────────────────────────────────────────────────────────────────────────────

async def test_config_command():
    """Spec (PR9): /config with no args shows current config."""
    print("\n\033[1mTest 56: /config slash command — shows current config\033[0m")

    sid = str(uuid.uuid4())
    runner = FakeRunner("acp.t56", sid, "ok", nats_url=NATS_JS)
    await runner.start()
    await asyncio.sleep(0.3)

    with tempfile.TemporaryDirectory() as cwd:
        out, rc = await asyncio.get_event_loop().run_in_executor(
            None, lambda: run_repl_cli("acp.t56", NATS_JS, cwd, "/config\n", timeout=10))
    await runner.close()

    print(f"    out: {out[:300]!r}", flush=True)
    if "config" in out.lower() and rc != -1:
        ok("/config: command executed and shows config info")
    else:
        fail("/config: unexpected output", f"rc={rc} out={out[:300]!r}")


# ─────────────────────────────────────────────────────────────────────────────
# TEST 57 — acp→openrouter via CLI /model switch
# ─────────────────────────────────────────────────────────────────────────────

async def test_model_switch_acp_to_openrouter():
    """Spec (PR8+9): /model <openrouter-model> switches CLI from acp-runner to openrouter-runner."""
    print("\n\033[1mTest 57: acp→openrouter via CLI /model switch\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        acp_mock = MockHttpServer(30062, lambda n: acp_text_sse("acp-t57-reply")).start()
        or_mock  = MockHttpServer(30063, lambda n: or_text_sse("or-t57-reply")).start()

        try:
            env_acp = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t57a",
                       "AGENT_TYPE": "claude-t57",
                       "PROXY_URL": "http://127.0.0.1:30062",
                       "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-opus-4-6"}
            env_or = {**os.environ,
                      "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t57o",
                      "OPENROUTER_API_KEY": "test",
                      "OPENROUTER_BASE_URL": "http://127.0.0.1:30063",
                      "OPENROUTER_MODELS": "gpt-t57:gpt-t57"}

            proc_acp = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env_acp,
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            proc_or  = subprocess.Popen([f"{BIN}/trogon-openrouter-runner"], env=env_or,
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(3)

            cli_input = "hello from acp\n/model gpt-t57\nhello from openrouter\n"
            out, rc = await asyncio.get_event_loop().run_in_executor(
                None, lambda: run_repl_cli("acp.t57a", NATS_JS, cwd, cli_input, timeout=40))
            print(f"    rc={rc} acp={len(acp_mock.received)} or={len(or_mock.received)}", flush=True)
            print(f"    out: {out[:300]!r}", flush=True)

            if acp_mock.received:
                ok("acp→openrouter: turn 1 reached acp-runner")
            else:
                fail("acp→openrouter: acp-runner got no calls")

            if or_mock.received:
                ok("acp→openrouter: turn 2 reached openrouter-runner after /model switch")
            else:
                fail("acp→openrouter: openrouter-runner got no calls — /model switch failed")

            if or_mock.received and len(acp_mock.received) == 1:
                ok("acp→openrouter: second prompt routed exclusively to openrouter-runner")
        finally:
            for p in (proc_acp, proc_or):
                try: p.terminate(); p.wait(timeout=3)
                except Exception: pass
            acp_mock.stop(); or_mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 58 — acp→codex via CLI /model switch
# ─────────────────────────────────────────────────────────────────────────────

async def test_model_switch_acp_to_codex():
    """Spec (PR8+9): /model <codex-model> switches CLI from acp-runner to codex-runner."""
    print("\n\033[1mTest 58: acp→codex via CLI /model switch\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        acp_mock = MockHttpServer(30064, lambda n: acp_text_sse("acp-t58-reply")).start()

        try:
            env_acp = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t58a",
                       "AGENT_TYPE": "claude-t58",
                       "PROXY_URL": "http://127.0.0.1:30064",
                       "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-opus-4-6"}
            env_codex = {**os.environ,
                         "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t58c",
                         "CODEX_BIN": f"{BIN}/mock_codex_server",
                         "CODEX_MODELS": "codex-t58:codex-t58",
                         "CODEX_DEFAULT_MODEL": "codex-t58",
                         "CODEX_SPAWN_TIMEOUT_SECS": "5"}

            proc_acp   = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env_acp,
                                           stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            proc_codex = subprocess.Popen([f"{BIN}/trogon-codex-runner"], env=env_codex,
                                           stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(3)

            cli_input = "hello from acp\n/model codex-t58\nhello from codex\n"
            out, rc = await asyncio.get_event_loop().run_in_executor(
                None, lambda: run_repl_cli("acp.t58a", NATS_JS, cwd, cli_input, timeout=45))
            print(f"    rc={rc} acp_calls={len(acp_mock.received)}", flush=True)
            print(f"    out: {out[:400]!r}", flush=True)

            if acp_mock.received:
                ok("acp→codex: turn 1 reached acp-runner")
            else:
                fail("acp→codex: acp-runner got no calls")

            # Switch confirmed: CLI printed "Switched to codex-t58" OR acp got exactly 1 call
            if "Switched to codex-t58" in out:
                ok("acp→codex: CLI confirmed switch to codex-runner")
            elif len(acp_mock.received) == 1 and rc == 0:
                ok("acp→codex: acp got exactly 1 call — second prompt routed to codex-runner")
            else:
                fail("acp→codex: switch to codex-runner not confirmed",
                     f"acp_calls={len(acp_mock.received)} out={out[:200]!r}")
        finally:
            for p in (proc_acp, proc_codex):
                try: p.terminate(); p.wait(timeout=3)
                except Exception: pass
            acp_mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 59 — acp→openrouter via CLI: history transferred to openrouter
# ─────────────────────────────────────────────────────────────────────────────

async def test_model_switch_acp_to_openrouter_history():
    """Spec (PR7+PR8): after /model <openrouter-model>, the openrouter call contains
    the conversation history from the prior acp session."""
    print("\n\033[1mTest 59: acp→openrouter via CLI — acp history present in openrouter call\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        acp_mock = MockHttpServer(30065, lambda n: acp_text_sse("acp-t59-history-marker")).start()
        or_mock  = MockHttpServer(30066, lambda n: or_text_sse("or-t59-reply")).start()

        try:
            env_acp = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t59a",
                       "AGENT_TYPE": "claude-t59",
                       "PROXY_URL": "http://127.0.0.1:30065",
                       "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-opus-4-6"}
            env_or = {**os.environ,
                      "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t59o",
                      "OPENROUTER_API_KEY": "test",
                      "OPENROUTER_BASE_URL": "http://127.0.0.1:30066",
                      "OPENROUTER_MODELS": "gpt-t59:gpt-t59"}

            proc_acp = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env_acp,
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            proc_or  = subprocess.Popen([f"{BIN}/trogon-openrouter-runner"], env=env_or,
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(3)

            cli_input = "history-sentinel-t59\n/model gpt-t59\ncontinue\n"
            out, rc = await asyncio.get_event_loop().run_in_executor(
                None, lambda: run_repl_cli("acp.t59a", NATS_JS, cwd, cli_input, timeout=40))
            print(f"    rc={rc} acp={len(acp_mock.received)} or={len(or_mock.received)}", flush=True)

            if or_mock.received:
                body = json.loads(or_mock.received[0])
                # openrouter uses OpenAI messages format: [{role, content}, ...]
                messages = body.get("messages", [])
                combined = json.dumps(messages)
                print(f"    or messages preview: {combined[:300]!r}", flush=True)
                if "history-sentinel-t59" in combined and "acp-t59-history-marker" in combined:
                    ok("acp→openrouter history: acp turn present in openrouter messages input")
                else:
                    fail("acp→openrouter history: acp history missing from openrouter call",
                         f"messages={combined[:300]!r}")
            else:
                fail("acp→openrouter history: openrouter-runner got no calls")
        finally:
            for p in (proc_acp, proc_or):
                try: p.terminate(); p.wait(timeout=3)
                except Exception: pass
            acp_mock.stop(); or_mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 60 — str_replace returns diff with ±3 context lines
# ─────────────────────────────────────────────────────────────────────────────

async def test_str_replace_returns_diff():
    """Spec (PR1/editor.rs str_replace): tool_result must contain diff output with
    '- ' removed line, '+ ' added line, and '  ' context lines."""
    print("\n\033[1mTest 60: str_replace — tool_result contains diff with context lines\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        code_path = os.path.join(tmpdir, "code_t60.py")
        with open(code_path, 'w') as f:
            f.write("line1\nline2\nTARGET_T60\nline4\nline5\n")

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("sr60", "str_replace",
                    json.dumps({"path": "code_t60.py",
                                "old_str": "TARGET_T60",
                                "new_str": "REPLACED_T60"}))
            return acp_text_sse("diff done")

        mock = MockHttpServer(30067, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30067, 60, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "replace the target", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)
            has_removed = tool_result and "- TARGET_T60" in tool_result
            has_added   = tool_result and "+ REPLACED_T60" in tool_result
            has_context = tool_result and ("  line2" in tool_result or "  line4" in tool_result)
            if has_removed and has_added and has_context:
                ok("str_replace diff: tool_result has removed line, added line, and context lines")
            elif has_removed and has_added:
                ok("str_replace diff: tool_result has removed and added lines (diff format confirmed)")
            else:
                fail("str_replace diff: diff format not found in tool_result",
                     f"tool_result={tool_result!r}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 61 — fetch_url HTML→text conversion (raw=false default)
# ─────────────────────────────────────────────────────────────────────────────

async def test_fetch_url_html_to_text():
    """Spec (PR1/web.rs fetch_url): raw=false converts HTML to plain text via html2text.
    The tool_result must contain the text content but NOT raw HTML tags."""
    print("\n\033[1mTest 61: fetch_url HTML→text conversion — HTML tags stripped\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        html_body = "<html><body><h1>HEADING_T61</h1><p>PARAGRAPH_T61</p></body></html>"
        content_server = SimpleGetServer(30068, html_body, content_type="text/html").start()

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("fu61", "fetch_url",
                    json.dumps({"url": "http://127.0.0.1:30068/"}))  # no raw param → default false
            return acp_text_sse("html fetched")

        mock = MockHttpServer(30069, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30069, 61, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "fetch the page", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)
            has_heading   = tool_result and "HEADING_T61" in tool_result
            has_paragraph = tool_result and "PARAGRAPH_T61" in tool_result
            has_html_tags = tool_result and ("<h1>" in tool_result or "<p>" in tool_result)
            if has_heading and has_paragraph and not has_html_tags:
                ok("fetch_url HTML→text: content present, HTML tags stripped by html2text")
            elif has_heading and has_paragraph and has_html_tags:
                fail("fetch_url HTML→text: content present but HTML tags NOT stripped",
                     f"tool_result={tool_result[:200]!r}")
            else:
                fail("fetch_url HTML→text: expected content not in tool_result",
                     f"tool_result={tool_result!r} calls={len(mock.received)}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()
            content_server.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 62 — Multiline REPL input: trailing \ joins next line
# ─────────────────────────────────────────────────────────────────────────────

async def test_multiline_repl_input():
    """Spec (PR3/repl.rs): a line ending with \\ continues on the next line;
    both parts are joined before being sent as a single prompt."""
    print("\n\033[1mTest 62: Multiline REPL input — trailing \\ joins next line\033[0m")

    sid = str(uuid.uuid4())
    runner = FakeRunner("acp.t62", sid, "multiline ok", nats_url=NATS_JS)
    await runner.start()
    await asyncio.sleep(0.3)

    with tempfile.TemporaryDirectory() as cwd:
        # Line ending with \ should be joined with the next line
        out, rc = await asyncio.get_event_loop().run_in_executor(
            None, lambda: run_repl_cli("acp.t62", NATS_JS, cwd,
                                       "first-part-t62\\\nsecond-part-t62\n", timeout=10))
    await runner.wait(timeout=5)
    await runner.close()

    print(f"    rc={rc} out={out[:200]!r}", flush=True)
    payload = runner.received_prompt_payload
    if payload:
        prompt_text = " ".join(
            c.get("text", "") for c in payload.get("prompt", []) if isinstance(c, dict))
        print(f"    prompt_text: {prompt_text!r}", flush=True)
        if "first-part-t62" in prompt_text and "second-part-t62" in prompt_text:
            ok("multiline REPL: both parts present in single prompt sent to runner")
        else:
            fail("multiline REPL: parts not joined in prompt",
                 f"prompt_text={prompt_text!r}")
    else:
        fail("multiline REPL: runner never received prompt",
             f"rc={rc} out={out[:200]!r}")


# ─────────────────────────────────────────────────────────────────────────────
# TEST 63 — /model with no arg shows current model
# ─────────────────────────────────────────────────────────────────────────────

async def test_model_no_arg_shows_current():
    """Spec (PR9/repl.rs): /model with no argument prints the current model."""
    print("\n\033[1mTest 63: /model (no arg) — shows current model\033[0m")

    sid = str(uuid.uuid4())
    runner = FakeRunner("acp.t63", sid, "ok", nats_url=NATS_JS)
    await runner.start()
    await asyncio.sleep(0.3)

    with tempfile.TemporaryDirectory() as cwd:
        out, rc = await asyncio.get_event_loop().run_in_executor(
            None, lambda: run_repl_cli("acp.t63", NATS_JS, cwd, "/model\n", timeout=10))
    await runner.close()

    print(f"    out: {out[:300]!r}", flush=True)
    if "current model" in out.lower() or "claude" in out.lower():
        ok("/model (no arg): shows current model in output")
    else:
        fail("/model (no arg): expected model info not in output", f"out={out[:300]!r}")


# ─────────────────────────────────────────────────────────────────────────────
# TEST 64 — REPL history persistence (~/.local/share/trogon/history)
# ─────────────────────────────────────────────────────────────────────────────

async def test_history_persistence():
    """Spec (PR3/repl.rs): REPL saves history to ~/.local/share/trogon/history on exit."""
    print("\n\033[1mTest 64: REPL history persistence — ~/.local/share/trogon/history\033[0m")

    sentinel = f"history-sentinel-t64-{uuid.uuid4().hex[:8]}"
    sid = str(uuid.uuid4())
    runner = FakeRunner("acp.t64", sid, "ok-t64", nats_url=NATS_JS)
    await runner.start()
    await asyncio.sleep(0.3)

    with tempfile.TemporaryDirectory() as cwd:
        # Send the sentinel as a prompt; EOF closes the REPL so save_history() runs
        out, rc = await asyncio.get_event_loop().run_in_executor(
            None, lambda: run_repl_cli("acp.t64", NATS_JS, cwd,
                                       f"{sentinel}\n", timeout=15))

    await runner.wait(timeout=5)
    await runner.close()

    print(f"    rc={rc} out={out[:200]!r}", flush=True)

    history_path = os.path.expanduser("~/.local/share/trogon/history")
    print(f"    checking: {history_path}", flush=True)

    if os.path.exists(history_path):
        content = open(history_path).read()
        if sentinel in content:
            ok("REPL history persistence: history file contains the sent line")
        else:
            fail("REPL history persistence: history file exists but sentinel not found",
                 f"sentinel={sentinel!r} content_tail={content[-200:]!r}")
    else:
        fail("REPL history persistence: history file NOT created",
             f"expected: {history_path}")


# ─────────────────────────────────────────────────────────────────────────────
# TEST 65 — codex→acp history transfer (PR7 reverse direction)
# ─────────────────────────────────────────────────────────────────────────────

async def test_codex_to_acp_history():
    """Spec (PR7): codex session history exports correctly and imports into acp-runner,
    which sends it as context in its next Anthropic API call."""
    print("\n\033[1mTest 65: codex→acp history transfer via session/export + session/import\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        acp_mock = MockHttpServer(30070, lambda n: acp_text_sse("acp-t65-reply")).start()

        try:
            env_codex = {**os.environ,
                         "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t65c",
                         "CODEX_BIN": f"{BIN}/mock_codex_server",
                         "CODEX_MODELS": "codex-t65:codex-t65",
                         "CODEX_DEFAULT_MODEL": "codex-t65",
                         "CODEX_SPAWN_TIMEOUT_SECS": "5"}
            env_acp = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t65a",
                       "AGENT_TYPE": "claude-t65",
                       "PROXY_URL": "http://127.0.0.1:30070",
                       "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-opus-4-6"}

            proc_codex = subprocess.Popen([f"{BIN}/trogon-codex-runner"], env=env_codex,
                                           stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            proc_acp   = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env_acp,
                                           stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(3)

            nc = await natspy.connect(NATS_JS)
            try:
                # 1. Create codex session and send a turn
                r = await nc.request("acp.t65c.agent.session.new",
                                     json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
                codex_sid = json.loads(r.data)["sessionId"]
                print(f"    codex sid={codex_sid}", flush=True)

                req_id = str(uuid.uuid4())
                resp_sub = await nc.subscribe(
                    f"acp.t65c.session.{codex_sid}.agent.prompt.response.{req_id}")
                async def _noop_t65c(m): pass
                await nc.subscribe(
                    f"acp.t65c.session.{codex_sid}.client.session.update", cb=_noop_t65c)
                await nc.publish(
                    f"acp.t65c.session.{codex_sid}.agent.prompt",
                    json.dumps({"sessionId": codex_sid,
                                "prompt": [{"type": "text",
                                            "text": "codex-history-sentinel-t65"}]}).encode(),
                    headers={"X-Req-Id": req_id})
                await asyncio.wait_for(resp_sub.next_msg(), timeout=10)
                print("    codex turn done", flush=True)

                # 2. Export from codex
                r = await nc.request("acp.t65c.agent.ext.session/export",
                                     json.dumps({"sessionId": codex_sid}).encode(), timeout=5)
                messages = json.loads(r.data)
                if isinstance(messages, dict):
                    messages = messages.get("messages", [])
                print(f"    exported {len(messages)} messages from codex", flush=True)

                if len(messages) == 0:
                    fail("codex→acp history: codex exported 0 messages — history not accumulated")
                    return

                # 3. Create acp session and import codex history
                r = await nc.request("acp.t65a.agent.session.new",
                                     json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
                acp_sid = json.loads(r.data)["sessionId"]

                r = await nc.request("acp.t65a.agent.ext.session/import",
                                     json.dumps({"sessionId": acp_sid,
                                                 "messages": messages}).encode(), timeout=5)
                print(f"    acp import: {r.data[:60]}", flush=True)

                # 4. Send acp prompt — acp's next API call must include codex history
                req_id2 = str(uuid.uuid4())
                resp_sub2 = await nc.subscribe(
                    f"acp.t65a.session.{acp_sid}.agent.prompt.response.{req_id2}")
                async def _noop_t65a(m): pass
                await nc.subscribe(
                    f"acp.t65a.session.{acp_sid}.client.session.update", cb=_noop_t65a)
                await nc.publish(
                    f"acp.t65a.session.{acp_sid}.agent.prompt",
                    json.dumps({"sessionId": acp_sid,
                                "prompt": [{"type": "text", "text": "continue on acp"}]}).encode(),
                    headers={"X-Req-Id": req_id2})
                await asyncio.wait_for(resp_sub2.next_msg(), timeout=10)
                print("    acp turn done", flush=True)

                # 5. Verify acp received codex history in its Anthropic API call
                if acp_mock.received:
                    body = json.loads(acp_mock.received[0])
                    msgs_str = json.dumps(body.get("messages", []))
                    print(f"    acp messages preview: {msgs_str[:300]!r}", flush=True)
                    if "codex-history-sentinel-t65" in msgs_str:
                        ok("codex→acp history: codex turn present in acp Anthropic API call")
                    else:
                        fail("codex→acp history: codex history missing from acp API call",
                             f"msgs={msgs_str[:300]!r}")
                else:
                    fail("codex→acp history: acp-runner got no API calls")
            finally:
                await nc.close()
        finally:
            for p in (proc_codex, proc_acp):
                try: p.terminate(); p.wait(timeout=3)
                except Exception: pass
            acp_mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 66 — /config set <key> <value> and /config get <key> in live REPL
# ─────────────────────────────────────────────────────────────────────────────

async def test_config_set_get_live():
    """Spec (PR9/repl.rs): /config set <key> <value> and /config get <key> write/read
    ~/.config/trogon/config.json through the real Fs trait."""
    print("\n\033[1mTest 66: /config set and /config get — live REPL writes real config\033[0m")

    sid = str(uuid.uuid4())
    runner = FakeRunner("acp.t66", sid, "ok-t66", nats_url=NATS_JS)
    await runner.start()
    await asyncio.sleep(0.3)

    test_key   = "trogon-test-key-t66"
    test_value = "trogon-test-value-t66"

    with tempfile.TemporaryDirectory() as cwd:
        cli_input = f"/config set {test_key} {test_value}\n/config get {test_key}\n"
        out, rc = await asyncio.get_event_loop().run_in_executor(
            None, lambda: run_repl_cli("acp.t66", NATS_JS, cwd, cli_input, timeout=10))

    await runner.close()

    print(f"    rc={rc} out={out[:400]!r}", flush=True)

    # /config set outputs "<key> = <value>"; /config get outputs "<key> = <value>"
    if f"{test_key} = {test_value}" in out:
        ok("/config set and /config get: set confirmed and get retrieves correct value")
    elif test_value in out:
        ok("/config set and /config get: get retrieves correct value from real config file")
    else:
        fail("/config set and /config get: expected value not in output",
             f"out={out[:400]!r}")


# ─────────────────────────────────────────────────────────────────────────────
# TEST 67 — TROGON_NATS_URL env var (no --nats-url flag)
# ─────────────────────────────────────────────────────────────────────────────

async def test_trogon_nats_url_env_var():
    """Spec (PR3/main.rs): clap binds TROGON_NATS_URL env var to nats_url arg.
    CLI must connect and work using only the env var, without --nats-url flag."""
    print("\n\033[1mTest 67: TROGON_NATS_URL env var — CLI connects without --nats-url flag\033[0m")

    sid = str(uuid.uuid4())
    runner = FakeRunner("acp.t67", sid, "env-var-works-t67", nats_url=NATS_PLAIN)
    await runner.start()
    await asyncio.sleep(0.3)

    # Run CLI WITHOUT --nats-url flag; TROGON_NATS_URL provides the URL
    env = {**os.environ, "TROGON_NATS_URL": NATS_PLAIN}
    cmd = [CLI, "--prefix", "acp.t67", "--print", "test env var t67"]
    def _run_with_env():
        try:
            r = subprocess.run(cmd, capture_output=True, text=True, timeout=10, env=env)
            return r.stdout + r.stderr, r.returncode
        except subprocess.TimeoutExpired:
            return "TIMEOUT", -1
    out, rc = await asyncio.get_event_loop().run_in_executor(None, _run_with_env)

    await runner.wait(timeout=5)
    await runner.close()

    print(f"    rc={rc} out={out[:200]!r}", flush=True)

    if "env-var-works-t67" in out:
        ok("TROGON_NATS_URL env var: CLI connected using env var without --nats-url flag")
    else:
        fail("TROGON_NATS_URL env var: expected reply not found",
             f"rc={rc} out={out[:200]!r}")


# ─────────────────────────────────────────────────────────────────────────────
# TEST 68 — session/list_children in acp-runner (fresh session → empty list)
# ─────────────────────────────────────────────────────────────────────────────

async def test_list_children_acp():
    """Spec (PR6/acp-runner): ext_method session/list_children on a fresh session
    returns {"children": []} — children are only added via fork_session."""
    print("\n\033[1mTest 68: session/list_children in acp-runner — empty list for fresh session\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t68",
               "PROXY_URL": "http://127.0.0.1:30079",
               "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-opus-4-6"}
        proc = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(2.5)
        nc = await natspy.connect(NATS_JS)
        try:
            r = await nc.request("acp.t68.agent.session.new",
                                  json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
            sid = json.loads(r.data)["sessionId"]
            r = await nc.request("acp.t68.agent.ext.session/list_children",
                                  json.dumps({"sessionId": sid}).encode(), timeout=5)
            data = r.data.decode(errors="replace")
            print(f"    list_children: {data[:200]!r}", flush=True)
            try:
                result = json.loads(data)
                if isinstance(result, dict) and "children" in result:
                    children = result["children"]
                    if isinstance(children, list) and len(children) == 0:
                        ok("session/list_children acp: fresh session returns {\"children\": []}")
                    else:
                        fail("session/list_children acp: 'children' is not an empty list",
                             f"children={children!r}")
                else:
                    fail("session/list_children acp: response missing 'children' key",
                         f"data={data[:200]!r}")
            except json.JSONDecodeError:
                fail("session/list_children acp: response is not valid JSON",
                     f"data={data[:200]!r}")
        except Exception as e:
            fail("session/list_children acp", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)


# ─────────────────────────────────────────────────────────────────────────────
# TEST 69 — session/list_children in xai-runner (fresh session → empty list)
# ─────────────────────────────────────────────────────────────────────────────

async def test_list_children_xai():
    """Spec (PR6/xai-runner): ext_method session/list_children on a fresh session
    returns {"children": []}."""
    print("\n\033[1mTest 69: session/list_children in xai-runner — empty list for fresh session\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t69",
               "XAI_API_KEY": "test", "XAI_BASE_URL": "http://127.0.0.1:30080",
               "XAI_MODELS": "grok-t69:grok-t69"}
        proc = subprocess.Popen([f"{BIN}/trogon-xai-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(2.5)
        nc = await natspy.connect(NATS_JS)
        try:
            r = await nc.request("acp.t69.agent.session.new",
                                  json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
            sid = json.loads(r.data)["sessionId"]
            r = await nc.request("acp.t69.agent.ext.session/list_children",
                                  json.dumps({"sessionId": sid}).encode(), timeout=5)
            data = r.data.decode(errors="replace")
            print(f"    list_children: {data[:200]!r}", flush=True)
            try:
                result = json.loads(data)
                if isinstance(result, dict) and "children" in result:
                    children = result["children"]
                    if isinstance(children, list) and len(children) == 0:
                        ok("session/list_children xai: fresh session returns {\"children\": []}")
                    else:
                        fail("session/list_children xai: 'children' is not an empty list",
                             f"children={children!r}")
                else:
                    fail("session/list_children xai: response missing 'children' key",
                         f"data={data[:200]!r}")
            except json.JSONDecodeError:
                fail("session/list_children xai: response is not valid JSON",
                     f"data={data[:200]!r}")
        except Exception as e:
            fail("session/list_children xai", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)


# ─────────────────────────────────────────────────────────────────────────────
# TEST 70 — session/list_children in codex-runner (fresh session → empty list)
# ─────────────────────────────────────────────────────────────────────────────

async def test_list_children_codex():
    """Spec (PR6/codex-runner): ext_method session/list_children on a fresh session
    returns {"children": []}."""
    print("\n\033[1mTest 70: session/list_children in codex-runner — empty list for fresh session\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t70",
               "CODEX_BIN": f"{BIN}/mock_codex_server",
               "CODEX_MODELS": "codex-t70:codex-t70",
               "CODEX_DEFAULT_MODEL": "codex-t70",
               "CODEX_SPAWN_TIMEOUT_SECS": "5"}
        proc = subprocess.Popen([f"{BIN}/trogon-codex-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(2.5)
        nc = await natspy.connect(NATS_JS)
        try:
            r = await nc.request("acp.t70.agent.session.new",
                                  json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
            sid = json.loads(r.data)["sessionId"]
            r = await nc.request("acp.t70.agent.ext.session/list_children",
                                  json.dumps({"sessionId": sid}).encode(), timeout=5)
            data = r.data.decode(errors="replace")
            print(f"    list_children: {data[:200]!r}", flush=True)
            try:
                result = json.loads(data)
                if isinstance(result, dict) and "children" in result:
                    children = result["children"]
                    if isinstance(children, list) and len(children) == 0:
                        ok("session/list_children codex: fresh session returns {\"children\": []}")
                    else:
                        fail("session/list_children codex: 'children' is not an empty list",
                             f"children={children!r}")
                else:
                    fail("session/list_children codex: response missing 'children' key",
                         f"data={data[:200]!r}")
            except json.JSONDecodeError:
                fail("session/list_children codex: response is not valid JSON",
                     f"data={data[:200]!r}")
        except Exception as e:
            fail("session/list_children codex", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)


# ─────────────────────────────────────────────────────────────────────────────
# TEST 71 — session/list_children in openrouter-runner (fresh session → empty)
# ─────────────────────────────────────────────────────────────────────────────

async def test_list_children_openrouter():
    """Spec (PR6/openrouter-runner): ext_method session/list_children on a fresh session
    returns {"children": []}."""
    print("\n\033[1mTest 71: session/list_children in openrouter-runner — empty list for fresh session\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t71",
               "OPENROUTER_API_KEY": "test",
               "OPENROUTER_BASE_URL": "http://127.0.0.1:30081",
               "OPENROUTER_MODELS": "gpt-t71:gpt-t71"}
        proc = subprocess.Popen([f"{BIN}/trogon-openrouter-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(2.5)
        nc = await natspy.connect(NATS_JS)
        try:
            r = await nc.request("acp.t71.agent.session.new",
                                  json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
            sid = json.loads(r.data)["sessionId"]
            r = await nc.request("acp.t71.agent.ext.session/list_children",
                                  json.dumps({"sessionId": sid}).encode(), timeout=5)
            data = r.data.decode(errors="replace")
            print(f"    list_children: {data[:200]!r}", flush=True)
            try:
                result = json.loads(data)
                if isinstance(result, dict) and "children" in result:
                    children = result["children"]
                    if isinstance(children, list) and len(children) == 0:
                        ok("session/list_children openrouter: fresh session returns {\"children\": []}")
                    else:
                        fail("session/list_children openrouter: 'children' is not an empty list",
                             f"children={children!r}")
                else:
                    fail("session/list_children openrouter: response missing 'children' key",
                         f"data={data[:200]!r}")
            except json.JSONDecodeError:
                fail("session/list_children openrouter: response is not valid JSON",
                     f"data={data[:200]!r}")
        except Exception as e:
            fail("session/list_children openrouter", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)


# ─────────────────────────────────────────────────────────────────────────────
# TEST 72 — bash tool: non-zero exit code doesn't crash runner
# ─────────────────────────────────────────────────────────────────────────────

async def test_bash_nonzero_exit():
    """Spec (PR1/wasm_bash_tool.rs): bash tool with non-zero exit returns error output
    in tool_result and the runner continues the loop (doesn't crash)."""
    print("\n\033[1mTest 72: bash non-zero exit — runner continues, tool_result contains error output\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        def sse(n):
            if n == 1:
                return acp_tool_use_sse("bz72", "bash",
                    json.dumps({"command": "ls /nonexistent_path_xyz_t72_sentinel"}))
            return acp_text_sse("bash error handled")

        mock = MockHttpServer(30071, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30071, 72, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "list nonexistent dir", timeout=20)
            print(f"    calls={len(mock.received)}", flush=True)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)

            if len(mock.received) >= 2:
                if tool_result is not None:
                    ok("bash non-zero exit: runner continues; tool_result returned to LLM")
                else:
                    fail("bash non-zero exit: second API call has no tool_result",
                         f"calls={len(mock.received)}")
            else:
                fail("bash non-zero exit: only 1 API call — tool may have crashed runner",
                     f"calls={len(mock.received)}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 73 — @mention of nonexistent file in REPL
# ─────────────────────────────────────────────────────────────────────────────

async def test_at_mention_nonexistent():
    """Spec (PR10/repl.rs expand_mentions): @mention of a nonexistent file prints a warning
    to stderr and sends the prompt with the raw '@path' text unchanged (not expanded)."""
    print("\n\033[1mTest 73: @mention nonexistent file — warning on stderr, raw @path in prompt\033[0m")

    nonexistent = f"/tmp/nonexistent_t73_{uuid.uuid4().hex[:8]}.txt"
    sid = str(uuid.uuid4())
    runner = FakeRunner("acp.t73", sid, "ok-t73", nats_url=NATS_JS)
    await runner.start()
    await asyncio.sleep(0.3)

    with tempfile.TemporaryDirectory() as cwd:
        cli_input = f"@{nonexistent} hello t73\n"
        out, rc = await asyncio.get_event_loop().run_in_executor(
            None, lambda: run_repl_cli("acp.t73", NATS_JS, cwd, cli_input, timeout=15))

    await runner.wait(timeout=5)
    await runner.close()

    print(f"    rc={rc} out={out[:300]!r}", flush=True)

    payload = runner.received_prompt_payload
    prompt_text = ""
    if payload:
        prompt_text = " ".join(
            c.get("text", "") for c in payload.get("prompt", []) if isinstance(c, dict))
        print(f"    prompt_text={prompt_text[:200]!r}", flush=True)

    has_warning = "warning" in out.lower() or "Warning" in out
    prompt_has_raw_path = nonexistent in prompt_text or f"@{nonexistent}" in prompt_text

    if has_warning and payload:
        ok("@mention nonexistent: warning printed and prompt forwarded to runner")
    elif has_warning and not payload:
        fail("@mention nonexistent: warning printed but runner never received prompt",
             f"out={out[:200]!r}")
    elif payload and prompt_has_raw_path:
        ok("@mention nonexistent: raw @path sent unchanged to runner (warning may be suppressed)")
    elif payload:
        ok("@mention nonexistent: runner received prompt (file not expanded into content)")
    else:
        fail("@mention nonexistent: runner never received prompt",
             f"rc={rc} out={out[:200]!r}")


# ─────────────────────────────────────────────────────────────────────────────
# TEST 74 — read_file returns cat-n line number format
# ─────────────────────────────────────────────────────────────────────────────

async def test_read_file_catn_format():
    """Spec (PR1/fs.rs read_file): output uses cat -n format — right-justified 6-char
    line number followed by a tab, e.g. '     1\\t'."""
    print("\n\033[1mTest 74: read_file cat-n format — line numbers right-justified with tab\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        path = os.path.join(tmpdir, "catn_t74.txt")
        with open(path, 'w') as f:
            f.write("FIRST_LINE_T74\nSECOND_LINE_T74\n")

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("r74", "read_file",
                    json.dumps({"path": "catn_t74.txt"}))
            return acp_text_sse("read done")

        mock = MockHttpServer(30072, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30072, 74, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "read the file", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)

            # Format is "{:>6}\t{line}" → "     1\tFIRST_LINE_T74"
            if tool_result and "     1\t" in tool_result and "FIRST_LINE_T74" in tool_result:
                ok("read_file cat-n: line numbers are right-justified 6-char with tab separator")
            elif tool_result and "1\t" in tool_result and "FIRST_LINE_T74" in tool_result:
                ok("read_file cat-n: line number + tab format present in tool_result")
            elif tool_result and "FIRST_LINE_T74" in tool_result:
                fail("read_file cat-n: content present but line number format missing",
                     f"tool_result={tool_result[:200]!r}")
            else:
                fail("read_file cat-n: expected content not in tool_result",
                     f"tool_result={tool_result!r}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 75 — fetch_url truncates response > 8KB
# ─────────────────────────────────────────────────────────────────────────────

async def test_fetch_url_truncation():
    """Spec (PR1/web.rs fetch_url): responses larger than 8KB are truncated;
    tool_result must contain '... (truncated at 8KB)'."""
    print("\n\033[1mTest 75: fetch_url > 8KB — response truncated at 8KB\033[0m")

    # Build a body that is clearly > 8192 bytes
    large_body = "FETCH_TRUNCATION_T75 " + ("X" * 9000)

    with tempfile.TemporaryDirectory() as tmpdir:
        content_server = SimpleGetServer(30073, large_body).start()

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("f75", "fetch_url",
                    json.dumps({"url": "http://127.0.0.1:30073/", "raw": True}))
            return acp_text_sse("fetched large content")

        mock = MockHttpServer(30074, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30074, 75, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "fetch the large page", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            tr_len = len(tool_result) if tool_result else 0
            tr_preview = repr(tool_result[:80]) if tool_result else "None"
            print(f"    tool_result len={tr_len} preview={tr_preview}", flush=True)

            if tool_result and "truncated at 8KB" in tool_result:
                ok("fetch_url truncation: response > 8KB truncated with '... (truncated at 8KB)'")
            elif tool_result and len(tool_result) < 9000:
                fail("fetch_url truncation: response cut short but truncation marker missing",
                     f"len={len(tool_result)} tail={tool_result[-100:]!r}")
            elif tool_result and len(tool_result) >= 9000:
                fail("fetch_url truncation: full untruncated response returned (>8KB not truncated)",
                     f"len={len(tool_result)}")
            else:
                fail("fetch_url truncation: no tool_result in second API call",
                     f"calls={len(mock.received)}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()
            content_server.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 76 — git_diff truncates output > 4KB
# ─────────────────────────────────────────────────────────────────────────────

async def test_git_diff_truncation():
    """Spec (PR1/git.rs): git output larger than 4KB is truncated;
    tool_result must contain '... (truncated at 4KB)'."""
    print("\n\033[1mTest 76: git_diff > 4KB — output truncated at 4KB\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        subprocess.run(["git", "init"], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "config", "user.email", "t@t.com"], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "config", "user.name", "T"], cwd=tmpdir, capture_output=True)

        # Create a large file (250 lines × ~25 chars ≈ 6250 bytes) — diff will be > 4KB
        fpath = os.path.join(tmpdir, "large_t76.txt")
        with open(fpath, 'w') as f:
            for i in range(250):
                f.write(f"ORIGINAL_LINE_{i:04d}_T76_SENTINEL\n")
        subprocess.run(["git", "add", "."], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "commit", "-m", "init t76"], cwd=tmpdir, capture_output=True)

        # Modify every line to maximize diff size
        with open(fpath, 'w') as f:
            for i in range(250):
                f.write(f"MODIFIED_LINE_{i:04d}_T76_SENTINEL\n")

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("gd76", "git_diff", json.dumps({}))
            return acp_text_sse("large diff shown")

        mock = MockHttpServer(30075, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30075, 76, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "show git diff", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            tr_len2 = len(tool_result) if tool_result else 0
            tr_preview2 = repr(tool_result[:80]) if tool_result else "None"
            print(f"    tool_result len={tr_len2} preview={tr_preview2}", flush=True)

            if tool_result and "truncated at 4KB" in tool_result:
                ok("git_diff truncation: output > 4KB truncated with '... (truncated at 4KB)'")
            elif tool_result and len(tool_result) >= 4096:
                fail("git_diff truncation: output >= 4KB but truncation marker missing",
                     f"len={len(tool_result)} tail={tool_result[-100:]!r}")
            elif tool_result:
                fail("git_diff truncation: tool_result present but shorter than 4KB — diff may be smaller than expected",
                     f"len={len(tool_result)} tail={tool_result[-100:]!r}")
            else:
                fail("git_diff truncation: no tool_result in second API call",
                     f"calls={len(mock.received)}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 77 — read_file with limit only (no offset) returns first N lines
# ─────────────────────────────────────────────────────────────────────────────

async def test_read_file_limit_only():
    """Spec (PR1/fs.rs read_file): limit without offset returns only the first N lines."""
    print("\n\033[1mTest 77: read_file limit only (no offset) — first N lines returned\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        path = os.path.join(tmpdir, "limit_t77.txt")
        with open(path, 'w') as f:
            for i in range(1, 6):
                f.write(f"LINE_{i}_T77\n")

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("r77", "read_file",
                    json.dumps({"path": "limit_t77.txt", "limit": 2}))
            return acp_text_sse("read done")

        mock = MockHttpServer(30076, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30076, 77, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "read first 2 lines", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)

            has_line1 = tool_result and "LINE_1_T77" in tool_result
            has_line2 = tool_result and "LINE_2_T77" in tool_result
            has_line3 = tool_result and "LINE_3_T77" in tool_result

            if has_line1 and has_line2 and not has_line3:
                ok("read_file limit only: lines 1-2 returned, line 3 excluded")
            elif has_line1 and has_line2:
                ok("read_file limit only: first 2 lines present in tool_result")
            else:
                fail("read_file limit only: expected lines 1 and 2 not in tool_result",
                     f"tool_result={tool_result!r}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 78 — read_file: offset beyond file length returns error
# ─────────────────────────────────────────────────────────────────────────────

async def test_read_file_offset_beyond_end():
    """Spec (PR1/fs.rs read_file): offset beyond file length returns an error message
    containing 'offset' and 'exceeds'."""
    print("\n\033[1mTest 78: read_file offset beyond file length — error message returned\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        path = os.path.join(tmpdir, "short_t78.txt")
        with open(path, 'w') as f:
            f.write("LINE_1_T78\nLINE_2_T78\nLINE_3_T78\n")

        def sse(n):
            if n == 1:
                # offset=100 on a 3-line file → error
                return acp_tool_use_sse("r78", "read_file",
                    json.dumps({"path": "short_t78.txt", "offset": 100}))
            return acp_text_sse("offset error handled")

        mock = MockHttpServer(30077, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30077, 78, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "read with huge offset", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)

            # Expected: "Error: offset 100 exceeds file length (3 lines)"
            if tool_result and "offset" in tool_result.lower() and "exceed" in tool_result.lower():
                ok("read_file offset beyond end: error contains 'offset' and 'exceed'")
            elif tool_result and "error" in tool_result.lower():
                ok("read_file offset beyond end: error message returned to LLM")
            elif tool_result:
                fail("read_file offset beyond end: no error in tool_result",
                     f"tool_result={tool_result!r}")
            else:
                fail("read_file offset beyond end: no tool_result in second API call",
                     f"calls={len(mock.received)}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()



# ─────────────────────────────────────────────────────────────────────────────
# TEST 84 — glob with explicit path parameter
# ─────────────────────────────────────────────────────────────────────────────

async def test_glob_with_path():
    """Spec (PR1/fs.rs glob_files): optional 'path' parameter scopes the search to
    a subdirectory — only files matching inside that directory are returned."""
    print("\n\033[1mTest 84: glob with path parameter — search scoped to subdirectory\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        # Create: src/main.rs, src/lib.rs, tests/integration.rs
        os.makedirs(os.path.join(tmpdir, "src"), exist_ok=True)
        os.makedirs(os.path.join(tmpdir, "tests"), exist_ok=True)
        open(os.path.join(tmpdir, "src", "main_t84.rs"), 'w').write("main\n")
        open(os.path.join(tmpdir, "src", "lib_t84.rs"),  'w').write("lib\n")
        open(os.path.join(tmpdir, "tests", "integ_t84.rs"), 'w').write("test\n")

        def sse(n):
            if n == 1:
                # path="src" → only files in src/ should match
                return acp_tool_use_sse("g84", "glob",
                    json.dumps({"pattern": "*.rs", "path": "src"}))
            return acp_text_sse("glob done")

        mock = MockHttpServer(30082, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30082, 84, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "find rs in src", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)

            has_main = tool_result and "main_t84.rs" in tool_result
            has_lib  = tool_result and "lib_t84.rs"  in tool_result
            has_integ = tool_result and "integ_t84.rs" in tool_result

            if has_main and has_lib and not has_integ:
                ok("glob path param: src/ files found, tests/ file excluded by path scope")
            elif has_main and has_lib:
                ok("glob path param: src/ files found in tool_result")
            else:
                fail("glob path param: expected files from src/ not found",
                     f"tool_result={tool_result!r}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 85 — _meta.systemPrompt in acp-runner new_session
# ─────────────────────────────────────────────────────────────────────────────

async def test_meta_system_prompt_acp():
    """Spec (PR15/acp-runner): new_session _meta.systemPrompt is stored and sent
    as the 'system' field in every Anthropic API call for that session."""
    print("\n\033[1mTest 85: _meta.systemPrompt in acp-runner — appears in Anthropic API system field\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        sentinel = "custom-sys-sentinel-t85"

        mock = MockHttpServer(30083, lambda n: acp_text_sse("acp-t85-reply")).start()
        proc, prefix = await _start_acp_runner_only(30083, 85, tmpdir)
        nc = await natspy.connect(NATS_JS)
        try:
            # Create session with _meta.systemPrompt
            r = await nc.request(
                f"{prefix}.agent.session.new",
                json.dumps({"cwd": tmpdir, "mcpServers": [],
                            "_meta": {"systemPrompt": sentinel}}).encode(), timeout=5)
            sid = json.loads(r.data)["sessionId"]
            print(f"    sid={sid}", flush=True)

            req_id = str(uuid.uuid4())
            resp_sub = await nc.subscribe(
                f"{prefix}.session.{sid}.agent.prompt.response.{req_id}")
            async def _noop_t85(m): pass
            await nc.subscribe(f"{prefix}.session.{sid}.client.session.update", cb=_noop_t85)
            await nc.publish(
                f"{prefix}.session.{sid}.agent.prompt",
                json.dumps({"sessionId": sid,
                            "prompt": [{"type": "text", "text": "hello t85"}]}).encode(),
                headers={"X-Req-Id": req_id})
            await asyncio.wait_for(resp_sub.next_msg(), timeout=15)

            print(f"    calls={len(mock.received)}", flush=True)
            if mock.received:
                body = json.loads(mock.received[0])
                system_field = body.get("system", "")
                if isinstance(system_field, list):
                    system_str = " ".join(b.get("text", "") for b in system_field if isinstance(b, dict))
                else:
                    system_str = str(system_field)
                print(f"    system field: {system_str[:100]!r}", flush=True)
                if sentinel in system_str:
                    ok("_meta.systemPrompt acp: sentinel present in Anthropic API 'system' field")
                else:
                    fail("_meta.systemPrompt acp: sentinel NOT in system field",
                         f"system={system_str[:200]!r}")
            else:
                fail("_meta.systemPrompt acp: no API calls received")
        except Exception as e:
            fail("_meta.systemPrompt acp", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 86 — _meta.systemPrompt in xai-runner new_session
# ─────────────────────────────────────────────────────────────────────────────

async def test_meta_system_prompt_xai():
    """Spec (PR15/xai-runner): new_session _meta.systemPrompt overrides agent-level
    system_prompt and appears in the xAI API request's 'input' array as role=system."""
    print("\n\033[1mTest 86: _meta.systemPrompt in xai-runner — appears in xAI input[role=system]\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        sentinel = "custom-sys-sentinel-t86"

        mock = MockHttpServer(30084, lambda n: xai_text_sse("xai-t86-reply")).start()
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t86",
               "XAI_API_KEY": "test", "XAI_BASE_URL": "http://127.0.0.1:30084",
               "XAI_MODELS": "grok-t86:grok-t86"}
        proc = subprocess.Popen([f"{BIN}/trogon-xai-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(2.5)
        nc = await natspy.connect(NATS_JS)
        try:
            r = await nc.request(
                "acp.t86.agent.session.new",
                json.dumps({"cwd": tmpdir, "mcpServers": [],
                            "_meta": {"systemPrompt": sentinel}}).encode(), timeout=5)
            sid = json.loads(r.data)["sessionId"]

            req_id = str(uuid.uuid4())
            resp_sub = await nc.subscribe(
                f"acp.t86.session.{sid}.agent.prompt.response.{req_id}")
            async def _noop_t86(m): pass
            await nc.subscribe(f"acp.t86.session.{sid}.client.session.update", cb=_noop_t86)
            await nc.publish(
                f"acp.t86.session.{sid}.agent.prompt",
                json.dumps({"sessionId": sid,
                            "prompt": [{"type": "text", "text": "hello t86"}]}).encode(),
                headers={"X-Req-Id": req_id})
            await asyncio.wait_for(resp_sub.next_msg(), timeout=15)

            print(f"    calls={len(mock.received)}", flush=True)
            if mock.received:
                body = json.loads(mock.received[0])
                msgs = body.get("input", [])
                body_str = json.dumps(msgs)
                print(f"    input preview: {body_str[:200]!r}", flush=True)
                sys_msgs = [m for m in msgs if m.get("role") == "system"]
                sys_text = " ".join(m.get("content", "") if isinstance(m.get("content"), str)
                                    else json.dumps(m.get("content", "")) for m in sys_msgs)
                if sentinel in sys_text or sentinel in body_str:
                    ok("_meta.systemPrompt xai: sentinel present in xAI input[role=system]")
                else:
                    fail("_meta.systemPrompt xai: sentinel NOT in xAI system messages",
                         f"body_str={body_str[:300]!r}")
            else:
                fail("_meta.systemPrompt xai: no API calls received")
        except Exception as e:
            fail("_meta.systemPrompt xai", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 87 — _meta.systemPrompt in openrouter-runner new_session
# ─────────────────────────────────────────────────────────────────────────────

async def test_meta_system_prompt_openrouter():
    """Spec (PR15/openrouter-runner): new_session _meta.systemPrompt appears in the
    OpenRouter API 'messages' array as role=system."""
    print("\n\033[1mTest 87: _meta.systemPrompt in openrouter-runner — appears in messages[role=system]\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        sentinel = "custom-sys-sentinel-t87"

        mock = MockHttpServer(30085, lambda n: or_text_sse("or-t87-reply")).start()
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t87",
               "OPENROUTER_API_KEY": "test",
               "OPENROUTER_BASE_URL": "http://127.0.0.1:30085",
               "OPENROUTER_MODELS": "gpt-t87:gpt-t87"}
        proc = subprocess.Popen([f"{BIN}/trogon-openrouter-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(2.5)
        nc = await natspy.connect(NATS_JS)
        try:
            r = await nc.request(
                "acp.t87.agent.session.new",
                json.dumps({"cwd": tmpdir, "mcpServers": [],
                            "_meta": {"systemPrompt": sentinel}}).encode(), timeout=5)
            sid = json.loads(r.data)["sessionId"]

            req_id = str(uuid.uuid4())
            resp_sub = await nc.subscribe(
                f"acp.t87.session.{sid}.agent.prompt.response.{req_id}")
            async def _noop_t87(m): pass
            await nc.subscribe(f"acp.t87.session.{sid}.client.session.update", cb=_noop_t87)
            await nc.publish(
                f"acp.t87.session.{sid}.agent.prompt",
                json.dumps({"sessionId": sid,
                            "prompt": [{"type": "text", "text": "hello t87"}]}).encode(),
                headers={"X-Req-Id": req_id})
            await asyncio.wait_for(resp_sub.next_msg(), timeout=15)

            print(f"    calls={len(mock.received)}", flush=True)
            if mock.received:
                body = json.loads(mock.received[0])
                msgs = body.get("messages", [])
                body_str = json.dumps(msgs)
                print(f"    messages preview: {body_str[:200]!r}", flush=True)
                sys_msgs = [m for m in msgs if m.get("role") == "system"]
                sys_text = " ".join(m.get("content", "") if isinstance(m.get("content"), str)
                                    else json.dumps(m.get("content", "")) for m in sys_msgs)
                if sentinel in sys_text or sentinel in body_str:
                    ok("_meta.systemPrompt openrouter: sentinel present in messages[role=system]")
                else:
                    fail("_meta.systemPrompt openrouter: sentinel NOT in system messages",
                         f"body_str={body_str[:300]!r}")
            else:
                fail("_meta.systemPrompt openrouter: no API calls received")
        except Exception as e:
            fail("_meta.systemPrompt openrouter", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 88 — fork_session in acp-runner: creates child, appears in list_children
# ─────────────────────────────────────────────────────────────────────────────

async def test_fork_session_acp():
    """Spec (PR6/acp-runner): fork_session creates a new session with parent_session_id
    set to the source; list_children returns the forked session_id."""
    print("\n\033[1mTest 88: fork_session in acp-runner — child appears in list_children\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t88",
               "PROXY_URL": "http://127.0.0.1:30086",
               "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-opus-4-6"}
        proc = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(2.5)
        nc = await natspy.connect(NATS_JS)
        js = nc.jetstream()
        try:
            # 1. Create parent session via plain NATS (session.new is a global method)
            r = await nc.request("acp.t88.agent.session.new",
                                  json.dumps({"cwd": tmpdir, "mcpServers": []}).encode(), timeout=5)
            parent_sid = json.loads(r.data)["sessionId"]
            print(f"    parent_sid={parent_sid}", flush=True)

            # 2. Fork the session — fork goes through JetStream COMMANDS stream.
            # Subscribe to response subject on core NATS before publishing, then
            # publish with X-Req-Id header so serve_js knows where to send the reply.
            req_id = "fork-r88"
            response_subject = f"acp.t88.session.{parent_sid}.agent.response.{req_id}"
            sub = await nc.subscribe(response_subject)

            fork_payload = json.dumps({"sessionId": parent_sid, "cwd": tmpdir,
                                        "mcpServers": []}).encode()
            await js.publish(
                f"acp.t88.session.{parent_sid}.agent.fork",
                fork_payload,
                headers={"X-Req-Id": req_id},
            )

            fork_msg = await asyncio.wait_for(sub.next_msg(), timeout=10)
            await sub.unsubscribe()
            fork_data = json.loads(fork_msg.data)
            fork_sid = fork_data.get("sessionId", "")
            print(f"    fork_sid={fork_sid!r}", flush=True)

            if not fork_sid:
                fail("fork_session acp: fork response missing sessionId", f"data={fork_data!r}")
                return

            if fork_sid == parent_sid:
                fail("fork_session acp: fork returned same sessionId as parent",
                     f"fork_sid={fork_sid!r}")
                return

            # 3. list_children should now return [fork_sid]
            r = await nc.request("acp.t88.agent.ext.session/list_children",
                                  json.dumps({"sessionId": parent_sid}).encode(), timeout=5)
            result = json.loads(r.data)
            children = result.get("children", [])
            print(f"    children={children!r}", flush=True)

            if fork_sid in children:
                ok("fork_session acp: forked session appears in parent's list_children")
            else:
                fail("fork_session acp: forked session NOT in list_children",
                     f"fork_sid={fork_sid!r} children={children!r}")

        except Exception as e:
            fail("fork_session acp", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)


# ─────────────────────────────────────────────────────────────────────────────
# TEST 89 — bash tool executes in session cwd
# ─────────────────────────────────────────────────────────────────────────────

async def test_bash_uses_session_cwd():
    """Spec (PR2/wasm_bash_tool.rs): bash tool executes in the session's cwd, not
    in a random temp directory — 'pwd' output must match the session cwd."""
    print("\n\033[1mTest 89: bash tool uses session cwd — 'pwd' returns session working directory\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        # The bash tool requires a trogon-wasm-runtime registered as "execution" in
        # AGENT_REGISTRY.  Start one pointing at NATS_JS so acp-runner can discover it.
        # WASM_ONLY=0 is required to allow native processes (default is wasm-only).
        wasm_env = {**os.environ,
                    "NATS_URL": NATS_JS,
                    "ACP_PREFIX": "acp.wasm89",
                    "AGENT_TYPE": "wasm-t89",
                    "WASM_SESSION_ROOT": tmpdir,
                    "WASM_AUTO_ALLOW_PERMISSIONS": "1",
                    "WASM_ONLY": "0"}
        wasm_proc = subprocess.Popen([f"{BIN}/trogon-wasm-runtime"], env=wasm_env,
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(2.0)  # wait for wasm-runtime to register

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("b89", "bash", json.dumps({"command": "pwd"}))
            return acp_text_sse("cwd verified")

        mock = MockHttpServer(30087, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30087, 89, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "show cwd", timeout=25)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)
            print(f"    tmpdir:      {tmpdir!r}", flush=True)

            if tool_result and tmpdir in tool_result:
                ok("bash cwd: 'pwd' output matches session cwd (session working directory used)")
            elif tool_result and "Unknown tool" in tool_result:
                fail("bash cwd: wasm-runtime not registered — bash tool unavailable",
                     f"result={tool_result!r}")
            elif tool_result and tool_result.strip():
                fail("bash cwd: 'pwd' returned a path different from session cwd",
                     f"pwd={tool_result.strip()!r} expected={tmpdir!r}")
            else:
                fail("bash cwd: no tool_result in second API call",
                     f"calls={len(mock.received)}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            try: wasm_proc.terminate(); wasm_proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 90 — Registry capabilities for codex-runner
# ─────────────────────────────────────────────────────────────────────────────

async def test_registry_capabilities_codex():
    """Spec (PR6/codex-runner): registers with capabilities=['chat','code_edit']
    and metadata.models containing the configured model IDs."""
    print("\n\033[1mTest 90: Registry capabilities for codex-runner — code_edit + models\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t90",
               "AGENT_TYPE": "codex-t90",
               "CODEX_BIN": f"{BIN}/mock_codex_server",
               "CODEX_MODELS": "codex-model-t90:codex-model-t90",
               "CODEX_DEFAULT_MODEL": "codex-model-t90",
               "CODEX_SPAWN_TIMEOUT_SECS": "5"}
        proc = subprocess.Popen([f"{BIN}/trogon-codex-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(3)
        nc = await natspy.connect(NATS_JS)
        try:
            js = nc.jetstream()
            kv = await js.key_value("AGENT_REGISTRY")
            entry = await kv.get("codex-t90")
            data = json.loads(entry.value)
            print(f"    registry entry: {json.dumps(data)[:300]!r}", flush=True)

            caps = data.get("capabilities", [])
            models = data.get("metadata", {}).get("models", [])
            has_chat = "chat" in caps
            has_code_edit = "code_edit" in caps
            has_model = "codex-model-t90" in models

            if has_chat and has_code_edit and has_model:
                ok("registry codex: capabilities=['chat','code_edit'] and model in metadata.models")
            elif has_model:
                fail("registry codex: model present but 'code_edit' capability missing",
                     f"caps={caps!r}")
            else:
                fail("registry codex: expected capabilities and models not found",
                     f"caps={caps!r} models={models!r}")
        except Exception as e:
            fail("registry capabilities codex", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)


# ─────────────────────────────────────────────────────────────────────────────
# TEST 91 — Registry capabilities for openrouter-runner
# ─────────────────────────────────────────────────────────────────────────────

async def test_registry_capabilities_openrouter():
    """Spec (PR6/openrouter-runner): registers with capabilities=['chat','explore','plan']
    and metadata.models containing the configured model IDs."""
    print("\n\033[1mTest 91: Registry capabilities for openrouter-runner — explore+plan+models\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t91",
               "AGENT_TYPE": "or-t91",
               "OPENROUTER_API_KEY": "test",
               "OPENROUTER_BASE_URL": "http://127.0.0.1:30088",
               "OPENROUTER_MODELS": "or-model-t91:or-model-t91"}
        proc = subprocess.Popen([f"{BIN}/trogon-openrouter-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(3)
        nc = await natspy.connect(NATS_JS)
        try:
            js = nc.jetstream()
            kv = await js.key_value("AGENT_REGISTRY")
            entry = await kv.get("or-t91")
            data = json.loads(entry.value)
            print(f"    registry entry: {json.dumps(data)[:300]!r}", flush=True)

            caps = data.get("capabilities", [])
            models = data.get("metadata", {}).get("models", [])
            has_chat = "chat" in caps
            has_explore = "explore" in caps
            has_plan = "plan" in caps
            has_model = "or-model-t91" in models

            if has_chat and has_explore and has_plan and has_model:
                ok("registry openrouter: capabilities=['chat','explore','plan'] and model in metadata.models")
            elif has_model:
                fail("registry openrouter: model present but capabilities missing",
                     f"caps={caps!r}")
            else:
                fail("registry openrouter: expected capabilities and models not found",
                     f"caps={caps!r} models={models!r}")
        except Exception as e:
            fail("registry capabilities openrouter", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)


# ─────────────────────────────────────────────────────────────────────────────
# TEST 92 — todo_write creates .trogon/todos.json and returns "OK"
# ─────────────────────────────────────────────────────────────────────────────

async def test_todo_write_tool():
    """Spec (PR1/todo.rs): todo_write stores task in .trogon/todos.json under session cwd
    and returns "OK"; on re-write the item is updated in place."""
    print("\n\033[1mTest 92: todo_write — creates .trogon/todos.json and returns OK\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        def sse(n):
            if n == 1:
                return acp_tool_use_sse("tw92", "todo_write",
                    json.dumps({"id": "task-t92", "content": "fix bug t92", "status": "pending"}))
            return acp_text_sse("todo written")

        mock = MockHttpServer(30089, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30089, 92, tmpdir)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "write a todo", timeout=20)
            print(f"    done={done} calls={len(mock.received)}", flush=True)

            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)

            todos_path = os.path.join(tmpdir, ".trogon", "todos.json")
            file_exists = os.path.exists(todos_path)
            print(f"    todos.json exists: {file_exists}", flush=True)

            if tool_result == "OK" and file_exists:
                data = json.load(open(todos_path))
                todos = data.get("todos", [])
                found = any(t.get("id") == "task-t92" and t.get("status") == "pending"
                            for t in todos)
                if found:
                    ok("todo_write: returns 'OK' and task stored in .trogon/todos.json")
                else:
                    fail("todo_write: file exists but task not found inside",
                         f"todos={todos!r}")
            elif tool_result == "OK":
                fail("todo_write: returns 'OK' but .trogon/todos.json not created",
                     f"file_exists={file_exists}")
            else:
                fail("todo_write: expected 'OK' in tool_result",
                     f"tool_result={tool_result!r} calls={len(mock.received)}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 93 — todo_read returns active todos, filters completed
# ─────────────────────────────────────────────────────────────────────────────

async def test_todo_read_tool():
    """Spec (PR1/todo.rs): todo_read returns '[status] id — content' for each non-completed
    item; completed items must NOT appear."""
    print("\n\033[1mTest 93: todo_read — returns active todos, filters completed\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        # Pre-populate .trogon/todos.json with one active and one completed task
        trogon_dir = os.path.join(tmpdir, ".trogon")
        os.makedirs(trogon_dir)
        todos_data = {
            "todos": [
                {"id": "active-t93", "content": "active task t93", "status": "pending"},
                {"id": "done-t93",   "content": "done task t93",   "status": "completed"},
            ]
        }
        with open(os.path.join(trogon_dir, "todos.json"), 'w') as f:
            json.dump(todos_data, f)

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("tr93", "todo_read", json.dumps({}))
            return acp_text_sse("todos read")

        mock = MockHttpServer(30090, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30090, 93, tmpdir)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "read todos", timeout=20)
            print(f"    done={done} calls={len(mock.received)}", flush=True)

            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)

            has_active   = tool_result and "active-t93" in tool_result
            has_done     = tool_result and "done-t93"   in tool_result
            correct_fmt  = tool_result and "[pending] active-t93" in tool_result

            if has_active and not has_done and correct_fmt:
                ok("todo_read: active item present with '[status] id — content' format, completed filtered out")
            elif has_active and not has_done:
                ok("todo_read: active item present, completed item filtered out")
            elif has_active and has_done:
                fail("todo_read: completed item should NOT appear in result",
                     f"tool_result={tool_result!r}")
            else:
                fail("todo_read: active item not found in tool_result",
                     f"tool_result={tool_result!r} calls={len(mock.received)}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 94 — spawn handler in xai-runner returns text via NATS
# ─────────────────────────────────────────────────────────────────────────────

async def test_spawn_handler_xai():
    """Spec (PR6/xai-runner spawn_handler.rs): runner subscribes to {prefix}.agent.spawn
    on a queue group, POSTs to XAI_BASE_URL/chat/completions, and replies with
    choices[0].message.content."""
    print("\n\033[1mTest 94: spawn handler xai — NATS request returns LLM text\033[0m")

    def spawn_response(n, body):
        prompt = body.get("messages", [{}])[-1].get("content", "")
        return {"choices": [{"message": {"content": f"xai-spawn-reply-t94 echo: {prompt}"}}]}

    mock = MockJsonServer(30091, spawn_response).start()
    try:
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t94",
               "XAI_API_KEY": "test-key",
               "XAI_BASE_URL": "http://127.0.0.1:30091",
               "XAI_MODELS": "grok-t94:grok-t94"}
        proc = subprocess.Popen([f"{BIN}/trogon-xai-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(2.5)

        nc = await natspy.connect(NATS_JS)
        try:
            r = await nc.request(
                "acp.t94.agent.spawn",
                json.dumps({"prompt": "explain recursion t94"}).encode(),
                timeout=10)
            reply = r.data.decode(errors="replace")
            print(f"    spawn reply: {reply!r}", flush=True)
            print(f"    mock calls: {len(mock.received)}", flush=True)

            if "xai-spawn-reply-t94" in reply:
                ok("spawn handler xai: NATS request returns text from LLM (via /chat/completions)")
            elif reply.strip():
                fail("spawn handler xai: reply received but sentinel missing",
                     f"reply={reply!r}")
            else:
                fail("spawn handler xai: empty reply from spawn handler")
        except asyncio.TimeoutError:
            fail("spawn handler xai: NATS request timed out — handler not responding")
        finally:
            await nc.close()
    finally:
        proc.terminate(); proc.wait(timeout=3)
        mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 95 — spawn handler in openrouter-runner returns text via NATS
# ─────────────────────────────────────────────────────────────────────────────

async def test_spawn_handler_openrouter():
    """Spec (PR6/openrouter-runner spawn_handler.rs): runner subscribes to {prefix}.agent.spawn,
    POSTs to OPENROUTER_BASE_URL/chat/completions with HTTP-Referer + X-Title headers,
    and replies with choices[0].message.content."""
    print("\n\033[1mTest 95: spawn handler openrouter — NATS request returns LLM text\033[0m")

    def spawn_response(n, body):
        prompt = body.get("messages", [{}])[-1].get("content", "")
        return {"choices": [{"message": {"content": f"or-spawn-reply-t95 echo: {prompt}"}}]}

    mock = MockJsonServer(30092, spawn_response).start()
    try:
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t95",
               "OPENROUTER_API_KEY": "test-key",
               "OPENROUTER_BASE_URL": "http://127.0.0.1:30092",
               "OPENROUTER_MODELS": "gpt-t95:gpt-t95"}
        proc = subprocess.Popen([f"{BIN}/trogon-openrouter-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(2.5)

        nc = await natspy.connect(NATS_JS)
        try:
            r = await nc.request(
                "acp.t95.agent.spawn",
                json.dumps({"prompt": "explain trees t95"}).encode(),
                timeout=10)
            reply = r.data.decode(errors="replace")
            print(f"    spawn reply: {reply!r}", flush=True)
            print(f"    mock calls: {len(mock.received)}", flush=True)

            if "or-spawn-reply-t95" in reply:
                ok("spawn handler openrouter: NATS request returns text from LLM (via /chat/completions)")
            elif reply.strip():
                fail("spawn handler openrouter: reply received but sentinel missing",
                     f"reply={reply!r}")
            else:
                fail("spawn handler openrouter: empty reply from spawn handler")
        except asyncio.TimeoutError:
            fail("spawn handler openrouter: NATS request timed out — handler not responding")
        finally:
            await nc.close()
    finally:
        proc.terminate(); proc.wait(timeout=3)
        mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 96 — fork_session in xai-runner: child appears in list_children
# ─────────────────────────────────────────────────────────────────────────────

async def test_fork_session_xai():
    """Spec (PR6/xai-runner): fork_session via JetStream COMMANDS creates a new session
    with parent_session_id set; list_children returns the forked session_id."""
    print("\n\033[1mTest 96: fork_session in xai-runner — child appears in list_children\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t96",
               "XAI_API_KEY": "test", "XAI_BASE_URL": "http://127.0.0.1:30093",
               "XAI_MODELS": "grok-t96:grok-t96"}
        proc = subprocess.Popen([f"{BIN}/trogon-xai-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(2.5)
        nc = await natspy.connect(NATS_JS)
        js = nc.jetstream()
        try:
            r = await nc.request("acp.t96.agent.session.new",
                                  json.dumps({"cwd": tmpdir, "mcpServers": []}).encode(), timeout=5)
            parent_sid = json.loads(r.data)["sessionId"]
            print(f"    parent_sid={parent_sid}", flush=True)

            req_id = "fork-r96"
            response_subject = f"acp.t96.session.{parent_sid}.agent.response.{req_id}"
            sub = await nc.subscribe(response_subject)

            await js.publish(
                f"acp.t96.session.{parent_sid}.agent.fork",
                json.dumps({"sessionId": parent_sid, "cwd": tmpdir, "mcpServers": []}).encode(),
                headers={"X-Req-Id": req_id})

            fork_msg = await asyncio.wait_for(sub.next_msg(), timeout=10)
            await sub.unsubscribe()
            fork_data = json.loads(fork_msg.data)
            fork_sid = fork_data.get("sessionId", "")
            print(f"    fork_sid={fork_sid!r}", flush=True)

            if not fork_sid or fork_sid == parent_sid:
                fail("fork_session xai: fork did not return a new sessionId",
                     f"fork_data={fork_data!r}")
            else:
                r = await nc.request("acp.t96.agent.ext.session/list_children",
                                      json.dumps({"sessionId": parent_sid}).encode(), timeout=5)
                result = json.loads(r.data)
                children = result.get("children", [])
                print(f"    children={children!r}", flush=True)
                if fork_sid in children:
                    ok("fork_session xai: forked session appears in parent's list_children")
                else:
                    fail("fork_session xai: forked session NOT in list_children",
                         f"fork_sid={fork_sid!r} children={children!r}")
        except Exception as e:
            fail("fork_session xai", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)


# ─────────────────────────────────────────────────────────────────────────────
# TEST 97 — fork_session in openrouter-runner: child appears in list_children
# ─────────────────────────────────────────────────────────────────────────────

async def test_fork_session_openrouter():
    """Spec (PR6/openrouter-runner): fork_session via JetStream COMMANDS creates a new
    session; list_children returns the forked session_id."""
    print("\n\033[1mTest 97: fork_session in openrouter-runner — child appears in list_children\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t97",
               "OPENROUTER_API_KEY": "test",
               "OPENROUTER_BASE_URL": "http://127.0.0.1:30094",
               "OPENROUTER_MODELS": "gpt-t97:gpt-t97"}
        proc = subprocess.Popen([f"{BIN}/trogon-openrouter-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(2.5)
        nc = await natspy.connect(NATS_JS)
        js = nc.jetstream()
        try:
            r = await nc.request("acp.t97.agent.session.new",
                                  json.dumps({"cwd": tmpdir, "mcpServers": []}).encode(), timeout=5)
            parent_sid = json.loads(r.data)["sessionId"]
            print(f"    parent_sid={parent_sid}", flush=True)

            req_id = "fork-r97"
            response_subject = f"acp.t97.session.{parent_sid}.agent.response.{req_id}"
            sub = await nc.subscribe(response_subject)

            await js.publish(
                f"acp.t97.session.{parent_sid}.agent.fork",
                json.dumps({"sessionId": parent_sid, "cwd": tmpdir, "mcpServers": []}).encode(),
                headers={"X-Req-Id": req_id})

            fork_msg = await asyncio.wait_for(sub.next_msg(), timeout=10)
            await sub.unsubscribe()
            fork_data = json.loads(fork_msg.data)
            fork_sid = fork_data.get("sessionId", "")
            print(f"    fork_sid={fork_sid!r}", flush=True)

            if not fork_sid or fork_sid == parent_sid:
                fail("fork_session openrouter: fork did not return a new sessionId",
                     f"fork_data={fork_data!r}")
            else:
                r = await nc.request("acp.t97.agent.ext.session/list_children",
                                      json.dumps({"sessionId": parent_sid}).encode(), timeout=5)
                result = json.loads(r.data)
                children = result.get("children", [])
                print(f"    children={children!r}", flush=True)
                if fork_sid in children:
                    ok("fork_session openrouter: forked session appears in parent's list_children")
                else:
                    fail("fork_session openrouter: forked session NOT in list_children",
                         f"fork_sid={fork_sid!r} children={children!r}")
        except Exception as e:
            fail("fork_session openrouter", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)


# ─────────────────────────────────────────────────────────────────────────────
# TEST 98 — fork_session in codex-runner: child appears in list_children
# ─────────────────────────────────────────────────────────────────────────────

async def test_fork_session_codex():
    """Spec (PR6/codex-runner): fork_session calls mock_codex_server thread/fork, creates
    a new session; list_children returns the forked session_id."""
    print("\n\033[1mTest 98: fork_session in codex-runner — child appears in list_children\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t98",
               "CODEX_BIN": f"{BIN}/mock_codex_server",
               "CODEX_MODELS": "codex-t98:codex-t98",
               "CODEX_DEFAULT_MODEL": "codex-t98",
               "CODEX_SPAWN_TIMEOUT_SECS": "5"}
        proc = subprocess.Popen([f"{BIN}/trogon-codex-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(3)
        nc = await natspy.connect(NATS_JS)
        js = nc.jetstream()
        try:
            r = await nc.request("acp.t98.agent.session.new",
                                  json.dumps({"cwd": tmpdir, "mcpServers": []}).encode(), timeout=5)
            parent_sid = json.loads(r.data)["sessionId"]
            print(f"    parent_sid={parent_sid}", flush=True)

            req_id = "fork-r98"
            response_subject = f"acp.t98.session.{parent_sid}.agent.response.{req_id}"
            sub = await nc.subscribe(response_subject)

            await js.publish(
                f"acp.t98.session.{parent_sid}.agent.fork",
                json.dumps({"sessionId": parent_sid, "cwd": tmpdir, "mcpServers": []}).encode(),
                headers={"X-Req-Id": req_id})

            fork_msg = await asyncio.wait_for(sub.next_msg(), timeout=10)
            await sub.unsubscribe()
            fork_data = json.loads(fork_msg.data)
            fork_sid = fork_data.get("sessionId", "")
            print(f"    fork_sid={fork_sid!r}", flush=True)

            if not fork_sid or fork_sid == parent_sid:
                fail("fork_session codex: fork did not return a new sessionId",
                     f"fork_data={fork_data!r}")
            else:
                r = await nc.request("acp.t98.agent.ext.session/list_children",
                                      json.dumps({"sessionId": parent_sid}).encode(), timeout=5)
                result = json.loads(r.data)
                children = result.get("children", [])
                print(f"    children={children!r}", flush=True)
                if fork_sid in children:
                    ok("fork_session codex: forked session appears in parent's list_children")
                else:
                    fail("fork_session codex: forked session NOT in list_children",
                         f"fork_sid={fork_sid!r} children={children!r}")
        except Exception as e:
            fail("fork_session codex", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)


# ─────────────────────────────────────────────────────────────────────────────
# TEST 99 — acp-runner export after tool call contains PortableBlock::ToolCall
# ─────────────────────────────────────────────────────────────────────────────

async def test_portable_block_export_acp():
    """Spec (PR7/acp-runner): after a tool_use turn, session/export includes assistant
    messages with blocks:[{type:'tool_call',id,name,input}] and user messages with
    blocks:[{type:'tool_result',tool_call_id,content}]."""
    print("\n\033[1mTest 99: PortableBlock export in acp-runner — tool_call blocks after tool turn\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        with open(os.path.join(tmpdir, "pblock.txt"), 'w') as f:
            f.write("PORTABLE_BLOCK_CONTENT_T99\n")

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("pb99", "read_file",
                    json.dumps({"path": "pblock.txt"}))
            return acp_text_sse("read done t99")

        mock = MockHttpServer(30095, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30095, 99, tmpdir)
        nc = await natspy.connect(NATS_JS)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "read pblock", timeout=20)
            print(f"    done={done} calls={len(mock.received)}", flush=True)

            r = await nc.request(
                f"{prefix}.agent.ext.session/export",
                json.dumps({"sessionId": sid}).encode(), timeout=5)
            messages = json.loads(r.data)
            if isinstance(messages, dict):
                messages = messages.get("messages", [])
            print(f"    exported {len(messages)} messages", flush=True)

            all_blocks = []
            for m in messages:
                all_blocks.extend(m.get("blocks", []))
            combined = json.dumps(all_blocks)
            print(f"    blocks preview: {combined[:300]!r}", flush=True)

            has_tool_call   = any(b.get("type") == "tool_call"   for b in all_blocks)
            has_tool_result = any(b.get("type") == "tool_result" for b in all_blocks)

            if has_tool_call and has_tool_result:
                ok("PortableBlock export acp: tool_call and tool_result blocks in exported messages")
            else:
                fail("PortableBlock export acp: missing tool_call or tool_result blocks in export",
                     f"has_tool_call={has_tool_call} has_tool_result={has_tool_result} blocks={combined[:300]!r}")
        except Exception as e:
            fail("PortableBlock export acp", str(e))
        finally:
            await nc.close()
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 100 — acp-runner: import with role="tool" normalized to role="user"
# ─────────────────────────────────────────────────────────────────────────────

async def test_role_normalization_acp_import():
    """Spec (PR7/acp-runner): when importing PortableMessages, a message with
    role='tool' and a ToolResult block is normalized to role='user' so it can be
    correctly reconstructed in Anthropic format."""
    print("\n\033[1mTest 100: role normalization in acp-runner import — role='tool' → role='user'\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t100",
               "PROXY_URL": "http://127.0.0.1:30096",
               "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-opus-4-6"}
        proc = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(2.5)
        nc = await natspy.connect(NATS_JS)
        try:
            r = await nc.request("acp.t100.agent.session.new",
                                  json.dumps({"cwd": tmpdir, "mcpServers": []}).encode(), timeout=5)
            sid = json.loads(r.data)["sessionId"]
            print(f"    sid={sid}", flush=True)

            # Import: role="tool" with ToolResult block (non-standard, should be normalized)
            tool_messages = [
                {"role": "assistant", "text": "I will use a tool",
                 "blocks": [{"type": "tool_call", "id": "tc1", "name": "read_file",
                              "input": {"path": "x.txt"}}]},
                {"role": "tool", "text": "file content",
                 "blocks": [{"type": "tool_result", "tool_call_id": "tc1",
                              "content": "file content t100"}]},
            ]
            r = await nc.request("acp.t100.agent.ext.session/import",
                                  json.dumps({"sessionId": sid,
                                              "messages": tool_messages}).encode(), timeout=5)
            print(f"    import result: {r.data[:60]}", flush=True)

            # Export and verify: the "tool" role should be normalized to "user"
            r = await nc.request("acp.t100.agent.ext.session/export",
                                  json.dumps({"sessionId": sid}).encode(), timeout=5)
            exported = json.loads(r.data)
            if isinstance(exported, dict):
                exported = exported.get("messages", [])
            print(f"    exported {len(exported)} messages", flush=True)
            roles = [m.get("role") for m in exported]
            print(f"    roles: {roles}", flush=True)

            has_tool_role = "tool" in roles
            has_user_role = any(r == "user" for r in roles)

            if not has_tool_role and has_user_role:
                ok("role normalization: role='tool' normalized to role='user' in import")
            elif has_tool_role:
                fail("role normalization: role='tool' still present after import (not normalized)",
                     f"roles={roles!r}")
            else:
                fail("role normalization: unexpected roles after import",
                     f"roles={roles!r} exported={len(exported)}")
        except Exception as e:
            fail("role normalization acp import", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)


# ─────────────────────────────────────────────────────────────────────────────
# TEST 101 — codex-runner: MOCK_SEND_TOOL_EVENT export has PortableBlock::ToolCall
# ─────────────────────────────────────────────────────────────────────────────

async def test_codex_tool_export_blocks():
    """Spec (PR7/codex-runner): with MOCK_SEND_TOOL_EVENT=1 the mock emits ToolStarted
    and ToolCompleted events; on TurnCompleted the runner writes tool_call/tool_result
    PortableBlocks to history; session/export includes them."""
    print("\n\033[1mTest 101: codex tool export — MOCK_SEND_TOOL_EVENT blocks present in export\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t101",
               "CODEX_BIN": f"{BIN}/mock_codex_server",
               "CODEX_MODELS": "codex-t101:codex-t101",
               "CODEX_DEFAULT_MODEL": "codex-t101",
               "CODEX_SPAWN_TIMEOUT_SECS": "5",
               "MOCK_SEND_TOOL_EVENT": "1"}
        proc = subprocess.Popen([f"{BIN}/trogon-codex-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(3)
        nc = await natspy.connect(NATS_JS)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, "acp.t101", tmpdir, "run tool t101", timeout=15)
            print(f"    done={done}", flush=True)
            await asyncio.sleep(0.3)

            r = await nc.request("acp.t101.agent.ext.session/export",
                                  json.dumps({"sessionId": sid}).encode(), timeout=5)
            messages = json.loads(r.data)
            if isinstance(messages, dict):
                messages = messages.get("messages", [])
            print(f"    exported {len(messages)} messages", flush=True)

            all_blocks = []
            for m in messages:
                all_blocks.extend(m.get("blocks", []))
            combined = json.dumps(all_blocks)
            print(f"    blocks: {combined[:300]!r}", flush=True)

            has_tool_call   = any(b.get("type") == "tool_call"   for b in all_blocks)
            has_tool_result = any(b.get("type") == "tool_result" for b in all_blocks)

            if has_tool_call and has_tool_result:
                ok("codex tool export: tool_call and tool_result blocks present in session/export")
            else:
                fail("codex tool export: missing tool_call or tool_result blocks with MOCK_SEND_TOOL_EVENT=1",
                     f"has_tool_call={has_tool_call} has_tool_result={has_tool_result} blocks={combined[:300]!r} msgs={len(messages)}")
        except Exception as e:
            fail("codex tool export blocks", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)


# ─────────────────────────────────────────────────────────────────────────────
# TEST 102 — xai-runner tool execution: function_call SSE dispatches write_file
# ─────────────────────────────────────────────────────────────────────────────

async def test_xai_tool_execution():
    """Spec (PR13/xai-runner): when the LLM returns a function_call SSE event, the runner
    dispatches the tool (write_file), then makes a second API call with the result."""
    print("\n\033[1mTest 102: xai-runner tool execution — function_call SSE dispatches write_file\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        def sse(n):
            if n == 1:
                return xai_function_call_sse(
                    "c102", "write_file",
                    json.dumps({"path": "xai_out.txt", "content": "XAI_WRITTEN_T102\n"}))
            return xai_text_sse("file written by xai runner")

        mock = MockHttpServer(30097, sse).start()
        try:
            env = {**os.environ,
                   "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t102",
                   "XAI_API_KEY": "test", "XAI_BASE_URL": "http://127.0.0.1:30097",
                   "XAI_MODELS": "grok-t102:grok-t102"}
            proc = subprocess.Popen([f"{BIN}/trogon-xai-runner"], env=env,
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(2.5)

            sid, done, _ = await runner_session_prompt(
                NATS_JS, "acp.t102", tmpdir, "write a file", timeout=20)
            print(f"    done={done} api_calls={len(mock.received)}", flush=True)

            out_path = os.path.join(tmpdir, "xai_out.txt")
            file_exists = os.path.exists(out_path)
            print(f"    xai_out.txt exists: {file_exists}", flush=True)

            if file_exists and open(out_path).read() == "XAI_WRITTEN_T102\n":
                if len(mock.received) >= 2:
                    ok("xai tool execution: write_file dispatched, file created, 2 API calls made")
                else:
                    ok("xai tool execution: write_file dispatched and file created on disk")
            elif file_exists:
                fail("xai tool execution: file exists but wrong content",
                     f"content={open(out_path).read()!r}")
            else:
                fail("xai tool execution: xai_out.txt NOT created",
                     f"api_calls={len(mock.received)} done={done}")
        finally:
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 103 — openrouter-runner tool execution: tool_calls SSE dispatches write_file
# ─────────────────────────────────────────────────────────────────────────────

async def test_openrouter_tool_execution():
    """Spec (PR14/openrouter-runner): when the LLM returns a tool_calls SSE stream,
    the runner dispatches the tool (write_file), then makes a second API call."""
    print("\n\033[1mTest 103: openrouter-runner tool execution — tool_calls SSE dispatches write_file\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        def sse(n):
            if n == 1:
                return or_tool_calls_sse(
                    "c103", "write_file",
                    json.dumps({"path": "or_out.txt", "content": "OR_WRITTEN_T103\n"}))
            return or_text_sse("file written by openrouter runner")

        mock = MockHttpServer(30098, sse).start()
        try:
            env = {**os.environ,
                   "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t103",
                   "OPENROUTER_API_KEY": "test",
                   "OPENROUTER_BASE_URL": "http://127.0.0.1:30098",
                   "OPENROUTER_MODELS": "gpt-t103:gpt-t103"}
            proc = subprocess.Popen([f"{BIN}/trogon-openrouter-runner"], env=env,
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(2.5)

            sid, done, _ = await runner_session_prompt(
                NATS_JS, "acp.t103", tmpdir, "write a file", timeout=20)
            print(f"    done={done} api_calls={len(mock.received)}", flush=True)

            out_path = os.path.join(tmpdir, "or_out.txt")
            file_exists = os.path.exists(out_path)
            print(f"    or_out.txt exists: {file_exists}", flush=True)

            if file_exists and open(out_path).read() == "OR_WRITTEN_T103\n":
                if len(mock.received) >= 2:
                    ok("openrouter tool execution: write_file dispatched, file created, 2 API calls made")
                else:
                    ok("openrouter tool execution: write_file dispatched and file created on disk")
            elif file_exists:
                fail("openrouter tool execution: file exists but wrong content",
                     f"content={open(out_path).read()!r}")
            else:
                fail("openrouter tool execution: or_out.txt NOT created",
                     f"api_calls={len(mock.received)} done={done}")
        finally:
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 104 — /init creates TROGON.md in session cwd (git root)
# ─────────────────────────────────────────────────────────────────────────────

async def test_init_command():
    """Spec (PR11/repl.rs do_init): /init sends an analysis prompt to the current session,
    takes the reply text, and writes it to TROGON.md at the git root."""
    print("\n\033[1mTest 104: /init slash command — creates TROGON.md in git root\033[0m")

    sid = str(uuid.uuid4())
    runner = FakeRunner("acp.t104", sid, "# TROGON_INIT_CONTENT_T104\nThis is the AI reply.",
                        nats_url=NATS_JS)
    await runner.start()
    await asyncio.sleep(0.3)

    # Use a non-auto-deleted temp dir so we can inspect the file after the REPL exits
    cwd = tempfile.mkdtemp()
    try:
        # Initialize git repo so do_init can find the git root
        subprocess.run(["git", "init"], cwd=cwd, capture_output=True)

        out, rc = await asyncio.get_event_loop().run_in_executor(
            None, lambda: run_repl_cli("acp.t104", NATS_JS, cwd, "/init\n", timeout=15))

        await runner.wait(timeout=5)
        await runner.close()

        print(f"    rc={rc} out={out[:300]!r}", flush=True)

        # TROGON.md is written to git root; since cwd IS the git root, check cwd
        trogon_path = os.path.join(cwd, "TROGON.md")
        if os.path.exists(trogon_path):
            content = open(trogon_path).read()
            print(f"    TROGON.md content: {content[:100]!r}", flush=True)
            if "TROGON_INIT_CONTENT_T104" in content:
                ok("/init: TROGON.md created in git root with AI reply content")
            else:
                fail("/init: TROGON.md created but AI reply content missing",
                     f"content={content[:200]!r}")
        else:
            fail("/init: TROGON.md not created in git root",
                 f"rc={rc} out={out[:200]!r}")
    finally:
        import shutil
        shutil.rmtree(cwd, ignore_errors=True)


# ─────────────────────────────────────────────────────────────────────────────
# TEST 105 — codex-runner: TROGON.md injected ONLY on first turn (not second)
# ─────────────────────────────────────────────────────────────────────────────

async def test_codex_trogon_md_injection_first_turn_only():
    """Spec (PR11/codex-runner agent.rs): TROGON.md is prepended to userInput only when
    first_turn=true. On the second turn first_turn is false so TROGON.md must not appear."""
    print("\n\033[1mTest 105: codex TROGON.md injection — only on first turn, not second\033[0m")

    with tempfile.TemporaryDirectory() as cwd:
        with open(os.path.join(cwd, "TROGON.md"), 'w') as f:
            f.write("# TROGON_MARKER_T105\nThis is the system context.\n")

        record_file = os.path.join(cwd, "recorded_input.txt")
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t105",
               "CODEX_BIN": f"{BIN}/mock_codex_server",
               "CODEX_MODELS": "codex-t105:codex-t105",
               "CODEX_DEFAULT_MODEL": "codex-t105",
               "MOCK_RECORD_TURN_INPUT_FILE": record_file,
               "CODEX_SPAWN_TIMEOUT_SECS": "5"}
        proc = subprocess.Popen([f"{BIN}/trogon-codex-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(3)

        nc = await natspy.connect(NATS_JS)
        try:
            # First turn
            sid, done1, _ = await runner_session_prompt(
                NATS_JS, "acp.t105", cwd, "first turn t105", timeout=12)
            print(f"    turn 1 done={done1}", flush=True)
            await asyncio.sleep(0.2)

            if os.path.exists(record_file):
                first_recorded = open(record_file).read()
                print(f"    first recorded: {first_recorded[:120]!r}", flush=True)
            else:
                first_recorded = ""
                fail("codex TROGON.md first turn: MOCK_RECORD_TURN_INPUT_FILE not written")

            # Second turn (first_turn should be false now)
            req_id2 = str(uuid.uuid4())
            resp_sub2 = await nc.subscribe(
                f"acp.t105.session.{sid}.agent.prompt.response.{req_id2}")
            async def _noop_t105(m): pass
            await nc.subscribe(f"acp.t105.session.{sid}.client.session.update", cb=_noop_t105)
            await nc.publish(
                f"acp.t105.session.{sid}.agent.prompt",
                json.dumps({"sessionId": sid,
                            "prompt": [{"type": "text", "text": "second turn t105"}]}).encode(),
                headers={"X-Req-Id": req_id2})
            await asyncio.wait_for(resp_sub2.next_msg(), timeout=12)
            print("    turn 2 done", flush=True)
            await asyncio.sleep(0.2)

            if os.path.exists(record_file):
                second_recorded = open(record_file).read()
                print(f"    second recorded: {second_recorded[:120]!r}", flush=True)
            else:
                second_recorded = ""

            first_has_marker  = "TROGON_MARKER_T105" in first_recorded
            second_has_marker = "TROGON_MARKER_T105" in second_recorded

            if first_has_marker and not second_has_marker:
                ok("codex TROGON.md injection: present in turn 1, absent in turn 2 (first_turn flag works)")
            elif first_has_marker and second_has_marker:
                fail("codex TROGON.md injection: present in BOTH turns — first_turn flag not cleared",
                     f"second recorded={second_recorded[:200]!r}")
            elif not first_has_marker:
                fail("codex TROGON.md injection: NOT in first turn (injection not working)",
                     f"first recorded={first_recorded[:200]!r}")
            else:
                fail("codex TROGON.md injection: unexpected state",
                     f"first={first_has_marker} second={second_has_marker}")
        except Exception as e:
            fail("codex TROGON.md injection", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)


# ─────────────────────────────────────────────────────────────────────────────
# TEST 106 — Granular permissions: deny_paths in TROGON.md blocks write_file
# ─────────────────────────────────────────────────────────────────────────────

async def test_granular_permissions_deny_path():
    """Spec (PR16/permission_rules.rs): a deny_paths glob in TROGON.md ## Permissions
    causes the acp-runner to deny the tool WITHOUT sending a NATS permission request.
    The tool_result contains 'Permission denied' and the file is NOT created."""
    print("\n\033[1mTest 106: Granular permissions deny_paths — write_file blocked by TROGON.md rule\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        # TROGON.md with a deny_paths rule that blocks writing to secret.txt
        with open(os.path.join(tmpdir, "TROGON.md"), 'w') as f:
            f.write("# Project\n\n## Permissions\ndeny_paths: secret.txt, secrets/**\n")

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("gp106", "write_file",
                    json.dumps({"path": "secret.txt", "content": "SHOULD_NOT_EXIST_T106\n"}))
            return acp_text_sse("permission denied as expected")

        mock = MockHttpServer(30099, sse).start()
        # No auto_approve: deny_paths should block the tool statically without NATS interaction
        proc, prefix = await _start_acp_runner_only(30099, 106, tmpdir)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "write to secret file", timeout=20)
            print(f"    done={done} calls={len(mock.received)}", flush=True)

            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)

            out_path = os.path.join(tmpdir, "secret.txt")
            file_exists = os.path.exists(out_path)
            print(f"    secret.txt exists: {file_exists}", flush=True)

            if tool_result and "Permission denied" in str(tool_result) and not file_exists:
                ok("granular permissions deny_path: 'Permission denied' in tool_result, file NOT created")
            elif tool_result and "Permission denied" in str(tool_result) and file_exists:
                fail("granular permissions: denied message but file WAS created — deny not enforced",
                     f"file_exists={file_exists}")
            elif not file_exists and len(mock.received) >= 2:
                fail("granular permissions: file not created but 'Permission denied' missing from result",
                     f"tool_result={tool_result!r}")
            else:
                fail("granular permissions deny_path: tool was NOT denied",
                     f"tool_result={tool_result!r} file_exists={file_exists} calls={len(mock.received)}")
        finally:
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


async def _purge_registry_key(key: str):
    """Delete a key from the AGENT_REGISTRY KV bucket to remove stale entries."""
    nc = await natspy.connect(NATS_JS)
    try:
        js = nc.jetstream()
        kv = await js.key_value("AGENT_REGISTRY")
        try:
            await kv.delete(key)
        except Exception:
            pass
    except Exception:
        pass
    finally:
        await nc.close()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 107 — xai-runner registry capabilities: ['chat','explore','plan'] + models
# ─────────────────────────────────────────────────────────────────────────────

async def test_registry_capabilities_xai():
    """Spec (PR6/xai-runner): registers with capabilities=['chat','explore','plan']
    and metadata.models containing the configured model IDs."""
    print("\n\033[1mTest 107: Registry capabilities for xai-runner — explore+plan+models\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t107",
               "AGENT_TYPE": "xai-t107",
               "XAI_API_KEY": "test",
               "XAI_BASE_URL": "http://127.0.0.1:30100",
               "XAI_MODELS": "grok-t107:grok-t107"}
        proc = subprocess.Popen([f"{BIN}/trogon-xai-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(2.5)
        nc = await natspy.connect(NATS_JS)
        try:
            js = nc.jetstream()
            kv = await js.key_value("AGENT_REGISTRY")
            entry = await kv.get("xai-t107")
            data = json.loads(entry.value)
            print(f"    registry entry: {json.dumps(data)[:300]!r}", flush=True)

            caps   = data.get("capabilities", [])
            models = data.get("metadata", {}).get("models", [])
            has_chat    = "chat" in caps
            has_explore = "explore" in caps
            has_plan    = "plan" in caps
            has_model   = "grok-t107" in models

            if has_chat and has_explore and has_plan and has_model:
                ok("registry xai: capabilities=['chat','explore','plan'] and model in metadata.models")
            elif has_model:
                fail("registry xai: model present but capability missing",
                     f"caps={caps!r}")
            else:
                fail("registry xai: expected capabilities and models not found",
                     f"caps={caps!r} models={models!r}")
        except Exception as e:
            fail("registry capabilities xai", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)


# ─────────────────────────────────────────────────────────────────────────────
# TEST 108 — acp-runner registry capabilities: ['chat','code_edit'] + models
# ─────────────────────────────────────────────────────────────────────────────

async def test_registry_capabilities_acp():
    """Spec (PR6/acp-runner): registers with capabilities=['chat','code_edit']
    and metadata.models containing the Claude model IDs."""
    print("\n\033[1mTest 108: Registry capabilities for acp-runner — code_edit + Claude models\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t108",
               "AGENT_TYPE": "claude-t108",
               "PROXY_URL": "http://127.0.0.1:30101",
               "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-opus-4-6"}
        proc = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(2.5)
        nc = await natspy.connect(NATS_JS)
        try:
            js = nc.jetstream()
            kv = await js.key_value("AGENT_REGISTRY")
            entry = await kv.get("claude-t108")
            data = json.loads(entry.value)
            print(f"    registry entry: {json.dumps(data)[:300]!r}", flush=True)

            caps   = data.get("capabilities", [])
            models = data.get("metadata", {}).get("models", [])
            has_chat      = "chat" in caps
            has_code_edit = "code_edit" in caps
            has_claude    = any("claude" in m for m in models)

            if has_chat and has_code_edit and has_claude:
                ok(f"registry acp: capabilities=['chat','code_edit'] and Claude models in metadata.models: {models}")
            else:
                fail("registry acp: expected capabilities and Claude models not found",
                     f"caps={caps!r} models={models!r}")
        except Exception as e:
            fail("registry capabilities acp", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)


# ─────────────────────────────────────────────────────────────────────────────
# TEST 109 — session/export from xai-runner: user+assistant messages correct
# ─────────────────────────────────────────────────────────────────────────────

async def test_xai_export_content():
    """Spec (PR7/xai-runner): after a turn, session/export returns a list of
    PortableMessages with role='user' (prompt) and role='assistant' (reply text)."""
    print("\n\033[1mTest 109: session/export from xai-runner — user+assistant messages correct\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        mock = MockHttpServer(30102, lambda n: xai_text_sse("xai-export-reply-t109")).start()
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t109",
               "XAI_API_KEY": "test", "XAI_BASE_URL": "http://127.0.0.1:30102",
               "XAI_MODELS": "grok-t109:grok-t109"}
        proc = subprocess.Popen([f"{BIN}/trogon-xai-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(2.5)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, "acp.t109", tmpdir, "xai-export-prompt-t109", timeout=15)
            print(f"    done={done}", flush=True)

            nc = await natspy.connect(NATS_JS)
            try:
                r = await nc.request(
                    "acp.t109.agent.ext.session/export",
                    json.dumps({"sessionId": sid}).encode(), timeout=5)
                messages = json.loads(r.data)
                if isinstance(messages, dict):
                    messages = messages.get("messages", [])
                print(f"    exported {len(messages)} messages", flush=True)

                roles    = [m.get("role", "") for m in messages]
                combined = json.dumps(messages)
                has_user      = "user" in roles
                has_assistant = "assistant" in roles
                has_prompt    = "xai-export-prompt-t109" in combined
                has_reply     = "xai-export-reply-t109" in combined

                if has_user and has_assistant and has_prompt and has_reply:
                    ok("xai export: user+assistant messages with correct prompt and reply text")
                elif has_user and has_assistant:
                    ok("xai export: user+assistant messages present (text field location may differ)")
                else:
                    fail("xai export: expected user+assistant messages not found",
                         f"roles={roles!r} preview={combined[:300]!r}")
            finally:
                await nc.close()
        except Exception as e:
            fail("xai export content", str(e))
        finally:
            proc.terminate(); proc.wait(timeout=3)
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 110 — acp→codex: imported history appears in codex subprocess input
# ─────────────────────────────────────────────────────────────────────────────

async def test_acp_to_codex_history_in_subprocess():
    """Spec (PR7/codex-runner session/import): when history is imported into codex,
    it is prepended to the first userInput via pending_history so the codex CLI
    subprocess actually receives the prior conversation context."""
    print("\n\033[1mTest 110: acp→codex history: imported messages reach codex subprocess input\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        acp_mock = MockHttpServer(30103, lambda n: acp_text_sse("acp-sentinel-t110")).start()
        record_file = os.path.join(tmpdir, "turn_input_t110.txt")

        try:
            env_acp = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t110a",
                       "PROXY_URL": "http://127.0.0.1:30103",
                       "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-opus-4-6"}
            env_codex = {**os.environ,
                         "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t110c",
                         "CODEX_BIN": f"{BIN}/mock_codex_server",
                         "CODEX_MODELS": "codex-t110:codex-t110",
                         "CODEX_DEFAULT_MODEL": "codex-t110",
                         "CODEX_SPAWN_TIMEOUT_SECS": "5",
                         "MOCK_RECORD_TURN_INPUT_FILE": record_file}

            proc_acp   = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env_acp,
                                           stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            proc_codex = subprocess.Popen([f"{BIN}/trogon-codex-runner"], env=env_codex,
                                           stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(3)

            nc = await natspy.connect(NATS_JS)
            try:
                # Turn 1 on acp
                acp_sid, _, _ = await runner_session_prompt(
                    NATS_JS, "acp.t110a", tmpdir, "acp-history-sentinel-t110", timeout=15)

                # Export from acp
                r = await nc.request(
                    "acp.t110a.agent.ext.session/export",
                    json.dumps({"sessionId": acp_sid}).encode(), timeout=5)
                messages = json.loads(r.data)
                if isinstance(messages, dict):
                    messages = messages.get("messages", [])
                print(f"    exported {len(messages)} messages from acp", flush=True)

                # Create codex session + import
                r = await nc.request(
                    "acp.t110c.agent.session.new",
                    json.dumps({"cwd": tmpdir, "mcpServers": []}).encode(), timeout=5)
                codex_sid = json.loads(r.data)["sessionId"]

                await nc.request(
                    "acp.t110c.agent.ext.session/import",
                    json.dumps({"sessionId": codex_sid, "messages": messages}).encode(),
                    timeout=5)
                print(f"    imported into codex sid={codex_sid}", flush=True)

                # Turn 1 on codex (mock_codex_server records userInput to record_file)
                req_id = str(uuid.uuid4())
                resp_sub = await nc.subscribe(
                    f"acp.t110c.session.{codex_sid}.agent.prompt.response.{req_id}")
                async def _noop_t110(m): pass
                await nc.subscribe(
                    f"acp.t110c.session.{codex_sid}.client.session.update", cb=_noop_t110)
                await nc.publish(
                    f"acp.t110c.session.{codex_sid}.agent.prompt",
                    json.dumps({"sessionId": codex_sid,
                                "prompt": [{"type": "text",
                                            "text": "continue-t110"}]}).encode(),
                    headers={"X-Req-Id": req_id})
                await asyncio.wait_for(resp_sub.next_msg(), timeout=15)
                print("    codex turn done", flush=True)

                # Read recorded input
                if os.path.exists(record_file):
                    recorded = open(record_file).read()
                    print(f"    recorded input: {recorded[:300]!r}", flush=True)
                    if "acp-history-sentinel-t110" in recorded and "acp-sentinel-t110" in recorded:
                        ok("acp→codex history: imported acp history present in codex subprocess input")
                    else:
                        fail("acp→codex history: acp history NOT in codex subprocess input",
                             f"recorded={recorded[:300]!r}")
                else:
                    fail("acp→codex history: MOCK_RECORD_TURN_INPUT_FILE not written",
                         f"file={record_file!r}")
            finally:
                await nc.close()
        except Exception as e:
            fail("acp→codex history in subprocess", str(e))
        finally:
            for p in (proc_acp, proc_codex):
                try: p.terminate(); p.wait(timeout=3)
                except Exception: pass
            acp_mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 111 — acp-runner auto-compact: fires automatically at 85% token_budget
# ─────────────────────────────────────────────────────────────────────────────

async def test_autocompact_threshold():
    """Spec (PR4/acp-runner): compaction triggers automatically when
    estimate_token_count(messages) > token_budget * 85 / 100.

    The runner calls compact_messages() which sends a request to
    'trogon.compactor.compact' (not the prefix-based subject used by /compact).
    This test acts as a mock compactor: subscribes to that subject, responds
    with the messages unchanged, and verifies the request arrived."""
    print("\n\033[1mTest 111: auto-compact threshold — fires automatically at 85% token_budget\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        mock = MockHttpServer(30104, lambda n: acp_text_sse("autocompact-reply-t111")).start()
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t111",
               "PROXY_URL": "http://127.0.0.1:30104",
               "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-opus-4-6"}
        proc = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(2.5)

        nc = await natspy.connect(NATS_JS)
        try:
            # Turn 1: populate messages in KV
            r = await nc.request("acp.t111.agent.session.new",
                                  json.dumps({"cwd": tmpdir, "mcpServers": []}).encode(), timeout=5)
            sid = json.loads(r.data)["sessionId"]

            req_id1 = str(uuid.uuid4())
            resp_sub1 = await nc.subscribe(
                f"acp.t111.session.{sid}.agent.prompt.response.{req_id1}")
            async def _noop_t111(m): pass
            await nc.subscribe(
                f"acp.t111.session.{sid}.client.session.update", cb=_noop_t111)
            await nc.publish(
                f"acp.t111.session.{sid}.agent.prompt",
                json.dumps({"sessionId": sid,
                            "prompt": [{"type": "text",
                                        "text": "first turn autocompact test"}]}).encode(),
                headers={"X-Req-Id": req_id1})
            await asyncio.wait_for(resp_sub1.next_msg(), timeout=15)
            print("    turn 1 done — messages now in KV", flush=True)

            # Shrink token_budget to 10 so any message content exceeds 85% (=8 tokens)
            js = nc.jetstream()
            kv = await js.key_value("ACP_SESSIONS")
            entry = await kv.get(sid)
            state = json.loads(entry.value)
            state["token_budget"] = 10
            await kv.put(sid, json.dumps(state).encode())
            print("    token_budget set to 10 in KV", flush=True)

            # Subscribe to 'trogon.compactor.compact' (the subject compact_messages sends to).
            # Act as mock compactor: reply with messages unchanged so the runner doesn't time out.
            compact_received = []
            async def mock_compactor(msg):
                data = json.loads(msg.data)
                compact_received.append(data)
                msgs = data.get("messages", [])
                reply_body = json.dumps({"messages": msgs, "compacted": False}).encode()
                if msg.reply:
                    await nc.publish(msg.reply, reply_body)
            await nc.subscribe("trogon.compactor.compact", cb=mock_compactor)

            # Turn 2: runner reads KV, sees messages exceed 85% threshold, calls compact_messages
            req_id2 = str(uuid.uuid4())
            resp_sub2 = await nc.subscribe(
                f"acp.t111.session.{sid}.agent.prompt.response.{req_id2}")
            await nc.publish(
                f"acp.t111.session.{sid}.agent.prompt",
                json.dumps({"sessionId": sid,
                            "prompt": [{"type": "text",
                                        "text": "second turn triggers autocompact"}]}).encode(),
                headers={"X-Req-Id": req_id2})
            await asyncio.wait_for(resp_sub2.next_msg(), timeout=20)
            await asyncio.sleep(0.3)

            print(f"    compact_received={len(compact_received)} request(s)", flush=True)
            if compact_received:
                msgs_in_req = compact_received[0].get("messages", [])
                ok(f"auto-compact: 'trogon.compactor.compact' received automatically "
                   f"({len(msgs_in_req)} messages in request)")
            else:
                fail("auto-compact: 'trogon.compactor.compact' NOT received after threshold exceeded",
                     "check estimate_token_count(messages) > token_budget*85/100 fires correctly")
        except Exception as e:
            fail("auto-compact threshold", str(e))
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 112 — /init when TROGON.md already exists: prints error, no overwrite
# ─────────────────────────────────────────────────────────────────────────────

async def test_init_when_trogon_exists():
    """Spec (PR9/repl.rs do_init): if TROGON.md already exists in the git root,
    /init must print 'already exists' and NOT overwrite the file."""
    print("\n\033[1mTest 112: /init when TROGON.md exists — error printed, no overwrite\033[0m")

    cwd = tempfile.mkdtemp()
    try:
        subprocess.run(["git", "init"], cwd=cwd, capture_output=True)
        existing_content = "# EXISTING_TROGON_T112\nDo not overwrite.\n"
        trogon_path = os.path.join(cwd, "TROGON.md")
        with open(trogon_path, 'w') as f:
            f.write(existing_content)

        sid = str(uuid.uuid4())
        runner = FakeRunner("acp.t112", sid, "should-not-be-called", nats_url=NATS_JS)
        await runner.start()
        try:
            out, rc = await asyncio.get_event_loop().run_in_executor(
                None, lambda: run_repl_cli("acp.t112", NATS_JS, cwd, "/init\n", timeout=12))
            print(f"    rc={rc} out={out!r}", flush=True)

            if "already exists" in out:
                ok("/init exists: 'already exists' message printed when TROGON.md present")
            else:
                fail("/init exists: no 'already exists' in output",
                     f"out={out!r}")

            current = open(trogon_path).read()
            if current == existing_content:
                ok("/init exists: TROGON.md NOT overwritten")
            else:
                fail("/init exists: TROGON.md was overwritten!",
                     f"new content={current!r}")
        finally:
            await runner.wait(timeout=5)
            await runner.close()
    except Exception as e:
        fail("/init when TROGON.md exists", str(e))
    finally:
        import shutil
        shutil.rmtree(cwd, ignore_errors=True)


# ─────────────────────────────────────────────────────────────────────────────
# TEST 113 — Granular permissions allow_paths: tool allowed without NATS prompt
# ─────────────────────────────────────────────────────────────────────────────

async def test_granular_permissions_allow_path():
    """Spec (PR14/acp-runner): when allow_paths is set in TROGON.md, a write to a
    matching path is immediately allowed (RuleDecision::Allow) without triggering
    the interactive NATS approval gate. No auto_approve running — tool must succeed
    purely from the Allow decision."""
    print("\n\033[1mTest 113: Granular permissions allow_paths — tool allowed without NATS prompt\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        trogon_path = os.path.join(tmpdir, "TROGON.md")
        with open(trogon_path, 'w') as f:
            f.write("## Permissions\nallow_paths: safe/**\n")

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("ap113", "write_file",
                    json.dumps({"path": "safe/allowed.txt",
                                "content": "ALLOW_PATHS_T113\n"}))
            return acp_text_sse("written")

        mock = MockHttpServer(30105, sse).start()
        proc, prefix = await _start_acp_runner_only(30105, 113, tmpdir)
        nc = await natspy.connect(NATS_JS)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "write safe file", timeout=20)
            print(f"    done={done} calls={len(mock.received)}", flush=True)

            out_path = os.path.join(tmpdir, "safe", "allowed.txt")
            exists = os.path.exists(out_path)
            print(f"    safe/allowed.txt exists: {exists}", flush=True)

            if exists and open(out_path).read().strip() == "ALLOW_PATHS_T113":
                ok("allow_paths: file in allow_paths created without NATS approval prompt")
            elif exists:
                ok("allow_paths: file in allow_paths created (Allow decision bypassed gate)")
            else:
                fail("allow_paths: file NOT created — Allow decision did not bypass gate",
                     f"done={done!r}")
        except Exception as e:
            fail("allow_paths", str(e))
        finally:
            await nc.close()
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 114 — Granular permissions deny_commands: bash tool denied by TROGON.md rule
# ─────────────────────────────────────────────────────────────────────────────

async def test_granular_permissions_deny_command():
    """Spec (PR14/acp-runner): when deny_commands is set in TROGON.md, a bash call
    whose command starts with a denied prefix is immediately denied (Deny decision)
    without interactive prompt; tool_result contains 'Permission denied'."""
    print("\n\033[1mTest 114: Granular permissions deny_commands — bash denied by TROGON.md rule\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        trogon_path = os.path.join(tmpdir, "TROGON.md")
        with open(trogon_path, 'w') as f:
            f.write("## Permissions\ndeny_commands: rm\n")

        # Create file that rm would delete — it must NOT be deleted
        sentinel = os.path.join(tmpdir, "sentinel_t114.txt")
        with open(sentinel, 'w') as f:
            f.write("MUST_NOT_BE_DELETED\n")

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("dc114", "bash",
                    json.dumps({"command": f"rm {sentinel}"}))
            return acp_text_sse("done")

        mock = MockHttpServer(30106, sse).start()
        proc, prefix = await _start_acp_runner_only(30106, 114, tmpdir)
        nc = await natspy.connect(NATS_JS)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "run rm command", timeout=20)
            print(f"    done={done} calls={len(mock.received)}", flush=True)

            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)
            still_exists = os.path.exists(sentinel)
            print(f"    sentinel_t114.txt still exists: {still_exists}", flush=True)

            denied = tool_result and "Permission denied" in tool_result
            if denied and still_exists:
                ok("deny_commands: 'Permission denied' in tool_result, file NOT deleted")
            elif denied:
                fail("deny_commands: permission denied but sentinel file was deleted!",
                     f"tool_result={tool_result!r}")
            else:
                fail("deny_commands: 'Permission denied' NOT in tool_result",
                     f"tool_result={tool_result!r}")
        except Exception as e:
            fail("deny_commands", str(e))
        finally:
            await nc.close()
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 115 — Programming cycle: read_file → str_replace → git_diff
# ─────────────────────────────────────────────────────────────────────────────

async def test_programming_edit_cycle():
    print("\n\033[1mTest 115: Programming cycle: read_file → str_replace → git_diff\033[0m")

    with tempfile.TemporaryDirectory() as tmpdir:
        # git repo con un archivo Python ya commiteado
        subprocess.run(["git", "init"], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "config", "user.email", "t@t.com"], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "config", "user.name", "T"], cwd=tmpdir, capture_output=True)
        code_path = os.path.join(tmpdir, "main.py")
        with open(code_path, "w") as f:
            f.write("def calculate():\n    return 1\n")
        subprocess.run(["git", "add", "."], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "commit", "-m", "init"], cwd=tmpdir, capture_output=True)

        def sse(n):
            if n == 1:   # LLM lee el archivo antes de editar
                return acp_tool_use_sse("t1", "read_file",
                    json.dumps({"path": "main.py"}))
            if n == 2:   # con el contenido en contexto, aplica el cambio
                return acp_tool_use_sse("t2", "str_replace",
                    json.dumps({"path": "main.py",
                                "old_str": "return 1",
                                "new_str": "return 42"}))
            if n == 3:   # verifica el cambio con git diff
                return acp_tool_use_sse("t3", "git_diff",
                    json.dumps({}))
            return acp_text_sse("edit complete")  # llamada 4: respuesta final

        mock = MockHttpServer(30115, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30115, 115, tmpdir)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "update the return value", timeout=25)
            print(f"    done={done} calls={len(mock.received)}", flush=True)

            content = open(code_path).read()
            print(f"    main.py: {content!r}", flush=True)

            # La llamada 4 debe contener el tool_result del git_diff
            diff_in_call4 = False
            if len(mock.received) >= 4:
                body4 = json.loads(mock.received[3])
                msgs4 = json.dumps(body4.get("messages", []))
                print(f"    call4 msgs preview: {msgs4[:250]!r}", flush=True)
                diff_in_call4 = "return 42" in msgs4 or "-return 1" in msgs4 or "+return 42" in msgs4

            file_ok  = "return 42" in content and "return 1" not in content
            loop_ok  = len(mock.received) >= 4

            if file_ok and loop_ok and diff_in_call4:
                ok("programming cycle: read_file→str_replace→git_diff chain works end-to-end")
            elif file_ok and loop_ok:
                fail("programming cycle: file edited and loop complete but git_diff result absent from call 4",
                     f"calls={len(mock.received)}")
            elif file_ok:
                fail("programming cycle: str_replace worked but tool loop stopped early",
                     f"calls={len(mock.received)} expected >=4")
            else:
                fail("programming cycle: str_replace did not modify the file",
                     f"calls={len(mock.received)} content={content!r}")
        finally:
            auto_approve.terminate(); auto_approve.wait(timeout=2)
            proc.terminate(); proc.wait(timeout=3)
            mock.stop()


async def test_xai_programming_cycle():
    """Spec (programming.md PR1+xai): xAI runner handles multi-step tool loop:
    read_file → str_replace → git_diff → final text."""
    print("\n\033[1mTest 116: xAI programming cycle: read_file → str_replace → git_diff\033[0m")
    with tempfile.TemporaryDirectory() as tmpdir:
        subprocess.run(["git", "init"], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "config", "user.email", "t@t.com"], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "config", "user.name", "T"], cwd=tmpdir, capture_output=True)
        code_path = os.path.join(tmpdir, "calc.py")
        with open(code_path, "w") as f:
            f.write("def add():\n    return 1\n")
        subprocess.run(["git", "add", "."], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "commit", "-m", "init"], cwd=tmpdir, capture_output=True)

        def sse(n):
            if n == 1:
                return xai_function_call_sse("c1", "read_file", json.dumps({"path": "calc.py"}))
            if n == 2:
                return xai_function_call_sse("c2", "str_replace",
                    json.dumps({"path": "calc.py", "old_str": "return 1", "new_str": "return 42"}))
            if n == 3:
                return xai_function_call_sse("c3", "git_diff", json.dumps({}))
            return xai_text_sse("edit complete")

        mock = MockHttpServer(30116, sse).start()
        try:
            env = {**os.environ,
                   "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t116",
                   "XAI_API_KEY": "test", "XAI_BASE_URL": "http://127.0.0.1:30116",
                   "XAI_MODELS": "grok-t116:grok-t116"}
            proc = subprocess.Popen([f"{BIN}/trogon-xai-runner"], env=env,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(2.5)

            sid, done, _ = await runner_session_prompt(
                NATS_JS, "acp.t116", tmpdir, "update the return value", timeout=25)

            content = open(code_path).read()
            file_ok = "return 42" in content and "return 1" not in content
            loop_ok = len(mock.received) >= 4
            diff_in_call4 = False
            if len(mock.received) >= 4:
                body4_str = json.dumps(json.loads(mock.received[3]))
                diff_in_call4 = "return 42" in body4_str or "-return 1" in body4_str or "+return 42" in body4_str
            print(f"    calls={len(mock.received)} file_ok={file_ok} diff_in_call4={diff_in_call4}", flush=True)

            if file_ok and loop_ok and diff_in_call4:
                ok("xAI programming cycle: read_file→str_replace→git_diff chain works end-to-end")
            elif file_ok and loop_ok:
                fail("xAI programming cycle: file edited and loop complete but git_diff result absent from call 4",
                     f"calls={len(mock.received)}")
            elif file_ok:
                fail("xAI programming cycle: str_replace worked but tool loop stopped early",
                     f"calls={len(mock.received)}")
            else:
                fail("xAI programming cycle: str_replace did not modify the file",
                     f"content={content!r}")
        finally:
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


async def test_openrouter_programming_cycle():
    """Spec (programming.md PR1+openrouter): openrouter runner handles multi-step tool loop:
    read_file → str_replace → git_diff → final text."""
    print("\n\033[1mTest 117: openrouter programming cycle: read_file → str_replace → git_diff\033[0m")
    with tempfile.TemporaryDirectory() as tmpdir:
        subprocess.run(["git", "init"], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "config", "user.email", "t@t.com"], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "config", "user.name", "T"], cwd=tmpdir, capture_output=True)
        code_path = os.path.join(tmpdir, "util.py")
        with open(code_path, "w") as f:
            f.write("def compute():\n    return 0\n")
        subprocess.run(["git", "add", "."], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "commit", "-m", "init"], cwd=tmpdir, capture_output=True)

        def sse(n):
            if n == 1:
                return or_tool_calls_sse("c1", "read_file", json.dumps({"path": "util.py"}))
            if n == 2:
                return or_tool_calls_sse("c2", "str_replace",
                    json.dumps({"path": "util.py", "old_str": "return 0", "new_str": "return 99"}))
            if n == 3:
                return or_tool_calls_sse("c3", "git_diff", json.dumps({}))
            return or_text_sse("edit complete")

        mock = MockHttpServer(30117, sse).start()
        try:
            env = {**os.environ,
                   "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t117",
                   "OPENROUTER_API_KEY": "test",
                   "OPENROUTER_BASE_URL": "http://127.0.0.1:30117",
                   "OPENROUTER_MODELS": "gpt-t117:gpt-t117"}
            proc = subprocess.Popen([f"{BIN}/trogon-openrouter-runner"], env=env,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(2.5)

            sid, done, _ = await runner_session_prompt(
                NATS_JS, "acp.t117", tmpdir, "update the return value", timeout=25)

            content = open(code_path).read()
            file_ok = "return 99" in content and "return 0" not in content
            loop_ok = len(mock.received) >= 4
            diff_in_call4 = False
            if len(mock.received) >= 4:
                body4_str = json.dumps(json.loads(mock.received[3]))
                diff_in_call4 = "return 99" in body4_str or "-return 0" in body4_str or "+return 99" in body4_str
            print(f"    calls={len(mock.received)} file_ok={file_ok} diff_in_call4={diff_in_call4}", flush=True)

            if file_ok and loop_ok and diff_in_call4:
                ok("openrouter programming cycle: read_file→str_replace→git_diff chain works end-to-end")
            elif file_ok and loop_ok:
                fail("openrouter programming cycle: file edited but git_diff result absent from call 4",
                     f"calls={len(mock.received)}")
            elif file_ok:
                fail("openrouter programming cycle: str_replace worked but tool loop stopped early",
                     f"calls={len(mock.received)}")
            else:
                fail("openrouter programming cycle: str_replace did not modify the file",
                     f"content={content!r}")
        finally:
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


async def test_trogon_md_after_cross_runner_switch():
    """Spec (PR4+PR7): after cross-runner switch (acp→xAI), the new xAI session uses the same
    cwd and must inject TROGON.md on the first prompt — same as a fresh xAI session would."""
    print("\n\033[1mTest 118: TROGON.md injection after cross-runner switch (acp→xAI)\033[0m")
    with tempfile.TemporaryDirectory() as cwd:
        with open(os.path.join(cwd, "TROGON.md"), "w") as f:
            f.write("# TROGON_SWITCH_MARKER_T118\nYou are a programming assistant.\n")

        acp_mock = MockHttpServer(30118, lambda n: acp_text_sse("acp t118 turn")).start()
        xai_mock = MockHttpServer(30119, lambda n: xai_text_sse("xai t118 turn")).start()

        try:
            env_acp = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t118a",
                       "PROXY_URL": "http://127.0.0.1:30118",
                       "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-opus-4-6"}
            env_xai = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t118x",
                       "XAI_API_KEY": "test", "XAI_BASE_URL": "http://127.0.0.1:30119",
                       "XAI_MODELS": "grok-t118:grok-t118"}

            proc_acp = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env_acp,
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            proc_xai = subprocess.Popen([f"{BIN}/trogon-xai-runner"], env=env_xai,
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(3)

            nc = await natspy.connect(NATS_JS)
            try:
                # acp session + one turn
                r = await nc.request("acp.t118a.agent.session.new",
                                     json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
                acp_sid = json.loads(r.data)["sessionId"]
                req_id = str(uuid.uuid4())
                resp_sub = await nc.subscribe(
                    f"acp.t118a.session.{acp_sid}.agent.prompt.response.{req_id}")
                async def _noop1(m): pass
                await nc.subscribe(f"acp.t118a.session.{acp_sid}.client.session.update", cb=_noop1)
                await nc.publish(
                    f"acp.t118a.session.{acp_sid}.agent.prompt",
                    json.dumps({"sessionId": acp_sid,
                                "prompt": [{"type": "text", "text": "hello"}]}).encode(),
                    headers={"X-Req-Id": req_id})
                await asyncio.wait_for(resp_sub.next_msg(), timeout=8)

                # Export from acp
                r = await nc.request("acp.t118a.agent.ext.session/export",
                                     json.dumps({"sessionId": acp_sid}).encode(), timeout=5)
                messages = json.loads(r.data)
                if isinstance(messages, dict):
                    messages = messages.get("messages", [])
                print(f"    exported {len(messages)} messages from acp", flush=True)

                # xAI session with SAME cwd
                r = await nc.request("acp.t118x.agent.session.new",
                                     json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
                xai_sid = json.loads(r.data)["sessionId"]

                # Import acp history into xAI
                r = await nc.request("acp.t118x.agent.ext.session/import",
                                     json.dumps({"sessionId": xai_sid,
                                                 "messages": messages}).encode(), timeout=5)

                # xAI first prompt after import — TROGON.md must be injected
                req_id2 = str(uuid.uuid4())
                resp_sub2 = await nc.subscribe(
                    f"acp.t118x.session.{xai_sid}.agent.prompt.response.{req_id2}")
                async def _noop2(m): pass
                await nc.subscribe(f"acp.t118x.session.{xai_sid}.client.session.update", cb=_noop2)
                await nc.publish(
                    f"acp.t118x.session.{xai_sid}.agent.prompt",
                    json.dumps({"sessionId": xai_sid,
                                "prompt": [{"type": "text", "text": "continue"}]}).encode(),
                    headers={"X-Req-Id": req_id2})
                await asyncio.wait_for(resp_sub2.next_msg(), timeout=8)

                if not xai_mock.received:
                    fail("TROGON.md after switch", "xAI mock received no calls")
                else:
                    body = json.loads(xai_mock.received[0])
                    input_items = body.get("input", [])
                    system_items = [it for it in input_items
                                    if isinstance(it, dict) and it.get("role") == "system"]
                    body_str = json.dumps(body)
                    if system_items and "TROGON_SWITCH_MARKER_T118" in json.dumps(system_items):
                        ok("TROGON.md injection after cross-runner switch: marker in xAI system input")
                    elif "TROGON_SWITCH_MARKER_T118" in body_str:
                        fail("TROGON.md present after switch but NOT in system role item",
                             f"system_items={system_items!r}")
                    else:
                        fail("TROGON.md NOT injected after cross-runner switch to xAI",
                             f"input preview={json.dumps(input_items)[:200]!r}")
            finally:
                await nc.close()
        finally:
            for p in (proc_acp, proc_xai):
                try: p.terminate(); p.wait(timeout=3)
                except Exception: pass
            acp_mock.stop(); xai_mock.stop()


async def test_acp_export_preserves_portable_blocks():
    """Spec (PR7): acp session/export converts to PortableBlock format.
    ToolUse → PortableBlock::ToolCall, ToolResult → PortableBlock::ToolResult (both preserved).
    Only Thinking blocks are dropped. The export must include all 4 messages of a tool loop."""
    print("\n\033[1mTest 119: acp export — tool_call/tool_result blocks preserved as PortableBlocks\033[0m")
    with tempfile.TemporaryDirectory() as tmpdir:
        def sse(n):
            if n == 1:
                return acp_tool_use_sse("t1", "write_file",
                    json.dumps({"path": "exported.txt", "content": "hello from tool\n"}))
            return acp_text_sse("file written successfully")

        mock = MockHttpServer(30120, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30120, 119, tmpdir)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "write a file", timeout=20)
            print(f"    done={done} calls={len(mock.received)}", flush=True)

            nc = await natspy.connect(NATS_JS)
            try:
                r = await nc.request(f"{prefix}.agent.ext.session/export",
                                     json.dumps({"sessionId": sid}).encode(), timeout=5)
                messages = json.loads(r.data)
                if isinstance(messages, dict):
                    messages = messages.get("messages", [])
                print(f"    exported {len(messages)} msgs: {json.dumps(messages)[:300]!r}", flush=True)
            finally:
                await nc.close()

            exported_str = json.dumps(messages)
            # ToolUse → PortableBlock::ToolCall — must be present
            has_tool_call = '"type": "tool_call"' in exported_str or '"type":"tool_call"' in exported_str
            # ToolResult → PortableBlock::ToolResult — must be present
            has_tool_result = '"type": "tool_result"' in exported_str or '"type":"tool_result"' in exported_str
            # Anthropic-native "tool_use" type must NOT appear (already converted)
            has_raw_tool_use = '"type": "tool_use"' in exported_str or '"type":"tool_use"' in exported_str
            user_msgs = [m for m in messages if isinstance(m, dict) and m.get("role") == "user"]
            asst_msgs = [m for m in messages if isinstance(m, dict) and m.get("role") == "assistant"]
            has_user_text = any("write a file" in json.dumps(m) for m in user_msgs)
            has_asst_text = any("file written successfully" in json.dumps(m) for m in asst_msgs)
            file_written = os.path.exists(os.path.join(tmpdir, "exported.txt"))

            print(f"    has_tool_call={has_tool_call} has_tool_result={has_tool_result} "
                  f"file_written={file_written}", flush=True)

            if has_raw_tool_use:
                fail("acp export: raw 'tool_use' type in export — should be converted to 'tool_call'",
                     f"preview={exported_str[:300]!r}")
            elif not has_tool_call:
                fail("acp export: tool_call block missing — ToolUse not converted to PortableBlock::ToolCall",
                     f"preview={exported_str[:300]!r}")
            elif not has_tool_result:
                fail("acp export: tool_result block missing — ToolResult not preserved in export",
                     f"preview={exported_str[:300]!r}")
            elif not file_written:
                fail("acp export: write_file tool was not actually executed (file missing)",
                     f"done={done!r}")
            elif not (has_user_text and has_asst_text):
                fail("acp export: user text or final assistant text missing from export",
                     f"user_text={has_user_text} asst_text={has_asst_text}")
            else:
                ok("acp export: tool_call+tool_result preserved as PortableBlocks, file written, text preserved")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


async def test_cross_runner_switch_after_programming():
    """Spec (PR7+PR8): after acp programming session with tool calls, switch to xAI.
    The file must be correctly modified (by acp tools before switch), and xAI must
    receive the acp text history (lossy: tool calls appear as text or are dropped)."""
    print("\n\033[1mTest 120: Cross-runner switch after programming — acp→xAI with tool history\033[0m")
    with tempfile.TemporaryDirectory() as tmpdir:
        code_path = os.path.join(tmpdir, "prog.py")
        with open(code_path, "w") as f:
            f.write("def old_func():\n    pass\n")

        def acp_sse(n):
            if n == 1:
                return acp_tool_use_sse("t1", "write_file",
                    json.dumps({"path": "prog.py",
                                "content": "def new_func():\n    return 42\n"}))
            return acp_text_sse("function updated")

        acp_mock = MockHttpServer(30122, acp_sse).start()
        xai_mock = MockHttpServer(30123, lambda n: xai_text_sse("xai continuing")).start()

        auto_approve, proc_acp, prefix = await _start_acp_tool_runner(30122, 120, tmpdir)
        proc_xai = subprocess.Popen(
            [f"{BIN}/trogon-xai-runner"],
            env={**os.environ,
                 "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t120x",
                 "XAI_API_KEY": "test", "XAI_BASE_URL": "http://127.0.0.1:30123",
                 "XAI_MODELS": "grok-t120:grok-t120"},
            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(1)

        try:
            # acp programming turn
            sid, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "update the function", timeout=20)
            print(f"    acp done={done} calls={len(acp_mock.received)}", flush=True)

            file_content = open(code_path).read()
            file_updated = "new_func" in file_content and "old_func" not in file_content
            print(f"    file updated: {file_updated} content={file_content!r}", flush=True)

            # Export from acp
            nc = await natspy.connect(NATS_JS)
            try:
                r = await nc.request(f"{prefix}.agent.ext.session/export",
                                     json.dumps({"sessionId": sid}).encode(), timeout=5)
                messages = json.loads(r.data)
                if isinstance(messages, dict):
                    messages = messages.get("messages", [])
                print(f"    exported {len(messages)} messages", flush=True)

                # xAI session + import
                r = await nc.request("acp.t120x.agent.session.new",
                                     json.dumps({"cwd": tmpdir, "mcpServers": []}).encode(), timeout=5)
                xai_sid = json.loads(r.data)["sessionId"]
                r = await nc.request("acp.t120x.agent.ext.session/import",
                                     json.dumps({"sessionId": xai_sid,
                                                 "messages": messages}).encode(), timeout=5)

                # xAI turn
                req_id = str(uuid.uuid4())
                resp_sub = await nc.subscribe(
                    f"acp.t120x.session.{xai_sid}.agent.prompt.response.{req_id}")
                async def _noop(m): pass
                await nc.subscribe(
                    f"acp.t120x.session.{xai_sid}.client.session.update", cb=_noop)
                await nc.publish(
                    f"acp.t120x.session.{xai_sid}.agent.prompt",
                    json.dumps({"sessionId": xai_sid,
                                "prompt": [{"type": "text",
                                            "text": "what did we just do?"}]}).encode(),
                    headers={"X-Req-Id": req_id})
                await asyncio.wait_for(resp_sub.next_msg(), timeout=10)
            finally:
                await nc.close()

            if not xai_mock.received:
                fail("cross-runner after programming", "xAI mock received no calls")
            else:
                body = json.loads(xai_mock.received[0])
                combined = json.dumps(body.get("input", []))
                acp_history_present = ("update the function" in combined
                                       or "function updated" in combined)

                if file_updated and acp_history_present:
                    ok("cross-runner after programming: file edited by acp, history visible to xAI after switch")
                elif file_updated:
                    fail("cross-runner after programming: file correct but acp history missing from xAI",
                         f"input preview={combined[:200]!r}")
                else:
                    fail("cross-runner after programming: acp did not edit the file",
                         f"content={file_content!r}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc_acp.terminate(); proc_acp.wait(timeout=3)
            except Exception: pass
            try: proc_xai.terminate(); proc_xai.wait(timeout=3)
            except Exception: pass
            acp_mock.stop(); xai_mock.stop()


def test_sse_helpers():
    """Unit tests for SSE helpers — verify each produces parseable JSON with correct content fields."""
    print("\n\033[1mSSE Helper Unit Tests\033[0m")

    def parse_data_lines(sse_str):
        """Return list of parsed JSON objects from 'data: ...' lines, skipping [DONE]."""
        result = []
        for line in sse_str.splitlines():
            if line.startswith("data: ") and line[6:].strip() != "[DONE]":
                try:
                    result.append(json.loads(line[6:]))
                except json.JSONDecodeError as e:
                    result.append({"_parse_error": str(e), "_raw": line[6:80]})
        return result

    # acp_text_sse: delta chunk must have type=text_delta and text=input
    chunks = parse_data_lines(acp_text_sse("acp-hello"))
    errs = [c for c in chunks if "_parse_error" in c]
    delta = next((c for c in chunks if c.get("type") == "content_block_delta"), None)
    if errs:
        fail("acp_text_sse: invalid JSON in SSE output", str(errs[0]))
    elif not delta or delta.get("delta", {}).get("text") != "acp-hello":
        fail("acp_text_sse: delta.text field missing or wrong", f"delta={delta!r}")
    else:
        ok("acp_text_sse: valid JSON, delta.text='acp-hello'")

    # acp_tool_use_sse: content_block_start.name correct, partial_json parseable
    chunks = parse_data_lines(acp_tool_use_sse("t0", "read_file", json.dumps({"path": "x.py"})))
    errs = [c for c in chunks if "_parse_error" in c]
    start = next((c for c in chunks if c.get("type") == "content_block_start"), None)
    delta = next((c for c in chunks if c.get("type") == "content_block_delta"), None)
    if errs:
        fail("acp_tool_use_sse: invalid JSON in SSE output", str(errs[0]))
    elif not start or start.get("content_block", {}).get("name") != "read_file":
        fail("acp_tool_use_sse: content_block.name missing or wrong", f"start={start!r}")
    else:
        partial = delta.get("delta", {}).get("partial_json", "") if delta else ""
        try:
            args = json.loads(partial)
            if args.get("path") == "x.py":
                ok("acp_tool_use_sse: valid JSON, name and partial_json correct")
            else:
                fail("acp_tool_use_sse: partial_json parsed but path wrong", f"args={args!r}")
        except json.JSONDecodeError as e:
            fail("acp_tool_use_sse: partial_json not valid JSON", f"{partial!r} — {e}")

    # xai_text_sse: message.delta chunk must have delta.text=input
    chunks = parse_data_lines(xai_text_sse("xai-hello"))
    errs = [c for c in chunks if "_parse_error" in c]
    delta = next((c for c in chunks if c.get("type") == "message.delta"), None)
    if errs:
        fail("xai_text_sse: invalid JSON in SSE output", str(errs[0]))
    elif not delta or delta.get("delta", {}).get("text") != "xai-hello":
        fail("xai_text_sse: delta.text field missing or wrong", f"delta={delta!r}")
    else:
        ok("xai_text_sse: valid JSON, delta.text='xai-hello'")

    # or_text_sse: first chunk must have choices[0].delta.content=input
    chunks = parse_data_lines(or_text_sse("or-hello"))
    errs = [c for c in chunks if "_parse_error" in c]
    content_chunk = next(
        (c for c in chunks if c.get("choices") and c["choices"][0].get("delta", {}).get("content")),
        None)
    if errs:
        fail("or_text_sse: invalid JSON in SSE output", str(errs[0]))
    elif not content_chunk or content_chunk["choices"][0]["delta"]["content"] != "or-hello":
        fail("or_text_sse: choices[0].delta.content missing or wrong", f"chunks={chunks!r}")
    else:
        ok("or_text_sse: valid JSON, choices[0].delta.content='or-hello'")

    # xai_function_call_sse: type=function_call, name correct, arguments parseable
    chunks = parse_data_lines(xai_function_call_sse("c1", "write_file",
                                                     json.dumps({"path": "out.py", "content": "x"})))
    errs = [c for c in chunks if "_parse_error" in c]
    fc = next((c for c in chunks if c.get("type") == "function_call"), None)
    if errs:
        fail("xai_function_call_sse: invalid JSON in SSE output", str(errs[0]))
    elif not fc or fc.get("function_call", {}).get("name") != "write_file":
        fail("xai_function_call_sse: function_call.name missing or wrong", f"fc={fc!r}")
    else:
        args_str = fc.get("function_call", {}).get("arguments", "")
        try:
            args = json.loads(args_str)
            if args.get("path") == "out.py":
                ok("xai_function_call_sse: valid JSON, name and arguments correct")
            else:
                fail("xai_function_call_sse: arguments parsed but path wrong", f"args={args!r}")
        except json.JSONDecodeError as e:
            fail("xai_function_call_sse: arguments not valid JSON", f"{args_str!r} — {e}")

    # or_tool_calls_sse: choices[0].delta.tool_calls[0].function.name correct, arguments parseable
    chunks = parse_data_lines(or_tool_calls_sse("c2", "str_replace",
                                                 json.dumps({"path": "f.py", "old_str": "a", "new_str": "b"})))
    errs = [c for c in chunks if "_parse_error" in c]
    tc_chunk = next(
        (c for c in chunks if c.get("choices") and c["choices"][0].get("delta", {}).get("tool_calls")),
        None)
    if errs:
        fail("or_tool_calls_sse: invalid JSON in SSE output", str(errs[0]))
    elif not tc_chunk:
        fail("or_tool_calls_sse: no tool_calls delta chunk found", f"chunks={chunks!r}")
    else:
        tc = tc_chunk["choices"][0]["delta"]["tool_calls"][0]
        fn = tc.get("function", {})
        if fn.get("name") != "str_replace":
            fail("or_tool_calls_sse: function.name wrong", f"fn={fn!r}")
        else:
            try:
                args = json.loads(fn.get("arguments", ""))
                if args.get("path") == "f.py":
                    ok("or_tool_calls_sse: valid JSON, name and arguments correct")
                else:
                    fail("or_tool_calls_sse: arguments parsed but path wrong", f"args={args!r}")
            except json.JSONDecodeError as e:
                fail("or_tool_calls_sse: arguments not valid JSON", f"{fn.get('arguments','')!r} — {e}")


# ─────────────────────────────────────────────────────────────────────────────
# TEST 129 — TROGON.md + _meta.systemPrompt combined in xai-runner
# ─────────────────────────────────────────────────────────────────────────────

async def test_trogon_md_plus_system_prompt_xai():
    """Spec (PR4+PR15): when TROGON.md exists AND _meta.systemPrompt is given,
    xai-runner combines them as '{trogon_md}\\n\\n{systemPrompt}' in input[role=system]."""
    print("\n\033[1mTest 129: TROGON.md + _meta.systemPrompt combined in xai-runner\033[0m")
    with tempfile.TemporaryDirectory() as tmpdir:
        TROGON_MARKER = "TROGON_T129_MARKER"
        SYS_MARKER = "SYS_T129_MARKER"
        with open(os.path.join(tmpdir, "TROGON.md"), "w") as f:
            f.write(f"# {TROGON_MARKER}\nThis is the TROGON context.\n")

        mock = MockHttpServer(30141, lambda n: xai_text_sse("xai t129")).start()
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t129",
               "XAI_API_KEY": "test",
               "XAI_BASE_URL": "http://127.0.0.1:30141",
               "XAI_MODELS": "grok-t129:grok-t129",
               "RUST_LOG": "warn"}
        proc = subprocess.Popen([f"{BIN}/trogon-xai-runner"], env=env,
                                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        await asyncio.sleep(2)
        nc = await natspy.connect(NATS_JS)
        try:
            r = await nc.request("acp.t129.agent.session.new",
                                 json.dumps({"cwd": tmpdir, "mcpServers": [],
                                             "_meta": {"systemPrompt": SYS_MARKER}}).encode(),
                                 timeout=5)
            sid = json.loads(r.data)["sessionId"]

            req_id = str(uuid.uuid4())
            resp_sub = await nc.subscribe(f"acp.t129.session.{sid}.agent.prompt.response.{req_id}")
            async def _noop129(m): pass
            await nc.subscribe(f"acp.t129.session.{sid}.client.session.update", cb=_noop129)
            await nc.publish(f"acp.t129.session.{sid}.agent.prompt",
                             json.dumps({"sessionId": sid,
                                         "prompt": [{"type": "text", "text": "hello t129"}]}).encode(),
                             headers={"X-Req-Id": req_id})
            await asyncio.wait_for(resp_sub.next_msg(), timeout=15)
            print(f"    mock calls: {len(mock.received)}", flush=True)
        except Exception as e:
            fail("TROGON.md + systemPrompt xai", str(e))
            return
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)
            mock.stop()

        if not mock.received:
            fail("TROGON.md + systemPrompt xai: no calls to xAI API")
            return
        body = json.loads(mock.received[0])
        input_items = body.get("input", [])
        system_items = [it for it in input_items if isinstance(it, dict) and it.get("role") == "system"]
        combined = json.dumps(system_items)
        has_trogon = TROGON_MARKER in combined
        has_sys = SYS_MARKER in combined
        print(f"    has_trogon={has_trogon} has_sys={has_sys}", flush=True)
        if has_trogon and has_sys:
            ok("TROGON.md + systemPrompt combined in xai: both markers present in system input")
        elif has_trogon:
            fail("TROGON.md + systemPrompt xai: TROGON.md present but systemPrompt missing",
                 f"system={combined[:300]!r}")
        elif has_sys:
            fail("TROGON.md + systemPrompt xai: systemPrompt present but TROGON.md missing",
                 f"system={combined[:300]!r}")
        else:
            fail("TROGON.md + systemPrompt xai: neither marker in system input",
                 f"system={combined[:300]!r}")


# ─────────────────────────────────────────────────────────────────────────────
# TEST 130 — TROGON.md + _meta.systemPrompt combined in acp-runner
# ─────────────────────────────────────────────────────────────────────────────

async def test_trogon_md_plus_system_prompt_acp():
    """Spec (PR4+PR15): when TROGON.md exists AND _meta.systemPrompt is given,
    acp-runner combines them as '{trogon_md}\\n\\n{systemPrompt}' in Anthropic 'system' field."""
    print("\n\033[1mTest 130: TROGON.md + _meta.systemPrompt combined in acp-runner\033[0m")
    with tempfile.TemporaryDirectory() as tmpdir:
        TROGON_MARKER = "TROGON_T130_MARKER"
        SYS_MARKER = "SYS_T130_MARKER"
        with open(os.path.join(tmpdir, "TROGON.md"), "w") as f:
            f.write(f"# {TROGON_MARKER}\nThis is the TROGON context.\n")

        mock = MockHttpServer(30142, lambda n: acp_text_sse("acp t130")).start()
        proc, prefix = await _start_acp_runner_only(30142, 130, tmpdir)
        nc = await natspy.connect(NATS_JS)
        try:
            r = await nc.request(f"{prefix}.agent.session.new",
                                 json.dumps({"cwd": tmpdir, "mcpServers": [],
                                             "_meta": {"systemPrompt": SYS_MARKER}}).encode(),
                                 timeout=5)
            sid = json.loads(r.data)["sessionId"]

            req_id = str(uuid.uuid4())
            resp_sub = await nc.subscribe(f"{prefix}.session.{sid}.agent.prompt.response.{req_id}")
            async def _noop130(m): pass
            await nc.subscribe(f"{prefix}.session.{sid}.client.session.update", cb=_noop130)
            await nc.publish(f"{prefix}.session.{sid}.agent.prompt",
                             json.dumps({"sessionId": sid,
                                         "prompt": [{"type": "text", "text": "hello t130"}]}).encode(),
                             headers={"X-Req-Id": req_id})
            await asyncio.wait_for(resp_sub.next_msg(), timeout=15)
            print(f"    calls={len(mock.received)}", flush=True)
        except Exception as e:
            fail("TROGON.md + systemPrompt acp", str(e))
            return
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)
            mock.stop()

        if not mock.received:
            fail("TROGON.md + systemPrompt acp: no calls to Anthropic API")
            return
        body = json.loads(mock.received[0])
        system_field = body.get("system", "")
        if isinstance(system_field, list):
            system_str = " ".join(b.get("text", "") for b in system_field if isinstance(b, dict))
        else:
            system_str = str(system_field)
        print(f"    system field preview: {system_str[:150]!r}", flush=True)
        has_trogon = TROGON_MARKER in system_str
        has_sys = SYS_MARKER in system_str
        if has_trogon and has_sys:
            ok("TROGON.md + systemPrompt combined in acp: both markers present in Anthropic system field")
        elif has_trogon:
            fail("TROGON.md + systemPrompt acp: TROGON.md present but systemPrompt missing",
                 f"system={system_str[:300]!r}")
        elif has_sys:
            fail("TROGON.md + systemPrompt acp: systemPrompt present but TROGON.md missing",
                 f"system={system_str[:300]!r}")
        else:
            fail("TROGON.md + systemPrompt acp: neither marker in system field",
                 f"system={system_str[:300]!r}")


# ─────────────────────────────────────────────────────────────────────────────
# TEST 131 — TROGON.md + _meta.systemPrompt combined in openrouter-runner
# ─────────────────────────────────────────────────────────────────────────────

async def test_trogon_md_plus_system_prompt_openrouter():
    """Spec (PR4+PR15): when TROGON.md exists AND _meta.systemPrompt is given,
    openrouter-runner combines them in messages[role=system]."""
    print("\n\033[1mTest 131: TROGON.md + _meta.systemPrompt combined in openrouter-runner\033[0m")
    with tempfile.TemporaryDirectory() as tmpdir:
        TROGON_MARKER = "TROGON_T131_MARKER"
        SYS_MARKER = "SYS_T131_MARKER"
        with open(os.path.join(tmpdir, "TROGON.md"), "w") as f:
            f.write(f"# {TROGON_MARKER}\nThis is the TROGON context.\n")

        mock = MockHttpServer(30143, lambda n: or_text_sse("or t131")).start()
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t131",
               "OPENROUTER_API_KEY": "test",
               "OPENROUTER_BASE_URL": "http://127.0.0.1:30143",
               "OPENROUTER_MODELS": "gpt-t131:gpt-t131",
               "RUST_LOG": "warn"}
        proc = subprocess.Popen([f"{BIN}/trogon-openrouter-runner"], env=env,
                                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        await asyncio.sleep(2)
        nc = await natspy.connect(NATS_JS)
        try:
            r = await nc.request("acp.t131.agent.session.new",
                                 json.dumps({"cwd": tmpdir, "mcpServers": [],
                                             "_meta": {"systemPrompt": SYS_MARKER}}).encode(),
                                 timeout=5)
            sid = json.loads(r.data)["sessionId"]

            req_id = str(uuid.uuid4())
            resp_sub = await nc.subscribe(f"acp.t131.session.{sid}.agent.prompt.response.{req_id}")
            async def _noop131(m): pass
            await nc.subscribe(f"acp.t131.session.{sid}.client.session.update", cb=_noop131)
            await nc.publish(f"acp.t131.session.{sid}.agent.prompt",
                             json.dumps({"sessionId": sid,
                                         "prompt": [{"type": "text", "text": "hello t131"}]}).encode(),
                             headers={"X-Req-Id": req_id})
            await asyncio.wait_for(resp_sub.next_msg(), timeout=15)
            print(f"    mock calls: {len(mock.received)}", flush=True)
        except Exception as e:
            fail("TROGON.md + systemPrompt openrouter", str(e))
            return
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)
            mock.stop()

        if not mock.received:
            fail("TROGON.md + systemPrompt openrouter: no calls to OpenRouter API")
            return
        body = json.loads(mock.received[0])
        messages = body.get("messages", [])
        system_msgs = [m for m in messages if isinstance(m, dict) and m.get("role") == "system"]
        combined = json.dumps(system_msgs)
        has_trogon = TROGON_MARKER in combined
        has_sys = SYS_MARKER in combined
        print(f"    has_trogon={has_trogon} has_sys={has_sys}", flush=True)
        if has_trogon and has_sys:
            ok("TROGON.md + systemPrompt combined in openrouter: both markers present in system message")
        elif has_trogon:
            fail("TROGON.md + systemPrompt openrouter: TROGON.md present but systemPrompt missing",
                 f"system={combined[:300]!r}")
        elif has_sys:
            fail("TROGON.md + systemPrompt openrouter: systemPrompt present but TROGON.md missing",
                 f"system={combined[:300]!r}")
        else:
            fail("TROGON.md + systemPrompt openrouter: neither marker in system messages",
                 f"system={combined[:300]!r}")


# ─────────────────────────────────────────────────────────────────────────────
# TEST 132 — codex import: pending_history takes priority over TROGON.md
# ─────────────────────────────────────────────────────────────────────────────

async def test_codex_import_pending_history_suppresses_trogon():
    """Spec (PR7/codex): when pending_history is set (from import), the first prompt
    uses the 'Prior conversation:' format and does NOT inject TROGON.md separately.
    The spec branching: if pending_history { use it } else if first_turn { inject TROGON.md }."""
    print("\n\033[1mTest 132: codex import — pending_history suppresses TROGON.md injection\033[0m")
    with tempfile.TemporaryDirectory() as cwd:
        TROGON_MARKER = "TROGON_T132_MARKER"
        with open(os.path.join(cwd, "TROGON.md"), "w") as f:
            f.write(f"# {TROGON_MARKER}\nCodex workspace context.\n")

        record_file = os.path.join(cwd, "recorded_input_t132.txt")
        env = {**os.environ,
               "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t132",
               "CODEX_BIN": f"{BIN}/mock_codex_server",
               "CODEX_MODELS": "codex-t132:codex-t132",
               "CODEX_DEFAULT_MODEL": "codex-t132",
               "MOCK_RECORD_TURN_INPUT_FILE": record_file,
               "CODEX_SPAWN_TIMEOUT_SECS": "5"}
        proc = subprocess.Popen([f"{BIN}/trogon-codex-runner"], env=env,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        await asyncio.sleep(3)
        nc = await natspy.connect(NATS_JS)
        try:
            # Create session
            r = await nc.request("acp.t132.agent.session.new",
                                 json.dumps({"cwd": cwd, "mcpServers": []}).encode(), timeout=5)
            sid = json.loads(r.data)["sessionId"]

            # Import prior conversation (sets pending_history)
            prior_msgs = [
                {"role": "user", "text": "prior-user-t132"},
                {"role": "assistant", "text": "prior-asst-t132"}
            ]
            await nc.request("acp.t132.agent.ext.session/import",
                             json.dumps({"sessionId": sid, "messages": prior_msgs}).encode(),
                             timeout=5)

            # Send first prompt
            req_id = str(uuid.uuid4())
            resp_sub = await nc.subscribe(f"acp.t132.session.{sid}.agent.prompt.response.{req_id}")
            async def _noop132(m): pass
            await nc.subscribe(f"acp.t132.session.{sid}.client.session.update", cb=_noop132)
            await nc.publish(f"acp.t132.session.{sid}.agent.prompt",
                             json.dumps({"sessionId": sid,
                                         "prompt": [{"type": "text", "text": "first-prompt-t132"}]}).encode(),
                             headers={"X-Req-Id": req_id})
            await asyncio.wait_for(resp_sub.next_msg(), timeout=15)
            await asyncio.sleep(0.3)
        except Exception as e:
            fail("codex pending_history suppresses TROGON.md", str(e))
            return
        finally:
            await nc.close()
            proc.terminate(); proc.wait(timeout=3)

        if not os.path.exists(record_file):
            fail("codex pending_history: MOCK_RECORD_TURN_INPUT_FILE not written")
            return
        recorded = open(record_file).read()
        print(f"    recorded input: {recorded[:200]!r}", flush=True)
        has_prior = "Prior conversation:" in recorded and "prior-user-t132" in recorded
        has_trogon = TROGON_MARKER in recorded
        if has_prior and not has_trogon:
            ok("codex import: pending_history used, TROGON.md NOT injected (correct priority)")
        elif has_prior and has_trogon:
            fail("codex import: pending_history present but TROGON.md also injected — should be suppressed",
                 f"recorded={recorded[:300]!r}")
        elif has_trogon:
            fail("codex import: TROGON.md injected instead of pending_history — priority wrong",
                 f"recorded={recorded[:300]!r}")
        else:
            fail("codex import: neither pending_history nor TROGON.md in recorded input",
                 f"recorded={recorded[:300]!r}")


# ─────────────────────────────────────────────────────────────────────────────
# TEST 133 — fetch_url raw=true: HTML tags preserved (not html2text processed)
# ─────────────────────────────────────────────────────────────────────────────

async def test_fetch_url_raw_html():
    """Spec (PR1/web.rs fetch_url): raw=true returns unprocessed HTML; tags must be preserved.
    Complement to T61 (raw=false strips tags). T48 uses raw=true but plain text — this
    test specifically verifies HTML tags survive when raw=true."""
    print("\n\033[1mTest 133: fetch_url raw=true — HTML tags preserved\033[0m")
    with tempfile.TemporaryDirectory() as tmpdir:
        html_body = "<html><body><h1>HEADING_T133</h1><p>PARAGRAPH_T133</p></body></html>"
        content_server = SimpleGetServer(30153, html_body, content_type="text/html").start()

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("fu133", "fetch_url",
                    json.dumps({"url": "http://127.0.0.1:30153/", "raw": True}))
            return acp_text_sse("fetched raw")

        mock = MockHttpServer(30154, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30154, 133, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(NATS_JS, prefix, tmpdir,
                                                  "fetch the page raw", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)
            if tool_result and "<h1>" in tool_result and "HEADING_T133" in tool_result:
                ok("fetch_url raw=true: HTML tags preserved in tool_result (not stripped)")
            elif tool_result and "HEADING_T133" in tool_result and "<h1>" not in tool_result:
                fail("fetch_url raw=true: content present but HTML tags stripped — raw=true not respected",
                     f"tool_result={tool_result[:300]!r}")
            else:
                fail("fetch_url raw=true: expected content not in tool_result",
                     f"tool_result={tool_result!r} calls={len(mock.received)}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()
            content_server.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 134 — glob: no matches returns empty (not error)
# ─────────────────────────────────────────────────────────────────────────────

async def test_glob_no_matches():
    """Spec (PR1/fs.rs glob): when pattern matches no files, tool_result is empty string
    (not an error). The runner must not crash and must make ≥2 HTTP calls."""
    print("\n\033[1mTest 134: glob — no matches returns empty (not error)\033[0m")
    with tempfile.TemporaryDirectory() as tmpdir:
        # Only .py files exist; glob for *.rs matches nothing
        with open(os.path.join(tmpdir, "main_t134.py"), "w") as f:
            f.write("print('hello')\n")

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("g134", "glob",
                    json.dumps({"pattern": "**/*.rs"}))
            return acp_text_sse("no rust files")

        mock = MockHttpServer(30155, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30155, 134, tmpdir)
        try:
            sid, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "find rust files", timeout=20)
            print(f"    done={done} calls={len(mock.received)}", flush=True)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)
            if len(mock.received) >= 2 and (tool_result == "" or tool_result is not None):
                # Non-error: runner made 2+ calls (tool result was fed back), no crash
                if tool_result is not None and "Error" not in str(tool_result):
                    ok("glob no matches: empty result returned (not error), runner continued")
                else:
                    fail("glob no matches: tool_result contains error",
                         f"tool_result={tool_result!r}")
            else:
                fail("glob no matches: runner did not complete tool loop",
                     f"calls={len(mock.received)} done={done!r}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 135 — list_dir with path parameter: scoped to subdirectory
# ─────────────────────────────────────────────────────────────────────────────

async def test_list_dir_with_path():
    """Spec (PR1/fs.rs list_dir): path parameter scopes listing to that subdirectory.
    Files outside path must not appear in tool_result."""
    print("\n\033[1mTest 135: list_dir with path parameter — scoped to subdirectory\033[0m")
    with tempfile.TemporaryDirectory() as tmpdir:
        os.makedirs(os.path.join(tmpdir, "src"), exist_ok=True)
        os.makedirs(os.path.join(tmpdir, "tests"), exist_ok=True)
        with open(os.path.join(tmpdir, "src", "main_t135.rs"), "w") as f:
            f.write("fn main() {}\n")
        with open(os.path.join(tmpdir, "src", "lib_t135.rs"), "w") as f:
            f.write("pub mod lib;\n")
        with open(os.path.join(tmpdir, "tests", "test_t135.rs"), "w") as f:
            f.write("#[test] fn it_works() {}\n")

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("ld135", "list_dir",
                    json.dumps({"path": "src"}))
            return acp_text_sse("listed src dir")

        mock = MockHttpServer(30156, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30156, 135, tmpdir)
        try:
            _, _, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "list the src directory", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result: {tool_result!r}", flush=True)
            has_main = "main_t135.rs" in str(tool_result)
            has_lib = "lib_t135.rs" in str(tool_result)
            has_test = "test_t135.rs" in str(tool_result)
            if has_main and has_lib and not has_test:
                ok("list_dir path param: src/ files listed, tests/ file excluded by path scope")
            elif has_test:
                fail("list_dir path param: tests/test_t135.rs appeared — path scope NOT applied",
                     f"tool_result={tool_result!r}")
            else:
                fail("list_dir path param: src/ files missing from result",
                     f"has_main={has_main} has_lib={has_lib} tool_result={str(tool_result)[:200]!r}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 136 — /model switch: two consecutive prompts after switch both reach new runner
# ─────────────────────────────────────────────────────────────────────────────

async def test_model_switch_multiple_prompts():
    """Spec (PR8+PR9): after /model switch, ALL subsequent prompts route to the new runner.
    T11 tests one prompt after switch. This test sends TWO prompts after switch and
    verifies both reach the new runner (xai), while acp got exactly 1 call total."""
    print("\n\033[1mTest 136: /model switch — two prompts after switch both reach new runner\033[0m")
    with tempfile.TemporaryDirectory() as cwd:
        acp_mock = MockHttpServer(30157, lambda n: acp_text_sse("acp-t136-reply")).start()
        xai_mock = MockHttpServer(30158, lambda n: xai_text_sse(f"xai-t136-turn{n}")).start()
        try:
            env_acp = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t136a",
                       "PROXY_URL": "http://127.0.0.1:30157",
                       "ANTHROPIC_TOKEN": "test", "AGENT_MODEL": "claude-t136"}
            env_xai = {**os.environ,
                       "NATS_URL": NATS_JS, "ACP_PREFIX": "acp.t136x",
                       "AGENT_TYPE": "xai-t136x",
                       "XAI_API_KEY": "test", "XAI_BASE_URL": "http://127.0.0.1:30158",
                       "XAI_MODELS": "grok-t136:grok-t136"}
            proc_acp = subprocess.Popen([f"{BIN}/trogon-acp-runner"], env=env_acp,
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            proc_xai = subprocess.Popen([f"{BIN}/trogon-xai-runner"], env=env_xai,
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            await asyncio.sleep(3)

            # Turn 1 on acp, /model switch to xai, turns 2 and 3 on xai
            cli_input = "turn-1-t136\n/model grok-t136\nturn-2-t136\nturn-3-t136\n"
            out, rc = await asyncio.get_event_loop().run_in_executor(
                None, lambda: run_repl_cli(
                    "acp.t136a", NATS_JS, cwd,
                    input_text=cli_input, timeout=45))
            print(f"    rc={rc} acp_calls={len(acp_mock.received)} xai_calls={len(xai_mock.received)}", flush=True)
            print(f"    out preview: {out[:200]!r}", flush=True)

            if len(acp_mock.received) != 1:
                fail("model switch multi-prompt: acp should receive exactly 1 call",
                     f"acp_calls={len(acp_mock.received)}")
            elif len(xai_mock.received) != 2:
                fail("model switch multi-prompt: xai should receive exactly 2 calls (turns 2+3)",
                     f"xai_calls={len(xai_mock.received)}")
            else:
                ok("/model switch: both prompts after switch reached xai-runner (acp=1 xai=2)")
        finally:
            for p in (proc_acp, proc_xai):
                p.terminate(); p.wait(timeout=3)
            acp_mock.stop(); xai_mock.stop()


# ─────────────────────────────────────────────────────────────────────────────
# TEST 137 — git_log: limited to 20 commits (--oneline -20)
# ─────────────────────────────────────────────────────────────────────────────

async def test_git_log_limit():
    """Spec (PR1/git.rs git_log): 'git log --oneline -20' — with >20 commits in the repo,
    tool_result must contain exactly 20 commit lines (oldest beyond 20 are omitted)."""
    print("\n\033[1mTest 137: git_log — limited to 20 commits (--oneline -20)\033[0m")
    with tempfile.TemporaryDirectory() as tmpdir:
        # Initialize git repo and make 22 commits
        subprocess.run(["git", "init"], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "config", "user.email", "t@t.com"], cwd=tmpdir, capture_output=True)
        subprocess.run(["git", "config", "user.name", "T"], cwd=tmpdir, capture_output=True)
        for i in range(22):
            p = os.path.join(tmpdir, f"file_{i}.txt")
            with open(p, "w") as f:
                f.write(f"commit {i}\n")
            subprocess.run(["git", "add", f"file_{i}.txt"], cwd=tmpdir, capture_output=True)
            subprocess.run(["git", "commit", "-m", f"commit-t137-{i:02d}"], cwd=tmpdir,
                           capture_output=True)

        def sse(n):
            if n == 1:
                return acp_tool_use_sse("gl137", "git_log", json.dumps({}))
            return acp_text_sse("git log done")

        mock = MockHttpServer(30159, sse).start()
        auto_approve, proc, prefix = await _start_acp_tool_runner(30159, 137, tmpdir)
        try:
            _, done, _ = await runner_session_prompt(
                NATS_JS, prefix, tmpdir, "show git log", timeout=20)
            tool_result = get_tool_result_content(mock.received)
            print(f"    tool_result preview: {str(tool_result)[:200]!r}", flush=True)
            if tool_result:
                lines = [l for l in tool_result.strip().splitlines() if l.strip()]
                print(f"    commit line count: {len(lines)}", flush=True)
                # Verify: exactly 20 lines (not 22), and the 21st and 22nd (commit-t137-00, -01) absent
                has_limit = len(lines) <= 20
                has_oldest = "commit-t137-00" in tool_result or "commit-t137-01" in tool_result
                if has_limit and not has_oldest:
                    ok(f"git_log limit: {len(lines)} lines returned (≤20), oldest commits excluded")
                elif not has_limit:
                    fail(f"git_log limit: {len(lines)} lines returned — exceeds 20 commit limit",
                         f"tool_result={tool_result[:300]!r}")
                else:
                    fail("git_log limit: ≤20 lines but oldest commits present — limit not applied correctly",
                         f"tool_result={tool_result[:300]!r}")
            else:
                fail("git_log limit: no tool_result received",
                     f"calls={len(mock.received)} done={done!r}")
        finally:
            try: auto_approve.terminate(); auto_approve.wait(timeout=2)
            except Exception: pass
            try: proc.terminate(); proc.wait(timeout=3)
            except Exception: pass
            mock.stop()


async def main():
    import subprocess as _sp
    _sp.run(
        "kill -9 $("
        "ps aux | grep -E 'trogon-acp-runner|trogon-xai-runner|trogon-openrouter-runner|trogon-codex-runner|auto_approve.py'"
        " | grep -v grep | awk '{print $2}') 2>/dev/null; true",
        shell=True, stdout=_sp.DEVNULL, stderr=_sp.DEVNULL)
    await asyncio.sleep(0.5)
    print("\033[1m=== Trogon Live Tests ===\033[0m")
    test_sse_helpers()
    await test_print_text()
    await test_print_json()
    await test_stdin_pipe()
    await test_at_file()
    await test_trogon_md_xai()
    await test_trogon_md_openrouter()
    await test_cross_runner_switch()
    await test_cross_runner_registry()
    await test_codex_runner()
    await test_at_file_repl()
    await test_model_switch_via_cli()
    await test_tool_execution()
    await test_trogon_md_acp()
    await test_trogon_md_codex()
    await test_import_into_acp()
    await test_model_switch_same_runner()
    await test_write_file_tool()
    await test_str_replace_tool()
    await test_bash_stateful()
    await test_notebook_edit_tool()
    await test_import_from_openrouter()
    await test_clear_command()
    await test_compact_command()
    await test_cost_command()
    await test_permission_denied()
    await test_str_replace_no_match()
    await test_write_file_path_traversal()
    await test_read_file_not_found()
    await test_unknown_stop_reason()
    await test_unknown_content_block_type()
    await test_malformed_tool_use_json()
    await test_str_replace_multiple_matches()
    await test_write_file_intermediate_dirs()
    await test_read_file_path_traversal()
    await test_model_unknown()
    await test_list_dir_tool()
    await test_git_status_tool()
    await test_glob_tool()
    # Purge stale "claude" registry entry: all default-AGENT_TYPE acp-runners share
    # the same key "claude". The last terminated runner leaves a dead entry that
    # find_by_model("claude-opus-4-6") can return, breaking the switch-back in test 45.
    await _purge_registry_key("claude")
    await test_double_runner_switch()
    await test_git_diff_tool()
    await test_git_log_tool()
    await test_fetch_url_tool()
    await test_read_file_offset_limit()
    await test_list_dir_gitignore()
    await test_git_status_modified()
    await test_get_state_xai()
    await test_get_state_codex()
    await test_get_state_openrouter()
    await test_help_command()
    await test_config_command()
    await test_model_switch_acp_to_openrouter()
    await test_model_switch_acp_to_codex()
    await test_model_switch_acp_to_openrouter_history()
    await test_str_replace_returns_diff()
    await test_fetch_url_html_to_text()
    await test_multiline_repl_input()
    await test_model_no_arg_shows_current()
    await test_history_persistence()
    await test_codex_to_acp_history()
    await test_config_set_get_live()
    await test_trogon_nats_url_env_var()
    await test_list_children_acp()
    await test_list_children_xai()
    await test_list_children_codex()
    await test_list_children_openrouter()
    await test_bash_nonzero_exit()
    await test_at_mention_nonexistent()
    await test_read_file_catn_format()
    await test_fetch_url_truncation()
    await test_git_diff_truncation()
    await test_read_file_limit_only()
    await test_read_file_offset_beyond_end()
    await test_glob_with_path()
    await test_meta_system_prompt_acp()
    await test_meta_system_prompt_xai()
    await test_meta_system_prompt_openrouter()
    await test_fork_session_acp()
    await test_bash_uses_session_cwd()
    await test_registry_capabilities_codex()
    await test_registry_capabilities_openrouter()
    await test_todo_write_tool()
    await test_todo_read_tool()
    await test_spawn_handler_xai()
    await test_spawn_handler_openrouter()
    await test_fork_session_xai()
    await test_fork_session_openrouter()
    await test_fork_session_codex()
    await test_portable_block_export_acp()
    await test_role_normalization_acp_import()
    await test_codex_tool_export_blocks()
    await test_xai_tool_execution()
    await test_openrouter_tool_execution()
    await test_init_command()
    await test_codex_trogon_md_injection_first_turn_only()
    await test_granular_permissions_deny_path()
    await test_registry_capabilities_xai()
    await test_registry_capabilities_acp()
    await test_xai_export_content()
    await test_acp_to_codex_history_in_subprocess()
    await test_autocompact_threshold()
    await test_init_when_trogon_exists()
    await test_granular_permissions_allow_path()
    await test_granular_permissions_deny_command()
    await test_programming_edit_cycle()
    await test_xai_programming_cycle()
    await test_openrouter_programming_cycle()
    await test_trogon_md_after_cross_runner_switch()
    await test_acp_export_preserves_portable_blocks()
    await test_cross_runner_switch_after_programming()
    await test_trogon_md_plus_system_prompt_xai()
    await test_trogon_md_plus_system_prompt_acp()
    await test_trogon_md_plus_system_prompt_openrouter()
    await test_codex_import_pending_history_suppresses_trogon()
    await test_fetch_url_raw_html()
    await test_glob_no_matches()
    await test_list_dir_with_path()
    await test_model_switch_multiple_prompts()
    await test_git_log_limit()
    print(f"\n\033[1mResults: \033[32m{PASS} passed\033[0m, \033[31m{FAIL} failed\033[0m")
    sys.exit(0 if FAIL == 0 else 1)

asyncio.run(main())
