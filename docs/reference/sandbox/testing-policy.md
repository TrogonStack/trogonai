# Sandbox testing policy

The rules below keep the sandbox crates under `rsworkspace/crates/sandbox` from drifting.

## Ignored tests

An `#[ignore]` test never runs under `cargo test`, so it can rot silently.

- Give every ignored test a reason: `#[ignore = "requires Linux, bwrap, and curl"]`. A bare `#[ignore]` hides what the test needs.
- An ignored test needs something that runs it, such as a CI job, a live verification run or a documented command. Say which in the reason or in the test's doc comment.
- Tests that need an external service (a NATS server, a Docker daemon, a vendor account) are ignored with a reason naming the requirement, and run only where that requirement exists.

Every ignored test is listed in `rsworkspace/crates/sandbox/ignored-tests.tsv` with its path, test function and reason. The file is generated, so do not edit it by hand. A new ignored test needs a reason. The count of bare `#[ignore]` attributes is capped (`MAX_BARE` in the task) and only goes down, so add a reason when you touch one. After you add, remove or rename an ignored test, regenerate the inventory and commit it with the change:

```sh
mise run github:workflows:ci-sandbox-ratchets:ignored-tests -- --write
```

Without `--write` the same task fails when the tree and the inventory differ in either direction, which is what CI runs.

## File size ratchet

No Rust file under `rsworkspace/crates/sandbox` may exceed `max(1000, its baseline)` lines. `rsworkspace/crates/sandbox/file-size-baseline.json` records the files that were already over 1000 lines and their size then. Those files may not grow, and no other file may cross 1000. To make room, split the file.

```sh
mise run github:workflows:ci-sandbox-ratchets:file-sizes
mise run github:workflows:ci-sandbox-ratchets:file-sizes -- --update
```

The first command checks, and CI runs it. `--update` only lowers entries and drops files that fell to 1000 lines or fewer. It never raises a ceiling or adds a file.

## Clippy bans and nextest profiles

`rsworkspace/clippy.toml` bans a few methods, each with a reason. Add a ban only after the code complies with it.

`rsworkspace/.config/nextest.toml` defines a `ci` profile that reports only failures, shows each failure immediately and again in the final summary, and does not stop at the first failure.

```sh
cargo nextest run --profile ci -p trogon-sandbox-contract
```

Doctests are not run by nextest; run `cargo test --doc` for them.

## Fuzzing

`rsworkspace/crates/sandbox/trogon-sandbox-egress-proxy/fuzz` holds cargo-fuzz targets `proxied_head` and `chunked_body` for the HTTP request head and chunked body parsers. They reach the parsers through the `fuzzing` feature of the crate, which no other build enables. The `Sandbox Fuzz` workflow (`sandbox-fuzz.yml`) runs each target weekly and on demand and uploads crashing inputs. To run one locally, with `cargo-fuzz` and a nightly toolchain installed:

```sh
cd rsworkspace/crates/sandbox/trogon-sandbox-egress-proxy
cargo +nightly fuzz run proxied_head -- -max_total_time=60
```

## Charts and alert rules

The `mise run github:workflows:ci-sandbox-deploy-checks:check-charts` task runs `helm lint` on the umbrella chart and every subchart, `helm template` with the default values and with the authorization settings the chart requires, `promtool check rules` on the rendered PrometheusRule resources, and `promtool test rules` on the files in `devops/helm/charts/trogon-sandbox/ci/rule-tests/`. It needs `helm`, `yq` and `promtool` on the path. Add a `*.test.yaml` beside the others when you add an alert.
