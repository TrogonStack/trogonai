# trogon-bugbot implementation plan

## What already exists

The basic BugBot already works. `trogon-agent/src/handlers/pr_review.rs` has a complete
7-step prompt that fires on `pull_request` events (`opened`, `reopened`, `synchronize`):

1. Fetches `.trogon/memory.md` for repo conventions
2. Reads existing PR comments
3. Calls `list_pr_files` to see changed files
4. Calls `get_pr_diff` for the unified diff
5. Calls `get_file_contents` for additional context
6. Posts a review comment via `post_pr_comment`
7. Updates `.trogon/memory.md` if conventions were learned

The full pipeline is also already in place:

```
GitHub webhook → trogon-source-github → NATS JetStream (github.pull_request)
    → trogon-agent runner → pr_review fallback handler
    → agent with tools → post_pr_comment
```

Deduplication per webhook event is handled by CAS on the `AGENT_PROMISES` NATS KV
bucket (each stream sequence gets a unique promise ID; concurrent workers cannot
process the same message twice).

**This plan is about extending the existing handler, not building a new crate.**

---

## What this plan adds

Inline diff annotations — the Cursor BugBot differentiator. Without `post_pr_review`,
the agent posts one general comment on the PR thread. With it, comments appear attached
to specific lines in the diff.

Additionally: draft PR filtering, binary/large file handling, SHA-based dedup for
rapid force-pushes.

---

## Gap 1 — `post_pr_review` tool (inline diff comments)

**Problem:** `post_pr_comment` posts to `/issues/{number}/comments` (thread comment).
GitHub inline review comments require:
```
POST /repos/{owner}/{repo}/pulls/{number}/reviews
{
  "commit_id": "<head SHA>",
  "body": "Overall summary",
  "event": "COMMENT" | "APPROVE" | "REQUEST_CHANGES",
  "comments": [{ "path": "src/lib.rs", "position": 7, "body": "..." }]
}
```
`position` is a **1-based index into the raw unified diff hunk** for that file, not a
line number. The LLM cannot reliably compute this without help.

**Solution — two steps:**

**Step 1: `annotate_diff(patch: &str) -> String`** helper in `tools/github.rs`

Prefix each line of a file's `patch` field with its position number:

```
1  @@ -10,6 +10,8 @@
2   fn existing_line() {
3  +    let x = dangerous_call();
4   }
```

The LLM reads the annotated patch and references position numbers directly. No
arithmetic required.

**Step 2: `post_pr_review` tool** in `tools/github.rs`

```rust
// Input schema:
// {
//   "owner": "...", "repo": "...", "pr_number": 42,
//   "commit_sha": "abc123",
//   "body": "Overall review summary",
//   "event": "COMMENT",
//   "comments": [{ "path": "src/lib.rs", "position": 3, "body": "Risky call" }]
// }
POST {proxy_url}/github/repos/{owner}/{repo}/pulls/{pr_number}/reviews
```

Register it in `pr_review_tools()` alongside the existing tools. The proxy already
routes all paths under `/github/` transparently to `api.github.com` — no proxy changes
needed.

---

## Gap 2 — Commit SHA must reach the agent

**Problem:** `post_pr_review` requires `commit_id` (the PR head SHA). The handler
currently builds the prompt without it.

**Solution:** Extract `payload["pull_request"]["head"]["sha"]` in `pr_review.rs` and
inject it into the prompt:

```rust
let head_sha = payload["pull_request"]["head"]["sha"]
    .as_str()
    .unwrap_or_default();

let prompt = format!(
    "... The current head commit SHA is `{head_sha}` — use it as `commit_sha` \
     when calling `post_pr_review`. ..."
);
```

---

## Gap 3 — Draft PR filtering

**Problem:** The handler fires for `opened` actions including draft PRs. Draft PRs
should not be reviewed until marked ready.

**Solution:** Add an early return in `pr_review.rs`:

```rust
if payload["pull_request"]["draft"].as_bool().unwrap_or(false) {
    return None;
}
```

---

## Gap 4 — Large / binary file handling

**Problem:** GitHub omits the `patch` field from `list_pr_files` responses for:
- Files with diffs over 1 MB
- Binary files

The agent receives a null or missing `patch` and has no guidance on what to do.

**Solution:** Add to the system prompt:

> If a file in `list_pr_files` has no `patch` field, skip it — it is either binary
> or too large to diff. Do not attempt to post inline comments for that file.

---

## Gap 5 — Deduplication for rapid force-pushes

**Problem:** The existing promise-based CAS deduplicates per webhook delivery (stream
sequence). Multiple `synchronize` events from rapid force-pushes each get a different
sequence number and each spawn a separate review session. Users accumulate reviews for
intermediate commits.

**Solution:** In `pr_review.rs`, before running the agent, write a key to the existing
`AGENT_PROMISES` KV bucket keyed by commit SHA:

```rust
let dedup_key = format!("pr-review-sha.{owner}.{repo}.{head_sha}");
if promise_store.get(&dedup_key).await?.is_some() {
    return None; // already reviewed this commit
}
promise_store.put(&dedup_key, b"1").await?;
```

Since each force-push produces a new SHA, only the first event per commit is processed.

---

## Gap 6 — Custom rules: `.trogon/memory.md`

The existing handler already reads `.trogon/memory.md` from the repository and injects
it into the prompt as repo conventions, and updates it when new conventions are learned.
This replaces the NATS KV custom rules approach — no new infrastructure needed. Teams
add rules by committing to `.trogon/memory.md` in their repo.

---

## Updated system prompt

Replace the current step 6 in `pr_review.rs` with:

```
6. For each modified file that has a `patch` field:
   a. The patch lines are numbered starting at 1 — use those numbers as `position`
      when calling `post_pr_review`.
   b. Identify bugs, security issues, or correctness problems.
   c. Call `post_pr_review` once with all inline comments. Set `commit_sha` to the
      head SHA provided above. Set `event` to "COMMENT" unless you are certain the
      code is correct (APPROVE) or has a critical issue (REQUEST_CHANGES).
   d. If a file has no `patch` field, skip it.
```

---

## Operational requirements (not code)

- **GitHub webhook** configured on the target repository, pointing to the running
  `trogon-source-github` service, with `pull_request` event type selected.
- **GitHub token** in the secret proxy must have `pull_requests: write` scope
  (in addition to the existing `issues: write` used by `post_pr_comment`).
- **`trogon-source-github`** and **`trogon-agent`** services must be running.

---

## Delivery order

| # | File | What |
|---|---|---|
| 1 | `trogon-agent/src/tools/github.rs` | Add `annotate_diff(patch: &str) -> String` helper |
| 2 | `trogon-agent/src/tools/github.rs` | Add `post_pr_review` tool |
| 3 | `trogon-agent/src/tools/github.rs` | Register `post_pr_review` in `pr_review_tools()` |
| 4 | `trogon-agent/src/handlers/pr_review.rs` | Extract head SHA from payload, inject into prompt |
| 5 | `trogon-agent/src/handlers/pr_review.rs` | Add draft PR early return |
| 6 | `trogon-agent/src/handlers/pr_review.rs` | Add commit SHA dedup via `AGENT_PROMISES` KV |
| 7 | `trogon-agent/src/handlers/pr_review.rs` | Update prompt: use positions, `post_pr_review`, skip patch-less files |
| 8 | `trogon-agent/tests/` | Unit test for `annotate_diff` |
| 9 | `trogon-agent/tests/` | Integration test for `post_pr_review` tool |
| 10 | `trogon-agent/tests/` | Handler tests: draft skip, SHA dedup, prompt contains head SHA |
| 11 | ops / README | Document `pull_requests: write` token requirement |

Steps 1–3 are independent of 4–7 and can be done in parallel.
Steps 8–10 can be written alongside their corresponding implementation steps.
