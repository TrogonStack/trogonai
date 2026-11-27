# Sandbox Proxmox worker

`trogon-sandbox-proxmox-worker` is a sandbox worker for a Proxmox VE host. It leases
one linked clone of a template VM per sandbox, runs commands and file operations
through the QEMU guest agent, answers lease operations over NATS, and keeps what
it created recoverable across restarts. The `trogon-sandbox-proxmox-worker` binary
runs it.

## Types

- `ProxmoxConfig` holds the worker's settings, built from `ProxmoxConfigParts`.
  `VmId`, `ResourcePoolId` and `ProxyNetwork` are the parsed pieces. `AntiSpoof`
  chooses whether a guest NIC that lacks MAC and IP filtering refuses the lease or
  only warns.
- `ProxmoxApi` is the REST client for one node. It waits for asynchronous tasks
  up to a deadline and tells an absent VM apart from an unreadable one. Besides
  what the provider uses, it can make a linked clone, destroy a VM, take a
  snapshot that keeps the VM's memory and roll back to a named snapshot, each
  returning once its task finishes.
- `ProxmoxProvider` implements `trogon_sandbox_protocol::Provider`. It clones the
  template, boots the clone, runs commands, writes and reads files, and destroys
  the clone. A journal per lease and per dispatch attempt lets a restarted worker
  finish or undo what a previous run left half done.
- `ProxmoxWorker` serves a provider on NATS: targeted lease dispatch through the
  durable attempt ledger, cleanup requests, lease operations, heartbeats and the
  authority check.
- `ProxmoxProvider::spike` checks a live host end to end: it leases one small
  networkless VM, runs a command that verifies the guest identity, environment
  and working directory, round-trips a binary file, destroys the VM and confirms
  it is gone. It writes one JSON line per phase and fails with `SpikeError`.

## Behavior

- A lease must be ephemeral, not prewarmed and unprivileged, and ask for the
  worker's image. Exposed ports, persistence, workspace sources and secret
  bindings are refused.
- Without a proxy network the clone has no network interface. With one, the worker
  attaches a single NIC, sets firewall rules and per-NIC address filters, reads
  them back, and routes the guest through a per-lease egress proxy.
- A second worker cannot open a state directory that another one holds, and the
  worker identity persists across restarts.
- On start and on shutdown the worker destroys every clone it owns. A VM id that
  was reused by someone else is never touched.

## Fit

The wire types come from `trogon-sandbox-contract`. Dispatch verification, the
attempt ledger, the authority guard, heartbeats, large-payload transfer and the
guest capture protocol come from `trogon-sandbox-protocol`. The per-lease proxy
comes from `trogon-sandbox-egress-proxy`.

## Running

The binary reads its settings from flags or `TROGON_SANDBOX_*` environment
variables, including the API token id and secret. It stops on interrupt after
in-flight requests finish and destroys the clones it holds.

`--spike` runs the live host check once, prints its JSON lines and exits without
connecting to NATS. The ignored `the_spike_passes_against_a_live_host` test runs
it against the host named by the `TROGON_SANDBOX_PROXMOX_*` variables:
`cargo test -p trogon-sandbox-proxmox-worker --test binary -- --ignored`.
