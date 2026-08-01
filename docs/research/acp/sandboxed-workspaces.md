# Sandboxed Per-Session Workspaces for trogonai's acp-host

Produced 2026-07-30 by a source-grounded research agent; single-pass,
claims cite primary docs; flagged inferences are marked inline.

Grounding note: trogonai's `acp-host` is the ACP client-host role (spawns
`gemini --acp`, `codex-acp`, `claude-agent-acp`, `goose acp` as stdio
children, serves their `fs/read_text_file`, `fs/write_text_file`,
`terminal/*` callbacks, and stages inbound channel media under
`media/inbound/*`). Per prior ACP research, "the agent never
touches disk directly; the caller mediates everything. This is the authz
hook point for trogonai" (see [Host Role and Invocation
Mechanics](./host-role-and-invocation.md)). This dossier grounds that hook
point in how the rest of the industry builds the same boundary.

## 1. Isolation landscape

| Primitive | Contains | Cold start | Density | Cost/complexity |
|---|---|---|---|---|
| Path validation only, no OS sandbox | Nothing at the OS level; app-level canonicalize+prefix checks | ~0ms | Unbounded | Lowest, but demonstrably exploitable |
| bubblewrap (bwrap) | Mount/user/net/PID/IPC/UTS namespaces, unprivileged, no root/setuid | ms-scale, no official benchmark (one dev measured `ls` at ~3ms) | Not published | Low; no daemon, no image pulls |
| Landlock LSM | Filesystem (and newer, network) ACLs attached to a process, unprivileged | Not published; plain syscalls, no forked helper | N/A | Low; official Rust crate `landlock` (landlock-lsm/rust-landlock) |
| seccomp-bpf | Syscall filtering only, complements namespaces/Landlock | Not published, single prctl/seccomp call | N/A | Low; Rust crates `seccompiler` (rust-vmm/Firecracker-derived), `libseccomp-rs` |
| Anthropic sandbox-runtime (srt) | bwrap+Landlock+seccomp (Linux), Seatbelt (macOS), WFP (Windows alpha); wraps the *whole process* | Not published | Not published | Medium; open source, github.com/anthropic-experimental/sandbox-runtime, "beta research preview" |
| Docker/OCI containers | Namespaces + overlay FS, shares host kernel | ~0.5-1.2s cold on SSD per a 2026 arXiv benchmark; namespace setup itself only 8-10ms | Third-party estimate ~400-500/host at ~10MB overhead each (not vendor-verified) | Medium-high; needs a daemon; docker-in-docker is a well-documented failure mode (storage-driver nesting breaks, requires `--privileged`) |
| gVisor (runsc) | User-space "application kernel" intercepting syscalls; not a namespace wrapper, not a VM | "Milliseconds" per gVisor's own marketing, no exact figure found | Not published | Medium-high; drop-in OCI runtime; used by Google (GKE Sandbox, "millions of instances daily"), OpenAI ("higher-risk tasks, code execution"), and Anthropic ("securely contain code execution within claude.ai") |
| Firecracker microVMs | Full KVM VM, 5-device minimal model, own kernel per instance | "As little as 125ms" (Firecracker's own figure) | <5MB overhead/instance, "thousands... on the same machine," up to 150 creations/sec/host | High; Linux/KVM only, no macOS |
| macOS Seatbelt (sandbox-exec) | FS allow/deny with default-write-to-cwd, network via localhost-proxy-only | No hard benchmark; Claude Code's docs call it "minimal" overhead | N/A | Medium; officially deprecated by Apple (man page says so) but still what Claude Code, Codex CLI, Bazel, and Gemini CLI actually ship for macOS today |

Hosted platforms layer these primitives as managed services: E2B is
Firecracker-based (each sandbox = one microVM with its own kernel, per
e2b.dev's own blog), Daytona defaults to OCI containers with an optional
dedicated-VM tier for stronger isolation, Modal runs on gVisor plus its own
scheduler/filesystem layer, and Morph's mechanism is unconfirmed in its own
docs (its "Infinibranch" snapshot/branch/restore language is consistent with
a Firecracker-style approach but that is inference, not a documented fact).

Two demonstrated failure modes make "path validation only" concretely
disqualifying rather than merely theoretically weak: Anthropic's own
filesystem MCP server shipped a naive prefix-match path check and was hit by
two CVEs (CVE-2025-53110, string-prefix match instead of path-boundary
match; CVE-2025-53109, symlink-resolution fallback that skipped
re-validating the symlink target), and Endor Labs found path traversal is
"the single most common MCP server vulnerability" across an audit of 2,614
MCP implementations (82% of file-operation-bearing servers affected).
Sources: cymulate.com/blog/cve-2025-53109-53110-escaperoute-anthropic,
cybersecuritynews.com/anthropics-mcp-server-vulnerability.

## 2. What real agent products actually do

**OpenClaw** (docs.openclaw.ai/gateway/sandboxing, confirmed verbatim):
Docker is the default backend; workspace mount path depends on configured
access level (`ro` at `/agent`, `rw` at `/workspace`, `none` gives an
isolated sandbox dir under `~/.openclaw/sandboxes`). Default posture is
`network:"none"`, `readOnlyRoot:true`, `capDrop:["ALL"]`. Path/symlink
validation "resolves it again through the deepest existing ancestor before
re-checking blocked paths," guarding against escapes like
`/workspace/run-link/new-file` resolving outside the sandbox. The docs are
explicit that this is "not a perfect security boundary, but it materially
limits filesystem and process access when the model does something dumb,"
and that admin-configured bind mounts deliberately "bypass the sandbox
filesystem."

**Claude Code / Anthropic's sandbox-runtime (srt)**: two distinct layers.
The built-in sandboxed Bash tool (Seatbelt on macOS, bubblewrap on
Linux/WSL2) restricts only Bash and its children; other built-in tools and
MCP servers/hooks "run unconstrained on the host" unless wrapped in the
separate, open-source `@anthropic-ai/sandbox-runtime`, which wraps the
*entire process* in the same OS primitives. Filesystem model is
deny-then-allow-override for reads, deny-by-default-allow-only for writes.
Network is default-deny, proxy-enforced (HTTP proxy + SOCKS5), with
domain allow/deny lists. Anthropic's own docs state the load-bearing
principle directly: "Permission modes decide whether a tool call runs and
whether you are prompted first. Isolation restricts what a command can
access once it runs. The two work together," and explicitly warn that
`--dangerously-skip-permissions` is *not* an isolation boundary and should
always be paired with a container, VM, or srt.
Sources: github.com/anthropic-experimental/sandbox-runtime,
code.claude.com/docs/en/sandbox-environments,
code.claude.com/docs/en/sandboxing.

**Codex CLI (OpenAI)**: three sandbox modes, `read-only`,
`workspace-write` (scoped to cwd + tmp dirs, network off by default unless
`network_access=true` is set), and `danger-full-access`. macOS uses Seatbelt
via `sandbox-exec`; Linux uses Landlock + seccomp for the same mode
semantics. (One secondary-sourced claim, not from OpenAI's own docs: many
CI/container environments can't grant the kernel capabilities Landlock
needs, pushing those environments toward `danger-full-access` in practice.
Flagged as plausible but unverified against primary docs.

**E2B**: Firecracker microVM per sandbox, kernel-level isolation.
Pause/resume is a true VM snapshot-restore (5-30ms resume per Firecracker's
own figures), preserving filesystem and memory together, or filesystem-only
via `keepMemory:false`. Session duration caps: 24h continuous run on Pro
tier (1h Hobby), reset by pause/resume cycles.

**Zed and the ACP boundary question (the pivotal finding)**: Zed's own
sandboxing docs state verbatim, "Sandboxing applies only to Zed Agent. It
does not sandbox Zed itself, language servers, extensions, tasks, your
normal terminal tabs, **External Agents**, or Terminal Threads"
(zed.dev/docs/ai/sandboxing). External Agents are exactly the ACP-connected
processes trogonai is modeling (gemini --acp, codex-acp, claude-agent-acp).
Zed does not OS-sandbox them at all; it treats the RPC serving surface
(`fs/read_text_file`, `fs/write_text_file`) plus permission prompts as the
entire security boundary (zed.dev/docs/ai/external-agents). The ACP spec
itself confirms this is by design, not an oversight: its "Additional
Workspace Roots" RFD states "this field does not create a new privilege
model... Clients that need root boundaries to be enforced in that
deployment model SHOULD apply operating-system or runtime sandboxing
consistent with the declared root set"
(agentclientprotocol.com/rfds/additional-directories). The spec pushes
enforcement entirely onto the client and does not mandate any path
restriction or process sandboxing. This means Zed, the reference ACP
client, chose RPC-boundary-only enforcement and explicitly does not
consider that a completed security story for external agents; it is a
convenience/architecture boundary, not a hardened one.

## 3. The ACP angle: sandbox-the-process vs validate-at-the-RPC-boundary vs both

The dominant pattern among products that treat agent CLIs as genuinely
untrusted (OpenClaw, Claude Code's srt, Codex CLI, E2B) is whole-process OS
or VM-level sandboxing, with RPC/tool-call permission prompts layered on
top as defense-in-depth, not as the sole boundary. Zed is the outlier and
the minority pattern: RPC-boundary-only, explicitly scoped away from
external agents, with the spec's own text agreeing that's a client
implementation choice rather than a protocol guarantee.

For trogonai this comparison resolves cleanly given the architecture
already in place: `acp-host` already IS the party serving `fs/*` and
`terminal/*` ("the agent never touches disk directly; the caller mediates
everything," per [Host Role and Invocation
Mechanics](./host-role-and-invocation.md)). That gives RPC-boundary
validation for free as a first layer, but the Zed precedent plus the CVE
history above show that RPC-boundary validation alone (server-side path
canonicalization and prefix checks) is exactly the pattern that produced
real CVEs elsewhere. It should be a floor, not a ceiling. Both layers are
warranted: validate every `fs/*` and `terminal/*` RPC path against the
session's workspace root regardless of what sandbox wraps the child (belt),
AND wrap the child process itself in an OS sandbox (bubblewrap+Landlock on
Linux, Seatbelt on macOS) so that anything the agent does outside the RPC
surface, such as spawning its own subprocesses, using its own filesystem
APIs if it's a hosted-adapter binary rather than a pure stdio pipe, or
hitting the network directly, is still contained (suspenders).
Real ACP clients do not converge on one answer here: Zed relies on
suspenders-optional (RPC checks, no process sandbox for externals); Claude
Code and OpenClaw run both layers when they act as an agent host.

## 4. Per-session workspace lifecycle

**Provisioning.** Two provisioning strategies solve different problems and
trogonai needs both, not one. For the git-tracked codebase itself, **git
worktree per session** is the emergent pattern specifically for "one repo,
many concurrent coding-agent sessions": one shared `.git` object store,
each session gets an independent working directory and index, avoiding
file-level lock contention at a fraction of `git clone`'s disk/time cost.
This is what Conductor (conductor.build) and comparable multi-agent
orchestrators are built around, and Anthropic's own Claude Code guidance
(via secondary sources) recommends worktrees for running multiple
simultaneous sessions on one project. One source is explicit that this is
git-level convenience only, not a security boundary: "Git Worktrees Need
Runtime Isolation for Parallel AI Agent Development" (penligent.ai): the
working tree is still an ordinary host directory unless wrapped in a
sandbox. Hosted general-purpose sandbox platforms (E2B, Daytona, Modal)
solve a different problem: they aren't git-aware and don't share one repo
across tenants, so they provision by cloning a template/snapshot image into
a fresh microVM or container instead. trogonai should borrow that idea
selectively: cache expensive-to-rebuild state (installed toolchains,
`node_modules`) as a read-only overlay/volume mounted into each worktree,
rather than reinstalling per session, while using worktrees (not VM
cloning) for the codebase itself.

**Persistence.** Because a worktree is a directory on host/cluster storage
rather than a metered VM disk, persistence across turns and across a
session being paused and resumed later is close to free: the directory
just sits idle. This differs from the hosted platforms, where persistence
is an explicit, metered feature of the VM/container substrate: Daytona has
the most explicit three-tier model (`stopped` preserves filesystem/clears
memory, `archived` moves filesystem to object storage to free disk quota,
`paused` on VM-tier sandboxes preserves both filesystem and memory like a
true snapshot); E2B's pause/resume is a genuine VM snapshot-restore; Modal
caps sandboxes at a hard 24-hour maximum lifetime and requires an explicit
Filesystem Snapshot plus a fresh Sandbox object to persist beyond that. The
real design question for trogonai is not "how to persist" (it persists by
default) but "when to reclaim disk."

**Quotas.** Daytona is the only platform among those surveyed that
publishes a concrete default resource table in primary docs: 1 vCPU / 1
GiB RAM / 3 GiB disk by default (min 1 vCPU/1 GiB/1 GiB, max 4 vCPU/8
GiB/10 GiB; a separate GPU tier goes up to 16 vCPU/192GB/512GB). E2B and
Modal do not publish comparably explicit numeric defaults in the docs
pages checked. Since acp-host workspaces are worktrees on shared storage,
the relevant quota shape is disk-usage-per-workspace-directory (du-based)
plus a process-level cgroup/rlimit (CPU/memory/pids) applied through the
same sandbox layer already doing filesystem/network isolation. Daytona's
table is a reasonable anchor for "one interactive coding-agent session."

**TTL / cleanup.** Daytona's primary docs describe four independently
configurable intervals rather than one flat number: auto-stop (idle
minutes), auto-archive (moves a stopped container's filesystem to object
storage), auto-delete (minutes after continuously-stopped before hard
deletion; 0 means delete on stop), and an optional absolute wall-clock TTL.
E2B caps continuous running at 24h Pro/1h Hobby (reset by pause/resume) and
separately retains paused sandboxes for a bounded window before deletion
(commonly cited as 30 days across secondary sources, though one direct doc
fetch in this research returned "kept indefinitely"; this specific number
needs reconciliation before being cited as fact). Modal enforces a hard
24-hour lifetime cap plus an optional inactivity-based `idle_timeout`. None
of the three surveyed platforms documents disk-usage-based (LRU) eviction
as a policy distinct from time/inactivity; that is a gap trogonai would
have to design itself, not adopt off the shelf, and is likely necessary
once many worktrees accumulate on shared storage.

**Network egress.** Every platform converges on the same *mechanism*
(domain or CIDR allow/deny lists, SNI-based matching, proxy- or
firewall-enforced) while disagreeing on the *default posture*. OpenClaw
defaults to `network:"none"`; Anthropic's srt/Claude Code sandbox defaults
to deny-all with an explicit allowlist proxied through HTTP/SOCKS5. Modal
and E2B both default to allow-all outbound (Modal: "Sandboxes can make
outbound connections to any public IP address" by default, with opt-in
`block_network`, CIDR allowlist, or beta domain allowlist; E2B: allow-all
is the default firewall mode, with opt-in deny-all or user-defined
domain/CIDR rules, both updatable at runtime without restart). Given
acp-host wraps a small, enumerable set of known third-party agent CLIs
whose real network needs (model API endpoints, package registries) are
knowable in advance per agent type, a default-deny allowlist (matching
Anthropic's own posture for the structurally identical problem of wrapping
an agent CLI process) is both the strictest and the cheapest-to-construct
option, and is recommended over the hosted-platform default of allow-all.

## 5. Recommendation: tiered design for acp-host

**Tier 0, always on, no exceptions.** RPC-boundary validation inside
`acp-host` itself: every `fs/read_text_file`, `fs/write_text_file`, and
`terminal/*` call is checked against the requesting session's workspace
root using canonicalize-then-boundary-check (not string-prefix match, the
exact bug class behind CVE-2025-53110/53109), with symlink resolution
re-validated through the deepest existing ancestor (OpenClaw's documented
technique) so a symlink planted mid-session can't retarget a later write
outside the workspace. This tier is mandatory regardless of what process
sandbox wraps the child, because it is also what makes `media/inbound/*`
staging and workspace-relative resource_link references (per the
file-media-pipeline research) safe to serve. This layer alone is what Zed
ships for external ACP agents. trogonai should not stop here, given the
CVE precedent for RPC-boundary-only enforcement.

**Tier 1, default, for the common case (semi-trusted first-party-adjacent
CLIs).** Wrap the spawned child in an OS-native sandbox around a git
worktree: bubblewrap + Landlock + seccomp on Linux, Seatbelt on macOS for
local dev parity, mirroring Anthropic's own srt architecture (the closest
real precedent for "wrap an arbitrary agent CLI subprocess without a
container"). Default-deny network via an egress proxy with a per-agent-type
domain allowlist (model API host, package registry). Provision the
workspace as a worktree against the session's target repo, mount a shared
read-only cache/overlay for toolchain state, and enforce a disk-usage quota
per workspace directory plus process cgroups for CPU/memory/pids anchored
near Daytona's published default (1 vCPU/1 GiB/3 GiB) as a starting point.
Cleanup follows Daytona's graduated stop-idle to archive-cold to
delete-stale pipeline, with disk-pressure-triggered LRU eviction added on
top since no reference platform documents that and trogonai's shared
worktree storage needs it once many sessions accumulate.

**Tier 2, escalated, for untrusted or high-risk sessions (unknown
tenants, agent CLIs with looser first-party trust, or anything a policy
engine flags for elevated risk).** Escalate the process boundary to gVisor
(runsc) or a Firecracker-backed microVM rather than bubblewrap/Seatbelt,
matching the precedent OpenAI and Anthropic themselves use for "higher-risk
tasks, such as code execution" and "securely contain[ing] code execution."
This tier trades cold-start latency (still sub-200ms for Firecracker,
"milliseconds" claimed but unquantified for gVisor) for a hardware- or
kernel-boundary guarantee instead of a shared-kernel namespace guarantee.
Tier 2 is where a hosted-sandbox dependency (self-run gVisor/Firecracker,
or a managed provider) becomes justified. Tier 1 should remain
self-contained inside `acp-host` so the common path has no external
service dependency and works identically in local dev and production, per
the "same image runs as MicroVM in prod and Docker locally" principle
documented for Browser Use's architecture.

**Trigger for escalation**: this should be a policy-engine decision (the
same permission ladder trogonai's ACP `session/request_permission` handling
already needs, per the bridge-mechanics research), not a static
per-agent-type setting, e.g. trust level of the tenant/session, whether
the workspace holds credentials-adjacent files, or an explicit
elevated-risk flag from the channel adapter.

## Key claims

1. Anthropic's own `@anthropic-ai/sandbox-runtime` (open source,
   github.com/anthropic-experimental/sandbox-runtime) is the closest direct
   precedent for trogonai's exact problem: wrapping an arbitrary CLI
   subprocess in bubblewrap+Landlock+seccomp (Linux) or Seatbelt (macOS)
   without requiring a container, with default-deny filesystem writes and
   proxy-enforced default-deny network.
2. Zed, the ACP reference client, explicitly does NOT process-sandbox
   external ACP agents ("Sandboxing applies only to Zed Agent... It does
   not sandbox... External Agents") and relies solely on RPC-mediated file
   access plus permission prompts (zed.dev/docs/ai/sandboxing,
   zed.dev/docs/ai/external-agents).
3. The ACP spec itself declines to prescribe any isolation model, pushing
   the entire enforcement responsibility to the client
   (agentclientprotocol.com/rfds/additional-directories: "Clients... SHOULD
   apply operating-system or runtime sandboxing").
4. Path-validation-only enforcement is a demonstrated, not theoretical,
   failure mode: Anthropic's own filesystem MCP server was hit by
   CVE-2025-53109 and CVE-2025-53110 from exactly this pattern, and path
   traversal is "the single most common MCP server vulnerability" across
   an Endor Labs audit of 2,614 servers.
5. gVisor is the incumbent choice for AI-agent code-execution sandboxing at
   large-lab scale: Google (GKE Sandbox, "millions of gVisor sandbox
   instances running daily"), OpenAI ("higher-risk tasks, such as code
   execution"), and Anthropic ("securely contain code execution within
   claude.ai") all use it (gvisor.dev/users/).
6. Firecracker's own published figures (125ms boot, <5MB overhead/instance,
   thousands of microVMs per host, up to 150 creations/sec/host) are the
   basis for E2B's identical claims, since E2B is built directly on
   Firecracker microVMs (e2b.dev/blog/firecracker-vs-qemu).
7. Daytona is the only hosted sandbox platform among those surveyed that
   publishes concrete default resource quotas (1 vCPU / 1 GiB RAM / 3 GiB
   disk) and a graduated stop/archive/delete lifecycle in its primary docs
   (daytona.io/docs/en/sandboxes/).
8. Git worktree-per-session (not full clone or VM template cloning) is the
   emergent pattern specifically for "one repo, many concurrent
   coding-agent sessions," used by Conductor.build and recommended in
   Anthropic's own Claude Code multi-session guidance, but is explicitly
   git-level convenience, not a security boundary, on its own.

## Sources

- https://agentclientprotocol.com/rfds/additional-directories
- https://zed.dev/docs/ai/sandboxing
- https://zed.dev/docs/ai/external-agents
- https://docs.openclaw.ai/gateway/sandboxing
- https://github.com/anthropic-experimental/sandbox-runtime
- https://code.claude.com/docs/en/sandbox-environments
- https://code.claude.com/docs/en/sandboxing
- https://docs.onlinetool.cc/codex/docs/sandbox.html (mirror of openai/codex docs)
- https://github.com/openai/codex/blob/main/codex-cli/src/utils/agent/sandbox/macos-seatbelt.ts
- https://gvisor.dev/, https://gvisor.dev/users/, https://gvisor.dev/docs/architecture_guide/performance/
- https://firecracker-microvm.github.io/
- https://e2b.dev/blog/firecracker-vs-qemu, https://e2b.dev/docs/sandbox/persistence, https://e2b.dev/docs/sandbox/internet-access
- https://www.daytona.io/docs/en/sandboxes/, https://www.daytona.io/docs/en/network-limits/
- https://modal.com/docs/guide/sandbox, https://modal.com/docs/guide/sandbox-networking
- https://cloud.morph.so/docs/developers
- https://cymulate.com/blog/cve-2025-53109-53110-escaperoute-anthropic
- https://cybersecuritynews.com/anthropics-mcp-server-vulnerability
- https://www.augmentcode.com/guides/git-worktrees-parallel-ai-agent-execution
- https://developer.upsun.com/posts/ai/git-worktrees-for-parallel-ai-coding-agents
- https://www.penligent.ai/hackinglabs/git-worktrees-need-runtime-isolation-for-parallel-ai-agent-development
- https://github.com/containers/bubblewrap, https://github.com/landlock-lsm/rust-landlock
- https://man7.org/linux/man-pages/man2/seccomp.2.html
- Related: [Host Role and Invocation Mechanics](./host-role-and-invocation.md),
  [Channel Bridge Mechanics](./bridge-mechanics.md),
  [File and Media Pipeline](./file-media-pipeline.md)
