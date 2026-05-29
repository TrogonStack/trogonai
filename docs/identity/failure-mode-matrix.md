# Gateway failure-mode matrix

**Status:** Normative operator reference (Block C). Specifies default gateway behaviour when a dependency fails. Implementation may lag; rows marked **(today)** describe shipped Phase 1 behaviour in `trogon-mcp-gateway`.

**Cross-references:** [Identity overview](overview.md) (mesh identity, STS fail-closed), [Adaptive access](adaptive-access.md) (approvals, throttling), [MCP gateway plan](../../MCP_GATEWAY_PLAN.md) Block C (open design item), §6 JSON-RPC codes, §7 audit envelope, §9 backpressure.

---

## Behaviour modes

| Mode | Definition |
|---|---|
| **CLOSED** | The request is **denied** before side effects (no backend publish, no egress mint, no credential propagation). The client receives a Trogon JSON-RPC error (`-32100` … `-32199`). An audit envelope with outcome `deny` or `error` is emitted when the request path reached the gateway worker. |
| **OPEN-with-audit** | The request **proceeds** (forward, mint, or callback) while the failure is recorded. Used only where availability is explicitly traded for observability—never for authorization decisions on gated methods (`tools/call`, `resources/read`, egress mint in enforce mode). |
| **DEGRADED** | The gateway instance **stops accepting or completing work** for a class of traffic (subscription loss, partial fleet), or serves a reduced path (e.g. stale read model) without granting new privileged access. Differs from CLOSED per-request denial: operators treat this as infra/outage, not policy. |

**Note on `-32107`:** `rpc_codes.rs` defines both `AUTHZ_UNREACHABLE` and `APPROVAL_REQUIRED` as `-32107`. The plan allocates `-32107` to `authz_unreachable`; adaptive access uses the same code for `approval_required` with a structured `error.data` object. Clients must disambiguate via `error.message` and `error.data.approval_subject`.

---

## Failure-mode matrix

Wire audit subjects use the gateway publisher pattern **`{prefix}.audit.{outcome}.{direction}.{method_root}`** (default prefix `mcp`), e.g. `mcp.audit.error.request.tools`. See [agent-traffic.md](agent-traffic.md) §2.1.

| # | Class | Failure trigger (concrete) | Default behaviour | JSON-RPC code | Audit subject emitted | Operator override knob | Justification |
|---|---|---|---|---|---|---|---|
| 1 | SpiceDB unreachable | gRPC to `MCP_GATEWAY_SPICEDB_ENDPOINT` fails: connection refused, TLS error, deadline exceeded, HTTP 5xx from SpiceDB front-end. **(today)** Any `CheckBulkPermissions` transport/Status error becomes `AuthzError`. | **CLOSED** | `-32107` `AUTHZ_UNREACHABLE` (`rpc_codes::AUTHZ_UNREACHABLE`) | `{prefix}.audit.error.request.{method_root}` | Unset `MCP_GATEWAY_SPICEDB_ENDPOINT` ⇒ Phase 1 allow-all (dev only; not for production enforce). No fail-open knob for production PDP. | Authorization on gated methods must not proceed without a PDP answer. Silent allow would bypass tenant RBAC/ReBAC. |
| 2 | SpiceDB inconsistent ZedToken | Cached `ZedToken` in gateway session/process cache is rejected by SpiceDB (`at_least_as_fresh` consistency): snapshot too old, unknown token, revision mismatch. **(today)** Surfaced as bulk-check `Error` status → same path as row 1. | **CLOSED** (target); **DEGRADED** retry optional | `-32107` `AUTHZ_UNREACHABLE` **(today)**; `-32107` or `-32101` `policy_fault` **(proposed)** if retried with `minimize_latency` and still denied | `{prefix}.audit.error.request.{method_root}` | Per-bundle rule `spicedb.consistency_on_stale_token: retry_minimize_latency \| deny` in `mcp-gateway-config` KV **(proposed)**. Process cache clear via rolling restart. | Stale tokens must not produce permanent allow; retry at lower consistency is a performance trade-off, not a security bypass. Default CLOSED until retry succeeds. |
| 4 | WASM bundle signature invalid (load) | Hot-swap or startup: manifest NKey signature mismatch, unknown signer, digest failure. Gateway rejects load. | **CLOSED** (no new bundle) | N/A at request path; unhealthy instance **(proposed)** `/ready` false. Request path if no prior bundle: `-32108` `no_policy` **(proposed)** | `mcp.control.bundle.load_failed` **(proposed)** | Trust anchor list in KV `mcp-gateway-config/trusted_signers`; keep last good bundle pinned. | Unsigned or tampered policy must never execute; previous signed bundle remains authoritative. |
| 6 | CEL expression compile error (config load) | Bundle or built-in gate expression fails `Program::compile` at gateway startup or KV reload (syntax/type error). **(today)** `SpicedbGatePolicy::phase1_hardcoded` panics at init if compile fails. | **CLOSED** (instance fails load / does not adopt bad config) | N/A at request path while instance unhealthy **(proposed)**. If eval skipped: `-32101` `policy_fault` **(proposed)** | `mcp.control.policy.compile_failed` **(proposed)** | Fix bundle in KV; rollback version pointer. | Invalid policy must not run; fail the config surface, not individual requests with ambiguous semantics. |
| 7 | CEL expression runtime error | Evaluation error on request path: null deref, type mismatch, division by zero, host builtin failure. **(today)** `requires_spicedb_for_method` returns `PolicyError` → ingress error (no structured client JSON-RPC in all paths). | **CLOSED** | `-32101` `policy_fault` **(proposed;** plan §6 failClosed) | `{prefix}.audit.error.request.{method_root}` | Per-rule `on_error: deny \| log` in bundle **(proposed)**; default `deny` per plan § SpiceDB Integration. | Runtime CEL failure is a policy fault; default failClosed matches `*/call` and `*/read` posture. |
| 8 | Backend MCP server timeout | No NATS reply on `mcp.server.{server_id}.>` within `Config::operation_timeout()` after `request_with_headers`. **(today)** implemented. | **CLOSED** | `-32102` `BACKEND_TIMEOUT` (`rpc_codes::BACKEND_TIMEOUT`) | `{prefix}.audit.error.request.{method_root}` | `MCP_*` operation timeout via `mcp-nats` config env; per-server deadline in bundle/KV **(proposed)**. | Caller must not hang indefinitely; timeout is not authorization—no fallback allow. |
| 9 | Backend MCP server malformed JSON-RPC | Reply payload is not valid JSON, or JSON lacks required JSON-RPC 2.0 fields (`jsonrpc`, `id`/`result`/`error`). **(today)** opaque pass-through via `dispatch_backend_response`. | **CLOSED** (target) | `-32116` `backend_malformed` **(proposed)**; **(today)** pass-through may expose raw bytes to client | `{prefix}.audit.error.request.{method_root}` | Strict validation toggle in bundle `gateway.validate_backend_jsonrpc: true` **(proposed)**. | Gateway is the MCP termination point for clients; forwarding corrupt JSON breaks client SDKs and hides upstream faults. |
| 10 | Gateway worker saturated | Per-`server_id` or per-tenant inflight semaphore at cap; internal queue depth above threshold. Plan §9; not fully implemented in Phase 1 worker. | **CLOSED** | `-32105` `RATE_LIMITED` (`rpc_codes::RATE_LIMITED`) | `{prefix}.audit.deny.request.{method_root}` **(proposed)**; throttle via adaptive access emits same code **(today)** for context throttle | Caps in bundle / `mcp-gateway-config` KV (`inflight.server`, `inflight.tenant`); `MeshGatewayConfig.throttle` for `(tenant, agent_id, purpose)` **(today)** | Backpressure protects backends and fairness; saturation is not an authz bypass. |
| 11 | NATS partition | Gateway loses core/JetStream connectivity; queue-group subscription drops; heartbeats to `mcp.control.gateway.heartbeat.{instance_id}` fail. | **DEGRADED** (instance) | N/A (no ingress) | None while disconnected **(proposed)** last-known `{prefix}.audit.error.*` before partition | NATS client reconnect policy; scale replicas across failure domains. No "fail open" subscription. | Instance cannot enforce policy without messages; clients must retry other gateway members. Per-request CLOSED applies once a worker accepts a message it cannot complete. |
| 12 | JetStream KV unavailable | Session lookup in bucket `mcp-sessions` fails: KV timeout, bucket missing, permission denied. Required for session-scoped ZedToken, initialize state (plan § Session correlation). | **CLOSED** for session-bound RPCs | `-32101` `policy_fault` **(proposed)** or `-32107` **(proposed)** when session required | `{prefix}.audit.error.request.{method_root}` | Hard tenancy: per-account KV bucket; soft tenancy: key prefix `{tenant}/`. Session-affinity subject routing **(proposed Phase 3)** reduces KV hard dependency. | Operating without session state risks wrong ZedToken scope and broken initialize semantics; fail closed rather than guess. |
| 13 | STS unreachable / mesh JWKS stale | **STS:** `mcp.sts.exchange` request timeout or transport error during egress mint **(today)** → `-32107`. **JWKS:** `MCP_GATEWAY_JWT_JWKS_URI` fetch stale/failing while verifying ingress mesh JWT. | **CLOSED** in enforce mode; **OPEN-with-audit** in shadow for ingress-only metrics | `-32107` `AUTHZ_UNREACHABLE` for STS/mint **(today)**; `-32106` `AUTH_EXPIRED` / `-32110` `INVALID_TOKEN` for JWT verify | Gateway: `{prefix}.audit.error.request.{method_root}`. STS: `mcp.audit.sts.error` (`outcome=error`) | `MCP_GATEWAY_AGENT_IDENTITY=shadow` (ingress validate without hard deny); STS: `MCP_STS_TRUST_BUNDLE_PATH` + KV watch `mcp-trust-bundles/{domain}`; JWKS refresh interval on gateway | Mesh egress requires fresh minted token with correct `aud`; stale JWKS must not validate revoked keys. Shadow mode explicitly documented in [overview.md](overview.md). |
| 14 | Audit publisher backlog full | JetStream publish to `MCP_AUDIT` fails: stream at `max_messages`, ack timeout, broker slow. **(today)** `publish_audit` logs warning and returns; request already decided. | **OPEN-with-audit** (request proceeds; audit may be lost) | N/A (audit is side channel) | None if publish fails **(today)** | Increase stream retention/`max_messages`; scale JetStream; `MCP_GATEWAY_SKIP_AUDIT_STREAM_INIT` only for dev. **(proposed)** `MCP_GATEWAY_AUDIT_REQUIRED=1` fail CLOSED **(proposed)** for regulated tenants | Legal audit stream loss is operational risk, not authz; blocking MCP on audit backpressure creates asymmetric outage. Regulated deployments may opt into fail-closed via config **(proposed)**. |
| 15 | Trust bundle missing for SPIFFE trust domain | No PEM in `mcp-trust-bundles/{trust_domain}` KV and no file fallback for actor SVID verification on STS exchange or gateway egress attest path. | **CLOSED** | `-32107` via STS `server_error` / gateway mint **(today)**; `-32110` `INVALID_TOKEN` for unattested `wkl` in enforce **(today)** | `mcp.audit.sts.deny` or `mcp.audit.sts.error` with `decision_reason` containing trust failure | Publish bundle to KV key `{trust_domain}`; `MCP_STS_TRUST_BUNDLE_PATH` bootstrap; `MCP_STS_REQUIRE_ATTESTATION=0` dev shadow only | Workload identity cannot be verified without anchors; must not mint or accept forged `wkl`. |
| 16 | Approval timeout | No decision on `mcp.approvals.{request_id}` (or `mcp.approvals.step-up.{request_id}`) before `ttl_seconds`; malformed messages ignored until TTL. | **CLOSED** | `-32107` `APPROVAL_REQUIRED` (`rpc_codes::APPROVAL_REQUIRED`) with `error.data.reason` describing timeout/expiry | `{prefix}.audit.deny.request.{method_root}` **(proposed)** when ingress wired; library emits envelope only **(today partial)** | `MeshGatewayConfig.approval_ttl_secs`; risk thresholds in `MeshGatewayConfig.risk` | Human-in-the-loop calls must not proceed without explicit approve; timeout is deny, not allow. See [adaptive-access.md](adaptive-access.md). |

---

## JSON-RPC code quick reference

Constants in `rsworkspace/crates/trogon-mcp-gateway/src/rpc_codes.rs`:

| Code | Constant | Plan symbol |
|---|---|---|
| `-32100` | `POLICY_DENY` | `policy_deny` |
| `-32102` | `BACKEND_TIMEOUT` | `backend_timeout` |
| `-32103` | `BACKEND_UNREACHABLE` | `backend_unreachable` |
| `-32105` | `RATE_LIMITED` | `rate_limited` |
| `-32106` | `AUTH_EXPIRED` | `auth_expired` |
| `-32107` | `AUTHZ_UNREACHABLE` / `APPROVAL_REQUIRED` | `authz_unreachable` / `approval_required` |
| `-32109` | `AUDIENCE_MISMATCH` | `audience_mismatch` |
| `-32110` | `INVALID_TOKEN` | (enforce-mode token class) |
| `-32113`–`-32115` | act-chain codes | act-chain violations |
| `-32117` | `AGENT_IDENTITY_REQUIRED` | agent identity |
| `-32118` | `AUTH_REQUIRED` | auth required |

**Proposed in plan, not yet in `rpc_codes.rs`:** `-32101` `policy_fault`, `-32108` `no_policy`, `-32104` `schema_unknown`, `-32116` `backend_malformed` (this document).

---

## Invariants

1. **No allow without PDP** — For methods where CEL/`requires_spicedb` is true, an allow decision requires a successful SpiceDB check (or explicit Phase 1 dev bypass with endpoint unset). SpiceDB unreachable is always CLOSED in production configurations.

2. **No allow cache past trust** — Cached allow decisions (ZedToken, bulk-check results, approval cache entries) must not outlive the freshness guarantees of the associated ZedToken, trust bundle revision, or approval `expires_at`. Stale ZedToken retry must not widen permissions.

3. **No credential propagation on deny** — CLOSED paths must not publish to `mcp.server.*`, mint mesh tokens on egress, or forward inbound bearer tokens when enforce mode applies.

4. **Audit outcome matches decision** — `allow` audit envelopes are emitted only after all gates that apply to the method have succeeded. `deny`/`error` outcomes are emitted for CLOSED paths that reached the worker.

5. **Enforce mode is strict** — `MCP_GATEWAY_AGENT_IDENTITY=enforce` requires mesh token mint for egress; STS or trust failures are CLOSED, not shadow-allowed. See [overview.md](overview.md) rollout table.

6. **Approval is explicit** — Parked calls resume only on valid `approve` decision before TTL; timeout and deny are CLOSED with `-32107` and no backend forward.

7. **Backend timeout is not allow** — Missing backend reply never falls through to synthetic success; client receives `-32102`.

8. **Signed policy only** — Production bundles must verify NKey signature before activation; invalid signature never replaces the active bundle.

9. **CEL/WASM fault defaults closed** — Policy evaluation faults use failClosed defaults for mutating methods (`*/call`, `*/read`, `*/write`); fail-open with audit applies only to rules explicitly marked for non-mutating paths (e.g. `*/list` shaping per plan § SpiceDB Integration).

10. **Audit loss does not imply allow in enforce** — OPEN-with-audit on audit backlog (row 14) applies to the audit side channel only; it does not relax SpiceDB, STS, or approval CLOSED rules.

---

## Implementation phases

| Phase | Rows principally affected |
|---|---|
| Phase 1 **(today)** | 1, 8, 13 (STS mint), 14, 15 (via STS), 16 (library); partial 7, 10 |
| Phase 2 | 2, 6, 7, 10, 12 (session KV) |
| Phase 3 | 3, 4, 5, 9 (strict validation), bundle distribution |

When implementation diverges from this matrix, update this document in the same change set as the code (Block C completion criterion).
