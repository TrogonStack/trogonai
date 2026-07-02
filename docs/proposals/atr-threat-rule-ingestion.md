# Proposal: ATR Threat-Rule Corpus Ingestion for Tier-2 CEL

Status: Proposal (not approved, not implemented)
Tracks: MS_AGENT_GOV_TOOLKIT_WORKITEMS.md WI-11
Scope: this document is a decision brief. It contains no code changes and proposes none as a prerequisite.

## 1. Summary

Agent Threat Rules (ATR) is an MIT-licensed, external detection-rule corpus for AI agent and LLM threats (upstream taxonomy: 10 categories, 419 rules; a curated subset of 108 rules is highlighted upstream as high-confidence, with claimed 99.6% precision / 96.9% recall and CVE regression coverage). The Agent Governance Toolkit (AGT) ships two community examples that turn ATR YAML into AGT `PolicyDocument` YAML: `examples/atr-import/` (per-category compiler) and `examples/atr-community-rules/` (a curated bundle plus a `sync_atr_rules.py` converter and `test_atr_policy.py` regression suite).

TrogonAi's Tier-2 authorization gate (`a2a-gateway`'s `tier2_cel` module) has no external threat-rule corpus today and no YAML-to-CEL compilation step. Before any ATR content is pulled in, WI-11 asks a prior question explicitly: does Tier-2 want a declarative-YAML-to-CEL compiler at all, as a general capability, or is ATR ingestion better handled as a one-time content port?

This document recommends the one-time hand-port of a small, hand-picked subset of the curated 108-rule set, expressed directly as `.cel` files, with no general-purpose compiler built as a prerequisite. It also documents, with a concrete sampled mapping table, that a large fraction of the ATR corpus as currently authored does not translate to Tier-2 CEL predicates at all, because ATR rules are written against fields (`user_input`, `tool_response` free text, call-attempt counters) that do not exist among Tier-2's five bound variables.

## 2. Background: what Tier-2 CEL can see

Tier-2 evaluation (`rsworkspace/crates/a2a-gateway/src/policy/tier2_cel/evaluator.rs`, `bind_evaluation_context`) binds exactly five CEL variables per call, built from `Tier2EvaluationContext` (`rsworkspace/crates/a2a-gateway/src/policy/tier2/mod.rs`):

| CEL variable | Source | Shape |
|---|---|---|
| `request` | `ctx.request_method()`, `ctx.request_params()` | `{method: string, params: <json-rpc params>}` |
| `caller` | `ctx.caller_id()` (`SpiceDbSubject`) | `{id: string \| null}` |
| `agent` | `ctx.agent_id()` (`A2aAgentId`) | `{id: string}` |
| `task` | `ctx.task_id()` (`A2aTaskId`) | `{id: string \| null}` |
| `headers` | `ctx.headers()` | `map<string, string>` (wire-protocol headers, pass-through) |

Evaluation is stateless per call: rules are independently-evaluated `.cel` boolean expressions, first-`false`-wins, any error (including a non-boolean result) fails closed to `Deny`. There is no request body free-text field, no model-output text field, no per-agent call history, no rate/attempt counter, and no cross-request state of any kind visible to a rule. `request.params` is the JSON-RPC params blob (method arguments), not a generic "user input" or "tool description" string; whether a given ATR rule can be expressed at all depends entirely on whether the thing it inspects lives inside that params object, in a header, or nowhere Tier-2 can see.

## 3. Background: what ATR rules look like

Both AGT examples resolve to the same underlying shape once compiled. Inspecting the shipped `atr_security_policy.yaml` (nominally "15 rules," actually 1,555 compiled detection patterns from 287 production rules once every regex condition is expanded 1:1 into a rule row) shows every single rule has this shape:

```yaml
- name: atr-2026-00030
  condition:
    field: user_input        # or: tool_description | tool_response
    operator: matches         # always "matches" (PCRE/JS regex) in the sampled corpus
    value: (?i)(?:ignore|disregard|forget|override...)\s+...
  action: deny
  priority: 100
  message: '[ATR-2026-00030] Cross-Agent Attack Detection'
```

`sync_atr_rules.py`'s `CATEGORY_TO_FIELD` map confirms the field vocabulary ATR rules are written against: `user_input`, `tool_description`, `tool_response`. There is no `field` value in the sampled corpus that maps to a request method, a caller identity, a task id, or a transport header. ATR rules are fundamentally content-inspection rules over free text the agent sends or receives, not identity/authorization rules over request metadata. This is a structural mismatch with Tier-2's binding set, independent of any compiler question.

## 4. The core decision: build a YAML-to-CEL compiler, or not

### Option A: Build a general declarative-YAML-to-CEL compiler (reject)

This would mean defining a Tier-2-native `PolicyDocument`-equivalent (field/operator/value/action/priority/message), a compiler that renders each rule to a `.cel` expression, and an ongoing sync path to re-pull ATR (or any future declarative source) on schedule.

Reasons to reject as a prerequisite:
- The field vocabulary ATR expects (`user_input`, `tool_description`, `tool_response`) does not exist in `Tier2EvaluationContext` today. A compiler cannot manufacture bindings that the evaluator does not expose; building the compiler first only produces CEL expressions that reference undefined variables.
- AGT's own compiler (`import_atr.py`, `sync_atr_rules.py`) is Rego/YAML-and-Python-specific (regex validation via Python's `re` module, `PolicyDocument` schema, `agent_os` loader conventions). None of the code is reusable against `cel-interpreter`; only the mapping *logic* (severity-to-priority, category-to-field) is a useful reference, and CEL has no direct `matches`-with-flags regex operator parity guarantee with Python's `re` module, so a compiler would also need to re-validate every regex against CEL's actual regex engine, not just Python's.
- A generalized compiler is a standing piece of infrastructure (schema, validator, sync job, drift risk against upstream ATR releases) built to serve a single one-time import. That is infrastructure in search of a second user.
- Effort is materially higher than a hand port (see Section 7) for a benefit that, per Section 5, only applies to a minority of rules anyway.

### Option B: Hand-translate a curated subset of the 108-rule set once (recommended)

Manually select the subset of ATR rules whose `field` maps onto something Tier-2 can actually see today (chiefly `request.params` fields, when an agent's tool-call arguments carry the string ATR wants to inspect, and `headers`), write each as a native `.cel` file by hand, and stop there. No compiler, no sync job, no ongoing coupling to upstream ATR releases.

This is recommended because:
- It matches the actual size of the addressable subset (Section 5 estimates well under half the sampled rules translate cleanly).
- It produces plain `.cel` files indistinguishable from any other Tier-2 rule, reviewed and merged through the existing hot-reload/sorted-path bundle mechanism with no new subsystem.
- It leaves the door open to a compiler later, if and only if a second declarative source shows up that would amortize the cost (the current toolkit-comparison analysis in `.trogonai/analysis/agt-vs-trogonai/relate-policy.internal.trogonai.md` section (d) already flags this as "medium effort, worth scoping," not "build now").

### Option C: Vendor ATR content as test fixtures only, ingest no rules (fallback)

Keep the ATR corpus (or a curated slice of it) purely as a source of adversarial test payloads (the strings from `TestCVECoverageDenied`/`TestSemanticKernelCVECoverageDenied` in `test_atr_policy.py`), used to build negative test fixtures for whatever Tier-2 rules already exist or get written independently, without importing ATR's own rule text as authoritative logic.

This is a reasonable fallback if the decision-maker judges that even the hand-port subset (Option B) is not worth the ongoing maintenance of externally-sourced regexes, but still wants ATR's payload corpus as a regression source. It captures the "CVE regression tests carried over" value (Section 6) with the smallest footprint. It does not on its own close any detection gap; it only stress-tests gaps already closed elsewhere.

### Recommendation

Adopt Option B (hand-translate a small curated subset, once, no compiler) as the primary path, and adopt Option C's CVE payloads as regression fixtures regardless of which primary option is chosen, since they are cheap and portable independent of the compiler question. Do not build Option A. Revisit Option A only if a second, unrelated declarative-policy source (not just ATR) makes a general compiler worth its standing maintenance cost.

## 5. Sampled mapping table

Ten rules sampled across distinct ATR categories/themes present in the shipped `atr_security_policy.yaml`, each shown as it would translate to a Tier-2 CEL predicate given today's five bindings. "Fits" means the predicate is expressible without adding a new binding; "does not fit" names the binding gap.

| # | ATR rule (id / title) | ATR field | Fits Tier-2 today? | CEL predicate (or blocking gap) |
|---|---|---|---|---|
| 1 | ATR-2026-00030 / Cross-Agent Attack Detection (instruction-override phrase) | `user_input` | Does not fit | No field carries a free-text "user input" or agent-to-agent message string. `request.params` is JSON-RPC method arguments, not a chat/message payload; if a specific method's params schema has a known string argument (e.g. a `message` field on an A2A `message/send` call), this becomes expressible as `request.params.message.matches("(?i)ignore\\s+(all\\s+)?previous instructions")`, but only for that one method, and only if CEL's regex engine is confirmed to support the same syntax as the source pattern. As shipped (generic "user_input"), no binding. |
| 2 | ATR-2026-00012 / Unauthorized Tool Call Detection (path traversal in `tool_description`, e.g. `../../etc/passwd`) | `tool_description` | Does not fit | Tier-2 has no `tool_description` binding; tool metadata is not part of `request`, `caller`, `agent`, `task`, or `headers`. Would need a new binding carrying the resolved tool/skill descriptor text for the method being called, populated before evaluation. |
| 3 | ATR-2026-00012 / Unauthorized Tool Call Detection (shell metacharacter + binary name in `tool_description`) | `tool_description` | Does not fit | Same gap as #2. |
| 4 | ATR-2026-00064 / Over-Permissioned MCP Skill (`sudo`/`chmod`/`setcap` in `user_input`) | `user_input` | Partially fits | If the privileged-command string appears as a specific `request.params` argument (e.g. a `command` field on a shell-exec-shaped A2A method), expressible as `!request.params.command.matches("(?i)^(sudo\|runas\|doas\|pkexec\|gsudo)\\s+")`. Only works per-method, wherever the argument name and shape are known ahead of time; does not generalize across arbitrary methods the way ATR's blanket `user_input` field does. |
| 5 | ATR-2026-00146 / Environment Variable Existence Probing (env-var name + "is defined/set" phrase in `tool_response`) | `tool_response` | Does not fit | Tier-2 evaluates before the call executes and has no binding for a tool's response payload at all (Tier-2 is a pre-call gate, not a response filter). This is arguably a Tier-3 redaction/post-processing concern (`policy/tier3_redaction/`), not a Tier-2 gate, independent of the missing binding. |
| 6 | ATR-2026-00050 / Runaway Agent Loop Detection (`retry attempt 3`, `attempt 2 of 5` phrasing in `user_input`) | `user_input` | Does not fit (and arguably mis-modeled upstream) | Two separate gaps: no free-text input binding, and this rule pattern-matches the *word* "retry" rather than actually counting retries. Tier-2 has no cross-call state (call count, rate window, attempt counter) at all; expressing true runaway-loop detection needs a new stateful binding (e.g. `task.attempt_count` or a rate-limiter-fed field), not just a text field. Even with a `user_input` binding added, this rule would remain a weak proxy for the actual failure mode it claims to detect. |
| 7 | ATR-2026-00051 / Agent Resource Exhaustion Detection | `user_input` | Does not fit | Same state gap as #6: resource exhaustion is a metering concern, not a text-matching concern. Needs a counter/budget binding, conceptually adjacent to the `DYNAMIC-POLICY-CONDITIONS`-style temporal/budget layer already flagged as a separate, unrelated adoption candidate in `relate-policy.internal.trogonai.md` (b.1). |
| 8 | ATR-2026-00099 / High-Risk Tool Invocation Without Human Confirmation (payment/transfer keywords in `user_input`) | `user_input` | Partially fits | If the A2A method itself is a known "sensitive" method name (`request.method`), a coarser Tier-2 rule can require caller identity be present at all: `caller.id != null \|\| request.method != "payments.transfer"`. This is a materially weaker rule than ATR's (it gates on method name, not on keyword content of arbitrary params), but it is the closest honest translation available without a params-schema-aware content binding per method. |
| 9 | (illustrative, not literally in the sampled file) Caller/agent identity gating, e.g. "deny calls to an admin-scoped method from an unauthenticated caller" | n/a (not an ATR field; representative of what Tier-2 is actually good at) | Fits cleanly | `request.method != "admin.override" \|\| caller.id != null`, this is the shape of rule Tier-2's existing five bindings were designed for: identity and method gating, not content inspection. Included to make the contrast explicit: ATR and Tier-2 CEL are largely solving different layers of the problem. |
| 10 | ATR "Obfuscated Credential Exfiltration via Encoding" / "Bulk Environment Variable Harvesting and Exfiltration" (`user_input` / `tool_response`, encoded-secret patterns) | `user_input` / `tool_response` | Does not fit | Same missing-field gap as #1/#5; additionally this class of rule is best enforced as an egress/output filter (Tier-3 redaction), not a pre-call Tier-2 gate, since the exfiltration happens in the response, not the request. |

**Count: 7 of the 10 sampled rules (#1, #2, #3, #5, #6, #7, #10) do not fit today's five bindings at all. 2 of the 10 (#4, #8) partially fit, only under a narrowed, method-specific rewrite that is weaker than the original ATR rule. Only 1 of the 10 (#9, an illustrative non-ATR example included for contrast) fits cleanly, and it is not actually drawn from the ATR corpus; it demonstrates the kind of rule Tier-2's bindings are already suited for.**

This is consistent with Section 3's structural finding: ATR is a content-inspection corpus (free-text pattern matching over user input, tool descriptions, and tool responses), while Tier-2 CEL is an identity-and-method authorization gate (caller, agent, task, request method, headers). The overlap is narrow. Any hand-port (Option B) should be scoped to the narrow overlap (content that happens to live in a known `request.params` field for a specific method, or in `headers`) rather than treated as "port the corpus" in general.

## 6. Licensing and attribution

- **Rule text**: ATR (`github.com/Agent-Threat-Rule/agent-threat-rules`) is MIT-licensed. The rule *text* (pattern descriptions, category taxonomy, regex bodies, rule titles/ids) is freely reusable, including commercially, with attribution. A hand-ported `.cel` rule that encodes an ATR pattern should carry a source comment citing the upstream rule id (e.g. `ATR-2026-00064`) and a pointer to the upstream repository, plus the MIT notice, consistent with how AGT's own examples attribute it (`atr-import/README.md`, `atr-community-rules/README.md` both state "ATR is MIT-licensed" and link the source).
- **AGT's compiler code** (`import_atr.py`, `sync_atr_rules.py`, the `PolicyDocument`/Rego pipeline): this is AGT's own code, also MIT-licensed as part of the AGT repository, but it is Python/Rego-specific and architecturally incompatible with a CEL target (different schema, different regex engine assumptions, different loader). It is not being copied under this proposal; at most its *mapping logic* (severity-to-priority scale, category-to-field heuristic) is used as a design reference, not as reused code. No attribution obligation is triggered by referencing a design idea, but if any literal snippet (even a comment structure or a specific regex) is copied verbatim from AGT's compiler rather than from raw ATR rule text, treat it as AGT-derived and attribute accordingly.
- **Recommendation**: keep a single `THIRD_PARTY_NOTICES` entry (or equivalent, matching however TrogonAi already tracks vendored MIT content) naming ATR, its license, its upstream URL, and the specific rule ids ported, updated each time a new rule is hand-added. Do not claim the precision/recall figures as verified by TrogonAi (see Section 8).

## 7. Effort estimate per alternative

| Alternative | Scope | Rough effort |
|---|---|---|
| A: General YAML-to-CEL compiler | Define schema, build compiler + regex-compatibility validator against `cel-interpreter`, build sync/refresh job, add new bindings the schema would need (tool description/response, params-schema awareness), write tests | Weeks, not days; ongoing maintenance cost (upstream ATR drift, regex-engine parity bugs) after initial build. Not recommended (Section 4). |
| B: Hand-translate a curated subset once | Pick ~10-30 rules from the narrow overlap identified in Section 5 (method-specific `request.params` content, `headers`), write each as a native `.cel` file with a source-attribution comment, add fixture tests per rule | Low days (roughly 0.5-1 day per rule once the first one establishes the pattern, given how few rules cleanly fit; most of the effort is in judging fit, not writing CEL). Recommended primary path. |
| C: Vendor ATR payloads as test fixtures only | Extract the CVE-linked payload strings from `test_atr_policy.py` (Flowise `overrideConfig` RCE, `mcp-atlassian` path traversal, Semantic Kernel lambda-eval and startup-persistence payloads) into a Tier-2 fixture format, run them against whatever rules already exist (from Option B or otherwise), assert deny/allow | Low days; smaller than B since no new detection logic is authored, only regression payloads are captured. Can run in parallel with B or standalone. |

None of the three alternatives require the general compiler as a prerequisite; B and C can both proceed independently and in parallel.

## 8. Carrying over the CVE regression tests

`atr-community-rules/test_atr_policy.py` includes named CVE regression coverage:

- `CVE-2025-59528` (Flowise `overrideConfig` child_process RCE): payload lives in `tool_description`.
- `CVE-2026-33032` (Nginx UI MCP privileged tool invocation / auth bypass): payload lives in `tool_description`.
- `CVE-2026-27825` (`mcp-atlassian` path traversal to `authorized_keys`): payload lives in `tool_description`.
- `CVE-2026-26030` (Semantic Kernel unsafe lambda interpolation / dynamic import traversal): payloads live in `user_input`.
- `CVE-2026-25592` (Semantic Kernel startup persistence chain): payloads live in `user_input` and `tool_description`.

All five CVE payload families use `tool_description` or `user_input`, i.e. the same fields flagged in Section 5 as not directly bindable in Tier-2 today. Carrying these over honestly means:

1. **If Option B ports a rule that happens to cover one of these payload shapes for a specific, known A2A method** (e.g. a rule restricting a shell-exec-shaped method's `request.params.command` field), the corresponding CVE payload string becomes a Tier-2 fixture test asserting `Deny{rule}` for that payload routed through that method's params, and `Allow` for the paired benign payload the upstream suite also includes (e.g. `test_benign_lambda_filter_expression`, `test_benign_attachment_filename`). This preserves the "positive AND negative case" discipline the upstream suite already has: carry over both sides, not just the deny case.
2. **If no binding covers the payload's field** (the common case per Section 5), the CVE payload cannot be exercised as a Tier-2 CEL fixture at all until the relevant binding exists. In that case, record the gap explicitly (which CVE, which missing field) rather than silently dropping the test, so a future binding addition has a ready-made regression case to attach.
3. Fixture format should follow the existing `tier2-cel-test`-style pattern already scoped elsewhere in the toolkit-comparison analysis (WI referenced in `relate-policy.internal.trogonai.md`, item (d): "`agt test`'s fixture schema and replay-CLI pattern onto Tier-2 CEL"): a `{context, expected: Allow|Deny}` fixture consumed by a small test binary, rather than inventing a second fixture format specific to ATR.

## 9. Honesty about upstream claims

The 99.6% precision / 96.9% recall figures, and the "adopted by Cisco AI Defense and other security platforms" claim, are stated in `atr-community-rules/README.md` as upstream ATR claims. They have not been independently verified in this analysis or against TrogonAi's own traffic, and no TrogonAi-run benchmark exists to confirm or refute them. Any hand-ported rule (Option B) should be evaluated on its own merits against TrogonAi's actual false-positive tolerance, not treated as inheriting a precision/recall guarantee from the upstream figure. The 108-rule "curated" set is itself an upstream editorial selection (described as "high-confidence" in the README); this proposal's own Section 5 sample was drawn independently from the shipped compiled YAML and is not a re-sample of that curated 108, so the two sampling exercises should not be conflated.

## 10. Recommendation summary

Do not build a general YAML-to-CEL compiler as a prerequisite for ATR ingestion. Hand-translate a small subset of ATR rules directly to `.cel` files, scoped to the narrow set where a rule's inspected field genuinely maps onto `request.params` for a known method or onto `headers`, each with a source-attribution comment citing the ATR rule id and license. In parallel, lift the CVE regression payloads as Tier-2 test fixtures wherever a corresponding rule exists, and record the remaining CVE payloads as explicit known gaps tied to specific missing bindings (tool description/response content, cross-call attempt/rate state) rather than silently dropping them. Treat the upstream precision/recall figures as unverified marketing claims, not as a property TrogonAi inherits by importing the rule text.
