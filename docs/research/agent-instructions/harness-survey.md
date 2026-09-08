# Harness survey: standing instruction inputs

Part of the agent instructions research corpus.
Evidence for the original survey was retrieved 2026-07-30 from official
documentation and product source code. The DeepSeek Harness addition was
retrieved 2026-08-20 from release `dsh-v0.1.0-rc.8` at pinned commit
`141eb6fef83422698aef7a981029e843e8161534`. Documentation is mutable;
anchors name the pages and source paths checked. The question asked of every
product: what can an operator pass in as standing agent instructions, and what
is its exact shape?

## Summary table

| Harness | Primary inputs | Content payload |
|---|---|---|
| Claude Agent SDK | `systemPrompt` union | string or string list |
| Claude Code CLI | CLAUDE.md family, `.claude/rules/*.md`, flags | markdown |
| Anthropic Messages API | `system` | string or text blocks |
| Codex CLI / cloud | AGENTS.md, `config.toml` keys | markdown / string |
| DeepSeek Harness | `$DSH_HOME/AGENTS.md`, project candidates and local overlays | markdown |
| OpenAI Agents SDK | `instructions` | string or callable |
| Cursor | `.cursor/rules/*.mdc`, User/Team Rules, AGENTS.md | markdown |
| Grok Build (xAI) | AGENTS.md, CLAUDE.md, `.grok/rules/*.md` | markdown |
| Gemini CLI | GEMINI.md hierarchy, `system.md` override | markdown |
| OpenCode | AGENTS.md, `instructions` path array | markdown |
| Amp | AGENTS.md | markdown |
| Cline | `.clinerules` file or directory | markdown |
| Goose | `.goosehints`, AGENTS.md | plain text |
| Devin | Knowledge snippets, Playbooks, rules files | text |
| Aider | `--read` / `read:` list (opt-in) | text |

Every payload in the table is unstructured text, almost always markdown.
No surveyed harness accepts role-tagged chat messages as standing agent
instructions.

## Claude (Anthropic)

- Agent SDK `systemPrompt` is a tagged union. TypeScript, per the
  exported `Options` typing in `@anthropic-ai/claude-agent-sdk` 0.3.220
  (the published reference page documents only the string and preset
  forms, lagging the typings):
  `string | string[] | { type: 'preset', preset: 'claude_code',
  append?: string, excludeDynamicSections?: boolean }`. The typing's own
  example uses the list form to thread a dynamic cache boundary between
  prompt segments. Python adds a further variant,
  `{ type: 'file', path: str }`, used to avoid OS argument-length
  limits.
  Omitted means a minimal default, unlike the CLI which uses the full
  preset. Anchors:
  [TypeScript SDK reference](https://code.claude.com/docs/en/agent-sdk/typescript),
  [modifying system prompts](https://code.claude.com/docs/en/agent-sdk/modifying-system-prompts),
  [Python SDK reference](https://code.claude.com/docs/en/agent-sdk/python).
- CLAUDE.md family: managed policy path, then `~/.claude/CLAUDE.md`, then
  project `CLAUDE.md` or `.claude/CLAUDE.md`, then `CLAUDE.local.md`. All
  files concatenate; nothing overrides. `@path` imports recurse to a
  maximum depth of four hops. Anchor:
  [memory documentation](https://code.claude.com/docs/en/memory).
- `.claude/rules/*.md`: a directory of rule files with optional `paths:`
  glob frontmatter for conditional load; no frontmatter means
  unconditional, same priority as CLAUDE.md.
- Claude Code does not read AGENTS.md natively. The documentation states
  "Claude Code reads `CLAUDE.md`, not `AGENTS.md`"; the workaround is an
  `@AGENTS.md` import or a symlink. Feature requests have been open since
  August 2025 (anthropics/claude-code issue 6235 and duplicates).
- Full replacement and append flags: `--system-prompt`,
  `--system-prompt-file`, `--append-system-prompt`,
  `--append-system-prompt-file`; replace and append are mutually
  exclusive. Anchor:
  [CLI reference](https://code.claude.com/docs/en/cli-reference).
- Subagent definitions (`.claude/agents/*.md` or the programmatic
  `agents` option) are YAML frontmatter metadata plus a single markdown
  body that becomes the subagent's system prompt. Anchor:
  [subagents](https://code.claude.com/docs/en/sub-agents).
- Messages API `system` parameter: `string | TextBlockParam[]`, each
  block independently cacheable via `cache_control`. Anchors:
  [Messages API](https://platform.claude.com/docs/en/api/messages),
  [prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching).

## Codex (OpenAI)

Source-verified against `openai/codex` (`codex-rs/core/src/agents_md.rs`,
`codex-rs/config/src/config_toml.rs`).

- AGENTS.md discovery: global `~/.codex/AGENTS.override.md` then
  `~/.codex/AGENTS.md` (first non-empty wins), then one file per
  directory from the project root down to the cwd
  (`AGENTS.override.md`, `AGENTS.md`, then configured fallbacks).
  Composition is concatenation, not override: global text, a literal
  `--- project-doc ---` separator, then project files root to cwd. The
  project side is budgeted by `project_doc_max_bytes`, default 32 KiB,
  truncated on overflow. Anchor:
  [AGENTS.md guide](https://developers.openai.com/codex/guides/agents-md).
- `config.toml`: `instructions` (string, fallback replacement for the
  base prompt), `model_instructions_file` (a path whose contents fully
  replace the compiled-in base prompt; the source comment says users are
  "STRONGLY DISCOURAGED" from using it), `developer_instructions`
  (separate developer-role message), and `include_*` booleans that
  toggle individual built-in prompt blocks.
- Codex cloud adds auto-generated Memories under `~/.codex/memories`,
  described as generated state, not user-authored instructions. Anchor:
  [customization overview](https://learn.chatgpt.com/docs/customization/overview).
- Agents SDK `Agent.instructions`:
  `str | Callable[[RunContextWrapper, Agent], MaybeAwaitable[str]]`.
  Anchor:
  [agent reference](https://openai.github.io/openai-agents-python/ref/agent/).
- Retreat from structured prompt objects: the dashboard-managed Prompt
  object (`{prompt_id, version, variables}`) is deprecated with
  `v1/prompts` scheduled to shut down 2026-11-30, and the Assistants API
  (whose `instructions` is a string capped at 256,000 characters) shuts
  down 2026-08-26. Official guidance is to move prompt text into
  versioned application code. Anchor:
  [prompt object migration](https://developers.openai.com/api/docs/guides/prompting/migrate-from-prompt-object).

## DeepSeek Harness

Source-verified against DeepSeek Harness `dsh-v0.1.0-rc.8`, commit
`141eb6fef83422698aef7a981029e843e8161534`.

- First-party support is implemented by
  `@deepseek-ai/dsh-agent-instructions`. The default baseline loads the fixed
  user-global `$DSH_HOME/AGENTS.md`, then every existing project candidate from
  the discovered project root through the session cwd. The defaults are
  `AGENTS.md` and `CLAUDE.md`, followed in each directory by
  `AGENTS.local.md` and `CLAUDE.local.md`; the project root marker defaults to
  `.git`
  ([packages/context/agent-instructions/src/config.ts:11-14](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/context/agent-instructions/src/config.ts#L11-L14),
  [packages/context/agent-instructions/src/config.ts:39-46](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/context/agent-instructions/src/config.ts#L39-L46),
  [packages/context/agent-instructions/src/files.ts:267-308](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/context/agent-instructions/src/files.ts#L267-L308)).
- Scope and precedence are additive, broad to specific. User-global content is
  first, then root-to-cwd directories; base candidates precede local overlays
  in each directory. Distinct files all remain visible, while same-directory
  files whose whitespace-trimmed content is identical collapse to the earliest
  configured candidate. The rendered reminder tells the model that more
  specific instructions take precedence and that workspace files do not
  override system, developer, or direct user instructions
  ([packages/context/agent-instructions/README.md:9-13](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/context/agent-instructions/README.md#L9-L13),
  [packages/context/agent-instructions/src/render.ts:10-18](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/context/agent-instructions/src/render.ts#L10-L18),
  [packages/context/agent-instructions/src/render.ts:85-98](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/context/agent-instructions/src/render.ts#L85-L98)).
- Binding is per session and durable. The complete baseline joins the first
  eligible request as a user-role message. Nested scopes below the session cwd
  bind only after a successful first-party `read`, `write`, or `edit` reaches
  them. Later edits and removals append replacement or removal messages; there
  is no filesystem watcher, and shell `cd` does not trigger discovery
  ([packages/context/agent-instructions/README.md:7-17](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/context/agent-instructions/README.md#L7-L17),
  [packages/context/agent-instructions/README.md:49-55](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/context/agent-instructions/README.md#L49-L55)).
- Resume retains a compatible visible baseline. A change to discovery,
  precedence, project root, or budget identity appends one complete superseding
  baseline. `maxBytes` is required, each source defaults to a 1 MiB cap, and
  rendering preserves the most specific files first by dropping broader files
  before truncating the most-specific file
  ([packages/context/agent-instructions/README.md:49-78](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/context/agent-instructions/README.md#L49-L78)).

## Cursor

- Project Rules: `.cursor/rules/*.mdc`, a list of discrete rule files.
  Frontmatter is exactly three fields: `description` (string), `globs`
  (comma-separated patterns), `alwaysApply` (boolean). The combination
  yields four activation modes: always, auto-attached by glob,
  agent-requested via description, and manual via `@rule-name`. Rules
  can embed `@filename` references. Anchor:
  [Cursor rules](https://cursor.com/docs/rules).
- User Rules: one plain-text blob in settings, global, no frontmatter.
- Team Rules (Team/Enterprise): dashboard-managed text with globs and an
  enforcement toggle. Documented precedence: Team, then Project, then
  User.
- AGENTS.md: supported at the root and nested; more specific files take
  precedence. Community reports that background agents do not always
  load AGENTS.md are unverified against the documentation.
- Memories: auto-generated snippets proposed by a sidecar model and
  approved by the user; local, not team-shared unless promoted into a
  rule file.

## GitHub Copilot

- `.github/copilot-instructions.md`: one repo-wide markdown blob.
- `.github/instructions/*.instructions.md`: a list of files with
  `applyTo` glob frontmatter plus optional `description` and
  `excludeAgent`; all matching layers merge additively. Anchor:
  [repository instructions](https://docs.github.com/en/copilot/how-tos/configure-custom-instructions-in-your-ide/add-repository-instructions-in-your-ide).
- AGENTS.md: supported by the coding agent (multiple files, nearest in
  tree wins) and by VS Code behind `chat.useAgentsMdFile`.
- Web-level custom instructions exist at personal, organization, and
  repository scope; all relevant sets are provided together. Anchor:
  [response customization](https://docs.github.com/en/copilot/concepts/response-customization).

## Grok Build (xAI)

- xAI's CLI harness reads AGENTS.md, AGENT.md, CLAUDE.md and variants,
  plus every markdown file under `.grok/rules/`, and also reads
  `.claude/rules/` and `.cursor/rules/` for compatibility. There is no
  GROK.md. Precedence is global `~/.grok/` first, then each directory
  from root to cwd, deeper wins. Anchor:
  [project rules](https://docs.x.ai/build/features/project-rules).
- The xAI API system prompt is a plain string in both Chat Completions
  and Responses shapes; no content-block list was found.
- Launch dates (beta around May 2026, open-sourced 2026-07-15 at
  `xai-org/grok-build`) come from secondary sources and are unverified
  against xAI's own announcements.
- Significance for this corpus: the newest major entrant invented no
  format of its own and consumes competitors' markdown files directly.

## Gemini CLI

- GEMINI.md hierarchy: global `~/.gemini/GEMINI.md`, project root and
  ancestors, then subdirectories, all tiers concatenated. The context
  filename setting `context.fileName` accepts `string | string[]`
  (verified in `memoryTool.ts`), so AGENTS.md can be opted into, but it
  is not a shipped default.
- `GEMINI_SYSTEM_MD` fully replaces the built-in system prompt from a
  file, with template variables such as `${AgentSkills}` and
  `${AvailableTools}`. Anchor:
  [google-gemini/gemini-cli](https://github.com/google-gemini/gemini-cli).
- Google announced the CLI is folding into a successor product
  (Antigravity); the relation of that move to AGENTS.md adoption is
  unverified.

## OpenCode

- AGENTS.md: nearest file walking up from cwd (falls back to CLAUDE.md),
  plus a global copy; both apply together.
- `opencode.json` `instructions`: an array of paths, globs, or remote
  URLs, combined additively with AGENTS.md, for example
  `["CONTRIBUTING.md", "docs/guidelines.md", ".cursor/rules/*.md"]`.
- Agent definitions: markdown files whose frontmatter carries metadata
  (description, mode, model, permissions) and whose body is the prompt.
  Anchor: [opencode.ai](https://opencode.ai) and
  `anomalyco/opencode`.

## Amp (Sourcegraph)

- AGENTS.md (renamed from AGENT.md in 2025): loaded from the workspace
  root and all parents up to `$HOME`, plus global config copies; subtree
  files load on demand. Plain markdown, no frontmatter. Anchor:
  [Amp manual](https://ampcode.com/manual).

## Cline and Roo Code

- Cline `.clinerules`: a single file or a directory of markdown files
  combined in numeric filename order, with optional `paths:` glob
  frontmatter; global rules combine with workspace rules and the
  workspace wins conflicts. Anchor:
  [Cline rules](https://docs.cline.bot/features/cline-rules).
- Roo Code shut down 2026-05-15 (repository archived); its `.roo/rules/`
  directories and custom modes (`roleDefinition`, `customInstructions`
  fields) survive in community forks.

## Goose (Linux Foundation)

- `.goosehints`: global (`~/.config/goose/.goosehints`) plus per-project
  and per-directory files, all applied together, local wins conflicts;
  `@filename` injects another file. AGENTS.md is checked alongside
  `.goosehints` at each level, and the filename list is configurable.
  Anchor:
  [using goosehints](https://goose-docs.ai/docs/guides/context-engineering/using-goosehints).
- Block donated Goose to the Linux Foundation's Agentic AI Foundation
  (announced 2025-12-09, alongside MCP and AGENTS.md).

## Devin (Cognition), including Windsurf

- Knowledge: discrete managed snippets, each a trigger description plus
  content plus an optional macro, created through the UI or REST API,
  never a committed repo file; scoped org-wide, per-user, or pinned per
  repo. Anchor:
  [Knowledge](https://docs.devin.ai/product-guides/knowledge).
- Playbooks: named multi-step procedures with fixed sections, explicitly
  invoked, unlike Knowledge's automatic trigger-based retrieval.
- Windsurf was rebranded Devin Desktop on 2026-06-02 and its Cascade
  agent reached end of life 2026-07-01. Its rules format persists:
  `global_rules.md` capped at 6,000 characters, `.windsurf/rules/*.md`
  capped at 12,000 characters each, with `trigger` frontmatter taking
  `always_on`, `manual`, `model_decision`, or `glob`. Multiple context
  filenames (AGENTS.md, AGENTS.local.md, AGENT.md, `.windsurfrules`,
  CLAUDE.md) are all treated as always-on simultaneously.

## Aider

- All standing-file loading is opt-in: `--read FILE` is repeatable, and
  the `read:` config key takes a single path or a YAML list. Claims that
  Aider auto-loads AGENTS.md were not corroborated by its changelog.
  Anchor:
  [conventions](https://aider.chat/docs/usage/conventions.html).

## The AGENTS.md standard

- The spec is deliberately schemaless: standard markdown, any headings,
  no required structure. Nested files are described as nearest-wins,
  though Codex implements concatenation; a v1.1 draft proposing explicit
  jurisdiction and accumulation rules is open but unmerged. Anchor:
  [agents.md](https://agents.md).
- Governance moved to the Agentic AI Foundation under the Linux
  Foundation (announced 2025-12-09; platinum members include AWS,
  Anthropic, Block, Google, Microsoft, and OpenAI). Adopters as of
  mid-2026 include OpenAI Codex, GitHub Copilot, Cursor, Google Jules,
  Factory, Aider, Goose, OpenCode, Zed, Warp, VS Code, Cognition,
  JetBrains Junie, Amp, and Roo Code successors. Anchor:
  [Linux Foundation press release](https://www.linuxfoundation.org/press/linux-foundation-announces-the-formation-of-the-agentic-ai-foundation).

## The five shapes

Every surveyed input surface reduces to one of five forms:

1. Schemaless markdown documents concatenated by scope (AGENTS.md,
   CLAUDE.md, GEMINI.md, `.goosehints`). The dominant, standardized
   form.
2. Lists of rule objects whose metadata governs activation, never
   content structure: globs, triggers, always-on flags, descriptions for
   model-decided relevance (Cursor `.mdc`, Devin rules, Copilot
   `.instructions.md`, Claude `.claude/rules`, Cline). The payload
   inside each object is still a markdown string.
3. Small tagged unions at SDK and API boundaries: string, string list,
   preset plus append, or file (Claude), string or callable (OpenAI),
   string or text blocks (Anthropic API).
4. Platform-managed snippet stores with semantic triggers, generated
   rather than authored, held outside the agent definition (Devin
   Knowledge, Cursor and Windsurf and Codex Memories).
5. Full system-prompt replacement escape hatches, always a single string
   or file, always discouraged (Codex `model_instructions_file`, Gemini
   `GEMINI_SYSTEM_MD`, Claude `--system-prompt`).

Absent everywhere: role-tagged chat message lists as standing agent
instructions, and any schema imposed on the instruction content itself.

## Strategic signals

- Content converged; mechanics did not. Instruction text is portable
  markdown across every product, while injection (append versus replace,
  budgets, ordering, activation) is per-harness vocabulary. AGENTS.md
  exists precisely because the content half is portable.
- OpenAI is retreating from structured server-side prompt objects back
  to strings in versioned code (Prompt object and Assistants API
  shutdowns above).
- Hard size budgets exist in the wild: 32 KiB (Codex project docs),
  6,000 and 12,000 characters (Devin rules), 256,000 characters
  (Assistants `instructions` cap).
- The product churn inside twelve months (Windsurf to Devin Desktop, Roo
  Code shut down, Gemini CLI folding into a successor, Assistants API
  dying) argues against mirroring any single harness's structure in a
  platform contract.
