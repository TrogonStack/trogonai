#!/usr/bin/env python3
"""
Live smoke test — ACP protocol over NATS, sin API key.

Protocolo:
  - Operaciones globales (initialize, authenticate, session.new, session.list)
    → NATS request-reply simple
  - Operaciones de sesión (fork, load, close, set_model, set_mode, branch_at_index…)
    → JetStream publish con headers X-Req-Id / X-Session-Id;
      la respuesta llega en el subject:
        {prefix}.session.{sid}.agent.response.{req_id}

Run:
  ACP_PREFIX=smoke-live python3 live_test.py
"""

import asyncio
import json
import os
import sys
import uuid

import nats

NATS_URL = os.environ.get("NATS_URL", "nats://localhost:4222")
PREFIX   = os.environ.get("ACP_PREFIX", "smoke-live")
TIMEOUT  = 10.0

green  = "\033[32m"
red    = "\033[31m"
yellow = "\033[33m"
cyan   = "\033[36m"
reset  = "\033[0m"

passed = 0
failed = 0


def ok(label, detail=""):
    global passed; passed += 1
    sfx = f"  {yellow}{detail}{reset}" if detail else ""
    print(f"  {green}PASS{reset}  {label}{sfx}")


def warn(label, detail=""):
    sfx = f"  {yellow}{detail}{reset}" if detail else ""
    print(f"  {yellow}WARN{reset}  {label}{sfx}")


def fail(label, detail=""):
    global failed; failed += 1
    print(f"  {red}FAIL{reset}  {label}  {yellow}{detail}{reset}")


def section(title):
    print(f"\n{cyan}── {title} {'─' * max(0, 55 - len(title))}{reset}")


async def nats_req(nc, subject, payload):
    """Simple NATS request-reply (global operations)."""
    data = json.dumps(payload).encode()
    msg = await nc.request(subject, data, timeout=TIMEOUT)
    return json.loads(msg.data)


async def js_cmd(nc, js, session_id, operation, payload):
    """
    JetStream command for session-scoped operations.
    Subscribes to the response subject first, then publishes with X-Req-Id header.
    """
    req_id = str(uuid.uuid4())
    resp_subj = f"{PREFIX}.session.{session_id}.agent.response.{req_id}"

    # Subscribe before sending so we don't miss the reply
    sub = await nc.subscribe(resp_subj)

    headers = {"X-Req-Id": req_id, "X-Session-Id": session_id}

    subject = f"{PREFIX}.session.{session_id}.agent.{operation}"
    try:
        await js.publish(subject, json.dumps(payload).encode(), headers=headers)
    except Exception as e:
        await sub.unsubscribe()
        return {"error": f"publish failed: {e}"}

    try:
        msg = await asyncio.wait_for(sub.next_msg(), timeout=TIMEOUT)
        result = json.loads(msg.data)
    except asyncio.TimeoutError:
        result = {"error": f"timeout waiting for response to {operation}"}
    except Exception as e:
        result = {"error": str(e)}
    finally:
        try:
            await sub.unsubscribe()
        except Exception:
            pass

    return result


async def main():
    print(f"\n{'═' * 60}")
    print(f"  ACP live smoke test  (sin API key)")
    print(f"  NATS  : {NATS_URL}")
    print(f"  prefix: {PREFIX}")
    print(f"{'═' * 60}")

    nc = await nats.connect(NATS_URL)
    js = nc.jetstream()

    # ── 1. initialize ──────────────────────────────────────────────────────
    section("1. initialize")
    r = await nats_req(nc, f"{PREFIX}.agent.initialize", {"protocolVersion": 1})
    if "protocolVersion" in r:
        ok("protocol version present", str(r["protocolVersion"]))
    else:
        fail("initialize", str(r))
    caps = r.get("capabilities", {})
    print(f"       capabilities: {list(caps.keys()) or '(none listed)'}")

    # ── 2. authenticate ────────────────────────────────────────────────────
    section("2. authenticate  (method_id=agent, sin key)")
    r = await nats_req(nc, f"{PREFIX}.agent.authenticate", {"methodId": "agent"})
    if "error" not in r and "code" not in r:
        ok("authenticate succeeded")
    else:
        warn("authenticate", str(r.get("error") or r.get("message")))

    # ── 3. session.new ─────────────────────────────────────────────────────
    section("3. session.new")
    r = await nats_req(nc, f"{PREFIX}.agent.session.new",
                       {"cwd": "/tmp/live-test", "mcpServers": []})
    session_id = r.get("sessionId") or r.get("session_id")
    if session_id:
        ok("session creada", session_id)
        models = r.get("models", [])
        if isinstance(models, list) and models:
            mid = next((m["id"] for m in models if m.get("current")), models[0].get("id","?"))
            print(f"       model: {mid}")
        elif isinstance(models, dict):
            print(f"       currentModelId: {models.get('currentModelId','?')}")
    else:
        fail("session.new", str(r))
        await nc.close(); sys.exit(1)

    # ── 4. session.list ────────────────────────────────────────────────────
    section("4. session.list")
    r = await nats_req(nc, f"{PREFIX}.agent.session.list", {})
    ids = [s.get("sessionId") or s.get("id") for s in r.get("sessions", [])]
    if session_id in ids:
        ok("sesión aparece en lista", f"total={len(ids)}")
    else:
        fail("session missing from list", str(ids[:5]))

    # ── 5. session.load ────────────────────────────────────────────────────
    section("5. session.load")
    r = await js_cmd(nc, js, session_id, "load",
                     {"sessionId": session_id, "cwd": "/tmp/live-test", "mcpServers": []})
    if "error" not in r and "code" not in r:
        ok("load devuelve estado")
        opts = r.get("configOptions", [])
        print(f"       configOptions: {len(opts)}")
        mods = r.get("models", {})
        if isinstance(mods, dict):
            print(f"       currentModelId: {mods.get('currentModelId','?')}")
    else:
        fail("session.load", str(r))

    # ── 6. set_model ───────────────────────────────────────────────────────
    section("6. set_model  →  grok-3-mini")
    r = await js_cmd(nc, js, session_id, "set_model",
                     {"sessionId": session_id, "modelId": "grok-3-mini"})
    if "error" not in r and "code" not in r:
        ok("set_model acknowledged")
    else:
        warn("set_model", str(r.get("error") or r.get("message","")))

    # ── 7. set_config_option ───────────────────────────────────────────────
    section("7. set_config_option  →  web_search=on")
    r = await js_cmd(nc, js, session_id, "set_config_option",
                     {"sessionId": session_id, "configId": "web_search", "value": "on"})
    if "error" not in r and "code" not in r:
        ok("set_config_option acknowledged")
    else:
        warn("set_config_option", str(r.get("error") or r.get("message","")))

    # ── 8. set_mode ────────────────────────────────────────────────────────
    section("8. set_mode  →  default")
    r = await js_cmd(nc, js, session_id, "set_mode",
                     {"sessionId": session_id, "modeId": "default"})
    if "error" not in r and "code" not in r:
        ok("set_mode acknowledged")
    else:
        warn("set_mode", str(r.get("error") or r.get("message","")))

    # ── 9. session.fork  (session branching) ──────────────────────────────
    section("9. session.fork  (session branching)")
    r = await js_cmd(nc, js, session_id, "fork",
                     {"sessionId": session_id, "cwd": "/tmp/live-fork", "mcpServers": []})
    fork_id = r.get("sessionId") or r.get("session_id")
    if fork_id and fork_id != session_id:
        ok("fork devolvió nueva sesión", fork_id)
    else:
        fail("fork", str(r))
        fork_id = None

    # Verificar que ambas aparecen en lista
    r = await nats_req(nc, f"{PREFIX}.agent.session.list", {})
    ids = [s.get("sessionId") or s.get("id") for s in r.get("sessions", [])]
    if session_id in ids and (fork_id is None or fork_id in ids):
        ok("parent + fork presentes en lista", f"total={len(ids)}")
    else:
        fail("lista post-fork incompleta", str(ids[:5]))

    # ── 10. list_children ─────────────────────────────────────────────────
    section("10. list_children  (ext)")
    r = await js_cmd(nc, js, session_id, "list_children",
                     {"sessionId": session_id})
    if "error" not in r and "code" not in r:
        children = r.get("children", [])
        child_ids = [c.get("sessionId") or c.get("id") for c in children]
        if fork_id and fork_id in child_ids:
            ok("fork aparece como hijo del parent", str(child_ids))
        else:
            ok("list_children respondió", f"children={child_ids}")
    else:
        warn("list_children", str(r))

    # ── 11. branch_at_index(0) ─────────────────────────────────────────────
    section("11. branch_at_index(0)  (branching en índice)")
    r = await js_cmd(nc, js, session_id, "branch_at_index",
                     {"sessionId": session_id, "index": 0, "cwd": "/tmp/branch0"})
    branch_id = r.get("sessionId") or r.get("session_id")
    if branch_id and branch_id != session_id:
        ok("branch_at_index(0) devolvió nueva sesión", branch_id)
        await js_cmd(nc, js, branch_id, "close", {"sessionId": branch_id})
    else:
        warn("branch_at_index", str(r))

    # ── 12. resume ────────────────────────────────────────────────────────
    section("12. resume")
    r = await js_cmd(nc, js, session_id, "resume",
                     {"sessionId": session_id, "cwd": "/tmp/live-test"})
    if "error" not in r and "code" not in r:
        ok("resume acknowledged")
    else:
        warn("resume", str(r))

    # ── 13. close parent ──────────────────────────────────────────────────
    section("13. session.close  (parent)")
    await js_cmd(nc, js, session_id, "close", {"sessionId": session_id})
    r = await nats_req(nc, f"{PREFIX}.agent.session.list", {})
    ids = [s.get("sessionId") or s.get("id") for s in r.get("sessions", [])]
    if session_id not in ids:
        ok("parent removido de la lista")
    else:
        fail("parent sigue en lista después de close")

    # ── 14. fork sobrevive al cierre del parent ────────────────────────────
    section("14. fork persiste después de cerrar parent")
    if fork_id:
        if fork_id in ids:
            ok("fork sigue vivo", fork_id)
        else:
            fail("fork desapareció con el parent", str(ids[:5]))

    # ── 15. close fork ────────────────────────────────────────────────────
    section("15. teardown  →  close fork")
    if fork_id:
        await js_cmd(nc, js, fork_id, "close", {"sessionId": fork_id})
        r = await nats_req(nc, f"{PREFIX}.agent.session.list", {})
        ids = [s.get("sessionId") or s.get("id") for s in r.get("sessions", [])]
        if fork_id not in ids:
            ok("fork cerrado limpiamente")
        else:
            fail("fork sigue en lista", str(ids[:5]))

    # ── logout ────────────────────────────────────────────────────────────
    section("16. logout")
    r = await nats_req(nc, f"{PREFIX}.agent.logout", {})
    if "error" not in r and "code" not in r:
        ok("logout acknowledged")
    else:
        warn("logout", str(r))

    # ── resumen ───────────────────────────────────────────────────────────
    await nc.close()
    print(f"\n{'═' * 60}")
    color = green if failed == 0 else red
    print(f"  {green}passed{reset}: {passed}   {red}failed{reset}: {failed}")
    print(f"  {color}{'OK — todo funciona.' if failed == 0 else 'Hay fallos.'}{reset}")
    print(f"{'═' * 60}\n")
    sys.exit(0 if failed == 0 else 1)


if __name__ == "__main__":
    asyncio.run(main())
