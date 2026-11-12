# runs-on-fallback

Chooses between an optional self-hosted runner and GitHub-hosted runners for a
workflow's jobs. The self-hosted runner is used only for a trusted event and only
while one of its runners is online and idle; anything else, including every
error, schedules on the hosted fallback.

`runs-on: [a, b]` means a runner with both labels, not either one, so a list
cannot express "this or that". The choice has to be made at runtime, and
[the contexts `runs-on` accepts][contexts] do not include `steps`, so no step
can set the `runs-on` of its own job. This goes in a job of its own, or in the
selection job a workflow already has, and the jobs that care read its output
through `needs`.

[contexts]: https://docs.github.com/en/actions/reference/workflows-and-actions/contexts

## Usage

```yaml
jobs:
  runner:
    runs-on: ubuntu-latest
    timeout-minutes: 5
    outputs:
      runs-on: ${{ steps.pick.outputs.runs-on }}
    steps:
      - id: pick
        uses: ./.github/actions/runs-on-fallback
        with:
          github-token: ${{ secrets.RUNNER_STATUS_TOKEN }}
          primary-labels: ${{ vars.SELF_HOSTED_RUNNER_LABELS }}
          scope: organization

  test:
    needs: runner
    runs-on: ${{ fromJSON(needs.runner.outputs.runs-on || '["ubuntu-latest"]') }}
    steps:
      - run: echo test
```

The `||` fallback matters. A job that reads an output of a job that failed or was
skipped gets an empty string, and an empty `runs-on` is a job queued against no
runner: no error, forever.

## The token

Listing self-hosted runners is not something the default `GITHUB_TOKEN` can do,
so without a token of your own this always selects the fallback. Use a GitHub
App installation token or a fine-grained personal access token with:

- `repository` scope: repository permission Administration, read.
- `organization` scope: organization permission Self-hosted runners, read.

Fork pull requests receive no secrets, so they have no token. They never reach
the API anyway, because trust is decided first.

## Trust

Decided before any API call, and an untrusted event never selects the
self-hosted runner, whatever it would find there.

| Event | Trusted |
| --- | --- |
| `push`, `workflow_dispatch`, `schedule`, `merge_group` | yes |
| `pull_request` from a branch of this repository | yes |
| `pull_request` from a fork | no (`fork-pull-request`) |
| `pull_request_target` and every other event | no (`untrusted-event`) |
| an event payload that cannot be read | no (`untrusted-event`) |

A self-hosted runner keeps its disk and usually its network between jobs, which
is why a fork's workflow edits must not reach it.

## Inputs

| Input | Default | |
| --- | --- | --- |
| `github-token` | | Token described above. Empty selects the fallback (`no-token`). |
| `primary-labels` | | Labels of the self-hosted runner, separated by commas or newlines. Empty selects the fallback without an API call (`not-configured`). |
| `fallback-labels` | `ubuntu-latest` | Labels used when the self-hosted runner is not chosen. |
| `scope` | `repository` | `repository` or `organization`. The organization is the owner of this repository. |
| `slots` | `1` | How many jobs to place. Each online, not busy runner carrying every primary label takes one slot, lowest first; the rest take the fallback. |

Labels match case-insensitively, and a runner may carry more labels than the
primary ones.

## Outputs

| Output | |
| --- | --- |
| `runners` | JSON array with exactly `slots` entries, each a label array. |
| `runs-on` | Slot 0, a JSON label array, for `fromJSON`. |
| `self-hosted` | `true` when slot 0 has the primary labels. |
| `self-hosted-slots` | How many slots have the primary labels. |
| `reason` | `idle-runner` (at least one slot is primary), `no-idle-runner`, `not-configured`, `no-token`, `api-error`, `untrusted-event` or `fork-pull-request`. |

The decision is also written to the step summary.

## Failing

Only a mistake in the workflow fails the step: an unknown `scope`, an
`slots` that is not a whole number of at least 1, or empty
`fallback-labels`. A rejected token, an API error or a network failure selects
the fallback and logs a warning with the reason (`api-error`).

## Slots

A runner takes one job at a time, so pointing several jobs at one idle runner
queues them one after another. Ask for one slot per job instead, and give each
job its own index:

```yaml
  runner:
    outputs:
      runners: ${{ steps.pick.outputs.runners }}
    steps:
      - id: pick
        uses: ./.github/actions/runs-on-fallback
        with:
          slots: 2
          # ...

  build:
    runs-on: ${{ fromJSON(needs.runner.outputs.runners || '[]')[0] || 'ubuntu-latest' }}
  lint:
    runs-on: ${{ fromJSON(needs.runner.outputs.runners || '[]')[1] || 'ubuntu-latest' }}
```

Idle runners fill the lowest slots first, so give slot 0 to the longest job. An
index past the end, or an empty output, yields `ubuntu-latest`.

## Races

Idle is read at selection time and the job is queued later. Concurrent workflows
can see the same idle runner, so a job may briefly queue on it. One machine can
register several runner instances to offer several slots.
