# Bazel coverage on fork pull requests

`CI Bazel` runs the fork coverage gate with a read-only token. `Bazel result`
requires that gate, including the 95% threshold, the per-file missed-line rules,
new uncovered statements, and the overall coverage-rate rule. The
`rust:coverage-baseline-reset` label on the current PR waives the diff policy;
it never waives the threshold or integrity checks.

The comparison uses the same pinned `TrogonStack/github-actions` coverage
helper and `pycobertura==3.0.0` as the normal coverage action. The converter's
source exclusions remain unchanged. Both baseline files come from one immutable
commit of `_xml_coverage_reports_bazel`, under `data/main/`. The helper verifies
the version 2 manifest, canonical XML digest, repository identity, Git blob
identities, source hashes and line bounds. The baseline source commit must be
an ancestor of the measured PR base. Missing or invalid baselines fail closed;
only the existing main workflow seeds or updates the baseline.

## Trusted publishing boundary

`CI Bazel Fork Coverage` runs from the default branch after `CI Bazel`
completes. Its comparison job has only Actions, contents and pull-request read
permissions. It independently repeats the comparison rather than trusting the
fork gate's exit status, Markdown, or manifest. A failed coverage gate can
therefore receive a failing report. The source run's `verify` job must have
succeeded. Cancelled runs and failed build/test/lint verification do not publish.

The source run is bound to the base repository ID and name, the `CI Bazel`
workflow ID and exact path, run ID, attempt, head repository, head branch and head
SHA. GitHub's commit-to-PR API must identify an unambiguous open PR with that
head. Any run-provided PR association must agree. This also supports fork events
whose `workflow_run.pull_requests` list is empty. The current PR's base branch,
base SHA, head and merge commit must agree with the artifact context. The merge
parents must be exactly the base and head. A newer run for the same fork branch and head,
including a run still in progress, invalidates the result.

The report artifact is named for the source attempt. Its GitHub metadata,
originating run and archive SHA-256 digest are verified. ZIP handling accepts
only the expected regular XML and context JSON files, with bounded sizes and
no extraction of producer-selected paths. The context is corroborating data,
never authority to select a repository, PR, command, URL or checkout. XML paths
are restricted to regular source blobs under `rsworkspace/` at the validated
merge commit. Source objects are read as data, without checking out or executing
fork code in either trusted job.

The publisher has pull-request write permission and read access needed to
revalidate the run and PR. It receives bounded JSON through the trusted
comparison job's output, never an artifact from the fork. It imports no
coverage helper and executes no report content. It validates the result schema,
recomputes the policy decision, escapes report text and mentions, and updates
only an unambiguous bot comment with the fixed coverage marker. It checks current
GitHub state and the consumer's current attempt again immediately before its
single comment write. It has no
contents write permission and never updates coverage storage or badges.

## Validation and operational limits

`CI Automation` runs the public task CLI fixtures with local Git objects, the
pinned shared helper and a fake `gh` executable. POST and PATCH requests are
recorded locally. The suite never comments on GitHub or writes a remote ref.
It covers policy failures above the threshold, reset-label limits, hostile
archives and metadata, stale runs, ambiguous associations, provenance failures,
pagination and state changes before publication.

The continuation must be present on the default branch to receive events.
Older runs without the attempt-specific artifact and context fail closed.
Use **Re-run all jobs** so each workflow attempt has its own verification,
artifact or comparison output. Partial reruns without those inputs are refused.
After a base or merge change, obtain a fresh pull-request run that measures the
current merge. GitHub reruns retain the original event SHA; rerunning an old
CI run or only its publisher cannot make stale coverage current. API failures, incomplete
pagination, unavailable source objects and comparison errors also fail closed.
The API listings are bounded to 1,000 entries per collection, archives to 16 MiB,
and XML reports to 128 MiB.

Fork code still controls its reported coverage counts. Source binding validates
which source those counts describe; it does not attest that a fork executed the
tests honestly. GitHub offers no atomic operation that checks PR state and
writes an issue comment together, so a state change can still occur after the
final read. Branch-scoped concurrency and the comment's source ordering prevent
an older completed publisher from intentionally replacing a newer report.

Hosted acceptance requires a real fork run after deployment: confirm the
original `Bazel result` rejects a regression above 95%, the trusted continuation
uses the expected permissions and attempt artifact, and passing and failing
reports update the intended PR. Exercise a re-run and a superseding head or base
change before treating local fixtures as hosted acceptance.
