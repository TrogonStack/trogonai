# Agent Memory

This file is read by trogon-agent at the start of every event as a system prompt.
It provides persistent context across PR reviews and issue triage sessions.

The agent can update this file by calling `update_file` on a new branch and
opening a pull request with `create_pull_request`. A human reviews and merges
the PR — this ensures all memory changes have an audit trail.

## Team Conventions

<!-- Add team-specific conventions here. Examples:
- We use conventional commits: feat/fix/chore/docs/test/refactor
- PRs require at least one approver before merge
- All public functions must have doc comments
- Tests live next to the code they test (unit) or in tests/ (integration)
-->

## Known Patterns

<!-- The agent will append learned patterns here over time. Examples:
- This repo uses X pattern for Y
- Avoid Z because of W
-->

## Recent Decisions

<!-- Important architectural or process decisions go here. -->
