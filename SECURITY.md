# Security Policy

## Reporting a Vulnerability

Please do not report security vulnerabilities through public GitHub issues,
discussions, or pull requests.

Instead, use GitHub's private vulnerability reporting for this repository:

1. Go to the [Security tab](https://github.com/TrogonStack/trogonai/security) of this repository.
2. Click **Report a vulnerability**.
3. Fill in as much detail as you can: affected crate(s) or path(s), a description
   of the issue, and steps to reproduce if available.

This opens a private conversation with maintainers and keeps the report out of
public view until a fix is available.

## Response SLAs

| Stage | Target |
|-------|--------|
| Acknowledgement | Within 3 business days |
| Initial assessment (severity, affected scope) | Within 7 business days |
| Fix or mitigation | Best effort, timeline shared with the reporter once severity is assessed |

These are targets, not contractual guarantees. TrogonAi is maintained on a
best-effort basis; severity and available maintainer time both affect how
quickly a given report can move.

## Scope

In scope:

- The Rust workspace under `rsworkspace/` (gateway, decider, identity/auth,
  NATS transport crates, and related tooling).
- CI/CD workflows and composite actions under `.github/`.
- Build and release tooling that produces artifacts consumed by third parties.

Out of scope:

- Vulnerabilities in third-party dependencies that are already tracked upstream
  or by Dependabot, unless TrogonAi's usage introduces additional exposure.
- Denial of service against local development tooling (e.g. `mise` tasks, local
  CLIs) that requires local code execution to trigger.
- Social engineering or phishing attacks against maintainers.

## Supported Versions

TrogonAi does not yet cut a single repo-wide semantic version. Components under
`rsworkspace/` are released independently via tagged releases
(see `.github/release-please-config.json`). Until that changes, security fixes
are applied against the `main` branch and included in the next release of each
affected component; there is no guarantee of backports to older tags.

## Disclosure Policy

We ask reporters to give us a reasonable window to investigate and ship a fix
before any public disclosure. We will keep you updated as we work through the
report and will credit you in the fix's release notes, unless you prefer to
stay anonymous.
