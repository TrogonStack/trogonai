# ACP Rust Crate Ecosystem Inventory

Scope: the Agent Client Protocol (ACP, agentclientprotocol.com), Zed's client-to-agent protocol. Not IBM's Agent Communication Protocol. All data verified live against the crates.io API, GitHub API (`gh api`), and the project's own docs as of 2026-07-30.

## Headline finding: the project moved to its own GitHub org

The canonical repos are now under a dedicated **`agentclientprotocol`** GitHub org, not `zed-industries`. `github.com/zed-industries/agent-client-protocol` now 301-redirects to `github.com/agentclientprotocol/agent-client-protocol`. Zed remains the maintainer of record (`authors = ["Zed <hi@zed.dev>"]` in the Rust workspace `Cargo.toml`), but the spec/schema and the Rust SDK live in two separate repos:

- **`agentclientprotocol/agent-client-protocol`**: the spec, JSON Schema (`schema/v1`, `schema/v2`), docs site source, and the `agent-client-protocol-schema` crate. 3,811 stars, 30 open issues (mostly docs additions for new third-party clients, a healthy ecosystem signal), pushed as recently as 2026-07-30.
- **`agentclientprotocol/rust-sdk`**: the Rust SDK workspace containing every other official crate. 174 stars, 6 open issues, daily commits, pushed 2026-07-28.

## Official family

| Crate | Latest | Released | Status |
|---|---|---|---|
| `agent-client-protocol` | 2.0.0 | 2026-07-23 | Active, core |
| `agent-client-protocol-schema` | 1.6.0 | 2026-07-21 | Active, core |
| `agent-client-protocol-derive` | 2.0.0 | 2026-07-23 | Active, core |
| `agent-client-protocol-http` | 2.0.0 | 2026-07-23 | Active, transport |
| `agent-client-protocol-polyfill` | 2.0.0 | 2026-07-23 | Active, low adoption |
| `agent-client-protocol-rmcp` | 3.0.0 | 2026-07-23 | Active, low adoption |
| `agent-client-protocol-conductor` | 2.0.0 | 2026-07-23 | Active, low adoption |
| `agent-client-protocol-tokio` | 0.11.1 | 2026-04-21 | **Superseded, no longer a workspace member** |
| `agent-client-protocol-trace-viewer` | 2.0.0 | 2026-07-23 | Active, dev tool |
| `agent-client-protocol-cookbook` | 2.0.0 | 2026-07-23 | Active, docs-as-code (not in the original ask, found while inventorying the workspace) |

All eight actively-maintained crates in the `rust-sdk` workspace release together on a coordinated train driven by `release-plz` (same-day version bumps across the family: 2026-06-24 → 1.0.0, 2026-06-29 → 1.0.1, 2026-07-06/07 → 1.1.0/1.2.0, 2026-07-20 → 1.3.0, 2026-07-23 → 2.0.0). `agent-client-protocol-rmcp` runs one major version ahead (3.0.0) because of its own upstream `rmcp` breaking changes.

**`agent-client-protocol-tokio` is dead weight.** It was last published 2026-04-21 at 0.11.1, has only ever had two versions, and is **not listed in the current `rust-sdk` workspace `Cargo.toml`**. Its process-spawning and `AcpAgent`/`Stdio` functionality was folded into the core `agent-client-protocol` crate as part of the 1.0/2.0 line; the 2.0 migration guide explicitly documents `AcpAgent` moving to `AcpAgentConfig` in the core crate. It still shows ~63K downloads from projects pinned to the old 0.x line, but there is nothing to gain by adding it new.

### Detail per crate

**`agent-client-protocol`** ([crates.io](https://crates.io/crates/agent-client-protocol), [docs.rs](https://docs.rs/agent-client-protocol), [repo](https://github.com/agentclientprotocol/rust-sdk))
Core protocol crate: Client/Agent/Proxy/Conductor roles, connection builders, handlers, protocol types. 77 published versions, ~3.4M all-time downloads. **Already adopted by trogonai** (pinned `=2.0.0`).

**`agent-client-protocol-schema`** ([crates.io](https://crates.io/crates/agent-client-protocol-schema), [repo](https://github.com/agentclientprotocol/agent-client-protocol))
Wire-format data model (requests, responses, notifications, JSON-RPC envelopes). 52 versions. **Already adopted by trogonai**, pinned `=1.5.0` (2026-07-20), one release behind current `1.6.0` (2026-07-21). The current `rust-sdk` workspace itself requires `=1.6.0`. Trivial bump, low risk, recommended now.

**`agent-client-protocol-derive`** ([crates.io](https://crates.io/crates/agent-client-protocol-derive))
Derive macros for ACP JSON-RPC traits. Tracks the core crate version exactly. **Already adopted.**

**`agent-client-protocol-http`** ([crates.io](https://crates.io/crates/agent-client-protocol-http))
HTTP/SSE and WebSocket transport. Younger (created 2026-06-19), zero known external reverse dependencies. **Already adopted by trogonai** (server feature, 2.0.0).

**`agent-client-protocol-polyfill`** ([crates.io](https://crates.io/crates/agent-client-protocol-polyfill))
Compatibility proxies, notably MCP-over-ACP adaptation for older clients/agents. Very low adoption (573 downloads) but actively developed (a v2 MCP-over-ACP bridge feature landed 2026-07-28). **Not adopted by trogonai.** Adopt later if trogonai needs to bridge ACP peers that only speak the older MCP-over-ACP declaration style.

**`agent-client-protocol-rmcp`** ([crates.io](https://crates.io/crates/agent-client-protocol-rmcp))
Integration with the official Rust MCP SDK (`rmcp`). Versioned independently at 3.0.0 due to upstream rmcp breaking changes. Small real usage (`aster-cli`, `aster-core` depend on it). **Not adopted by trogonai.** Adopt later if `agents-service` starts hosting/consuming MCP servers in Rust and wants rmcp-typed integration.

**`agent-client-protocol-conductor`** ([crates.io](https://crates.io/crates/agent-client-protocol-conductor))
Binary/library for orchestrating chains of ACP proxies between editor and agent. Actively developed (v2 proxy initialization landed 2026-07-28). **Not adopted by trogonai.** Adopt later if trogonai wants Rust-native proxy-chain composition (auditing, redaction, rate limiting) rather than building that into `agents-service`/`trogon-gateway` directly; trogonai's NATS-based transport plus gateway likely already covers this role today.

**`agent-client-protocol-tokio`** ([crates.io](https://crates.io/crates/agent-client-protocol-tokio))
Historically: Tokio utilities for spawning agent subprocesses and stdio wiring. **Skip.** Superseded by `agent-client-protocol` 2.0.0 itself; no longer a workspace member; no further releases planned.

**`agent-client-protocol-trace-viewer`** ([crates.io](https://crates.io/crates/agent-client-protocol-trace-viewer))
Interactive sequence-diagram viewer for conductor/ACP trace files. Dev tool, not a runtime dependency. **Not adopted by trogonai.** Adopt later as a local debugging aid, low priority.

**`agent-client-protocol-cookbook`** ([crates.io](https://crates.io/crates/agent-client-protocol-cookbook))
Cookbook of runnable ACP patterns. Documentation-as-code, minimal downloads. Worth reading when onboarding engineers to trogonai's ACP integration, not a dependency.

## Community and third-party crates

The `agent-client-protocol` crate has 76 crates.io reverse dependencies and `agent-client-protocol-schema` has 31 (some overlapping), reflecting a large and fast-growing third-party ecosystem by mid-2026. Most are small, narrowly-scoped, single-maintainer projects. The ones with clear real-world signal:

**Codex adapters** (a specific ask):
- **`agentclientprotocol/codex-acp`** ([GitHub](https://github.com/agentclientprotocol/codex-acp)) - now hosted under the **official ACP org itself**, 206 stars, pushed 2026-07-29. Exposes Codex CLI functionality (subagent launches as ACP tool calls, client-provided MCP servers, slash commands `/status`, `/mcp`, `/skills`, `/review`) to any ACP client. This is now the reference Codex bridge. Verify the exact crates.io publish name before depending on it; it was found via GitHub activity during this survey and its crates.io listing was not separately confirmed.
- **`cola-io/codex-acp`** ([GitHub](https://github.com/cola-io/codex-acp)) - the original community bridge, 142 stars, but stale since 2026-01-06, effectively superseded by the official one.
- **`brokk-codex-acp`** ([crates.io](https://crates.io/crates/brokk-codex-acp)) - alpha, backed by the codex app-server, 48 downloads, redundant with the official effort.

**Claude Code adapters**:
- **`claude-code-acp-rs`** ([crates.io](https://crates.io/crates/claude-code-acp-rs)) - Rust reimplementation of a Claude Code ACP Agent, 3,878 downloads, but no release since 2026-02-13, predates the 1.0/2.0 protocol wave; verify schema compatibility before use.
- **`claude-code-cli-acp`** ([crates.io](https://crates.io/crates/claude-code-cli-acp)) - a PTY bridge wrapping the real Claude Code CLI, very low adoption (26 downloads).

**Registries and client tooling**:
- **`acpr`** ([crates.io](https://crates.io/crates/acpr), [GitHub](https://github.com/agentclientprotocol/acpr)) - quasi-official (hosted under the `agentclientprotocol` org), runs agents from the public ACP registry.
- **`acp-cli`** ([crates.io](https://crates.io/crates/acp-cli)) - headless CLI client for ACP, useful for scripted smoke tests.
- **`acpx`** ([crates.io](https://crates.io/crates/acpx), [docs.rs](https://docs.rs/acpx/latest/acpx/)) - a lightweight alternative SDK, minimal traction versus the official crate.
- **`acp-agent`** (observerw/acp-agent-rs, [crates.io](https://crates.io/crates/acp-agent)) - pre-release (v0.0.0) registry discovery/launch CLI.
- **`vtcode-acp-client`** ([crates.io](https://crates.io/crates/vtcode-acp-client), part of [vinhnx/vtcode](https://github.com/vinhnx/vtcode)) - moderate downloads (15,890) but tightly coupled to that project and stale (2026-03-21).

**LLM-provider-as-agent adapters** (niche, single-maintainer): `deepseek-acp-adapter` and `acp-llm-adapter` ([euri10](https://github.com/euri10)), `acp-bridge` ([BlakeHung/acp-bridge](https://github.com/BlakeHung/acp-bridge)). These expose a bare LLM provider directly as an ACP agent, bypassing a full coding-agent framework; not directly reusable for a platform (like trogonai) that already hosts its own agents.

**Framework integrations**: `adk-acp` (ACP support for the [zavora-ai/adk-rust](https://github.com/zavora-ai/adk-rust) Agent Development Kit), `agentkit-acp` (ACP for [danielkov/agentkit](https://github.com/danielkov/agentkit)). Relevant only if trogonai adopts the underlying framework, not standalone.

The remaining ~60 reverse-dependency crate names surfaced (`zerostack`, `aether-*`, `bitrouter-*`, `ralph-adapters`, `zeph`/`zeph-acp`, `brokk-mjolnir`/`brokk-anvil`, `sdd-layer`, `grove-rs`, `rhaicp`, `awaken-server`, `ag-agent`, `determinishtic`, `sigit`, `luft-*`, `yolop`, `llm-wiki-engine`, `acompose`, `openheim`, `omniterm`, `wikidesk-server`, `clawbro`, `defect-*`, `vw-acp`/`vw-agent`, `iron-core`, `cyril`/`cyril-core`, `goose-sdk`/`goose-sdk-types`, `darq-core`, `boltz-acpx`, `alice-runtime`, `jamsession`, `telegram-acp`, `heka`, `thumper-cli`, `hagent-acp`, `pi-acpinator`, `thndrs`, `sacp`/`sacp-conductor`/`sacp-proxy`, `elizacp`, `bitrouter-sdk`, `anyclaw-sdk-types`, `embacle`, `koda-cli`, `agentos-sidecar`/`agentos-client`, `a8e-acp`, `aster-core`/`aster-cli`, `roder`/`roder-app-server`, `patchwork-acp`, `stakpak`, `octomind`) are real published crates but were not individually verified for maintenance health in this survey; they represent the long tail of a fast-growing ecosystem rather than platform-grade building blocks. `stakpak` ([crates.io](https://crates.io/crates/stakpak), a DevOps AI agent) and `octomind` ([crates.io](https://crates.io/crates/octomind), a session-based coding assistant) are the two with the most download traction (1,369 and 5,604 respectively) and might be worth a closer look if trogonai wants examples of production ACP agent implementations, but neither was deep-dived here.

## Non-Rust SDKs (ecosystem-health signal only)

- **TypeScript**: `@agentclientprotocol/sdk` on npm, latest `1.3.0` (2026-07-21), tracking the same release cadence as the Rust core. ([npm](https://www.npmjs.com/package/@agentclientprotocol/sdk))
- **Python**: official `python-sdk` repo under `agentclientprotocol` org, 289 GitHub stars, pushed 2026-07-27. A separate community PyPI package literally named `agent-client-protocol` also exists (v0.11.1, self-described as "by Zed Industries" attribution rather than an official publish) - worth distinguishing the official repo from that PyPI listing before depending on either.
- **Kotlin**: `acp-kotlin` is referenced in the spec repo's README as an official SDK, but no repo was found at `agentclientprotocol/acp-kotlin` during this survey (404 on GitHub API); it may live under a different org/name. Flag as unverified.
- **Java**: `java-sdk` repo under `agentclientprotocol` org, 61 stars, pushed 2026-06-11, the least active of the confirmed official SDKs.
- **Elixir**: **ACPex** ([lostbean/acpex](https://github.com/lostbean/acpex), [hex.pm](https://hex.pm/packages/acpex)), a community (not official) idiomatic Elixir implementation using OTP, a `GenServer`-based connection, native Erlang Ports for ndjson I/O, and an `Ecto.Schema`-based type system with camelCase/snake_case conversion. Currently at v0.1.x, a young but well-architected implementation. A second, broader community project, `ex_mcp` ([azmaveth/ex_mcp](https://github.com/azmaveth/ex_mcp)), bundles both MCP and ACP client/server support for Elixir.

Overall signal: the multi-language SDK spread (TS, Python, Kotlin claimed, Java, plus community Elixir) alongside 76+ third-party Rust reverse dependencies indicates a protocol that, by mid-2026, has moved well past a single-editor experiment into a genuine cross-vendor interoperability layer, comparable in ecosystem shape to where MCP was roughly a year into its own adoption curve.

## Recommendations for trogonai

1. **Adopt now**: bump `agent-client-protocol-schema` from `=1.5.0` to `=1.6.0` to match what the upstream `rust-sdk` workspace itself now requires.
2. **Already correctly adopted**: `agent-client-protocol` `=2.0.0`, `-derive`, `-http` `2.0.0` (server feature). Read the 2.0 migration guide against the hand-rolled JSON-RPC-over-NATS binding for the respond-to-route renames and `TransportFrame` channel changes, since those are exactly the low-level surfaces a custom transport binding would touch.
3. **Adopt later, with explicit triggers**:
   - `agent-client-protocol-polyfill`: when bridging ACP peers stuck on older MCP-over-ACP declarations.
   - `agent-client-protocol-rmcp`: when `agents-service` needs rmcp-typed MCP hosting/consumption in Rust.
   - `agent-client-protocol-conductor`: when Rust-native proxy-chain composition becomes cheaper than the current gateway-based approach.
   - `agent-client-protocol-trace-viewer`, `agent-client-protocol-cookbook`, `acpr`, `acp-cli`: developer-experience tools, adopt opportunistically, not architecturally significant.
   - `agentclientprotocol/codex-acp` / `claude-code-acp-rs`: if trogonai wants to host Codex or Claude Code as ACP-backed agents written in Rust rather than shelling out to their native CLIs or official non-Rust adapters.
4. **Skip**: `agent-client-protocol-tokio` (superseded, absorbed into core), and the long tail of narrow single-maintainer community crates (`acpx`, `acp-agent`, `vtcode-acp-client`, `deepseek-acp-adapter`, `acp-llm-adapter`, `cola-io/codex-acp`, `brokk-codex-acp`, `claude-code-cli-acp`) which either duplicate the official SDK's coverage or are too narrowly scoped/stale to be worth a dependency.

## Sources

- [agent-client-protocol on crates.io](https://crates.io/crates/agent-client-protocol)
- [agent-client-protocol-schema on crates.io](https://crates.io/crates/agent-client-protocol-schema)
- [agent-client-protocol-derive on crates.io](https://crates.io/crates/agent-client-protocol-derive)
- [agent-client-protocol-http on crates.io](https://crates.io/crates/agent-client-protocol-http)
- [agent-client-protocol-polyfill on crates.io](https://crates.io/crates/agent-client-protocol-polyfill)
- [agent-client-protocol-rmcp on crates.io](https://crates.io/crates/agent-client-protocol-rmcp)
- [agent-client-protocol-conductor on crates.io](https://crates.io/crates/agent-client-protocol-conductor)
- [agent-client-protocol-tokio on crates.io](https://crates.io/crates/agent-client-protocol-tokio)
- [agent-client-protocol-trace-viewer on crates.io](https://crates.io/crates/agent-client-protocol-trace-viewer)
- [agent-client-protocol-cookbook on crates.io](https://crates.io/crates/agent-client-protocol-cookbook)
- [agentclientprotocol/rust-sdk on GitHub](https://github.com/agentclientprotocol/rust-sdk)
- [agentclientprotocol/agent-client-protocol on GitHub](https://github.com/agentclientprotocol/agent-client-protocol)
- [Rust SDK docs site](https://agentclientprotocol.github.io/rust-sdk/)
- [2.0 migration guide](https://agentclientprotocol.github.io/rust-sdk/migration_v2.0.html)
- [acpr on crates.io](https://crates.io/crates/acpr) / [GitHub](https://github.com/agentclientprotocol/acpr)
- [agentclientprotocol/codex-acp on GitHub](https://github.com/agentclientprotocol/codex-acp)
- [cola-io/codex-acp on GitHub](https://github.com/cola-io/codex-acp)
- [brokk-codex-acp on crates.io](https://crates.io/crates/brokk-codex-acp)
- [claude-code-acp-rs on crates.io](https://crates.io/crates/claude-code-acp-rs)
- [claude-code-cli-acp on crates.io](https://crates.io/crates/claude-code-cli-acp) / [GitHub](https://github.com/moabualruz/claude-code-cli-acp)
- [vtcode-acp-client on crates.io](https://crates.io/crates/vtcode-acp-client) / [vinhnx/vtcode on GitHub](https://github.com/vinhnx/vtcode)
- [acp-cli on crates.io](https://crates.io/crates/acp-cli)
- [acpx on crates.io](https://crates.io/crates/acpx) / [docs.rs](https://docs.rs/acpx/latest/acpx/)
- [acp-agent (observerw/acp-agent-rs) on crates.io](https://crates.io/crates/acp-agent)
- [deepseek-acp-adapter on crates.io](https://crates.io/crates/deepseek-acp-adapter)
- [acp-llm-adapter on crates.io](https://crates.io/crates/acp-llm-adapter)
- [acp-bridge on crates.io](https://crates.io/crates/acp-bridge)
- [adk-acp on crates.io](https://crates.io/crates/adk-acp) / [zavora-ai/adk-rust on GitHub](https://github.com/zavora-ai/adk-rust)
- [agentkit-acp on crates.io](https://crates.io/crates/agentkit-acp)
- [stakpak on crates.io](https://crates.io/crates/stakpak)
- [octomind on crates.io](https://crates.io/crates/octomind)
- [@agentclientprotocol/sdk on npm](https://www.npmjs.com/package/@agentclientprotocol/sdk)
- [ACPex on GitHub](https://github.com/lostbean/acpex) / [hex.pm](https://hex.pm/packages/acpex)
- [ex_mcp on GitHub](https://github.com/azmaveth/ex_mcp)
