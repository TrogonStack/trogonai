Name: Jorge

This week I worked on:

- Streaming responses, backend only: switched the worker from buffering full AI responses to processing chunks as they arrive (Anthropic + other providers), redesigned the proxy-worker message protocol for incremental delivery, adapted the agent loop to consume the stream. Client endpoint unchanged for now.

- CLI as part of the Claude Code replacement initiative, completed several phases: REPL with full ACP session and NATS streaming, slash commands, non-interactive print mode with JSON output for CI/CD, and a JetBrains IDE plugin with inline diffs, chat panel, and editor action.

- OpenRouter runner, full implementation and merged: streams responses from OpenRouter, session management with fork/resume, retry on rate limits and service errors, response size guard, comprehensive test coverage.

- Managed agents, after studying Claude's Managed Agents release, shipped four new capabilities: cross-session memory extraction via LLM after sessions end (Dreaming), session quality evaluation against configurable rubrics (Outcomes), outbound webhooks to notify external systems when an agent finishes a task, and polished the multi-agent orchestration layer.

What changed because of this:

- Responses now stream in real time instead of waiting for the full generation, lower perceived latency across the board. Streaming also removes the timeout risk we had when buffering long responses and is more memory efficient since we no longer hold the full response in memory.
- The terminal CLI and JetBrains plugin now exist and are tested, meaningful progress on replacing Claude Code with our own stack. This unblocks building the tool execution layer on top of it.

- The platform now supports OpenRouter, giving us access to dozens of models behind a single integration and removing single-provider dependency on Anthropic.

- Agents can now remember context across sessions, be evaluated for quality automatically, and notify external systems on task completion without polling. Before this, every session started cold and there was no way to know if an agent did its job well. The core loop of a managed agent is closed end-to-end.

Business impact:

- Streaming is the baseline UX expectation for any AI product, this removes that gap before it becomes visible. It also unblocks the frontend streaming work, which is next.

- Replacing Claude Code with our own stack means owning the full developer experience: no external dependency, full control over the tool, and the ability to ship product-specific behaviors. The CLI and IDE plugin are a solid step forward on that, and the next layer, tool execution, can now be built on top.

- Having OpenRouter ready before any Anthropic pricing or availability issue is real risk reduction, not just cost optionality. It also opens the door to routing to cheaper models per use case.

- Dreaming, Outcomes and outbound webhooks are what turn a one-shot agent into something deployable in a real workflow. Claude's Managed Agents release just happened and building our own version now is time-sensitive. With this in place, agents learn over time, can be held accountable, and integrate with the systems around them without requiring those systems to poll.
