# Gateway ingress fault checklist

Checklist for chaos-testing `trogon-gateway`'s webhook ingress (HTTP handlers under
`rsworkspace/crates/trogon-gateway/src/source/*/server.rs`) against a general-purpose
fault taxonomy. The taxonomy enumerates fault classes an SRE program should consider;
it does not imply the gateway needs every fault type exercised in-process, since some
target concerns (prompt injection into an LLM, trust scoring, agent-to-agent
deadlocks) that do not exist at an HTTP webhook boundary.

The fault type list is drawn from the `FaultType` enum in the
AGENT-SRE-GOVERNANCE-1.0 specification.

## Scope

"trogon-gateway webhook ingress" means the Axum handlers that accept inbound
webhooks from external providers (GitHub, GitLab, Slack, Telegram, Twitter,
Linear, Notion, Sentry, Microsoft Graph, incident.io, Datadog) and publish
validated events onto JetStream via `ClaimCheckPublisher`. Reviewed sources for
this checklist: `src/source/datadog/server.rs` (newest, added in #456, includes
the "fail loudly when enabled integration omits webhook config" fix from #457),
`src/source/github/server.rs`, `src/source/gitlab/server.rs`,
`src/source/telegram/server.rs`, `src/source/incidentio/server.rs`, and their
respective `server/tests.rs` files, plus `trogon-nats`'s `publish.rs` and
`mocks.rs`.

## Infrastructure faults

| Fault type | Applies to gateway ingress | Existing coverage | Missing test |
| --- | --- | --- | --- |
| `LATENCY_INJECTION` | Not directly. The gateway does not call out to a downstream provider synchronously during ingress; the only network hop is the JetStream publish/ack round trip, and its latency is bounded by `nats_ack_timeout`, which folds into `TIMEOUT_INJECTION` below. | N/A | None needed. Injecting artificial latency without a corresponding timeout assertion would not exercise a distinct code path from the timeout case. |
| `ERROR_INJECTION` | Yes. A JetStream publish call can fail (broker rejects, connection drop) before the ack future resolves. | `publish_failure_returns_500` and `unroutable_publish_failure_returns_500` in `src/source/datadog/server/tests.rs` use `MockJetStreamPublisher::fail_next_js_publish()` to force `PublishOutcome::PublishFailed` and assert `500`. Equivalent tests exist for github, gitlab, telegram, incidentio. | None. Covered. |
| `TIMEOUT_INJECTION` | Yes. `publish_event` in `trogon-nats/src/jetstream/publish.rs` wraps the ack future in `tokio::time::timeout(ack_timeout, ack_future)`; a slow or hung JetStream ack must surface as `PublishOutcome::AckTimedOut` and a `500`. | Present for github (`ack_timeout_returns_500`), gitlab, telegram, and slack via a shared `ack_test_support.rs` helper (`AckFailPublisher::hanging()` + `NonZeroDuration::from_millis(10)`). **Datadog had no equivalent test before this work item.** | Added in this work item: `ack_timeout_returns_500` in `src/source/datadog/server/tests.rs`, using a new `hanging()` publisher that returns a never-resolving ack future, following the same pattern as `github`/`gitlab`/`telegram`. |

## Adversarial faults

| Fault type | Applies to gateway ingress | Existing coverage | Missing test |
| --- | --- | --- | --- |
| `PROMPT_INJECTION` | No. The gateway ingress path only validates a shared-secret token (or HMAC signature) and forwards the raw webhook body to NATS unopened; it does not construct or feed a prompt to an LLM, so there is no prompt-injection surface at this boundary. | N/A | None. Genuinely out of scope; downstream agent/tool layers that do parse payloads into prompts are where this fault type belongs. |
| `POLICY_BYPASS` | Partially, reframed as webhook-authentication bypass: can a caller skip or spoof the token/signature check? | `missing_token_returns_401_and_publishes_nothing` and `wrong_token_returns_401_and_publishes_nothing` (Datadog); equivalent signature-mismatch tests exist for github, gitlab, telegram, slack, incident.io. `custom_webhook_token_header_is_honored` also checks the header name cannot be bypassed by using the default name once a custom one is configured. | None. Covered as far as it applies (there is no separate CEL/SpiceDB policy evaluation in the ingress path itself; that happens downstream of the gateway). |
| `PRIVILEGE_ESCALATION` | No. The gateway ingress handler has no notion of caller identity/role beyond "possesses the shared secret or valid signature"; there is no privilege tier to escalate within this boundary. | N/A | None. Out of scope for this component. |
| `DATA_EXFILTRATION` | No. The handler is a one-way ingress (provider to gateway to NATS); it has no read/query surface an attacker could use to exfiltrate data, and the body is opaque bytes forwarded verbatim. | N/A | None. Out of scope for this component. |
| `TOOL_ABUSE` | No. The webhook ingress does not invoke tools; it is a pure ingestion boundary. | N/A | None. Out of scope for this component. |
| `IDENTITY_SPOOFING` | Yes, reframed as request forgery: can a caller impersonate the webhook provider without the shared secret/signature? | Same tests as `POLICY_BYPASS` above (missing/wrong token, invalid HMAC signature) cover this for every source that has a secret. Datadog additionally has `fresh_timestamp_publishes_when_tolerance_enabled` / `stale_timestamp_returns_401_and_publishes_nothing` covering replay-via-stale-timestamp spoofing when timestamp tolerance is enabled. | None. Covered. |

## Behavioral faults

| Fault type | Applies to gateway ingress | Existing coverage | Missing test |
| --- | --- | --- | --- |
| `DEADLOCK_INJECTION` | No. A single webhook handler invocation has no circular dependency on another agent or handler instance; each request is independent and stateless beyond the shared `AppState`. | N/A | None. Out of scope; this fault type targets multi-agent orchestration, not a stateless HTTP ingress handler. |
| `CONTRADICTORY_INSTRUCTION` | No. The ingress handler does not interpret instructions or directives from the payload; it only routes on `event_type` and forwards the body opaquely. Malformed or self-contradictory JSON is a parsing concern, covered under "malformed payloads" below rather than this fault type. | N/A | None. Out of scope as specified (no directive interpretation happens here). Related malformed-payload robustness is covered separately (see below). |
| `TRUST_PERTURBATION` | No. There is no numeric trust score in this codebase (explicitly a non-goal per `MS_AGENT_GOV_TOOLKIT_WORKITEMS.md`); the gateway's authorization is binary (valid secret/signature or not), which `POLICY_BYPASS`/`IDENTITY_SPOOFING` above already exercise. | N/A | None. Out of scope; no trust-score subsystem exists to perturb. |

## Related robustness gaps (not in the 12-value taxonomy, called out because the work item also asks for malformed-payload and replay coverage)

- **Malformed / oversized payloads.** Datadog already tested invalid JSON, missing `event_type`, and invalid `event_type` values (`invalid_json_publishes_unroutable_and_returns_ok`, `missing_event_type_publishes_unroutable_and_returns_ok`, `invalid_event_type_publishes_unroutable_and_returns_ok`). It had no test for a body exceeding `DefaultBodyLimit`/`HTTP_BODY_SIZE_MAX`, unlike `telegram` (`body_exceeding_limit_returns_413`). Added `body_exceeding_limit_returns_413` for Datadog in this work item, following the telegram pattern (smaller `DefaultBodyLimit` layered onto a fresh router so the test does not need a 2 MiB payload).
- **Replay / dedup-window abuse.** JetStream dedup is enforced server-side via the `Nats-Msg-Id` header within a stream's `duplicate_window`; `MockJetStreamPublisher` (used by Datadog, github, gitlab, telegram) does not simulate dedup at all; only the separate `MockJetStreamPublishMessage` mock (used by different call sites, not the webhook `ClaimCheckPublisher` path) can enqueue a `Duplicate` ack. This means true dedup enforcement is **not testable in-process** for the webhook ingress handlers without swapping in a different publisher abstraction, and doing so would test the mock, not the gateway. What is testable and was previously unverified for Datadog: that replaying the identical webhook body produces the identical dedup key (`NATS_MESSAGE_ID` = the payload's `id` field) on every delivery, so the gateway holds up its end of the at-least-once/dedup contract. Added `replayed_event_id_produces_identical_dedup_key` for Datadog in this work item. Also noted: unlike `incidentio`, Datadog's `provision()` does not set an explicit `duplicate_window` on its stream config, so it inherits the JetStream server default (2 minutes) rather than a value derived from `timestamp_tolerance`; this is a design gap worth a follow-up, not a test gap, and is left undocumented in code beyond this note.

## Summary

- 4 of 12 fault types apply to this boundary in some form and already had or now have coverage: `ERROR_INJECTION`, `TIMEOUT_INJECTION`, `POLICY_BYPASS`, `IDENTITY_SPOOFING`.
- 8 of 12 are genuinely out of scope for a stateless webhook-ingestion HTTP handler: `LATENCY_INJECTION` (folds into timeout), `PROMPT_INJECTION`, `PRIVILEGE_ESCALATION`, `DATA_EXFILTRATION`, `TOOL_ABUSE`, `DEADLOCK_INJECTION`, `CONTRADICTORY_INSTRUCTION`, `TRUST_PERTURBATION`. These target LLM prompt surfaces, multi-agent orchestration, or stateful trust subsystems that do not exist in `trogon-gateway`'s ingress layer.
- The two tests added by this work item (`ack_timeout_returns_500`, `body_exceeding_limit_returns_413`) close the gap between Datadog and the other webhook sources that already had this coverage; `replayed_event_id_produces_identical_dedup_key` adds new coverage no other source had.
