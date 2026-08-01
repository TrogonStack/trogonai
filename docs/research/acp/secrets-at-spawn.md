# Credential Injection at Spawn: Secrets Architecture for acp-host

Distills prior staged secret-management research into the
credential-injection flow acp-host needs. Produced 2026-07-30 by a
source-grounded distillation agent; single-pass, every claim cites a file.

## 1. What the staged sources conclude

The staged research converges: **OpenBao is the provider, and this is an
accepted decision, not an open question** (ADR 0023, accepted 2026-07-11).

- Secret management research splits "KMS" into two problems
  that must never share an interface: secret storage (values retrieved
  later, via `SecretRef`) and cryptographic operations (key never leaves
  custody, via `KeyRef`). Picks OpenBao under a strict FOSS acceptance
  rule that rejects HashiCorp Vault (BUSL-1.1) and Cosmian/Eviden
  (BUSL-1.1); Infisical Community disqualified because HSM root protection
  is enterprise-only.
- AWS-vs-OpenBao research (OpenBao v2.5.5): AWS KMS is a
  hardware-boundary custody service, not a secrets store. OpenBao Transit
  functionally replaces it with two honest gaps: software-only (no FIPS
  140-validated build, openbao/openbao#1409) and AWS-managed at-rest
  encryption can only call KMS. For self-hosted-first trogonai: OpenBao
  primary, AWS adapters non-default, matching the intended backend order
  (StaticConfig -> OpenBao -> AWS -> Valkey cache only).
- GitLab-OpenBao research: strongest production prior art.
  Per-tenant OpenBao namespaces (not path prefixes), inline JWT auth so
  Rails never holds a standing token, ephemeral scoped tokens,
  CEL-computed per-principal policy from claims. Cited by ADR 0023 as
  studied prior art.
- Basil research and clippings: Basil (host-local secrets broker,
  SO_PEERCRED attestation, default-deny, dual-sink audit) is architecturally
  the closest FOSS embodiment of the direction, but NOT production-adoptable:
  ~1 week old at research time, single-maintainer, pre-1.0 unstable wire
  protocol, no HA, no Kubernetes; `MintJwt` hardcodes JOSE `typ` and
  enforces no TTL ceiling. Posture: adopt the port/trait pattern, not the
  daemon; dev-only prototyping.

## 2. The spawn-time injection flow and where ADR 0023 slots in

ADR 0023 (accepted) fixes the shape: OpenBao as sole provider behind two
typed ports (`SecretStore` via `SecretRef`, `KeyManagement` via `KeyRef`);
a platform secrets service as the ONLY OpenBao client (deliberate
rejection of GitLab-runner-style direct reads, because trogonai's
consumers are long-lived multi-tenant internet-exposed processes, not
ephemeral single-tenant CI jobs); resolution over NATS core request/reply
authenticated by tenant-scoped NATS user JWTs; secret values never
traverse JetStream (only rotation/invalidation metadata does); secrets
service bootstraps to OpenBao with deployment-attested identity and fails
closed.

Mapped onto acp-host:

```
acp-host (about to spawn `gemini --acp` for company C, agent A)
  -> resolve SecretRef for (company=C, agent=A, provider=gemini)
     via NATS core request/reply
  -> secrets service reads/mints the scoped credential from OpenBao KV v2
  -> plaintext returned over core req/reply, held only in-process
  -> acp-host sets GEMINI_API_KEY in the child env block at spawn
  -> never persisted, never logged, never in any ACP message
```

Scoping: company is the tenant (aligned with the NATS account boundary,
ADR 0023 section 5); per-agent is a `SecretRef` dimension derived
deterministically by the secrets service's path-builder (GitLab
immutable-ID convention), not a new isolation primitive; per-session is
just a per-spawn resolve call scoped by the session's authenticated
company/agent identity.

Design tension flagged for an ADR: ADR 0032 (draft) governs trogonai's
OWN hosted implementations, which never see raw vendor keys (they get
session-scoped grant tokens; the model-access-service resolves real keys
at the outbound HTTP call). Third-party ACP CLIs are different: they read
`GEMINI_API_KEY`/`ANTHROPIC_API_KEY` from the OS environment and call the
vendor directly, with no model-access-service hop (short of building
per-vendor local HTTP shims, heavier and not yet justified). Resolution:
acp-host env-injection is a NEW CALLER of the ADR 0023 SecretStore
boundary, not a conflict with ADR 0032.

## 3. Hygiene requirements

- **Never in ACP messages** (ADR 0032 section 6, hard rule), extended by
  section 10 to a whole-system non-persistence proof: no credential in
  Agent, ACP, prompt, transcript, checkpoint, JetStream, NATS KV, object
  store, database, backup, log, trace, or crash state.
- **Never in logs/transcripts**: ADR 0023 dual audit (business context +
  correlation id joining OpenBao's audit log) with plaintext never
  recorded.
- **Redaction in streamed output**: a real gap the ADRs do not cover.
  Vendor CLIs can echo env during error dumps or `--debug` traces;
  acp-host needs its own output-scrubbing layer over child stdout/stderr
  before anything durable (CI-style masking, the GitLab pattern).
- **TTL and rotation**: caches bounded in minutes; event-driven
  invalidation as primary, TTL as backstop; invalidation is fan-out (every
  replica's own consumer), never queue-group work-sharing, or one replica
  keeps serving a revoked credential.
- **Audit**: which company/agent/session resolved which SecretRef at which
  spawn, correlation id threaded through (mirrors ADR 0032 section 10 and
  GitLab's principal/key-reference audit shape).

## 4. Prior art: headless keys and what breaks on rotation

gemini-cli (GEMINI_API_KEY or Vertex ADC; OAuth not headless), OpenClaw
(acpx spawns the harness, which "owns its provider login"; rotation =
operator edits config, next spawn picks it up), and Hermes (`hermes acp`
uses whatever credential Hermes holds in ~/.hermes/.env) all use the same
shallow env-var-at-spawn model, and NONE solve mid-session rotation. A
POSIX child's env block is immutable after spawn, so the only levers are:
(a) next-spawn correctness via cache invalidation, and (b) kill-and-respawn
for security-grade revocation. This mirrors ADR 0032 section 7's Drain vs
FailIfInUse split applied to OS child processes. CLIProxyAPI is
the corpus's most sophisticated credential-lifecycle prior art (pooling,
session affinity, failover, cooldown persistence); mirror its
control-plane/data-plane split if acp-host grows an admin surface, but not
its subscription-arbitrage use case.

## 5. Recommendation

Adopt ADR 0023 as-is; do not reopen the store question.

1. **Store**: OpenBao KV v2 behind the platform secrets service; one
   `SecretRef` per (company, agent, provider); path derived by the
   service, never supplied by acp-host.
2. **Resolution**: NATS core request/reply immediately before each spawn,
   authenticated by the session's tenant-scoped NATS JWT. No new
   credential type, no new network path.
3. **Injection**: plaintext lives only in acp-host process memory for the
   spawn call; written into the child env block; zero caching beyond the
   in-flight operation.
4. **Rotation**: bounded-TTL cache + fan-out invalidation; ordinary
   rotation takes effect next spawn (Drain-equivalent); security
   revocation kills and respawns immediately (FailIfInUse-equivalent).
5. **Verify before an ADR**: fail-closed spawn when resolution fails (no
   placeholder fallback), rotation event observed to evict any resolve
   cache, kill-and-respawn without leaking the old credential into crash
   artifacts, and confirmation that vendor CLI debug output cannot leak
   the injected env var into durable logs acp-host forwards.

## Key claims

- ADR 0023 (accepted 2026-07-11) already commits to OpenBao, a sole-client
  platform secrets service, and NATS core request/reply resolution; the
  "which store" question is settled.
- acp-host does not exist yet (zero process-spawning code in the ACP
  crates, per the adversarially verified product dossiers); env-injection
  at spawn is a new caller of the ADR 0023 boundary.
- Basil: borrow the broker port/trait shape, do not adopt the daemon
  (pre-1.0, no HA, policy hazards in MintJwt).
- ADR 0023 vs 0032: different consumers (third-party CLIs calling vendors
  directly vs hosted implementations behind the model-access-service); no
  conflict.
- Nobody in the corpus solves mid-session rotation; next-spawn
  invalidation plus kill-and-respawn is the only mechanism.
- GitLab's OpenBao integration is the production-scale reference for the
  namespace/inline-auth/CEL-policy pattern.

## Sources

Prior secret-management research (SECRET_MANAGEMNT, AWS_VS_OPENBAO,
GITLAB_OPENBAO, BASIL_RESEARCH, and the Basil clipping), plus prior
research on secret-store design, API keys, open connectors, and agent
service design QA. [Host role and
invocation](./host-role-and-invocation.md). Product dossiers
([gemini-cli](./products/gemini-cli.md),
[openclaw](./products/openclaw.md),
[hermes-agent](./products/hermes-agent.md)).
`tmp/trogonai` docs/adr/0023 (accepted) and 0032 (draft).
