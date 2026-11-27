# Sandbox Prometheus alert rules

PrometheusRule resources ship with the sandbox Helm subcharts. Enable or disable them with `metrics.prometheusRule.enabled` (default `true`) on each subchart.

Wire these rules into the selector labels of your Prometheus Operator `Prometheus` or `PrometheusRule` configuration so Alertmanager receives firing alerts.

## Chart locations

| Alert | Subchart | Template |
|-------|----------|----------|
| `SnapshotBuilderFailureRateHigh` | `trogon-sandbox-snapshot-builder` | `devops/helm/charts/trogon-sandbox-snapshot-builder/templates/prometheus-rules.yaml` |
| `BrokerUnsettledFreezes` | `trogon-sandbox-broker` | `devops/helm/charts/trogon-sandbox-broker/templates/prometheus-rules.yaml` |
| `BrokerDigestPinMismatch` | `trogon-sandbox-broker` | `devops/helm/charts/trogon-sandbox-broker/templates/prometheus-rules.yaml` |

All three carry the label `severity: warning`.

## Alert definitions

### SnapshotBuilderFailureRateHigh

- **Expression:** `rate(snapshot_builder_failed_total[10m]) > 0.1`
- **For:** 5m
- **Meaning:** The snapshot builder is failing exports or registry pushes at more than 0.1 failures per second, averaged over 10 minutes. The counter carries a `reason` label.
- **Response:** Check the builder logs, errors from the worker's snapshot export request, registry authentication, and rejections by the scan gate or the signing step. See [Observability](../../how-to/sandbox/observability.md).

### BrokerUnsettledFreezes

- **Expression:** `sandbox_broker_unsettled_freezes > 0`
- **For:** 15m
- **Meaning:** The broker gave up settling the pause or resume of one or more leases. The gauge counts those leases.
- **Response:** Inspect the worker holding the lease, then either `trogon-sandbox-admin lease retry-freeze <lease id>` to ask the worker again or `trogon-sandbox-admin lease clear-freeze <lease id>` to accept the worker's state as it is.

### BrokerDigestPinMismatch

- **Expression:** `increase(sandbox_broker_digest_pin_mismatch_total[15m]) > 0`
- **For:** none, the alert fires as soon as the counter increases.
- **Meaning:** A snapshot restore was refused because the artifact digest did not match the pinned digest.
- **Response:** Find the lease and the snapshot reference in the broker logs, then check whether the tag or the registry content changed since the digest was pinned. See [Snapshot store](../../architecture/sandbox/snapshot-store.md).

## See also

- [Observability](../../how-to/sandbox/observability.md)
- [Deploy the sandbox stack](../../how-to/sandbox/deploy.md)
- [Registry RBAC](../../architecture/sandbox/registry-rbac.md)
- [Volumes](../../architecture/sandbox/volumes.md)
