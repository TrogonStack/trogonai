# Sandbox testing policy

The rules below keep the sandbox crates under `rsworkspace/crates/sandbox` from drifting.

## Ignored tests

An `#[ignore]` test never runs under `cargo test`, so it can rot silently.

- Give every ignored test a reason: `#[ignore = "requires Linux, bwrap, and curl"]`. A bare `#[ignore]` hides what the test needs.
- An ignored test needs something that runs it, such as a CI job, a live verification run or a documented command. Say which in the reason or in the test's doc comment.
- Tests that need an external service (a NATS server, a Docker daemon, a vendor account) are ignored with a reason naming the requirement, and run only where that requirement exists.

## Clippy bans and nextest profiles

`rsworkspace/clippy.toml` bans a few methods, each with a reason. Add a ban only after the code complies with it.

`rsworkspace/.config/nextest.toml` defines a `ci` profile that reports only failures, shows each failure immediately and again in the final summary, and does not stop at the first failure.

```sh
cargo nextest run --profile ci -p trogon-sandbox-contract
```

Doctests are not run by nextest; run `cargo test --doc` for them.

## Charts and alert rules

The `mise run github:workflows:ci-sandbox-deploy-checks:check-charts` task runs `helm lint` on the umbrella chart and every subchart, `helm template` with the default values and with the authorization settings the chart requires, `promtool check rules` on the rendered PrometheusRule resources, and `promtool test rules` on the files in `devops/helm/charts/trogon-sandbox/ci/rule-tests/`. It needs `helm`, `yq` and `promtool` on the path. Add a `*.test.yaml` beside the others when you add an alert.
