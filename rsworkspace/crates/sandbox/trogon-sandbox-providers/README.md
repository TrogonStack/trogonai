# Sandbox providers

`trogon-sandbox-providers` provides the `SandboxProvider` and `SandboxHandle`
traits and the provider adapters that implement them: `LocalDockerProvider`,
`K8sMicroVmProvider`, and `SubprocessSandboxProvider`. Each adapter launches a
`SandboxSpec`, runs commands in the sandbox, reports a `SandboxStatus`, and tears
the sandbox down.

`translate_lease_request_to_sandbox_spec` turns a validated lease request from
`trogon-sandbox-contract` into a `SandboxSpec`. The provider-facing types and the
`SandboxError` umbrella are typed; each adapter has its own error enum
(`DockerError`, `K8sError`, `SubprocessError`) so callers can match on the cause.

The Docker adapter talks to the daemon through the `DockerEngine` trait. The
Kubernetes adapter talks to the API server through `kube`, and tracks lease
ownership so that sweeps only reclaim resources the provider created.

When a lease requires egress control, the adapters redirect traffic to the egress
proxy using the exempt proxy UID from the contract, and refuse workloads that run
as that UID.

## Features

- `volume-cache`: clones repositories through the shared volume cache from
  `trogon-sandbox-volumes` before falling back to a plain git clone.

## Tests

The Docker and Kubernetes tests run against in-process fake servers and need no
cluster or daemon. The secret store test starts a NATS server in a container and is
tagged `requires-network` for Bazel.
