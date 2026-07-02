# mcp-gateway

Pure-Rust MCP (Model Context Protocol) security scanner library. No I/O, no
NATS, no registry state, no external dependencies beyond what the workspace
already pins (`sha2`, `serde`, `serde_json`, `thiserror`).

Every detector is a pure function over the value types in this crate. The
library owns no registry or baseline storage; a caller (the future
`mcp-gateway` NATS service) supplies comparison state (known tools,
fingerprints, baselines) and calls these functions at `tools/list` response
time.

## What it scans

- **Typosquat / cross-server attack** (`scan::typosquat`): hand-rolled
  Levenshtein distance (no `regex`, no algorithm crate), flags tool names
  within edit distance 1-2 of a known tool name (minimum name length 4), and
  detects exact-name impersonation across servers.
- **Rug pull** (`scan::rug_pull`): SHA-256 fingerprint over a tool's
  description and canonical-JSON schema; flags when either hash changes for
  a previously observed tool.
- **Schema drift** (`scan::schema_drift`): deterministic per-tool and
  per-server fingerprints, diffed into an 8-value `DriftType` taxonomy
  (`ToolAdded`, `ToolRemoved`, `SchemaChanged`, `ParameterAdded`,
  `ParameterRemoved`, `TypeChanged`, `DescriptionChanged`,
  `RequiredChanged`) with severity classification matching
  MCP-SECURITY-GATEWAY-1.0 section 16.9. `SchemaChanged` is defined for API
  completeness but, matching upstream AGT behavior, is never emitted:
  every schema change is already classified into one of the more specific
  variants.
- **Hidden instructions** (`scan::hidden_instructions`): invisible Unicode
  (zero-width characters, BOM, bidi embedding/override/isolates, soft
  hyphen, word joiner, and the Unicode Tag block U+E0000-U+E007F), hidden
  HTML/Markdown comments, encoded payloads (long base64 runs, hex escape
  sequences), content hidden after excessive whitespace, and
  instruction-override phrases (`ignore previous`, `system:`, `disregard
  prior`, etc).
- **Description injection** (`scan::description_injection`): role-override
  phrases (`you are`, `must be called`, `mandatory`, ...), data-exfiltration
  phrases (`curl`, `send to`, `include the contents of`, ...), and
  privilege-escalation phrases (`sudo`, `root access`, `exec(`, ...).

None of the pattern matching uses a regex engine: this workspace has no
`regex` crate, so AGT's `re.compile(...)` patterns are reimplemented as
small ordered-word and substring matchers in `scan::text_pattern`.

## What is deferred

- **WI-01** (the `mcp-gateway` NATS service): tool-call/response
  interception, registry/baseline persistence and lifetime, message
  signing, session/auth enforcement, sliding rate limiting, audit trail
  sinks, and `TrustGatedMCPServer` trust/capability/circuit-breaker
  enforcement. This crate has no process, no network I/O, and holds no
  state across calls.
- **WI-05**: the CVE feed gate (OSV API lookup, 1-hour cache, fail-closed
  on network failure) is a separate work item and not implemented here.
- Schema-abuse property scanning (`ToolPoisoning` findings from suspicious
  required-field names, instructions embedded in schema default values or
  property descriptions) and confused-deputy detection are part of AGT's
  `MCPSecurityScanner` but outside WI-02/WI-03/WI-04's scope; they are not
  implemented in this crate.

See `tests/conformance.rs` for the full MUST-by-MUST breakdown of what
MCP-SECURITY-GATEWAY-1.0 section 20.1 requires from the scanner library
versus the gateway service layer.
