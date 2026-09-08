# SWE-agent: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-08-04. Source: `SWE-agent/SWE-agent`, pinned at commit `3ea751c087f32b16e039a2233dd6eefecef325d5`
(`fix: map multimodal subset to sb-cli's swe-bench-m (#1458)`, 2026-07-16),
MIT license. All paths below are relative to that repository root, not to
this platform's REPO.

- Primary anchor: `sweagent/agent/agents.py` (trajectory writer, `DefaultAgent`,
  `RetryAgent`).
- Secondary anchors: `sweagent/types.py` (wire types), `sweagent/run/run_batch.py`,
  `sweagent/run/run_replay.py`, `sweagent/run/run_single.py`,
  `sweagent/run/common.py`, `sweagent/agent/history_processors.py`,
  `sweagent/agent/reviewer.py`, `sweagent/inspector/server.py`,
  `docs/usage/trajectories.md`, `docs/usage/inspector.md`.

## Framing: is a trajectory a session store, or an output artifact?

**It is an output artifact that happens to contain a full transcript, not a
session store in the resume-a-conversation-tomorrow sense.** The docs say so
themselves: "Trajectories are the main output of SWE-agent. They are the best
way to understand what SWE-agent does" (`docs/usage/inspector.md:4`), and the
`.traj` file is introduced as "the main output file" of a run
(`docs/usage/trajectories.md:7`), not as a session record.

The four decisive tests, each answered from the code:

1. **Is a `.traj` file ever read back by the program to continue a run?** No.
   The only two readers of a `.traj` file's `history`/`trajectory` fields at
   runtime are `sweagent/run/run_replay.py` and
   `sweagent/run/run_traj_to_demo.py`, and neither continues the run that
   produced the file: `run_replay.py` starts a **brand-new** `SWEEnv` and a
   **brand-new** `DefaultAgent` (`sweagent/run/run_replay.py:186-192`,
   `_get_env`/`_get_agent`/`_get_run_single`) and re-executes the stored
   assistant actions through a `ReplayModel` that just plays back the
   `history` list instead of querying a real LLM
   (`sweagent/agent/models.py:464-481`). `run_traj_to_demo.py` extracts a
   filtered `history` into a YAML demo file for a human to hand-edit
   (`sweagent/run/run_traj_to_demo.py:39-58`). Neither reconstructs `self.history`,
   `self.trajectory`, `self.info`, or any other live agent attribute; they
   consume the JSON as data, not as agent state.
2. **Is there a resume-by-id path?** No. What exists instead is
   `RunBatch.should_skip` (`sweagent/run/run_batch.py:376-409`): if
   `<output_dir>/<instance_id>/<instance_id>.traj` already exists and its
   `info.exit_status` is a completed status, the instance is skipped entirely
   (never re-run, never continued). If the file is empty, unparsable, or has
   `exit_status in (None, "early_exit")`, the code calls
   `log_path.unlink()` (`sweagent/run/run_batch.py:391,400,405`) and the
   instance is **re-run from scratch** -- a fresh `output_dir.mkdir`, fresh
   `agent.setup()`, fresh environment, fresh `.traj`, overwriting the old
   path. This is "already done, skip" logic, explicitly not resume: nothing in
   this path loads `history` back into an agent, and an incomplete run is
   deleted, not continued.
3. **Append-as-you-go or written once at the end?** Append-as-you-go, but by
   whole-file rewrite, not by appending bytes. `DefaultAgent.run`'s main loop
   is `while not step_output.done: step_output = self.step(); self.save_trajectory()`
   (`sweagent/agent/agents.py:1284-1286`), so the trajectory file is rewritten
   to disk after **every** step, not just at the end. `save_trajectory`
   (`sweagent/agent/agents.py:779-787`) calls
   `self.traj_path.write_text(json.dumps(data, indent=2))` -- a single
   `write_text` call that serializes the entire accumulated `history` +
   `trajectory` + `info` from scratch each time. So a crash between steps
   loses at most the in-flight step, not the run so far; a crash **during**
   that `write_text` call can leave a torn/partial JSON file, because there is
   no temp-file-and-rename and no fsync anywhere in this path (none found in
   `sweagent/agent/agents.py` or `sweagent/utils/`).
4. **Is there per-run replay tooling, and does it reconstruct agent state or
   only display it?** Both kinds exist, and neither reconstructs agent state.
   The **inspector** (`sweagent/inspector/server.py`,
   `sweagent/inspector/static.py`) is read-only: it globs `**/*.traj`
   (`sweagent/inspector/server.py:274`, `sweagent/inspector/static.py:158`),
   loads each file with `json.load` (`sweagent/inspector/server.py:170`), and
   serves it to a browser-side viewer (`fileViewer.js`) or bakes it into a
   static HTML page (`sweagent/inspector/static.py:96-124`) -- display only,
   confirmed by `docs/usage/inspector.md:1-8`. **`run-replay`**
   (`sweagent/run/run_replay.py`) does execute actions again, but through a
   fresh environment and a fresh agent, as described in point 1: it
   reconstructs environment **output**, not agent **state**.

Taken together: a `.traj` file is scored by evaluation harnesses (SWE-bench's
`preds.json`), displayed by humans (inspector), and occasionally re-executed
for a different purpose (demo creation, debugging), but the running program
that produced it never reads it back to pick up where it left off. That is
the operational definition of an output artifact, not a session store.

## The storage model

The durable record for one instance-run is a single JSON file,
`<instance_id>.traj`, produced by
`DefaultAgent.get_trajectory_data`/`save_trajectory`
(`sweagent/agent/agents.py:762-787`). Its top-level shape, built at
`sweagent/agent/agents.py:768-777`:

```python
attempt_data = {
    "trajectory": self.trajectory,   # list[TrajectoryStep] -- post-hoc, per-step summary
    "history": self.history,         # list[HistoryItem] -- the literal LM conversation
    "info": self.info,               # AgentInfo -- exit status, submission, cost stats
}
attempt_data["replay_config"] = self.replay_config.model_dump_json() if ... else None
attempt_data["environment"] = self._env.name
```

There is no separate index, no sidecar summary file, no cache distinct from
the trajectory itself, and no derived/authoritative split: the one file is
computed fresh from in-memory Python objects (`self.trajectory`,
`self.history`, `self.info`) every time it is written, and nothing is ever
read back from it to reconstruct those objects (see Framing, above). It is
closest to **session-as-document**: a single mutable JSON document,
rewritten wholesale on every step, not an append-only log and not a
directory of separate records.

`RetryAgent` (the multi-attempt driver, see Subagents section) wraps this in
one more layer: its own `.traj` file is `{"attempts": [<attempt_data>, ...]}`,
optionally overlaid with a full copy of the chosen attempt's fields at the
top level (`sweagent/agent/agents.py:358-388`).

## Keying and identity

- **Instance id** (`ProblemStatement.id`) is the primary key of a run, and it
  is content-derived, not randomly minted, for the built-in problem statement
  types:
  - `TextProblemStatement.id` and `SWEBenchMultimodalProblemStatement.id`
    default to `hashlib.sha256(text).hexdigest()[:6]`
    (`sweagent/agent/problem_statement.py:84-86,183-185`).
  - `FileProblemStatement.id` is the sha256[:6] of the loaded file content
    (`sweagent/agent/problem_statement.py:117-119`).
  - `GithubIssue.id` is `f"{owner}__{repo}-i{issue_number}"`
    (`sweagent/agent/problem_statement.py:144-147`).
  - `EmptyProblemStatement.id` defaults to `str(uuid.uuid4())`
    (`sweagent/agent/problem_statement.py:58`) -- the one case that is
    randomly minted, used when there is no real problem statement (e.g. shell
    mode).
  - SWE-bench-sourced instances use the dataset's own `instance_id` string
    verbatim (`sweagent/run/batch_instances.py:97,166-167,408,429`).
- **Output path is the true identity for collision purposes.** The instance
  id becomes both a subdirectory name and the `.traj` file stem:
  `traj_path = output_dir / (self._problem_statement.id + ".traj")`
  (`sweagent/agent/agents.py:589`, and identically for `RetryAgent` at
  `sweagent/agent/agents.py:298`). `output_dir` for `run-batch` is
  `TRAJECTORY_DIR / user_id / f"{config_file}__{model_id}___{source_id}{suffix}"`
  (`sweagent/run/run_batch.py:103-117`) and for `run` (single) is
  `Path.cwd() / "trajectories" / user_id / f"{config_file}__{model_id}___{problem_id}"`
  (`sweagent/run/run_single.py:68-80`). `TRAJECTORY_DIR` itself defaults to
  `<package_root>/../trajectories`, overridable by the
  `SWE_AGENT_TRAJECTORY_DIR` env var (`sweagent/__init__.py:46-47`).
- **Two runs of the same instance under the same experiment directory do not
  coexist; they collide at the same path.** Same user, same config file
  name, same model id, same instance id -> same `output_dir` ->
  same `<instance_id>.traj` path. `RunBatch.should_skip`
  (`sweagent/run/run_batch.py:376-409`) is the only guard against this, and it
  guards by **skip-or-delete-and-redo**, not by identity disambiguation: a
  second run either does nothing (existing run looked complete) or unlinks
  the old file and overwrites it in place. The only way to get a
  non-colliding second copy is to change `output_dir`, `suffix`, the config
  file name, or the model id (all of which feed the path formula above), or
  to pass `--redo_existing` (`sweagent/run/run_batch.py:83-84`), which
  explicitly does not protect against the collision either -- it just accepts
  it.
- **Listing is directory-scoped, not indexed.** There is no session/run index
  file anywhere in the codebase; every consumer (`should_skip`, the
  inspector, `merge_predictions`) enumerates trajectories with a filesystem
  glob: `directory.glob("*.traj")` (`sweagent/run/remove_unfinished.py:20`),
  `directory.rglob("*.pred")` (`sweagent/run/merge_predictions.py:22`),
  `Path(self.traj_dir).glob("**/*.traj")`
  (`sweagent/inspector/server.py:274`, `sweagent/inspector/static.py:158`).
- **Relocation/rename:** there is no concept of a moved or renamed session.
  The identity is the file path itself; move the file and, from the program's
  point of view, that trajectory no longer exists at its old identity (there
  is no separate id field inside the JSON that a mover would need to keep in
  sync -- `info` and `trajectory`/`history` carry no `instance_id` field of
  their own; the id lives only in the filename and the parent problem
  statement object in memory).

## The store interface

There is no pluggable store adapter, no interface class, and no protocol for
the trajectory store. The interface below is **reconstructed** from the call
sites; every operation is a plain method on `DefaultAgent` or a module-level
function, and every "store" is just the local filesystem.

| Operation | Signature / call site | Effect and guarantee |
| --- | --- | --- |
| set traj path | `self.traj_path = output_dir / (id + ".traj")` -- `sweagent/agent/agents.py:589` (`DefaultAgent.setup`), `:298` (`RetryAgent.setup`) | Pure path computation; no I/O. Called once per instance/attempt setup. |
| write (full rewrite) | `DefaultAgent.save_trajectory()` -- `sweagent/agent/agents.py:779-787` | `traj_path.write_text(json.dumps(get_trajectory_data(), indent=2))`. Whole-file overwrite, called after every step (`agents.py:1286`) and again at run end. No lock, no temp file, no fsync. |
| write (retry rollup) | `RetryAgent.save_trajectory(choose)` -- `sweagent/agent/agents.py:385-388` | Same whole-file-overwrite mechanics, over the `{"attempts": [...]}` shape. |
| append in-memory step | `DefaultAgent.add_step_to_trajectory(step)` -- `sweagent/agent/agents.py:1220-1233` | Appends one `TrajectoryStep` dict to `self.trajectory` (an in-process Python list). Not itself durable; durability only happens on the next `save_trajectory()` call. |
| append in-memory history | `DefaultAgent._append_history(item)` -- `sweagent/agent/agents.py:556-559` | Appends one `HistoryItem` to `self.history`. Same non-durability caveat. |
| read for replay | `RunReplay.__init__` -- `sweagent/run/run_replay.py:85-88` | `json.loads(traj_path.read_text())` (or `yaml.safe_load` for a `.yaml` demo). One-shot full read; no cursor, no pagination. |
| read for skip-check | `RunBatch.should_skip` -- `sweagent/run/run_batch.py:384-409` | `json.loads(log_path.read_text())`, inspects only `info.exit_status`. Deletes the file (`log_path.unlink()`) on empty/invalid/incomplete content. |
| read for prediction extraction | `sweagent/run/extract_pred.py:11-19` | `json.loads(traj_path.read_text())`, pulls `info["submission"]`, writes a sibling `.pred` file. Manual/offline recovery tool ("If for some reason the .pred file isn't saved..."). |
| read for demo conversion | `convert_traj_to_action_demo` -- `sweagent/run/run_traj_to_demo.py:39-58` | Reads `history` + `replay_config`, filters to assistant/user/tool roles, writes a `.demo.yaml`. |
| read for listing/viewing | `sweagent/inspector/server.py:168-212`, `sweagent/inspector/static.py:49-124` | `json.load` per file for display; a `check_for_updates` poll (`sweagent/inspector/server.py:281-282`) diffs `st_mtime` across the glob to detect new/changed files for the live web UI. |
| delete (manual, bulk) | `remove_unfinished(base_dir, dry_run)` -- `sweagent/run/remove_unfinished.py:14-41` | Offline CLI tool. For every experiment directory with exactly one `.traj`, if `info.submission` is `None`, `shutil.rmtree(directory)`. Not invoked automatically by any run path. |

There is no versioned/expected-position precondition on the write operation
at all: `write_text` has no compare-and-swap, no ETag, no sequence check. The
only concurrency control is convention (one `DefaultAgent` per instance
directory, one directory per instance id), not an enforced lock.

## Write and append path

- **Mechanism:** whole-document rewrite via `Path.write_text`, once per
  agent step, at `sweagent/agent/agents.py:786-787`:
  `self.traj_path.write_text(json.dumps(data, indent=2))`. This is called
  from the run loop after every `self.step()`
  (`sweagent/agent/agents.py:1284-1286`), so functionally the file is
  "appended to" at step granularity even though the write is a full rewrite,
  not a byte-range append.
- **Ordering:** positional. `self.trajectory` and `self.history` are Python
  lists; order is list order, with no explicit sequence number or timestamp
  field in either `TrajectoryStep` (`sweagent/types.py:44-52`) or
  `HistoryItem` (`sweagent/types.py:62-73`). There is no `seq`, no
  `event_id`, no server timestamp anywhere in these types.
- **Durability/atomicity:** none beyond the OS's own write-syscall semantics.
  No temp-file-and-rename pattern, no advisory lock file, no fsync call
  appear anywhere in `sweagent/agent/agents.py` or the `sweagent/utils/`
  package (checked by grep across the tree; none found). A process killed
  mid-`write_text` can leave a truncated/invalid JSON file; `should_skip`'s
  `try: data = json.loads(content) except Exception: ... log_path.unlink()`
  (`sweagent/run/run_batch.py:394-406`) is the only place that anticipates
  and heals a torn file, and it heals by deletion, not repair.
- **Concurrency:** single-writer-per-instance by construction, not by lock.
  `run-batch`'s multi-worker mode (`ThreadPoolExecutor`,
  `sweagent/run/run_batch.py:268-289`) parallelizes across **different**
  instance ids, each with its own `output_dir / instance_id` path
  (`sweagent/run/run_batch.py:334`), so there is no observed multi-writer
  contention on one file in the normal flow. Nothing in the code would
  prevent two processes from racing on the same instance id's `.traj` path if
  invoked concurrently by hand; there is no lock file guarding it.
- **Delivery semantics:** best-effort, at-most-once from the store's point of
  view -- there is no retry-on-write-failure and no acknowledgement channel.
  If `write_text` raises, the exception propagates up through the agent loop
  like any other Python exception; there is no dedicated handling for a
  failed trajectory write in `sweagent/agent/agents.py`.

## Read and resume path

There is no resume path in the sense of "reconstruct a live agent from a
stored session" (see Framing). The reads that exist are all one-shot, whole
document loads for a different purpose than resuming:

- `run-replay` reads the whole file once at construction
  (`sweagent/run/run_replay.py:85-88`) and starts a new `RunSingle` /
  `DefaultAgent` / `SWEEnv` from scratch (`sweagent/run/run_replay.py:173-202`).
  It materializes the entire `history` array eagerly to build the replay
  actions file (`_create_actions_file`, `sweagent/run/run_replay.py:138-171`);
  there is no lazy/partial load.
- `should_skip` reads the whole file once, looks at one field
  (`info.exit_status`), and either returns a skip signal or deletes the file
  (`sweagent/run/run_batch.py:384-409`). It never loads `history` back into an
  agent.
- The inspector reads the whole file once per view/poll cycle
  (`sweagent/inspector/server.py:168-212`); `check_for_updates`
  (`sweagent/inspector/server.py:281-289`) re-scans `st_mtime` across the glob
  on each poll rather than tailing a log, so its "resume" of the view after a
  reload is just "re-read the file from disk," not incremental.

There is no pagination, no cursor, no offset, and no bound on transcript size
anywhere in these paths; `max_observation_length`
(`sweagent/agent/agents.py:79` in `TemplateConfig`, default 100,000 chars)
bounds what the **model sees** per observation, not what is stored or read
back from the trajectory file.

## Listing, summaries, and search

- **Enumeration is directory glob, always**, never an index: `*.traj`
  (`sweagent/run/remove_unfinished.py:20`), `**/*.traj`
  (`sweagent/inspector/server.py:274`, `sweagent/inspector/static.py:158`),
  `*.pred` (`sweagent/run/merge_predictions.py:22`). No cost figures for this
  at scale are stated anywhere in the docs or code; the whole design assumes
  a batch of hundreds to a few thousand instances (SWE-bench scale) processed
  once, not a live, growing store enumerated repeatedly.
- **No write-time summary sidecar exists** for a single trajectory. The
  closest thing is `preds.json`, but that is a **derived rollup across many
  instances**, not a per-trajectory metadata cache: `merge_predictions`
  (`sweagent/run/merge_predictions.py:14-45`) globs every `.pred` file (itself
  written per-instance by `save_predictions`,
  `sweagent/run/common.py:370-379`) and writes one JSON object keyed by
  `instance_id`, each value `{"model_name_or_path", "instance_id",
  "model_patch"}`. `run_batch_exit_statuses.yaml`
  (`sweagent/run/_progress.py` via
  `RunBatchProgressManager(..., yaml_report_path=output_dir /
  "run_batch_exit_statuses.yaml")`, `sweagent/run/run_batch.py:182-184`) is
  the nearest thing to a listing view: one exit status per instance for the
  current `run-batch` invocation, not a durable index rebuilt across runs.
- **No search subsystem** of any kind (no FTS, no vector index, no grep
  helper) exists over trajectory content in this codebase. Finding something
  in a trajectory means opening the JSON (in the inspector, a text editor, or
  `jsoneditoronline.org`, per `docs/usage/trajectories.md:44-47`).

## Entry/message structure and versioning

Two parallel records are kept per run, and the docs are explicit that they
serve different purposes: `history` is "all messages that were shown to the
LM" (`docs/usage/trajectories.md:22`, i.e. the literal LM conversation
including system/demo/observation turns) and `trajectory` is the
(thought, action, observation) summary "for every step of the agent"
(`docs/usage/trajectories.md:7-9`).

### `TrajectoryStep` (the `trajectory` array)

Defined as a `TypedDict` at `sweagent/types.py:44-52`:

```python
class TrajectoryStep(TypedDict):
    action: str
    observation: str
    response: str
    state: dict[str, str]
    thought: str
    execution_time: float
    query: list[dict[str, Any]]
    extra_info: dict[str, Any]
```

Populated verbatim at `add_step_to_trajectory`
(`sweagent/agent/agents.py:1220-1233`):

```python
trajectory_step = TrajectoryStep({
    "action": step.action, "observation": step.observation,
    "response": step.output, "thought": step.thought,
    "execution_time": step.execution_time, "state": step.state,
    "query": step.query, "extra_info": step.extra_info,
})
```

- `query` is "the exact input at the current step," replacing an older
  `message` field that meant "the input for the LM for the _next_ step" prior
  to SWE-agent 1.1.0 (`docs/usage/trajectories.md:27-30`) -- the one place the
  format's own docs mark a breaking, named schema change.
- `state` is the environment state dict returned by
  `ToolHandler.get_state` (`sweagent/tools/tools.py:337-348`), sourced from
  `/root/state.json` inside the sandboxed environment
  (`sweagent/tools/tools.py:317-335`). Its keys depend entirely on which tool
  bundles are enabled; the `diff_state` bundle
  (`tools/diff_state/config.yaml`, `tools/diff_state/bin/_state_diff_state`)
  adds a `diff` key holding a full `git diff --cached` at that step
  (`tools/diff_state/bin/_state_diff_state:17-30`), used later as the last
  resort for autosubmission (see Rewind/checkpoints, below).
- `extra_info` is a grab-bag populated by optional action-sampling
  strategies: when `action_sampler_config` is set,
  `step.extra_info.update(best.extra_info)`
  (`sweagent/agent/agents.py:1040`) folds in whatever the sampler's chosen
  candidate carried, including -- for samplers like
  `BinaryTrajectoryComparison` -- formatted text of the **rejected**
  candidates (`sweagent/agent/action_sampler.py:96-183`). Rejected candidates
  are not separately persisted; they exist only inside this one field of the
  single committed step, then are gone once the sampler's process ends
  (nothing durable references them beyond that step's JSON).

### `HistoryItem` (the `history` array)

Required fields via `_HistoryItem` and optional fields via `HistoryItem`
(`sweagent/types.py:56-73`):

```python
class _HistoryItem(TypedDict):
    role: str
    content: str | list[dict[str, Any]]
    message_type: Literal["thought", "action", "observation"]

class HistoryItem(_HistoryItem, total=False):
    agent: str
    is_demo: bool
    thought: str
    action: str | None
    tool_calls: list[dict[str, str]] | None
    tool_call_ids: list[str] | None
    tags: list[str]
    cache_control: dict[str, Any] | None
    thinking_blocks: list[dict[str, Any]] | None
```

Model messages are stored **verbatim as sent/received**: assistant turns are
appended with the raw model `content` and `tool_calls`
(`add_step_to_history`, `sweagent/agent/agents.py:714-727`), and templated
observation/user turns are appended with their rendered text
(`_add_templated_messages_to_history`,
`sweagent/agent/agents.py:675-712`). There is no separate "raw provider
response" versus "normalized" pair -- `history` **is** the wire content, save
for the history-processor view described below, which operates on a copy at
read time, not on the stored list.

`agent` (which named agent produced/owns the entry, `"main"` by default) is
the field that would let a reader separate multiple agents' turns in one
`history` array; see Subagents, below, for where this matters (or, per the
evidence, does not -- see that section).

### Identity/dedup

The store relies on no identity or dedup key for entries: no entry carries a
uuid, hash, or sequence number. Ordering is purely array position; there is
no defined behavior for detecting or dropping a duplicate entry, because
nothing ever appends to an existing on-disk trajectory incrementally (see
Framing) -- the whole array is rebuilt in memory and rewritten each time.

### Versioning

The only explicit, named schema-version marker in the whole system is the
`query`-replaces-`message` note in the docs
(`docs/usage/trajectories.md:27-30`), tied to product version "SWE-agent
1.1.0," not to a field inside the JSON itself. There is no `schema_version`
field anywhere in `TrajectoryStep`, `HistoryItem`, or the top-level
`get_trajectory_data()` dict. Two `swe_agent_hash`/`swe_agent_version` fields
do ride along in `AgentInfo` (`sweagent/types.py:94-95`, set at
`sweagent/agent/agents.py:596-599` from `get_agent_commit_hash()` /
`__version__`), which stamps *which build* produced the file, but this is a
provenance stamp, not a format-version field a reader is expected to branch
on. `sweagent/run/run_replay.py:96-103` is the one place that reacts to an
old format, and it does so by a `KeyError` on `replay_config` being absent,
raising `"Replay config not found in trajectory. Are you running on an old
trajectory?"` -- sniffing by absence, not by a version tag.

## Compaction and history management

SWE-agent's `history_processors` are explicitly a **model-view-only**
mechanism; they never touch the durable `history` list.

The chain runs inside the `messages` property, not against `self.history`
itself:

```python
@property
def messages(self) -> list[dict[str, Any]]:
    filtered_history = [entry for entry in self.history if entry["agent"] == self.name]
    messages = filtered_history
    for processor in self.history_processors:
        messages = processor(messages)
    return messages
```
(`sweagent/agent/agents.py:539-551`)

`self.history` (the thing persisted into `.traj` at
`sweagent/agent/agents.py:771`) is built by `messages` at
`filtered_history = [entry for entry in self.history if entry["agent"] ==
self.name]` (`sweagent/agent/agents.py:544`) -- a **shallow** list
comprehension: it is a new list, but its elements are the exact same dict
objects that live inside `self.history`. Whether a processor is safe for the
durable record therefore depends on whether it mutates `entry` in place or
copies it first, and the processors in
`sweagent/agent/history_processors.py` split on exactly this line:

- **Content-eliding processors copy first, so they cannot touch the durable
  record.** `LastNObservations.__call__`
  (`sweagent/agent/history_processors.py:157-176`) does
  `data = entry.copy()` (`:167`) before rewriting `data["content"]` to the
  "N lines omitted" placeholder; the original `entry` inside `self.history`
  is untouched. `ClosedWindowHistoryProcessor.__call__`
  (`sweagent/agent/history_processors.py:230-258`) likewise copies
  (`data = entry.copy()`, `:234`) before truncating a stale file-window.
  `RemoveRegex.__call__` (`sweagent/agent/history_processors.py:320-336`) uses
  `entry = copy.deepcopy(entry)` (`:322`) before stripping regex matches.
  `ImageParsingHistoryProcessor._process_entry`
  (`sweagent/agent/history_processors.py:352-360`) uses `entry =
  copy.deepcopy(entry)` (`:354`) before splicing in parsed image segments.
- **`CacheControlHistoryProcessor` is the one exception, and it does mutate
  the durable record.** Its `__call__`
  (`sweagent/agent/history_processors.py:287-303`) calls
  `_clear_cache_control(entry)` (`:293`) and, conditionally,
  `_set_cache_control(entry)` (`:299`) directly on `entry`, with no copy
  anywhere in the function. Both helpers mutate their argument's dict in
  place -- `_clear_cache_control` pops `cache_control` keys
  (`sweagent/agent/history_processors.py:46-51`), `_set_cache_control`
  assigns into `entry["content"]`/`entry["cache_control"]`
  (`:53-65`) -- so every call to `self.messages` that includes this processor
  in the chain **writes `cache_control` markers into the same dict objects
  stored in `self.history`**, which is exactly what gets serialized into
  `history` in the next `.traj` write. This is metadata (an
  Anthropic prompt-caching hint), not conversational content, but it is a
  concrete instance of a "model-view" processor leaking a side effect into
  the durable record -- the one place in this codebase where the
  clean split between "durable log" and "model-visible view" does not
  fully hold.

So, with that one metadata-only exception: **compaction shrinks only what the
next model call sees; the durable `history` array in the `.traj` file is
never shortened or content-redacted by a history processor.** There is no
marker, no external snapshot, and no in-place content rewrite left in the
log for elided observations or closed windows, because there is no separate
"log" from the model-visible view to begin with for those processors -- they
compute a filtered copy from the same `self.history` list that gets
persisted directly.

There is no replay/resume behavior that "crosses a compaction boundary,"
because nothing ever resumes through the durable record in the first place
(see Framing); `run-replay`'s reconstructed run applies whatever
`history_processors` the **replay config** specifies to the **replayed**
conversation as it is built turn by turn, independent of whatever compaction
the original run applied.

## Rewind, checkpoints, and fork

- **No rewind, undo, or branch operation exists.** Nothing in
  `sweagent/agent/agents.py`, `sweagent/run/`, or the CLI surface lets a user
  roll the trajectory back to an earlier step and continue differently; the
  `while not step_output.done` loop (`sweagent/agent/agents.py:1284`) only
  moves forward.
- **The closest thing to a checkpoint is the per-step `state.diff` value**
  from the optional `diff_state` tool bundle
  (`tools/diff_state/bin/_state_diff_state:17-30`): a full, non-deduplicated
  `git diff --cached` computed fresh every step and stored under
  `TrajectoryStep["state"]["diff"]`. It exists purely as a fallback for
  crash/error recovery, not for user-facing rewind:
  `attempt_autosubmission_after_error`
  (`sweagent/agent/agents.py:823-851`) reaches into
  `self.trajectory[-1]["state"]["diff"]` (`:836,840,843`) when the runtime has
  died, to autosubmit whatever patch was last captured. There is no
  content-addressing or diffing between steps -- each step's `diff` is a
  complete snapshot of the working tree's staged changes at that instant.
- **`RetryAgent` is retry, not fork.** `_next_attempt`
  (`sweagent/agent/agents.py:321-326`) calls `self._env.hard_reset()` and sets
  up a brand-new `DefaultAgent`; there is no shared-prefix history between
  attempt N and attempt N+1 -- each attempt starts from the same original
  problem statement in a reset environment, not from a point partway through
  a previous attempt. Lineage between attempts is recorded only by the
  `"attempts"` list index in the parent's `.traj`
  (`sweagent/agent/agents.py:362-383`), not by any explicit parent/child
  pointer inside a child's own trajectory file.
- The one appearance of the word "fork" in the codebase is git-repository
  forking for opening a pull request
  (`sweagent/run/hooks/open_pr.py:23,77-89`), unrelated to session/trajectory
  forking.

## Subagents and nested sessions

There is no live multi-agent or delegate-to-subagent path in this codebase
(confirmed by grep across `sweagent/` and `tools/` for "subagent"/"sub_agent",
which returns only comments describing `RetryAgent`'s attempts as
"sub-agent," at `sweagent/agent/agents.py:267,333,335`). What does exist,
and is the closest analogue:

- **`RetryAgent` attempts** (`sweagent/agent/agents.py:257-440`). Each attempt
  is a full, independent `DefaultAgent` instance
  (`_setup_agent`, `sweagent/agent/agents.py:303-319`), given its own output
  directory `output_dir / f"attempt_{self._i_attempt}"`
  (`:315`) and therefore its own, separately durable `<instance_id>.traj`
  file at that path (via the normal `DefaultAgent.setup` path,
  `sweagent/agent/agents.py:589`). This is a durable, separately identified
  child record (one file per attempt directory), **and** it is folded whole
  into the parent's own `.traj` under `"attempts": [...]`
  (`:358-364,385-388`) -- so the child transcript exists both as its own file
  and duplicated inside the parent's file. Nesting is bounded by
  `RetryLoopConfig.max_attempts` and a `cost_limit`
  (`sweagent/agent/reviewer.py:184-216`); there is no crash/rewind cascade to
  reconcile because each attempt is independent from setup.
- **The `name`/`agent` field on `HistoryItem`** (`sweagent/types.py:63`,
  filtered on in `messages`, `sweagent/agent/agents.py:544`) is designed to
  let multiple named agents share one `history` array (`ShellAgent` and its
  config support this generality, `sweagent/agent/agents.py:170-186`), but no
  code path in this codebase actually runs two agents concurrently against
  one shared environment/history; the mechanism exists for future/other
  configurations more than for an active subagent feature here.
- **Action samplers are not subagents.** `AbstractActionSampler`
  implementations (e.g. `AskColleagues`,
  `sweagent/agent/action_sampler.py:49-94`) query one or more model candidates
  for the *next single action* and choose the best one inline within
  `forward()` (`sweagent/agent/agents.py:1031-1040`); rejected candidates'
  text lives only in that one step's `extra_info` (see Entry structure,
  above), not as separate durable trajectories or identified child sessions.

## Retention, deletion, and multi-host

- **No TTL, no scheduled cleanup, no automatic retention policy** exists
  anywhere in the codebase or docs. Trajectory directories accumulate under
  `trajectories/<user>/<experiment>/` (`docs/usage/trajectories.md:59-83`)
  indefinitely unless a human intervenes.
- **The only cleanup tool is manual and opt-in:** `remove_unfinished`
  (`sweagent/run/remove_unfinished.py:14-41`), a separate CLI invocation that
  defaults to `dry_run=True` and only deletes a whole instance directory
  (`shutil.rmtree`) when it finds exactly one `.traj` with no
  `info.submission`. It is never called from `run`, `run-batch`, or any hook.
- **Deletion in the running-program path is incidental, not policy-driven:**
  the only automatic delete is `should_skip`'s `log_path.unlink()`
  (`sweagent/run/run_batch.py:391,400,405`) for empty/corrupt/incomplete
  files, which exists to enable a clean redo, not to reclaim space or enforce
  a lifecycle.
- **Multi-host is not addressed as a first-class concern.** The whole design
  assumes a single local filesystem the run process can write to directly:
  `output_dir.mkdir(parents=True, exist_ok=True)`
  (e.g. `sweagent/agent/agents.py:572`) and plain `Path.write_text` calls
  throughout, no remote-writeback, no network-filesystem handling, no
  crash-detection heartbeat file. `run-batch`'s only concession to
  concurrent writers is a small random start delay to avoid thundering-herd
  container startup (`sweagent/run/run_batch.py:295-297`), not a
  cross-process or cross-host coordination mechanism.

## Interop with foreign session stores

None found. SWE-agent reads only its own `.traj`/`.demo.yaml` files
(`run-replay`, `traj-to-demo`) and its own `.pred` files
(`merge-preds`); no code path in `sweagent/` reads another agent product's
session or transcript format.

## What this implies for our Session Store (our inference)

SWE-agent is the corpus's clearest **negative case**: a product that produces
a rich, complete, well-documented transcript file, yet has no notion of a
session as a resumable, addressable, evolving record. Three points are worth
carrying into our design as contrast, not as pattern to copy:

- **A transcript is not a session merely because it is complete and replayable.**
  The decisive test we used here -- is the artifact ever read back by the
  *producing* program to continue, versus read only by a *different*
  consumer (evaluator, viewer, demo tool) -- is a clean litmus test we should
  keep applying to every other product in this corpus, including our own
  design: our Session Store must be distinguishable from "just a good log
  file" by having an actual resume/read path that the same runtime uses.
- **"Skip if already done" is not resume, and conflating them is an easy
  mistake.** `should_skip`'s behavior (skip a complete run, delete-and-redo an
  incomplete one) looks superficially like idempotent resume but is neither
  incremental nor state-preserving; our platform's language for these two
  concepts (dedup-on-completion vs. resume-from-position) needs to stay
  sharply distinct in the ADRs, because a store implementation could easily
  drift toward SWE-agent's model by accident if "idempotent retry" and
  "resume" are not kept conceptually separate from day one.
- **Whole-document rewrite-per-step, with no lock/fsync/atomic-rename, is the
  failure mode our append-only log design is explicitly meant to avoid.**
  SWE-agent's crash exposure (a torn `write_text` mid-run) is exactly the
  scenario an append-only event log with a separate durable commit boundary
  (as documented for other products in this corpus) is built to eliminate;
  it is useful as a concrete "what we are not doing" example when justifying
  that architecture choice.

## Open questions

- Whether a torn/partial `.traj` write (process killed mid-`write_text`) has
  ever been observed to corrupt a file such that even `should_skip`'s
  `json.loads` fails silently in some encoding edge case; the code path
  exists (`sweagent/run/run_batch.py:394-406`) but no test fixture exercising
  the truncation scenario itself was found in `tests/`.
- Whether any out-of-tree fork or downstream consumer (e.g. a hosted
  SWE-agent service) adds an index, database, or resume layer on top of the
  `.traj` file convention documented here; this dossier is scoped to what
  ships in this commit of the open-source repository only.
- The exact production conditions under which `RunBatch.main_multi_worker`
  (`sweagent/run/run_batch.py:268-289`) could produce two threads writing the
  same instance id's path concurrently (e.g. a caller passing duplicate
  instance ids into one `run-batch` invocation) were not traced end to end;
  the instance-loading paths (`sweagent/run/batch_instances.py`) were not
  audited for duplicate-id guarantees.
- Whether `sweagent/inspector/server.py`'s live "check for updates" polling
  (`:281-289`) is used by anything beyond the bundled web viewer, or whether
  any other internal tooling treats a growing trajectory directory as a
  quasi-live feed.
