# Weekly Update

## This week I worked on

Investigated, implemented, and tested programming tool support for the openrouter-runner. Created a 17-step implementation plan, then implemented it: the SSE parser was extended to accumulate tool call fragments and emit a `ToolCallsReady` event, the prompt loop was wired to execute `trogon-tools` and bash via the wasm NATS backend, `enabled_tools` was added as a per-session config option that persists through fork and survives a runner restart via KV fallback, egress policy was added to block link-local URLs in `fetch_url`, and the commands observer was switched to `DeliverPolicy::New` so stale messages aren't replayed on restart. Tests cover KV round-trip for history and model, fork inheritance of enabled_tools, empty-tools fallback to full tool set on legacy snapshots, and three e2e scenarios against a real NATS server.

Implemented cross-runner session migration. Every runner — acp, xai, openrouter, and codex — now exports and imports conversation history as portable messages. acp-runner drops ToolUse and Thinking blocks on export by design, since code state lives on disk. codex-runner, which has no native history API, accumulates portable messages during each turn and prepends imported history as formatted text on the first user message after a switch. The CrossRunnerSwitcher in `trogon-cli` resolves the target runner from the registry, exports the session, opens a new session on the target, and pipes the JSON directly to import without deserializing — so it stays agnostic to each runner's internal message format. Bridges are cached per prefix so repeated switches reuse existing connections. A `RunnerSwitcher` trait was added to keep the REPL loop testable without a real NATS server. Tests cover all error paths, happy path, bridge reuse, import payload correctness, and e2e scenarios over a real NATS server for all four runners. The `/model` command in the CLI is now fully wired: cross-runner switches migrate the session transparently; same-runner model changes call `set_session_model` directly.

Token tracking across all three runners. xai-runner and openrouter-runner now accumulate input, output, and cache tokens across every tool round within a prompt, persist the totals to NATS KV at prompt end, reset to zero on fork, and expose them as camelCase fields in `list_sessions` session_meta. acp-runner got the same exposure in list_sessions and the fix that makes it persist on the two paths that were missing: `max_iterations` and `max_tokens` exits. Full test coverage across all four runners — completion, cancel, fork, multi-turn accumulation, and cache_read paths.

The programming smoke test suite went from 23 tests and 83 checks to 50 tests and 173 checks. The new tests cover every feature from the programming plan that lacked NATS end-to-end coverage: str_replace, search_files, todo_write/read, the `--print` CLI flag, auto-compaction via token_budget threshold, bash tool via the wasm-runtime NATS protocol, xai and openrouter and codex registry startup, `session/get_state` on multiple runners, and the full cross-runner model switch path — model lookup in AGENT_REGISTRY, session export, session import, all the way through a live switch from the CLI.

Fixed several runner issues blocking the programming plan: tool execution two-turn round-trip cycle wired into acp, xai, and openrouter runners; the CLI `set_model` response subject corrected; codex-runner switched to the JetStream session command protocol; and `PortableBlock::ToolCall` and `ToolResult` variants added so cross-runner session history carries tool state correctly.

## What changed because of this

The openrouter-runner is now a full programming runner — tool execution, bash, egress policy, session persistence across restarts, and enabled_tools filtering are all working and tested. Two non-Claude runners now have parity with acp-runner for coding tasks.

Cross-runner session migration is fully implemented end-to-end. A user can start a session on Claude, switch to Grok or any OpenRouter model mid-conversation, and the history arrives on the new runner intact. The switch is transparent from the CLI.

Per-session token accounting is now complete and consistent across every runner. Any session — whether it ran on Claude, Grok, or OpenRouter — reports the same token fields in the same place, with no silent gaps on error or cancel exits.

The programming test suite now validates the entire feature set end-to-end over real NATS, going from 23 to 50 tests in one week. Every feature built over the past weeks has continuous verification.

The runner fixes unblocked the full programming workflow. Before this week, a session that produced tool calls would stall — tool results were not making it back into the next turn. That is now resolved across all three runners, and the codex-runner can receive session commands over JetStream as intended.

## Business impact

The openrouter-runner reaching programming parity matters because OpenRouter is the access point for every model that isn't Claude or Grok — Gemini, Mistral, Llama, DeepSeek, and others. The programming assistant is now genuinely multi-model.

Mid-session model switching is the core differentiator of the product. A user can start on Claude, hit a cost ceiling or want a different model's strengths, switch to Grok or any OpenRouter model, and keep working without losing conversation context. That is now a real, tested, wired feature — not just a design.

Token tracking is the prerequisite for cost visibility and per-customer billing. Every session on every runner now generates accurate data, including on the paths that were silently dropping it before.

The programming test suite reaching 50 tests and 173 checks means the core product offering is continuously verified end-to-end. That coverage is what lets the team ship incrementally without regressions.

The runner fixes are what turn the programming plan from a design into a working system. Without tool execution completing the round-trip correctly, none of the coding workflows — reading files, applying edits, running git commands — would actually function. Fixing these issues across all runners in one pass means the platform is now in a state where it can be demoed and iterated on end-to-end.
