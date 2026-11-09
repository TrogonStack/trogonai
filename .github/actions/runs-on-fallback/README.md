# runs-on-fallback

Chooses between an optional self-hosted runner and GitHub-hosted runners for a
workflow's jobs. The self-hosted runner goes to a job only for an event that is
meant to reach it and only while one of its runners is online and idle; anything
else, including every API error, schedules on the hosted fallback.

`runs-on: [a, b]` means a runner with both labels, not either one, so a list
cannot express "this or that". The choice has to be made at runtime, and
[the contexts `runs-on` accepts][contexts] do not include `steps`, so no step
can set the `runs-on` of its own job. This goes in a job of its own, or in the
selection job a workflow already has, and the jobs that care read its output
through `needs`.

[contexts]: https://docs.github.com/en/actions/reference/workflows-and-actions/contexts

## Usage

Name the jobs that want a runner, longest first, and read each job's labels from
`runners` by name.

```yaml
jobs:
  runner:
    runs-on: ubuntu-latest
    timeout-minutes: 5
    outputs:
      runners: ${{ steps.pick.outputs.runners }}
    steps:
      # A local action is read from the workspace, so it needs a checkout.
      - uses: actions/checkout@<sha> # vX.Y.Z
        with:
          persist-credentials: false
      - id: pick
        uses: ./.github/actions/runs-on-fallback
        with:
          github-token: ${{ secrets.RUNNER_STATUS_TOKEN }}
          primary-labels: ${{ vars.SELF_HOSTED_RUNNER_LABELS }}
          scope: organization
          jobs: build lint

  build:
    needs: runner
    runs-on: ${{ fromJSON(needs.runner.outputs.runners).build }}
    steps:
      - run: echo build

  lint:
    needs: runner
    runs-on: ${{ fromJSON(needs.runner.outputs.runners).lint }}
    steps:
      - run: echo lint
```

Once the action is referenced by its published ref instead of `./`, it needs no
checkout and can be the first step.

`runners` holds an entry for every name in `jobs`, already set to the fallback
labels when the job is not self-hosted, so a job needs no fallback expression of
its own. The price is that the output must exist. The action never fails for an
API error, only for a bad input, and then the job that runs it fails and GitHub
skips the dependent jobs. Do not put `continue-on-error: true` on the step: the
job would succeed without the output, and the dependent jobs would fail with an
expression error on the empty output.

A job that uses the output also needs `needs:` on the job that produces it, or it
reads an empty string.

## The token

Listing self-hosted runners is not something the default `GITHUB_TOKEN` can do,
so without a token of your own this always selects the fallback. Use a GitHub
App installation token or a fine-grained personal access token with:

- `repository` scope: repository permission Administration, read.
- `organization` scope: organization permission Self-hosted runners, read.

Fork pull requests receive no secrets, so they have no token and fall back.

## Runner groups

With `scope: organization` the runners are listed per runner group, not
organization-wide, because a runner in a group that does not admit this
repository would leave the job queued forever. Only groups that GitHub reports as
visible to this repository are asked about. A group limited to selected
workflows (`restricted_to_workflows`) is skipped and counts as unavailable: which
workflows it admits cannot be checked reliably from here.

## Trust

The event check decides before any API call, and an event it does not trust never
selects the self-hosted runner. It is routing hygiene, not a security boundary.

| Event | Routed to the runner |
| --- | --- |
| `push`, `workflow_dispatch`, `schedule`, `merge_group` | yes |
| `pull_request` from a branch of this repository | yes |
| `pull_request` from a fork | no (`fork-pull-request`) |
| `pull_request_target` and every other event | no (`untrusted-event`) |
| an event payload that cannot be read | no (`untrusted-event`) |

On a `pull_request` from a fork, the fork controls the workflow files and this
action, so it can write the self-hosted labels into `runs-on` directly and skip
this check altogether. What keeps it out is configuration:

- Require approval for workflows from all external contributors in the
  repository's Actions settings.
- Limit the runner group to the repositories that need it, and allow public
  repositories in it only deliberately.
- Run ephemeral runners, one job each, on an isolated network, because a
  persistent runner keeps its disk and its network between jobs.
- Never use self-hosted runners with `pull_request_target`.

## Inputs

| Input | Default | |
| --- | --- | --- |
| `github-token` | | Token described above. Empty selects the fallback (`no-token`). |
| `primary-labels` | | Labels of the self-hosted runner, separated by commas or newlines. Empty selects the fallback without an API call (`not-configured`). |
| `fallback-labels` | `ubuntu-latest` | Labels used when a job is not self-hosted. |
| `scope` | `repository` | `repository` or `organization`. The organization is the owner of this repository. |
| `jobs` | `default` | Job names separated by spaces, commas or newlines, in priority order. Lowercase letters, digits, dashes and underscores, starting with a letter or digit, and unique. Each online, not busy runner carrying every primary label takes one job, first name first. |

Labels match case-insensitively, and a runner may carry more labels than the
primary ones. Every API request times out after 15 seconds and falls back.

## Outputs

| Output | |
| --- | --- |
| `runners` | JSON object with an entry for every name in `jobs`: the primary labels when that job is self-hosted, the fallback labels otherwise. |
| `runs-on` | The first job's labels, a JSON array, for a single job. |
| `self-hosted` | `true` when the first job is self-hosted. |
| `self-hosted-count` | How many jobs are self-hosted. |
| `reason` | `idle-runner` (at least one job is self-hosted), `no-idle-runner`, `not-configured`, `no-token`, `api-error`, `untrusted-event` or `fork-pull-request`. |

The decision is also written to the step summary.

## Failing

Only a mistake in the workflow fails the step: an unknown `scope`, a bad or
repeated name in `jobs`, or empty `fallback-labels`. A rejected token, an API
error, a timeout or a network failure selects the fallback and logs a warning
with the reason (`api-error`).

## Races

Idle is read at selection time and the job is queued later. Concurrent workflows
can see the same idle runner, so a job may briefly queue on it. One machine can
register several runner instances to offer several jobs at once.
