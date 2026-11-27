# Sandbox guest init

`trogon-sandbox-guest-init` is the PID 1 binary of an OCI-derived Firecracker sandbox
guest, plus the library that parses the kernel command line it boots with.

On Linux the binary mounts `/proc`, `/sys`, `/dev` (with the standard character
devices, `/dev/fuse` and `/dev/net/tun`), `/dev/pts`, `/dev/shm`, `/run`, `/tmp` and
cgroup v2, links the `/dev/fd` and standard stream aliases, records the actor id under
`/run/ate/actor-id`, starts `/.sandbox/agent` with a minimal environment, and then reaps
orphaned children. If the agent exits, init reports the wait status and parks. On other
targets the binary prints that it requires Linux and exits with status 1. Outside PID 1 it
refuses to run.

The library exposes `GuestBootConfig`, built from the kernel command line with
`ate.actor_id=<id>` and `sandbox.agent_port=<port>`; `GuestBootConfigError` says which
argument was rejected. `GuestActor` is either `Unbound` (no actor argument, or the `unbound`
marker) or `Bound` to a `GuestActorId`, which is never empty and never the marker.
`GuestAgentPort` is a nonzero 32-bit vsock port, defaulting to 8110, and `GuestAgentPortError`
separates a non-numeric value (keeping the integer parse error as its source) from zero. The
first occurrence of each argument wins.

`probes/` holds manual Linux scripts that boot the binary in namespaces and check the
mounts, pseudo terminals, shared memory, cgroup limits, nested namespaces, overlay and
netfilter support, and orphan reaping. They are not run by Cargo.

Other sandbox crates: `trogon-sandbox-contract` defines the validated lease, command and
operation contracts whose agent this init starts.
