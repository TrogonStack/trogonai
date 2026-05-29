#!/usr/bin/env python3
"""
PR 7 live smoke test — escenarios no cubiertos por integration tests.

Prueba 4 escenarios:
  1. openrouter-runner — session.new + import + export (sin prompt, sin API key)
  2. cross-runner       — export de xai-runner → import en openrouter-runner → re-export
  3. codex-runner       — session.new → import → prompt (mock) → export
                          (valida el mecanismo pending_history end-to-end)
  4. acp-runner         — session.new + import + export directo en KV

codex-runner: plain NATS (no JetStream) → prompt via request-reply funciona
openrouter-runner: JetStream para session commands → solo global/ext (no prompt)
"""
import asyncio
import json
import os
import subprocess
import sys
import time

NATS_URL = os.environ.get("NATS_URL", "nats://localhost:4222")
BASE = f"pr7-{os.getpid()}"

PASS = 0
FAIL = 0
procs = {}


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


def start(name, binary, env_extra):
    env = {**os.environ, "NATS_URL": NATS_URL, "RUST_LOG": "warn", **env_extra}
    procs[name] = subprocess.Popen(
        [binary], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )
    time.sleep(0.6)


def stop(name):
    p = procs.pop(name, None)
    if p:
        p.terminate()
        p.wait(timeout=3)


def stop_all():
    for p in list(procs.values()):
        p.terminate()
        p.wait(timeout=3)
    procs.clear()


async def req(nc, subject, params, timeout=8.0):
    import nats as nats_lib
    try:
        msg = await nc.request(subject, json.dumps(params).encode(), timeout=timeout)
        return json.loads(msg.data)
    except nats_lib.errors.NoRespondersError:
        return {"_error": "no_responders"}
    except Exception as e:
        return {"_error": str(e)}


async def part1_openrouter(nc):
    """openrouter-runner: import + export sin prompt ni API key."""
    print("\n── Part 1: openrouter-runner (no prompt, no API key) ──")
    prefix = f"{BASE}-or"
    start("openrouter", "./target/debug/trogon-openrouter-runner", {
        "ACP_PREFIX": prefix,
        "OPENROUTER_API_KEY": "",
    })
    info("openrouter-runner started")

    # session.new
    resp = await req(nc, f"{prefix}.agent.session.new", {"cwd": "/tmp", "mcpServers": []})
    sid = resp.get("sessionId")
    if not sid:
        fail(f"openrouter session.new failed: {resp}")
        stop("openrouter")
        return None

    ok(f"openrouter session.new: {sid}")

    export_subj = f"{prefix}.agent.ext.session/export"
    import_subj = f"{prefix}.agent.ext.session/import"

    # export empty
    resp = await req(nc, export_subj, {"sessionId": sid})
    if resp == []:
        ok("openrouter export empty session → []")
    else:
        fail(f"openrouter export empty: {resp}")

    # import 3 messages
    msgs = [
        {"role": "user", "text": "Hola"},
        {"role": "assistant", "text": "Buenos días"},
        {"role": "user", "text": "¿Qué es 5+5?"},
    ]
    resp = await req(nc, import_subj, {"sessionId": sid, "messages": msgs})
    if isinstance(resp, dict) and "code" not in resp:
        ok("openrouter import 3 messages")
    else:
        fail(f"openrouter import failed: {resp}")

    # export after import
    resp = await req(nc, export_subj, {"sessionId": sid})
    if resp == msgs:
        ok("openrouter export after import → exact match (3 messages)")
    else:
        fail(f"openrouter export mismatch: {resp}")

    stop("openrouter")
    return msgs  # para usar en cross-runner test


async def part2_cross_runner(nc, xai_history):
    """xai export → openrouter import → re-export: verifica portabilidad."""
    print("\n── Part 2: cross-runner (xai → openrouter) ──")

    prefix_xai = f"{BASE}-xai2"
    prefix_or2 = f"{BASE}-or2"

    start("xai2", "./target/debug/trogon-xai-runner", {
        "ACP_PREFIX": prefix_xai,
        "XAI_SESSION_BUCKET": f"PR7_{os.getpid()}_XAI",
        "XAI_API_KEY": "dummy",
    })
    start("or2", "./target/debug/trogon-openrouter-runner", {
        "ACP_PREFIX": prefix_or2,
        "OPENROUTER_API_KEY": "",
    })
    info("xai-runner + openrouter-runner started")

    # Create session in xai-runner and seed history
    resp = await req(nc, f"{prefix_xai}.agent.session.new", {"cwd": "/tmp", "mcpServers": []})
    sid_xai = resp.get("sessionId")
    if not sid_xai:
        fail(f"xai session.new failed: {resp}")
        stop("xai2"); stop("or2")
        return

    # Import history into xai-runner
    history = [
        {"role": "user", "text": "Tell me about Rust"},
        {"role": "assistant", "text": "Rust is a systems programming language."},
        {"role": "user", "text": "And Python?"},
        {"role": "assistant", "text": "Python is a high-level language."},
    ]
    resp = await req(nc, f"{prefix_xai}.agent.ext.session/import",
                     {"sessionId": sid_xai, "messages": history})
    if isinstance(resp, dict) and "code" in resp:
        fail(f"xai import failed: {resp}")
        stop("xai2"); stop("or2")
        return

    # Export from xai-runner
    exported = await req(nc, f"{prefix_xai}.agent.ext.session/export",
                         {"sessionId": sid_xai})
    if exported != history:
        fail(f"xai export mismatch: {exported}")
        stop("xai2"); stop("or2")
        return
    ok("xai-runner: import+export round-trip verified")

    # Create session in openrouter-runner and import xai history
    resp = await req(nc, f"{prefix_or2}.agent.session.new", {"cwd": "/tmp", "mcpServers": []})
    sid_or = resp.get("sessionId")
    if not sid_or:
        fail(f"openrouter session.new failed: {resp}")
        stop("xai2"); stop("or2")
        return

    resp = await req(nc, f"{prefix_or2}.agent.ext.session/import",
                     {"sessionId": sid_or, "messages": exported})
    if isinstance(resp, dict) and "code" in resp:
        fail(f"openrouter import failed: {resp}")
        stop("xai2"); stop("or2")
        return

    # Re-export from openrouter-runner → must match original
    re_exported = await req(nc, f"{prefix_or2}.agent.ext.session/export",
                            {"sessionId": sid_or})
    if re_exported == history:
        ok("cross-runner: xai export → openrouter import → re-export matches original (4 messages)")
    else:
        fail(f"cross-runner mismatch: {re_exported}")

    stop("xai2"); stop("or2")


async def part3_codex_pending_history(nc):
    """codex-runner: import → prompt → export verifica pending_history end-to-end."""
    print("\n── Part 3: codex-runner pending_history (mock CLI) ──")

    prefix = f"{BASE}-codex"
    mock_bin = os.path.abspath("./target/debug/mock_codex_server")

    start("codex", "./target/debug/trogon-codex-runner", {
        "ACP_PREFIX": prefix,
        "CODEX_BIN": mock_bin,
        "MOCK_SEND_N_TEXT_EVENTS": "1",
        "CODEX_SPAWN_TIMEOUT_SECS": "5",
    })
    info(f"codex-runner started (CODEX_BIN={mock_bin})")

    export_subj = f"{prefix}.agent.ext.session/export"
    import_subj = f"{prefix}.agent.ext.session/import"

    # session.new
    resp = await req(nc, f"{prefix}.agent.session.new", {"cwd": "/tmp", "mcpServers": []})
    sid = resp.get("sessionId")
    if not sid:
        fail(f"codex session.new failed: {resp}")
        stop("codex")
        return
    ok(f"codex session.new: {sid}")

    # export empty (sin pending_history)
    resp = await req(nc, export_subj, {"sessionId": sid})
    if resp == []:
        ok("codex export empty → []")
    else:
        fail(f"codex export empty unexpected: {resp}")

    # session/import → sets pending_history
    prior_conv = [
        {"role": "user", "text": "What is 1+1?"},
        {"role": "assistant", "text": "It is 2."},
    ]
    resp = await req(nc, import_subj, {"sessionId": sid, "messages": prior_conv})
    if isinstance(resp, dict) and "code" not in resp:
        ok("codex import sets pending_history")
    else:
        fail(f"codex import failed: {resp}")
        stop("codex")
        return

    # export after import (antes del prompt) — debe retornar los mensajes importados
    # (consistente con xai/openrouter/acp: no hace falta un prompt primero)
    resp = await req(nc, export_subj, {"sessionId": sid})
    if resp == prior_conv:
        ok("codex export after import (pre-prompt) → imported messages returned immediately")
    else:
        fail(f"codex export after import mismatch: {resp}")

    # prompt — codex-runner usa JetStream para recibir comandos de sesión.
    # nc.request() recibe el pub-ack de JetStream inmediatamente; el turn se
    # procesa de forma asíncrona. Esperamos 2 s para que TurnCompleted ocurra.
    prompt_subj = f"{prefix}.session.{sid}.agent.prompt"
    resp = await req(nc, prompt_subj,
                     {"sessionId": sid, "prompt": [{"type": "text", "text": "And 2+2?"}]},
                     timeout=15.0)
    if isinstance(resp, dict) and "code" in resp:
        fail(f"codex prompt failed: {resp}")
        stop("codex")
        return
    # resp es el pub-ack de JetStream (no PromptResponse); esperamos que el turn termine
    await asyncio.sleep(2.0)
    ok("codex prompt dispatched via JetStream (async turn processing)")

    # export after prompt — history = prior_conv + [user, assistant]
    # prior_conv (2) + user "And 2+2?" + assistant = 4 messages
    resp = await req(nc, export_subj, {"sessionId": sid})
    messages = resp if isinstance(resp, list) else None

    if messages is None:
        fail(f"codex export after prompt failed: {resp}")
        stop("codex")
        return

    expected_len = len(prior_conv) + 2  # imported + user + assistant
    if len(messages) >= expected_len:
        ok(f"codex export after prompt has {len(messages)} messages (imported + new turn)")

        # Los primeros mensajes son los importados
        if messages[:len(prior_conv)] == prior_conv:
            ok("codex: imported messages preserved at start of history")
        else:
            fail(f"codex: imported messages missing from history start: {messages[:len(prior_conv)]}")

        # El user message nuevo es el del prompt
        user_msg = messages[len(prior_conv)]
        if user_msg.get("text", "") == "And 2+2?":
            ok("codex: new user input stored correctly after imported messages")
        else:
            fail(f"codex: unexpected user message: {user_msg['text'][:100]}")

        # El assistant message cierra el turno
        asst_msg = messages[len(prior_conv) + 1] if len(messages) > len(prior_conv) + 1 else None
        if asst_msg and asst_msg.get("role") == "assistant" and asst_msg.get("text"):
            ok(f"codex: assistant response accumulated: '{asst_msg['text'][:60]}'")
        else:
            fail(f"codex: no assistant message in history, got: {messages}")
    else:
        fail(f"codex export after prompt: expected ≥{expected_len} messages, got {len(messages)}: {messages}")

    stop("codex")


async def part4_acp(nc):
    """acp-runner: import escribe directamente en KV; export lee de KV."""
    print("\n── Part 4: acp-runner (KV-native import/export) ──")

    prefix = f"{BASE}-acp"
    # acp-runner no necesita API key para ext_method
    start("acp", "./target/debug/trogon-acp-runner", {
        "ACP_PREFIX": prefix,
        "ANTHROPIC_TOKEN": "",
        "PROXY_URL": "http://127.0.0.1:1",  # nunca se llama para ext_method
    })
    info("acp-runner started")

    export_subj = f"{prefix}.agent.ext.session/export"
    import_subj = f"{prefix}.agent.ext.session/import"

    # session.new
    resp = await req(nc, f"{prefix}.agent.session.new", {"cwd": "/tmp", "mcpServers": []})
    sid = resp.get("sessionId")
    if not sid:
        fail(f"acp session.new failed: {resp}")
        stop("acp")
        return
    ok(f"acp session.new: {sid}")

    # export empty
    resp = await req(nc, export_subj, {"sessionId": sid})
    if resp == []:
        ok("acp export empty → []")
    else:
        fail(f"acp export empty unexpected: {resp}")

    # import 2 messages (escribe en KV)
    msgs = [
        {"role": "user", "text": "Hola desde acp"},
        {"role": "assistant", "text": "Respuesta de acp"},
    ]
    resp = await req(nc, import_subj, {"sessionId": sid, "messages": msgs})
    if isinstance(resp, dict) and "code" not in resp:
        ok("acp import writes to KV")
    else:
        fail(f"acp import failed: {resp}")

    # export (lee de KV)
    resp = await req(nc, export_subj, {"sessionId": sid})
    if resp == msgs:
        ok("acp export after import → exact match (reads from KV)")
    else:
        fail(f"acp export mismatch: {resp}")

    stop("acp")


async def part5_acp_kv_restart(nc):
    """acp-runner: KV persiste tras reiniciar el proceso (propiedad diferenciadora del diseño)."""
    print("\n── Part 5: acp-runner KV persistence across restart ──")

    prefix = f"{BASE}-acp5"
    env_extra = {
        "ACP_PREFIX": prefix,
        "ANTHROPIC_TOKEN": "",
        "PROXY_URL": "http://127.0.0.1:1",
    }

    start("acp5", "./target/debug/trogon-acp-runner", env_extra)
    info("acp-runner started (first instance)")

    export_subj = f"{prefix}.agent.ext.session/export"
    import_subj = f"{prefix}.agent.ext.session/import"

    # session.new
    resp = await req(nc, f"{prefix}.agent.session.new", {"cwd": "/tmp", "mcpServers": []})
    sid = resp.get("sessionId")
    if not sid:
        fail(f"acp5 session.new failed: {resp}")
        stop("acp5")
        return
    ok(f"acp5 session.new: {sid}")

    # import history
    msgs = [
        {"role": "user", "text": "Persisted message 1"},
        {"role": "assistant", "text": "Persisted reply 1"},
        {"role": "user", "text": "Persisted message 2"},
    ]
    resp = await req(nc, import_subj, {"sessionId": sid, "messages": msgs})
    if isinstance(resp, dict) and "code" not in resp:
        ok("acp5 import 3 messages → written to KV")
    else:
        fail(f"acp5 import failed: {resp}")
        stop("acp5")
        return

    # verify before restart
    resp = await req(nc, export_subj, {"sessionId": sid})
    if resp == msgs:
        ok("acp5 export before restart → exact match")
    else:
        fail(f"acp5 export before restart mismatch: {resp}")
        stop("acp5")
        return

    # kill and restart
    stop("acp5")
    info("acp-runner stopped — restarting...")
    start("acp5", "./target/debug/trogon-acp-runner", env_extra)
    info("acp-runner restarted (second instance)")

    # export after restart — must read from KV
    resp = await req(nc, export_subj, {"sessionId": sid})
    if resp == msgs:
        ok("acp5 export after restart → KV data persisted across process restart (3 messages)")
    else:
        fail(f"acp5 export after restart mismatch: {resp}")

    stop("acp5")


async def part6_xai_no_kv_persist(nc):
    """xai-runner: import es in-memory only — historia desaparece al reiniciar (diseño intencional)."""
    print("\n── Part 6: xai-runner in-memory only (no KV persist on restart) ──")

    prefix = f"{BASE}-xai6"
    env_extra = {
        "ACP_PREFIX": prefix,
        "XAI_SESSION_BUCKET": f"PR7_{os.getpid()}_XAI6",
        "XAI_API_KEY": "dummy",
    }

    start("xai6", "./target/debug/trogon-xai-runner", env_extra)
    info("xai-runner started (first instance)")

    export_subj = f"{prefix}.agent.ext.session/export"
    import_subj = f"{prefix}.agent.ext.session/import"

    resp = await req(nc, f"{prefix}.agent.session.new", {"cwd": "/tmp", "mcpServers": []})
    sid = resp.get("sessionId")
    if not sid:
        fail(f"xai6 session.new failed: {resp}")
        stop("xai6")
        return
    ok(f"xai6 session.new: {sid}")

    msgs = [
        {"role": "user", "text": "In-memory message"},
        {"role": "assistant", "text": "In-memory reply"},
    ]
    resp = await req(nc, import_subj, {"sessionId": sid, "messages": msgs})
    if isinstance(resp, dict) and "code" not in resp:
        ok("xai6 import 2 messages (in-memory only, no KV write)")
    else:
        fail(f"xai6 import failed: {resp}")
        stop("xai6")
        return

    resp = await req(nc, export_subj, {"sessionId": sid})
    if resp == msgs:
        ok("xai6 export before restart → exact match")
    else:
        fail(f"xai6 export before restart mismatch: {resp}")
        stop("xai6")
        return

    stop("xai6")
    info("xai-runner stopped — restarting...")
    start("xai6", "./target/debug/trogon-xai-runner", env_extra)
    info("xai-runner restarted (second instance)")

    # After restart the session doesn't exist — expect JSON-RPC error
    resp = await req(nc, export_subj, {"sessionId": sid})
    if isinstance(resp, dict) and "code" in resp:
        ok("xai6 export after restart → session gone (in-memory design confirmed, no KV persistence)")
    else:
        fail(f"xai6 expected error after restart, got: {resp}")

    stop("xai6")


async def part7_unknown_session_errors(nc):
    """openrouter, codex, acp: unknown sessionId → JSON-RPC error en import y export."""
    print("\n── Part 7: unknown sessionId → JSON-RPC error (openrouter, codex, acp) ──")

    # openrouter y codex tienen registro de sesiones en memoria → error para sessionId desconocido
    # acp es KV-native (sin registro): export devuelve [] y import crea la entrada en KV
    session_runners = [
        ("openrouter", "./target/debug/trogon-openrouter-runner",
         {"ACP_PREFIX": f"{BASE}-err-or", "OPENROUTER_API_KEY": ""}),
        ("codex", "./target/debug/trogon-codex-runner",
         {"ACP_PREFIX": f"{BASE}-err-cx", "CODEX_BIN": os.path.abspath("./target/debug/mock_codex_server"),
          "CODEX_SPAWN_TIMEOUT_SECS": "5"}),
    ]

    for name, binary, env_extra in session_runners:
        prefix = env_extra["ACP_PREFIX"]
        start(name, binary, env_extra)
        export_subj = f"{prefix}.agent.ext.session/export"
        import_subj = f"{prefix}.agent.ext.session/import"
        bogus = "no-such-session-999"

        resp = await req(nc, export_subj, {"sessionId": bogus})
        if isinstance(resp, dict) and "code" in resp:
            ok(f"{name}: export unknown session → JSON-RPC error (code={resp['code']})")
        else:
            fail(f"{name}: expected JSON-RPC error for export unknown session, got: {resp}")

        resp = await req(nc, import_subj, {"sessionId": bogus, "messages": [{"role": "user", "text": "x"}]})
        if isinstance(resp, dict) and "code" in resp:
            ok(f"{name}: import unknown session → JSON-RPC error (code={resp['code']})")
        else:
            fail(f"{name}: expected JSON-RPC error for import unknown session, got: {resp}")

        stop(name)

    # acp-runner: KV-native — sin registro de sesiones activas
    # export de sessionId desconocido → [] (KV vacío, no error)
    # import de sessionId desconocido → crea la entrada en KV (upsert)
    prefix_acp = f"{BASE}-err-acp"
    start("acp-err", "./target/debug/trogon-acp-runner",
          {"ACP_PREFIX": prefix_acp, "ANTHROPIC_TOKEN": "", "PROXY_URL": "http://127.0.0.1:1"})
    bogus = f"no-such-session-{os.getpid()}"

    resp = await req(nc, f"{prefix_acp}.agent.ext.session/export", {"sessionId": bogus})
    if resp == []:
        ok("acp: export unknown session → [] (KV-native: no session registry, empty KV = empty history)")
    else:
        fail(f"acp: expected [] for unknown session export, got: {resp}")

    msgs_bogus = [{"role": "user", "text": "upsert test"}]
    resp = await req(nc, f"{prefix_acp}.agent.ext.session/import",
                     {"sessionId": bogus, "messages": msgs_bogus})
    if isinstance(resp, dict) and "code" not in resp:
        # verify the upsert actually wrote to KV
        resp2 = await req(nc, f"{prefix_acp}.agent.ext.session/export", {"sessionId": bogus})
        if resp2 == msgs_bogus:
            ok("acp: import unknown session → KV upsert (creates entry without requiring prior session.new)")
        else:
            fail(f"acp: import upsert mismatch: {resp2}")
    else:
        fail(f"acp: import unknown session failed unexpectedly: {resp}")

    stop("acp-err")


async def main():
    import nats

    print()
    print("=== PR 7 live smoke test — escenarios no cubiertos por integration tests ===")
    print(f"prefix base : {BASE}")
    print(f"nats        : {NATS_URL}")
    print()

    nc = await nats.connect(NATS_URL)
    try:
        xai_history = await part1_openrouter(nc)
        await part2_cross_runner(nc, xai_history)
        await part3_codex_pending_history(nc)
        await part4_acp(nc)
        await part5_acp_kv_restart(nc)
        await part6_xai_no_kv_persist(nc)
        await part7_unknown_session_errors(nc)
    finally:
        await nc.close()
        stop_all()

    print()
    print("=== Resultados ===")
    print(f"  \033[32mpassed\033[0m: {PASS}")
    if FAIL > 0:
        print(f"  \033[31mfailed\033[0m: {FAIL}")
        sys.exit(1)
    else:
        print(f"  \033[31mfailed\033[0m: {FAIL}")
        print(f"\n\033[32mTodos los tests pasaron.\033[0m")


if __name__ == "__main__":
    asyncio.run(main())
