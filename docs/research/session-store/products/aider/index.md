# Aider: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-08-04. Aider is Apache-2.0 licensed
(`LICENSE.txt:1`). Version-sensitive claims were checked against this
authoritative anchor:

- Repo `Aider-AI/aider`, pinned commit `5dc9490bb35f9729ef2c95d00a19ccd30c26339c` (`aider/__init__.py`
  reports `__version__ = "0.86.3.dev"`). All `path:line` citations below are
  repo-root-relative to this tree at this commit.

**Headline finding, stated up front because it governs every section below:**
Aider deliberately has no session store. There is no session id, no
resumable transcript, no schema, and no migration path. What looks like a
transcript (`.aider.chat.history.md`) is written but, outside one narrow
opt-in flag, never read back by the program. The only things Aider makes
durable across restarts are the git history of the workspace and a rebuildable
code-search cache, neither of which is a conversation. This dossier documents
that absence as rigorously as a rich store would be documented, per the
instructions for this product.

## The storage model

There is no durable "session" object anywhere in the source. What exists is
a set of independent, unkeyed files scoped to a working directory:

| Path | Written by | Read back by the program? |
| --- | --- | --- |
| `.aider.chat.history.md` (`aider/args.py:274-276`) | `InputOutput.append_chat_history` (`aider/io.py:1117`) | Only if `--restore-chat-history` is passed, and only once, at `Coder.__init__` (`aider/coders/base_coder.py:519-523`). |
| `.aider.input.history` (`aider/args.py:271-272`) | `InputOutput.add_to_input_history` via `prompt_toolkit`'s `FileHistory` (`aider/io.py:740-745`, wrapping `append_string`) | Yes, every launch, for readline-style up-arrow recall (`InputOutput.get_input_history`, `aider/io.py:747-751`). This is terminal input history, not the conversation. |
| `.aider.llm.history` (opt-in, `aider/args.py:296-299`) | `InputOutput.log_llm_history` (`aider/io.py:755-765`) | No read call found anywhere in the tree. |
| `.aider.tags.cache.v{3,4}/` (`aider/repomap.py:43`) | `RepoMap` via `diskcache.Cache` (`aider/repomap.py:195`, `:220`) | Yes, every launch -- but this is a repo-map (code search) cache, not conversation state. |
| git commits made by aider | `GitRepo` / `Coder.aider_commit_hashes` | Yes, but only the in-process `set()` (`aider/coders/base_coder.py:349`), not anything on disk; see Rewind section. |

None of these is an append-only *event* log with a derived projection. The
chat history file is closest to a "log," but it is a flat Markdown transcript
with no framing, no sequence numbers, and -- critically -- is write-only output
for humans in the overwhelming majority of runs. The repo map cache is a
key-value cache of source-file tag extractions, keyed by file identity, not
by conversation.

**Conceptual model: none of the skeleton's categories fit cleanly.** If forced
to pick the closest label, `.aider.chat.history.md` is
**session-as-append-only-log written for humans**, and workspace durability
lives entirely in git (session-as-side-effect-on-the-repo). There is no
session-as-document, session-as-directory, or session-as-row anywhere in this
codebase.

## Keying and identity

There is no session id, session token, or any identifier minted per run.
Identity is purely **the working directory / git root**, expressed as fixed,
non-parameterized file names:

```python
default_input_history_file = (
    os.path.join(git_root, ".aider.input.history") if git_root else ".aider.input.history"
)
default_chat_history_file = (
    os.path.join(git_root, ".aider.chat.history.md") if git_root else ".aider.chat.history.md"
)
```
(`aider/args.py:271-276`)

Consequences of this scheme:

- **One transcript file per repo, shared by every invocation and every user**
  of that checkout. There is no per-launch, per-user, or per-branch
  partitioning; two people running aider against the same clone append to the
  same `.aider.chat.history.md`, with no author field distinguishing them.
- **No cross-project enumeration and no listing.** There is no command that
  lists "past sessions" -- nothing to list, because nothing is keyed as a
  session. `aider/commands.py` has no `cmd_sessions` or equivalent.
- **No relocation/rename reconciliation**, because there is no id to reconcile.
  If the working directory moves, the default paths simply resolve to a
  different (or absent) file; the old file is orphaned with no linkage.
- The file paths are overridable via `--chat-history-file` /
  `--input-history-file` (`aider/args.py:283-288`), so a user *could* impose
  their own keying discipline by hand, but Aider itself does not.

## The store interface

Aider has no pluggable store adapter or exported storage type. The table
below is a **reconstruction** of the effective operations from call sites;
there is no interface to quote verbatim.

| Operation | Call site | Inputs | Effect / guarantee |
| --- | --- | --- | --- |
| append (chat) | `InputOutput.append_chat_history` (`aider/io.py:1117-1136`) | formatted text | Opens the file in `"a"` mode, writes, closes. No lock, no fsync call, no return value checked by callers. On `PermissionError`/`OSError` it prints a warning and sets `self.chat_history_file = None`, permanently disabling further writes for that process (`aider/io.py:1133-1136`). |
| session-start marker | `InputOutput.__init__` (`aider/io.py:336`) | current timestamp | Appends `\n# aider chat started at {current_time}\n\n` once per process start -- the only structural marker in the file. |
| read-back (opt-in) | `Coder.__init__` (`aider/coders/base_coder.py:519-523`) | `self.io.chat_history_file` | Reads the **entire file** with `io.read_text`, parses it with `utils.split_chat_history_markdown`, and seeds `self.done_messages`. Gated by `restore_chat_history`, default `False` (`aider/args.py:289-294`). |
| clear (in-memory only) | `Commands._clear_chat_history` (`aider/commands.py:435-437`), invoked by `/clear` and `/reset` | none | Empties `self.coder.done_messages` and `self.coder.cur_messages`. **Does not touch the file on disk.** |
| append (input history) | `InputOutput.add_to_input_history` (`aider/io.py:740-745`) | one line of user input | `prompt_toolkit.history.FileHistory(...).append_string(inp)`. |
| read (input history) | `InputOutput.get_input_history` (`aider/io.py:747-751`) | none | `FileHistory(...).load_history_strings()`, used for readline recall, every launch. |
| append (LLM log, opt-in) | `InputOutput.log_llm_history` (`aider/io.py:755-765`) | role, content | Appends a `"{ROLE} {iso-timestamp}\n{content}\n"` block. No read call exists anywhere in the tree. |
| cache read/write (repo map) | `RepoMap.load_tags_cache` / cache miss path (`aider/repomap.py:217-224`, `:186-215`) | file mtime/path | `diskcache.Cache` keyed lookup; falls back to an in-memory `dict()` if the sqlite-backed cache can't be created (`aider/repomap.py:207-215`). This is a code-search cache, not conversation storage. |
| ad-hoc reparse (dev tool only) | `editblock_coder.py:main()` (`aider/coders/editblock_coder.py:630-651`) | a chat-history file path given as `argv[1]` | Standalone `if __name__ == "__main__"` debug entry point that reparses a saved history file to re-extract edit blocks for testing. Not invoked by the normal `aider` CLI path. |

There is no delete, list, summarize, fork, or pagination operation, because
there is no session object those verbs would act on.

## Write and append path

- **Append, one call per UI event**, not one call per turn: `user_input`,
  `ai_output`, `confirm_ask`, `prompt_ask`, and `_tool_message`/`tool_output`
  all call `append_chat_history` independently (`aider/io.py:789`, `:795`,
  `:905`, `:923`, `:960`, `:970`, `:973`, `:999`). A single turn therefore
  produces several separate appends: the user's message, then zero or more
  confirm/prompt Q&A lines (blockquoted), then the assistant's final content.
- **Ordering** is purely file-append order; there is no sequence number,
  UUID, or monotonic id on any line. Two concurrent aider processes writing
  to the same path can interleave their lines with no detection mechanism.
- **Durability/atomicity.** Plain `open(..., "a")` / `write()` / implicit
  close via the `with` block (`aider/io.py:1131`). No temp-file-and-rename,
  no explicit `fsync`, no transaction. Aider only guards against the file
  becoming *unwritable* (`PermissionError`/`OSError`), not against a torn or
  interleaved write.
- **Concurrency model:** effectively "best-effort, unmanaged multi-writer."
  No lock file, no advisory lock, no writer-identity field anywhere in
  `aider/io.py`. This is the opposite of the single-writer-per-session-with-
  fencing pattern seen in richer stores.
- **Delivery semantics:** best-effort, in-process only. A crash between the
  in-memory conversation state and the next `append_chat_history` call loses
  nothing extra (each call already wrote synchronously before returning), but
  there is no idempotence key, and a killed process mid-write can leave a
  partial trailing line with no healing path on the next launch.

## Read and resume path

- **Default behavior: the file is never read.** `restore_chat_history`
  defaults to `False` (`aider/args.py:289-294`), so a fresh `aider` invocation
  starts with empty `done_messages`/`cur_messages` regardless of how large
  `.aider.chat.history.md` has grown.
- **With `--restore-chat-history`:** `Coder.__init__` does one **full,
  eager** read of the whole file via `io.read_text`, then
  `utils.split_chat_history_markdown(history_md)` (`aider/coders/base_coder.py:519-522`),
  then immediately calls `self.summarize_start()` (`:523`) to run the restored
  messages back through `ChatSummary` if they exceed the model's history token
  budget (`aider/history.py`). There is no incremental read, no cursor, no
  offset, and no pagination -- the entire file is parsed on every restore.
- **Parsing is heuristic Markdown line-classification**, not a structured
  format: lines starting `"# "` are dropped (headers), `"> "` becomes a tool
  message, `"#### "` starts a new user message, anything else accumulates as
  assistant content (`aider/utils.py:148-188`, `split_chat_history_markdown`).
  This is a lossy reconstruction: original message boundaries are inferred
  from Markdown prefixes rather than stored as data.
- **What restore is for, in the project's own words:** the FAQ frames it as
  bringing *recent* context into a **new** session, not resuming a specific
  prior one -- "the chat history already includes recent changes made during
  the current session, so this tip is most useful when starting a new aider
  session" (`aider/website/docs/faq.md:142`). There is no concept of resuming
  *a particular* past conversation; `--restore-chat-history` replays whatever
  is currently in the one shared file for that repo.
- Resume does not read a database or an API; it reads the same flat file the
  human-readable log is written to. There is no separate "local cache vs
  durable store" distinction to draw, because there is only the one file.

## Listing, summaries, and search

- **No listing.** There is nothing to enumerate -- one file, one path, no
  index. `grep` across `aider/commands.py` finds no `cmd_sessions`,
  `cmd_history`, or `cmd_list` command.
- **No metadata sidecar.** The chat-started marker
  (`aider/io.py:336`) is the only structural annotation ever written into the
  file, and it is not indexed anywhere.
- **No search subsystem.** No FTS, no vector index, no grep-based search
  helper over the chat history exists in the source. (The FAQ's own
  aspirational note -- "Vector and keyword search against the chat history,
  repo map or codebase may help here," `aider/website/docs/ctags.md:232` -- is
  documentation of an *unimplemented* idea, not a shipped feature; it is
  quoted here because it is direct evidence the maintainers considered and
  did not build this.)
- The closest thing to "listing" is manual and human-driven: the FAQ tells
  users to open `.aider.chat.history.md` themselves and copy content out to
  make a GitHub Gist if they want to share a transcript
  (`aider/website/docs/faq.md:343`).

## Entry/message structure and versioning

There is no typed entry format. The stored unit is a **Markdown text
fragment per UI event**, not a tagged record:

- `user_input`: joins input lines with `"  \n#### "`, wrapped as
  `\n#### {line1}\n#### {line2}...` (`aider/io.py:779-789`). The `"#### "`
  prefix is a Markdown H4 marker used purely as a role tag for the parser.
- `ai_output`: the raw assistant text, stripped and padded with newlines,
  written verbatim with no wrapping (`aider/io.py:793-795`).
- `confirm_ask` / `prompt_ask` / `_tool_message` / `tool_output`: each writes
  a blockquoted line, `"> {text}"`, again with no field structure beyond the
  `>` prefix (`aider/io.py:905`, `:923`, `:960`, `:970`, `:973`, `:999`,
  `:1117-1121`).
- There is **no envelope**: no timestamp per line (only one timestamp at
  process start, `aider/io.py:336`), no message id, no parent/thread
  reference, no role enum beyond the three Markdown-prefix conventions the
  parser infers (`aider/utils.py:148-188`).
- The entry is **not opaque to any store**, because there is no store parsing
  it at write time; the only parser is the optional restore path, and it
  infers structure from prose formatting rather than reading a schema.
- **No format version field exists anywhere in the file.** There is nothing
  resembling `schema_version`. Because the parser is a heuristic Markdown
  splitter rather than a strict format reader, `split_chat_history_markdown`
  will silently accept a hand-edited or foreign Markdown file -- there is no
  version check to reject it, and correspondingly no migration mechanism,
  because there is no format to migrate from or to.

## Compaction and history management

- **In-memory only, and only on the opt-in restore path.** `ChatSummary`
  (`aider/history.py:7-13`) summarizes `done_messages` when they exceed
  `main_model.max_chat_history_tokens`. It is invoked from two places: at
  `Coder.__init__` right after a `--restore-chat-history` load
  (`aider/coders/base_coder.py:523`, `summarize_start()`), and on
  edit-format switches inside `Coder.create` when `summarize_from_coder` is
  true (`aider/coders/base_coder.py:158-166`).
- Summarization **rewrites the in-memory message list**; it never touches
  `.aider.chat.history.md`. The file keeps every line ever written, whether
  or not that content is still part of the model-visible context. This is the
  inverse of a "durable log survives, view shrinks" design: here, the *view*
  (in-memory `done_messages`) shrinks or gets replaced, while the file simply
  keeps growing with no corresponding compaction marker ever written back to
  it.
- **No explicit truncation, rotation, or size cap was found** for
  `.aider.chat.history.md`, `.aider.input.history`, or `.aider.llm.history`.
  All three are open-ended append targets for the lifetime of the repo
  checkout. Whether this unbounded growth is a real user-visible problem
  could not be confirmed from source alone (no size-warning code path, no
  linked issue in this tree) -- noted under Open questions rather than
  asserted as a known bug.

## Rewind, checkpoints, and fork

This is where Aider's actual durability investment shows up, and it is
**workspace/file-state durability, not conversation-session durability**:

- **`/undo` is real, git-based rewind of the workspace** -- not of the chat
  transcript. `Commands.raw_cmd_undo` (`aider/commands.py:560-618`) checks
  that the last commit's hash is in `self.coder.aider_commit_hashes`
  (`:573`), refuses if any changed file is dirty (`:591-595`) or absent from
  the parent tree (`:598-605`), then does the equivalent of `git reset` back
  to the parent commit for exactly those files. This is a **git operation on
  the working tree**, gated by an **in-memory, per-process** set of commit
  hashes aider itself made (`self.aider_commit_hashes = set()`,
  `aider/coders/base_coder.py:349`) -- restart the process and that set is
  empty, so `/undo` explicitly refuses commits from a prior process: "The
  last commit was not made by aider in this chat session"
  (`aider/commands.py:574`). Undo-ability is therefore per-process state
  layered on top of durable git commits, not a durable "checkpoint" record
  itself.
- **No conversation checkpoint, no branch/fork of a transcript exists.** There
  is no operation that snapshots `done_messages`/`cur_messages` to disk at a
  point in time, and no command that forks a conversation into a sibling.
- **The repo map tag cache is the other durable-but-not-conversational
  artifact.** `RepoMap.TAGS_CACHE_DIR = f".aider.tags.cache.v{CACHE_VERSION}"`
  (`aider/repomap.py:43`) is a `diskcache.Cache` (sqlite-backed) directory at
  the repo root, populated by `load_tags_cache`
  (`aider/repomap.py:217-222`) and rebuilt from scratch on any error
  (`tags_cache_error`, `aider/repomap.py:186-215`). It is keyed by source
  file identity (path/mtime), fully rebuildable from the working tree, and
  has nothing to do with session identity -- it exists to avoid re-parsing
  unchanged source files with tree-sitter on every launch.
- **`/save` and `/load` are file-context macros, not conversation
  persistence**, despite the naming. `cmd_save` writes a script of `/drop`,
  `/add`, `/read-only` commands that reconstructs which files are in context
  (`aider/commands.py:1497-1522`); `cmd_load` replays an arbitrary command
  script (`aider/commands.py:1465-1493`). Neither touches
  `done_messages`/`cur_messages` or the chat history file. This is a
  plausible naming collision an auditor could mistake for session save/load,
  so it is called out explicitly here.

## Subagents and nested sessions

Aider has no subagent concept. The nearest analog is **mode switching**
(`/code`, `/ask`, `/architect`) via `Coder.create(from_coder=...)`
(`aider/coders/base_coder.py:125-181`): switching formats copies
`done_messages`, `cur_messages`, `aider_commit_hashes`, and other state
directly between two in-process Python objects that **share the same `io`
instance** (`aider/coders/base_coder.py:146`, `:171-179`), so both "modes"
append to the identical `.aider.chat.history.md`. Architect mode goes one
step further: `ArchitectCoder.reply_completed` builds a fresh `editor_coder`
via `Coder.create(from_coder=self, ...)`, resets its `cur_messages`/
`done_messages` to empty, runs it, then folds its cost and commit hashes back
into the architect coder (`aider/coders/architect_coder.py:9-46`). This is an
**in-memory, same-process, same-file handoff** -- there is no nested session
directory, no parent-child link recorded anywhere durable, and no
child-transcript isolation: everything lands in the one shared chat-history
file, undifferentiated by which mode produced which line.

## Retention, deletion, and multi-host

- **No retention policy, no TTL, no scheduled cleanup** exists for any of the
  `.aider.*` files. Aider's own `check_gitignore` (`aider/main.py:155-171`)
  adds a `.aider*` glob to `.gitignore` so these files are excluded from the
  user's own git history (`aider/main.py:163-164`), which is the only
  "lifecycle" action taken on them -- keeping them out of version control, not
  managing their size or age.
- **No delete verb.** Nothing in `aider/commands.py` removes
  `.aider.chat.history.md`, `.aider.input.history`, or the tags cache
  directory; a user does this by hand with the filesystem.
- **Multi-host / multi-process is an unmanaged shared-filesystem assumption.**
  Every one of these files is a plain path under (or relative to) the git
  root, opened with ordinary POSIX append semantics and no lock file. Two
  processes (two terminals, two machines sharing a mounted checkout) writing
  concurrently is not detected, arbitrated, or fenced anywhere in
  `aider/io.py`. This is a structural non-goal rather than a guarded-against
  failure mode: there is no `authority`/lock file family of the kind seen in
  products that do build a store.

## Interop with foreign session stores

Not applicable. Aider does not import, discover, or resume any other
product's session/transcript format. No such code path exists in this tree.

## What this implies for our Session Store (our inference)

**Our inference:** in Aider, nothing is "a stored session" in the sense our
platform means the term. The product's durability budget went entirely into
the workspace (git commits, an in-memory guard restricting undo to
commits the current process made, and a rebuildable code-search cache), and
explicitly not into the conversation: the one file that looks like a
transcript is fire-and-forget output for humans, read back by the program
only behind an opt-in flag whose own documentation frames it as "seed a new
session with recent context," not "resume this exact prior session." There is
no append-only-log-with-derived-projection design to borrow here, because
there is no projection and, for the default path, no read at all.

The value of this dossier for our design is negative-space confirmation
rather than a pattern to import:

- It shows a mature, widely deployed, single-user-workstation product can
  ship for years with **zero** session-store investment, which is direct
  evidence that a durable, resumable, structured session store is a product
  choice, not a technical necessity for an LLM coding agent to be useful.
- It is a cautionary example on the append path: an unkeyed, unlocked,
  multi-writer-unsafe append target (`.aider.chat.history.md`) is exactly the
  failure mode our event-sourced store's per-session identity and
  single-writer fencing are meant to prevent.
- It reinforces that "workspace state" (git commits, undo-ability, file
  content) and "conversation state" (turns, messages, tool calls) are
  separable durability concerns with different natural stores -- Aider chose
  git for the former and nothing for the latter, which is a data point in
  favor of not conflating the two in our own model.

## Open questions

- Whether unbounded growth of `.aider.chat.history.md` (or `.aider.llm.history`,
  `.aider.input.history`) is a reported real-world pain point for
  long-lived repos could not be confirmed from source in this tree; no
  size-check, size-warning, or related code path was found either way.
- Whether any interleaved-write corruption from concurrent aider processes on
  a shared checkout has ever been observed or reported is not determinable
  from source; the code has no detection for it either way.
- Whether `--restore-chat-history` is commonly used in practice, versus being
  a rarely-set flag (default `False`), is not determinable from this tree;
  only the default and the gated code path are verifiable facts.
- `editblock_coder.py`'s `main()` (`aider/coders/editblock_coder.py:630-651`)
  is a standalone script that reparses a saved chat-history file for
  extracting edit blocks (apparently a debugging/analysis aid). Whether it is
  used in any documented workflow, or is purely a developer utility, was not
  determined.
