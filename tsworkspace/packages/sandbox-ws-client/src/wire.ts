import { z } from 'zod';

import type { Digest, ExecutionId, LeaseId, PullSecretId, ServiceAccountId, StdinSeq, TenantId } from './ids.ts';

export const providerKindSchema = z.enum([
  'k8s_micro_vm',
  'subprocess',
  'local_docker',
  'daytona',
  'modal',
  'e2b',
  'vercel',
  'space',
  'fly',
  'proxmox',
  'hyperlight',
  'v8_wasm',
  'kube_virt',
  'azure_container_apps',
  'substrate',
  'firecracker',
  'mock',
]);

export type ProviderKind = z.infer<typeof providerKindSchema>;

export type IsolationTier = 'container' | 'bwrap' | 'user_space_kernel' | 'micro_vm' | 'full_vm' | 'wasm_micro_vm';

export type LeaseStatus =
  | 'pending'
  | 'running'
  | 'paused'
  | 'suspending'
  | 'suspended'
  | 'waking'
  | 'cancelling'
  | 'released'
  | 'failed';

export type IdleAction = 'pause' | 'release';

export interface IdleStage {
  after_seconds: number;
  action: IdleAction;
}

export interface IdlePolicy {
  stages: IdleStage[];
}

export type LeaseReleasedReason = 'completed' | 'expired' | 'cancelled' | 'idle_timeout';

export type ArtifactType =
  | 'sandbox_rootfs_v1'
  | 'hyperlight_snapshot_v1'
  | 'firecracker_snapshot_v1'
  | 'kubevirt_disk_v1'
  | 'sbom_v1'
  | 'scan_report_v1';

export type VolumeMode = 'read_only' | 'read_write' | 'copy_on_write';

export interface OciImageRef {
  registry: string;
  repository: string;
  tag?: string;
  digest?: Digest;
}

export interface OciArtifactRef {
  registry: string;
  repository: string;
  digest: Digest;
  artifact_type: ArtifactType;
}

export interface SnapshotTag {
  tenant: TenantId;
  name: string;
}

export type SandboxImage =
  | { kind: 'registry'; reference: OciImageRef; pull_secret?: PullSecretId }
  | { kind: 'snapshot'; reference: OciArtifactRef; parent?: OciImageRef }
  | { kind: 'snapshot_tag'; tag: SnapshotTag; artifact_type: ArtifactType }
  | { kind: 'hyperlight_module'; reference: OciArtifactRef }
  | { kind: 'native_snapshot'; reference: NativeSnapshotRef };

/**
 * A snapshot a provider holds itself, with no digest or registry location. Only
 * a worker of `provider` can restore it, and only for the tenant that took it.
 * `expires_at` is unix seconds, absent when the provider reports no lifetime.
 */
export interface NativeSnapshotRef {
  provider: ProviderKind;
  id: string;
  expires_at?: number;
}

export const nativeSnapshotReferenceSchema = z.strictObject({
  provider: providerKindSchema,
  id: z
    .string()
    .min(1)
    .max(128)
    .regex(/^[A-Za-z0-9_.-]+$/),
  expires_at: z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER).optional(),
}) satisfies z.ZodType<NativeSnapshotRef>;

/** What a provider-native snapshot returns in place of an OCI receipt. */
export interface NativeSnapshotReceipt {
  reference: NativeSnapshotRef;
  bytes?: number;
}

export type RemoteMountSource = { backend: 's3'; bucket: string; prefix: string };

export interface RemoteMount {
  source: RemoteMountSource;
  mount_path: string;
  readonly: boolean;
}

export type SecretRef = { nats_kv: { bucket: string; key: string } } | { literal: { value: string } };

export type CloneAuth =
  | { none: Record<string, never> }
  | { bearer_token: { token_ref: SecretRef } }
  | { basic: { username: string; password_ref: SecretRef } };

export type WorkspaceSource =
  | { clone: { url: string; sha: string; base_sha: string | null; auth: CloneAuth } }
  | { host_mount: { path: string; readonly: boolean } }
  | { s3_volume: { bucket: string; prefix: string; mode: VolumeMode; snapshot?: Digest } }
  | { empty: Record<string, never> };

export interface RetentionPolicy {
  max_tags: number;
  max_age_days: number;
}

export interface PrewarmSpec {
  pool_size: number;
  providers: ProviderKind[];
}

export interface SnapshotPolicy {
  artifact_type: ArtifactType;
  target_repository: string;
  retention?: RetentionPolicy;
  prewarm?: PrewarmSpec;
}

export type BootMode = 'entrypoint_default' | 'bare';

export type StringMatcher = { exact: string } | { starts_with: string } | { regex: string };

export interface QueryMatcher {
  key: string;
  value: StringMatcher;
}

export interface HeaderMatcher {
  name: string;
  value: StringMatcher;
}

export interface MatchRule {
  path?: StringMatcher;
  methods?: string[];
  query_string?: QueryMatcher[];
  headers?: HeaderMatcher[];
}

export type CredentialProvider = 'registry' | 's3' | 'daytona' | 'modal' | 'azure' | 'docker' | 'ssh';

export type CredentialScope =
  | { kind: 'registry_pull'; repository_glob: string }
  | { kind: 'registry_push'; repository_glob: string }
  | { kind: 's3_read_only'; bucket: string; prefix: string }
  | { kind: 's3_read_write'; bucket: string; prefix: string }
  | { kind: 'daytona_api'; workspace_id: string }
  | { kind: 'azure_service_principal'; resource_id: string }
  | { kind: 'ssh_session'; lease_id: string };

export interface CredentialRequest {
  provider: CredentialProvider;
  pull_secret_id: PullSecretId;
  scope: CredentialScope;
  ttl_seconds: number;
  requester?: ServiceAccountId;
}

export type CredentialInjection = { header: string } | { query: string };

export interface CredentialTransform {
  match_rules?: MatchRule[];
  credential: CredentialRequest;
  injection: CredentialInjection;
}

export interface RequestForward {
  forward_url: string;
  match_rules?: MatchRule[];
  ttl_seconds?: number;
}

export type DomainRuleAction = 'pass_through' | { transform: CredentialTransform } | { forward: RequestForward };

export interface DomainRule {
  domain: string;
  action?: DomainRuleAction;
}

export type IpRange = string;

/**
 * Whether the egress proxy carries a connection it cannot read (raw bytes after
 * `CONNECT`, SOCKS5, or TLS it relays without terminating). Omitted means
 * `'refuse'`; `'allow'` is the explicit opt-in.
 */
export type OpaqueTunnelPolicy = 'allow' | 'refuse';

export type NetworkPolicy =
  | 'allow_all'
  | 'deny_all'
  | {
      user_defined: {
        rules: DomainRule[];
        allow_ranges: IpRange[];
        deny_ranges: IpRange[];
        opaque_tunnels?: OpaqueTunnelPolicy;
      };
    };

export interface DriveId {
  tenant: TenantId;
  name: string;
}

export type DriveMountMode = 'read_write' | 'read_only';

export interface DriveMount {
  drive_id: DriveId;
  mount_path: string;
  mode?: DriveMountMode;
}

export interface WasiPreopenSpec {
  host_dir: string;
  guest_path: string;
  readonly?: boolean;
}

export interface WasiEgressRule {
  host: string;
  port: number;
}

export interface WasmCapabilityGrant {
  preopens?: WasiPreopenSpec[];
  allowed_egress?: WasiEgressRule[];
}

export type SecretTarget = { env: string } | { file: string };

/** How a credential is located inside a file or env value so it can be masked. */
export type SecretExtract =
  | { regex: string }
  | { json_key: string[] }
  | { env_line: string }
  | { jwt: { mask_claims?: string[]; pattern?: string } };

/**
 * A credential the broker holds and the sandbox must never see in the clear.
 * Mirrors `sandbox_core::secret_binding::SecretBinding`; the real value is
 * fetched through `source` at runtime and is never part of this object.
 */
export interface SecretBinding {
  name: string;
  target: SecretTarget;
  mode: 'mask' | 'deny';
  extract?: SecretExtract;
  mask_duplicates?: boolean;
  on_extract_no_match?: 'error' | 'warn' | 'deny';
  strip_response_headers?: string[];
  inject_hosts: string[];
  source: CredentialRequest;
}

export interface SandboxSpec {
  image: SandboxImage;
  workspace: WorkspaceSource;
  env: Record<string, string>;
  cpu_seconds: number;
  memory_mib: number;
  /** Sustained CPU share in thousandths of a vCPU (500 is half a vCPU). Refused when no worker advertises a range containing it. */
  vcpu_millis?: number;
  /** Writable disk quota in MiB. Refused when no worker advertises a disk quota. */
  disk_mib?: number;
  snapshot_out?: SnapshotPolicy;
  network_policy?: NetworkPolicy;
  mounts?: RemoteMount[];
  drives?: DriveMount[];
  boot_mode?: BootMode;
  /** Ports `sandbox.domain(port)` may resolve. Up to 15, 1 to 65535. */
  exposed_ports?: number[];
  /** Credentials masked or withheld from the sandbox. Up to 64. */
  secret_bindings?: SecretBinding[];
  working_dir?: string;
  user?: string;
  privileged?: boolean;
  wasi_capabilities?: WasmCapabilityGrant;
  /** Releases the lease after this many seconds without activity. Enforced by the broker for every provider. */
  idle_timeout_seconds?: number;
  /**
   * Lifts the wall-clock deadline: the lease runs until cancelled or idle.
   * Needs `idle_timeout_seconds` (or an `idle_policy` release stage) and a broker
   * started with no-TTL leases allowed.
   */
  no_ttl?: boolean;
  /**
   * Staged idle handling, measured from the last activity. Set this or
   * `idle_timeout_seconds`, not both. Stages must have strictly increasing
   * `after_seconds` and strictly more severe actions (pause, then release).
   */
  idle_policy?: IdlePolicy;
  /** Per-lease data retention. Omitted means both toggles on. */
  retention?: RetentionToggles;
}

/**
 * Per-lease retention toggles. `monitoring: false` purges the lease's command
 * history when it ends; `snapshots: false` refuses every disk capture of it
 * (snapshot, fork, `snapshot_out`, automatic named persistence).
 */
export interface RetentionToggles {
  monitoring?: boolean;
  snapshots?: boolean;
}

export type HostImport = {
  kind: 'fetch_url';
  allow_domains: string[];
  max_body_bytes: number;
  timeout_seconds: number;
};

const capabilityU32 = z.number().int().min(0).max(0xffff_ffff);

export const capabilityRequirementsSchema = z.strictObject({
  needs_snapshot: z.boolean().default(false),
  needs_checkpoint_running: z.boolean().default(false),
  needs_restore: z.boolean().default(false),
  min_isolation_tier: z
    .enum(['container', 'bwrap', 'user_space_kernel', 'micro_vm', 'full_vm', 'wasm_micro_vm'])
    .optional(),
  max_cold_restore_p50_ms: capabilityU32.optional(),
  needs_warm_pool: z.boolean().default(false),
  needs_named_tags: z.boolean().default(false),
  needs_fuse_mounts: z.boolean().default(false),
  needs_privileged: z.boolean().optional(),
  needs_sandbox_users: z.boolean().optional(),
  needs_interactive_stdin: z.boolean().optional(),
  needs_named_processes: z.boolean().optional(),
  needs_output_streaming: z.boolean().optional(),
  needs_process_readiness: z.boolean().optional(),
  needs_graceful_stop: z.boolean().optional(),
  needs_command_restart: z.boolean().optional(),
  needs_port_preview: z.boolean().optional(),
  exposed_port_count: z.number().int().min(0).max(65535).optional(),
  needs_domain_allowlist: z.boolean().optional(),
  min_egress_enforcement: z.enum(['none', 'best_effort', 'mandatory']).optional(),
  needs_tls_intercept: z.boolean().optional(),
  needs_secret_substitution: z.boolean().optional(),
  needs_secret_bindings: z.boolean().optional(),
  needs_masked_files: z.boolean().optional(),
  needs_fs_policy: z.boolean().optional(),
  needs_pause_resume: z.boolean().optional(),
  needs_suspend: z.enum(['memory', 'disk']).optional(),
  needs_file_ops: z.boolean().optional(),
  needs_file_search: z.boolean().optional(),
  needs_file_archive: z.boolean().optional(),
  needs_output_redaction: z.boolean().optional(),
  needs_usage_gauges: z.boolean().optional(),
  needs_terminal: z.boolean().optional(),
  needs_network_policy_update: z.boolean().optional(),
  needs_sandbox_groups: z.boolean().optional(),
  vcpu_millis: capabilityU32.optional(),
  disk_mib: capabilityU32.optional(),
  needs_cidr_allowlist: z.boolean().optional(),
  needs_hardening_overrides: z.boolean().optional(),
  needs_native_passthrough: z.boolean().optional(),
  env_digest: z
    .string()
    .regex(/^sha256:[0-9a-f]{64}$/)
    .optional(),
  region: z.string().min(1).optional(),
  needs_change_tracking: z.boolean().optional(),
  needs_file_watch: z.boolean().optional(),
  needs_persistent_runtime: z.boolean().optional(),
  needs_dir_transfer: z.boolean().optional(),
  needs_browser: z.boolean().optional(),
  lease_lifetime_seconds: capabilityU32.optional(),
});

export type CapabilityRequirements = z.output<typeof capabilityRequirementsSchema>;
export type CapabilityRequirementsInput = z.input<typeof capabilityRequirementsSchema>;

/** Caller-defined key-value metadata; the broker rejects more than 5 entries. */
export type SandboxTags = Record<string, string>;

export interface LeaseRequest {
  operation_id?: string;
  tenant: TenantId;
  provider_pref: ProviderKind[];
  spec: SandboxSpec;
  ttl_seconds: number;
  idempotency_key: string;
  isolation_tier?: IsolationTier;
  host_imports: HostImport[];
  required_capabilities?: CapabilityRequirements;
  prewarm: boolean;
  tags?: SandboxTags;
  name?: string;
  /** Defaults to true server-side; only meaningful when `name` is set. */
  persistent?: boolean;
  /**
   * Vendor passthrough keyed by provider kind, for example
   * `{ daytona: { auto_stop_interval: 30 } }`. Denied unless the tenant is
   * allowed it; only the named provider's worker sees its entry.
   */
  native?: Record<string, Record<string, unknown>>;
  /** Region to place the sandbox in. Absent means any worker. */
  region?: string;
  /** Regions to try, in order, when no worker serves `region`. Needs `region`. */
  fallback_regions?: string[];
}

export interface WireSystemTime {
  secs_since_epoch: number;
  nanos_since_epoch: number;
}

export function wireSystemTimeToDate(t: WireSystemTime): Date {
  return new Date(t.secs_since_epoch * 1000 + t.nanos_since_epoch / 1e6);
}

export interface LeaseGranted {
  lease_id: LeaseId;
  provider: ProviderKind;
  sandbox_id: string;
  evidence_subject: string;
  deadline: WireSystemTime;
  /** True only when this response created the lease; absent from older brokers. */
  created?: boolean;
  /**
   * Optional features the lease's provider supports (see `SandboxFeature`).
   * Absent until the broker reports them; `Sandbox.capabilities` is then undefined.
   */
  capabilities?: string[];
  /**
   * The durable operation that created this lease, when the request carried an
   * `operation_id`. Absent otherwise and from older brokers.
   */
  operation_id?: string;
}

export interface ProviderCapabilityEntry {
  kind: ProviderKind;
  isolation_tiers: IsolationTier[];
  live_workers: number;
  worker_capabilities: number;
}

export interface CapabilitiesResponse {
  providers: ProviderCapabilityEntry[];
}

export interface LeaseListRow {
  lease_id: string;
  name?: string;
  tenant: string;
  provider: ProviderKind;
  image: SandboxImage;
  state: LeaseStatus;
  started_at: number;
  ttl_seconds: number;
  tags?: SandboxTags;
  idle_timeout_seconds?: number | null;
  /** Unix seconds of the last activity the broker saw; null until the first one. */
  last_activity_at?: number | null;
}

/** Filters for `sandbox_reap_leases`. At least one of `tags`, `older_than_seconds`, `idle_for_seconds` is required. */
export interface ReapLeasesOptions {
  tags?: SandboxTags;
  older_than_seconds?: number;
  idle_for_seconds?: number;
  /** Report what would be released without releasing anything. */
  dry_run?: boolean;
}

export interface ReapedLeaseSummary {
  lease_id: string;
  tenant: string;
  provider: ProviderKind;
  status: LeaseStatus;
  tags?: SandboxTags;
}

export type ReapLeasesResponse =
  | {
      outcome: 'reaped';
      dry_run: boolean;
      matched: ReapedLeaseSummary[];
      released: string[];
      failed: { lease_id: string; message: string }[];
    }
  | { outcome: 'denied'; denied: unknown };

export interface SnapshotListRow {
  digest: string;
  artifact_type: ArtifactType;
  tenant: string;
  published_at: number | null;
  bytes: number | null;
  registry: string;
  repository: string;
  lease_id: string;
  kv_key: string;
}

export interface SnapshotTagRow {
  tenant: string;
  name: string;
  digest: string;
  artifact_type: ArtifactType;
  set_at: number;
}

export interface SnapshotPublishedEvent {
  digest: string;
  artifact_type: ArtifactType;
  tenant: string;
  published_at: number;
  bytes: number;
  lease_id: string;
}

export interface WorkerCapabilitySummary {
  can_snapshot: boolean;
  can_restore: boolean;
  can_checkpoint_running: boolean;
  supports_warm_pool: boolean;
  supports_named_tags: boolean;
  isolation_tiers: IsolationTier[];
}

export interface WorkerHeartbeatEvent {
  provider: ProviderKind;
  worker_id: string;
  capability_summary: WorkerCapabilitySummary;
  cached_digests_count: number;
  cached_digests: string[];
  last_heartbeat: number;
  current_leases: number;
  max_concurrent_leases: number;
}

export type LeaseUpsertAction = 'delete' | 'upsert';

export interface LeaseDashboardEvent {
  action: LeaseUpsertAction;
  lease: LeaseListRow;
}

export interface RunCommandRequest {
  lease_id: LeaseId;
  command: string[];
  cwd?: string;
  env?: Record<string, string>;
  timeout_seconds?: number;
  detached?: boolean;
  /** When `true`, the worker publishes incremental output chunks to the evidence stream as the command runs. */
  stream_output?: boolean;
  run_as_user?: string;
  /** `interactive` requires `detached: true`. Defaults to `closed`. */
  stdin?: StdinMode;
  /** Defaults to `buffer`. */
  output_retention?: OutputRetention;
  /** `lease` means no per-command timeout and requires `detached: true`. Defaults to `timeout`. */
  deadline?: CommandDeadline;
  /** Per-stream retention cap in bytes; the broker clamps it to its ceiling. Defaults to 1 MiB. */
  max_output_bytes?: number;
  /**
   * Names the command and optionally attaches a readiness probe. Requires
   * `detached: true` and a provider whose profile has
   * `supports_named_processes` (and `supports_process_readiness` for a probe).
   */
  process?: ProcessSpec;
  /**
   * Literal values replaced with `[REDACTED]` in stdout, stderr and error text before
   * they are returned or streamed. Exact bytes only: encoded or transformed copies are
   * not caught, values under 6 bytes are ignored. Requires a provider whose capability
   * profile has `supports_output_redaction`; never logged or stored by the broker.
   */
  redact_values?: string[];
  /**
   * 1 to 128 characters of `[A-Za-z0-9_-]`. A repeat with a key the broker already
   * saw for this lease replays the first launch's reply instead of running again;
   * reusing it for a different command is refused.
   */
  launch_key?: string;
  /** Requires provider-owned cancellation authority before command acknowledgement. */
  require_launch_cancellation?: boolean;
}

/** A name matches `[A-Za-z0-9._-]{1,64}` and is unique per lease while its process runs. */
export interface ProcessSpec {
  name?: string;
  readiness?: ReadinessProbe;
}

export type ReadinessProbe = { kind: 'tcp_port'; port: number } | { kind: 'output_contains'; needle: string };

export type ProcessInfo = {
  execution_id: string;
  name?: string;
  readiness?: ReadinessProbe;
} & ({ state: 'running' } | { state: 'exited'; exit_code: number });

export interface ListProcessesRequest {
  lease_id: LeaseId;
}

export type ListProcessesResponse = { outcome: 'processes'; processes: ProcessInfo[] } | { outcome: 'unsupported' };

export interface WaitReadyRequest {
  lease_id: LeaseId;
  execution_id: string;
  /** Defaults to 30; the worker caps it at 300. */
  timeout_seconds?: number;
}

export type WaitReadyResponse =
  | { outcome: 'ready' }
  | { outcome: 'not_ready' }
  | { outcome: 'failed'; exit_code: number }
  | { outcome: 'no_probe' }
  | { outcome: 'not_found' }
  | { outcome: 'unsupported' };

/** Most ports one sandbox readiness request may name. */
export const MAX_READY_PORTS = 16;
/** Longest the broker blocks one sandbox readiness request, in seconds. */
export const MAX_WAIT_SANDBOX_READY_SECONDS = 300;

export interface WaitSandboxReadyRequest {
  lease_id: LeaseId;
  /** Sandbox-side ports that must accept connections: at most 16, non-zero, no duplicates. */
  ports?: number[];
  /** Defaults to 30; the broker caps it at 300. */
  timeout_seconds?: number;
}

export type WaitSandboxReadyResponse =
  | { outcome: 'ready' }
  /** `pending_ports` is empty while the lease itself was still pending. */
  | { outcome: 'not_ready'; pending_ports: number[] }
  | { outcome: 'unsupported' };

export interface KillProcessRequest {
  lease_id: LeaseId;
  name: string;
  signal?: UnixSignal;
}

export type CommandDeadline = 'timeout' | 'lease';

export type StdinMode = 'closed' | 'interactive';

/**
 * `stream_only` keeps no stdout on the worker: polling returns an empty
 * stdout and only the last 64 KiB of stderr.
 */
export type OutputRetention = 'buffer' | 'stream_only';

export const MAX_STDIN_CHUNK_BYTES = 64 * 1024;

export interface WriteStdinRequest {
  lease_id: LeaseId;
  execution_id: ExecutionId;
  seq: StdinSeq;
  data: number[];
  eof: boolean;
}

export type WriteStdinResponse =
  | { outcome: 'written'; next_seq: StdinSeq }
  | { outcome: 'duplicate'; next_seq: StdinSeq }
  | { outcome: 'buffered'; next_seq: StdinSeq }
  | { outcome: 'out_of_window'; next_seq: StdinSeq }
  | { outcome: 'backpressure'; next_seq: StdinSeq }
  | { outcome: 'stdin_closed' }
  | { outcome: 'stdin_not_interactive' }
  | { outcome: 'already_completed' }
  | { outcome: 'not_found' }
  | { outcome: 'unsupported' }
  | { outcome: 'denied' };

export interface CommandOutput {
  exit_code: number;
  stdout: number[];
  stderr: number[];
  /** The command hit its deadline and was killed; `exit_code` is -1. */
  timed_out?: boolean;
  /** stdout or stderr exceeded the retention cap and its middle was dropped. */
  truncated?: boolean;
}

export const commandLaunchOriginSchema = z.enum(['created', 'reused', 'unknown']);
export type CommandLaunchOrigin = z.infer<typeof commandLaunchOriginSchema>;

export type RunCommandResponse =
  | ({ outcome: 'completed' } & CommandOutput)
  | { outcome: 'started'; execution_id: string; origin?: CommandLaunchOrigin };

export interface PollCommandRequest {
  lease_id: LeaseId;
  execution_id: string;
}

export type PollCommandResponse =
  | { outcome: 'running' }
  | ({ outcome: 'completed' } & CommandOutput)
  | { outcome: 'not_found' };

export interface SandboxFileWrite {
  path: string;
  content: number[];
  mode?: number;
}

export interface WriteFilesRequest {
  lease_id: LeaseId;
  files: SandboxFileWrite[];
  as_user?: string;
}

export type WriteFilesResponse = { outcome: 'written' };

export interface ReadFileRequest {
  lease_id: LeaseId;
  path: string;
  as_user?: string;
}

/**
 * `not_found` is the typed reply for a missing file. Older gateways fail the
 * request with a message instead, which `Sandbox.readFile` still recognises.
 */
export type ReadFileResponse = { outcome: 'content'; data: number[] } | { outcome: 'not_found' };

export interface CreateSandboxUserRequest {
  lease_id: LeaseId;
  username: string;
}

export type CreateSandboxUserResponse =
  | { outcome: 'created'; home_dir: string }
  | { outcome: 'already_exists'; home_dir: string };

export interface CreateSandboxGroupRequest {
  lease_id: LeaseId;
  groupname: string;
}

export type CreateSandboxGroupResponse =
  | { outcome: 'created'; shared_dir: string }
  | { outcome: 'already_exists'; shared_dir: string };

export type GroupMembershipAction = 'add' | 'remove';

export interface ModifyGroupMembershipRequest {
  lease_id: LeaseId;
  username: string;
  groupname: string;
  action: GroupMembershipAction;
}

export type ModifyGroupMembershipResponse = { outcome: 'applied' };

export type UnixSignal = 'term' | 'kill' | 'int' | 'hup' | 'quit' | 'usr1' | 'usr2';

export interface StopLeaseRequest {
  lease_id: LeaseId;
}

export type StopLeaseResponse = { outcome: 'stopped' };

/** `unsupported` means the provider cannot pause a lease in place. */
export type PauseLeaseResponse = { outcome: 'paused'; reap_at: WireSystemTime } | { outcome: 'unsupported' };

/** Where a captured snapshot lives; `reference` is a ready-made snapshot image source. */
export interface LeaseSnapshotReceipt {
  reference: OciArtifactRef;
  bytes: number;
}

/**
 * `unsupported` means the provider cannot checkpoint a running lease.
 * `created_native` is sent only when the request set `accept_native`.
 */
export type SnapshotLeaseResponse =
  | { outcome: 'created'; receipt: LeaseSnapshotReceipt }
  | { outcome: 'created_native'; receipt: NativeSnapshotReceipt }
  | { outcome: 'unsupported' };

/** `unsupported` means the broker has no snapshot registry access. */
export type SnapshotDeleteResponse =
  | { outcome: 'deleted'; bytes: number }
  | { outcome: 'not_found' }
  | { outcome: 'unsupported' };

export const nativeSnapshotDeleteResponseSchema = z.discriminatedUnion('outcome', [
  z.strictObject({ outcome: z.literal('deleted') }),
  z.strictObject({ outcome: z.literal('not_found') }),
  z.strictObject({ outcome: z.literal('unsupported') }),
]);

export type NativeSnapshotDeleteResponse = z.infer<typeof nativeSnapshotDeleteResponseSchema>;

export type ResumeLeaseResponse = { outcome: 'resumed'; deadline: WireSystemTime } | { outcome: 'unsupported' };

/** How deep a suspend goes: `memory` keeps processes alive, `disk` keeps only the disk. */
export type SuspendLevel = 'memory' | 'disk';

/** `auto` takes whatever level the provider offers; the others must match it exactly. */
export type SuspendRequestLevel = 'auto' | SuspendLevel;

/**
 * `level` is where the suspend landed and `reap_at` is when the broker
 * releases the lease if nobody wakes it. A provider that cannot suspend to the
 * requested level is refused as a `lease_denied` error of kind `suspend_unsupported`.
 */
export type SuspendLeaseResponse = {
  outcome: 'suspended';
  level: SuspendLevel;
  reap_at: WireSystemTime;
  operation_id?: string;
};

/** A lease another request is waking is refused as `lease_waking`, which carries `retry_after_ms`. */
export type WakeLeaseResponse = {
  outcome: 'woken';
  deadline: WireSystemTime;
  operation_id?: string;
};

export interface DeleteLeaseRequest {
  lease_id: LeaseId;
}

export type DeleteLeaseResponse = { outcome: 'deleted' };

export interface ExtendLeaseTimeoutRequest {
  lease_id: LeaseId;
  additional_seconds: number;
}

export type ExtendLeaseTimeoutResponse = { outcome: 'extended'; new_deadline: WireSystemTime };

export interface LeaseForkRequest {
  source_lease_id: LeaseId;
  ttl_seconds: number;
  idempotency_key: string;
}

export interface NetworkPolicyUpdateRequest {
  lease_id: LeaseId;
  network_policy: NetworkPolicy;
}

export type NetworkPolicyUpdateResponse = { outcome: 'applied' };

export interface KillCommandRequest {
  lease_id: LeaseId;
  execution_id: string;
  signal?: UnixSignal;
}

export type KillCommandResponse = { outcome: 'signaled' | 'already_completed' | 'not_found' };

/** Default grace period the broker applies when none is given, in seconds. */
export const DEFAULT_STOP_GRACE_SECONDS = 10;
/** The broker clamps longer grace periods to this many seconds. */
export const MAX_STOP_GRACE_SECONDS = 300;

export interface StopCommandRequest {
  lease_id: LeaseId;
  execution_id: string;
  grace_seconds?: number;
}

/**
 * `stopped.signal` is `term` when the command exited within the grace period
 * and `kill` when it had to be killed. `unsupported` means the provider does
 * not declare `supports_graceful_stop`.
 */
export type StopCommandResponse =
  | { outcome: 'stopped'; signal: 'term' | 'kill' }
  | { outcome: 'already_completed' }
  | { outcome: 'not_found' }
  | { outcome: 'unsupported' };

export interface RestartCommandRequest {
  lease_id: LeaseId;
  execution_id: string;
  grace_seconds?: number;
}

/** `restarted.execution_id` names the new execution; the old one is finished. */
export type RestartCommandResponse =
  | { outcome: 'restarted'; execution_id: string }
  | { outcome: 'not_found' }
  | { outcome: 'unsupported' };

/** Default and maximum page size of `readLogs`, in bytes. */
export const DEFAULT_READ_LOGS_BYTES = 64 * 1024;
export const MAX_READ_LOGS_BYTES = 512 * 1024;

export interface ReadLogsRequest {
  lease_id: LeaseId;
  execution_id: string;
  after_seq?: number;
  max_bytes?: number;
  /** Long-poll: wait up to this long for a chunk when none is retained yet. The broker caps it. */
  wait_ms?: number;
}

export interface LogChunk {
  /** Durable cursor: pass the last one seen (or `next_seq`) as `after_seq`. */
  log_seq: number;
  stream: 'stdout' | 'stderr';
  /** The worker's own sequence number; not unique across streams. */
  seq: number;
  data: number[];
}

/**
 * `next_seq` is the `after_seq` of the following page. `has_more` is set when
 * the page stopped at `max_bytes`; `truncated` when retention already dropped
 * the start of the output. `exited` is set once the worker announced the exit,
 * so a page with it and no `has_more` is the last; older brokers omit it.
 * `unsupported` means the broker keeps no durable logs.
 */
export type ReadLogsResponse =
  | {
      outcome: 'logs';
      chunks: LogChunk[];
      next_seq: number;
      has_more: boolean;
      truncated: boolean;
      exited?: boolean;
    }
  | { outcome: 'unsupported' };

/**
 * One event of a `sandbox_follow_logs` stream, delivered on the stream id
 * `logs:<lease_id>:<execution_id>`. `chunk.log_seq` is the resume cursor.
 * `end` finishes the stream once the command has exited and all output was
 * sent; `unsupported` and `error` finish it too. `reconnecting` says the
 * gateway is retrying a failed read from the same cursor (nothing is lost or
 * repeated); `resync_required` says retention dropped output between
 * `from_cursor` and the next chunk.
 */
export type LogFollowEvent =
  | { type: 'chunk'; log_seq: number; stream: 'stdout' | 'stderr'; seq: number; data: number[] }
  | { type: 'end'; next_seq: number; truncated: boolean }
  | { type: 'unsupported' }
  | { type: 'reconnecting'; attempt: number }
  | { type: 'resync_required'; from_cursor: number }
  | { type: 'error'; code: string; message: string };

export interface UpdateTagsRequest {
  lease_id: LeaseId;
  tags: SandboxTags;
}

export type UpdateTagsResponse = { outcome: 'applied' };

export interface DriveId {
  tenant: TenantId;
  name: string;
}

export interface DriveRecord {
  id: DriveId;
  tenant: TenantId;
  size_gib: number;
  created_at: WireSystemTime;
  attached_lease?: LeaseId;
  /** Set for an S3-backed drive. */
  bucket?: string;
  /** Set for a drive backed by a provider-native volume. */
  provider_volume?: ProviderVolumeRef;
}

/** An existing provider-native volume, named by the provider and its own volume id. */
export interface ProviderVolumeRef {
  provider: ProviderKind;
  volume_id: string;
}

/** Name the drive's storage with exactly one of `bucket` or `provider_volume`. */
export interface CreateDriveRequest {
  drive_name: string;
  size_gib?: number;
  bucket?: string;
  provider_volume?: ProviderVolumeRef;
}

export type CreateDriveResponse = DriveRecord;

export type ListDrivesResponse = { drives: DriveRecord[] };

export interface DeleteDriveRequest {
  drive_name: string;
}

export type DeleteDriveResponse = { deleted: true };

export interface GetSandboxDomainRequest {
  lease_id: LeaseId;
  port: number;
  /** Ask the gateway to hold the reply until the port answers HTTP, up to this many milliseconds (capped at 120000). */
  wait_ms?: number;
}

/** How to reach a published port when a bare `host:port` is not enough. `headers` may carry a secret. */
export interface PortAccess {
  base_url: string;
  headers?: Record<string, string>;
}

/** The vendor's own handle for a sandbox, from `sandbox_native_handle`. */
export type NativeHandleResponse =
  | {
      outcome: 'available';
      provider: ProviderKind;
      vendor_id: string;
      url?: string;
      region?: string;
    }
  | { outcome: 'unsupported' }
  | { outcome: 'denied' };

/** Where and how to open an SSH session to a sandbox, from `sandbox_ssh_access`. `user` may be a credential. */
export interface SshAccess {
  host: string;
  port: number;
  user: string;
  /** The provider's ready-made `ssh` command line, when it gives one. */
  command?: string;
  /** Unix seconds at which the access stops working. */
  expires_at?: number;
}

export type SshAccessResponse =
  | ({ outcome: 'available' } & SshAccess)
  | { outcome: 'unsupported' }
  | { outcome: 'denied' };

export type GetSandboxDomainResponse =
  | {
      outcome: 'available';
      endpoint: string;
      /** Authenticated, tenant-bound preview URL. Present only when the gateway has preview access enabled. */
      url?: string;
      /** Unix seconds at which `url` stops working. Present together with `url`. */
      expires_at?: number;
      /** Set by providers that front the port with an authenticated URL (for example Daytona's preview URL plus token header). */
      access?: PortAccess;
    }
  | { outcome: 'port_not_exposed' }
  | { outcome: 'unsupported' }
  /** The request carried `wait_ms` and the port did not answer in time. */
  | { outcome: 'not_ready'; waited_ms: number }
  | { outcome: 'denied' };

/**
 * Row projection of a lease returned by `sandbox_get_lease_by_name`. Distinct
 * from {@link LeaseListRow} (the `sandbox_list_leases` projection): the two
 * tools are backed by different server-side shapes.
 */
export interface LeaseByNameRow {
  lease_id: string;
  tenant: string;
  provider: ProviderKind;
  status: LeaseStatus;
  created_at: WireSystemTime;
  deadline: WireSystemTime;
  name?: string;
  tags?: SandboxTags;
}

export interface DeleteNamedSandboxRequest {
  name: string;
}

/** `not_found` means no lease, snapshot tag or suspended actor existed under the name. */
export type DeleteNamedSandboxResponse = { outcome: 'deleted' } | { outcome: 'not_found' };

/** How a caller names an entity: by id, or by its tenant-scoped name. */
export type EntityRef = { id: string } | { name: string };

/** What the broker reports of a persistent sandbox entity. The spec template is never included. */
export interface SandboxEntityRow {
  entity_id: string;
  tenant: string;
  name?: string;
  created_at: WireSystemTime;
  metadata: Record<string, string>;
  drives: unknown[];
  head?: unknown;
  /** `{ state: 'running', lease }` while a lease runs it; the other states carry no lease. */
  state: unknown;
  generation: number;
  current_lease?: string;
}

/** `not_found` also answers an id owned by another tenant. */
export type GetEntityResponse = { outcome: 'found'; row: SandboxEntityRow } | { outcome: 'not_found' };

export interface ListEntitiesOptions {
  limit?: number;
  /** The previous page's `next_cursor`. */
  cursor?: string;
}

export interface ListEntitiesResponse {
  outcome: 'listed';
  rows: SandboxEntityRow[];
  /** Absent on the last page. */
  next_cursor?: string;
}

export type DeleteEntityResponse = { outcome: 'deleted' } | { outcome: 'not_found' };

export interface GetLeaseByNameRequest {
  name: string;
}

export type GetLeaseByNameResponse = { outcome: 'found'; row: LeaseByNameRow } | { outcome: 'not_found' };

/**
 * Row projection of a snapshot returned by `sandbox_snapshot_get` and
 * `sandbox_snapshot_tree`. Distinct from {@link SnapshotListRow} (the
 * `sandbox_list_snapshots` projection): the two tools are backed by different
 * server-side shapes.
 */
export interface SnapshotDetailRow {
  digest: string;
  tenant: string;
  artifact_type: ArtifactType;
  created_at: WireSystemTime;
  parent?: string;
}

export interface SnapshotGetRequest {
  digest: Digest;
}

export type SnapshotGetResponse = { outcome: 'found'; row: SnapshotDetailRow } | { outcome: 'not_found' };

export interface SnapshotTreeNode {
  digest: string;
  parent?: string;
  artifact_type: ArtifactType;
  created_at: WireSystemTime;
}

export interface SnapshotTreeRequest {
  digest: Digest;
}

export type SnapshotTreeResponse = { nodes: SnapshotTreeNode[] };

export interface SetSnapshotTagRequest {
  name: string;
  digest: Digest;
}

export type SetSnapshotTagResponse = { outcome: 'applied' } | { outcome: 'denied' };

export interface DeleteSnapshotTagRequest {
  name: string;
}

export type DeleteSnapshotTagResponse = { outcome: 'applied' } | { outcome: 'not_found' } | { outcome: 'denied' };

export interface SnapshotTagAuditEntry {
  name: string;
  digest: string;
  set_at: WireSystemTime;
}

export interface SnapshotTagHistoryRequest {
  name: string;
}

export type SnapshotTagHistoryResponse = { entries: SnapshotTagAuditEntry[] };

export type OutputStream = 'stdout' | 'stderr';

/**
 * One incremental chunk of a streamed command's output, published to the
 * lease's evidence stream under the `command.<execution_id>` subject suffix
 * (see `SandboxWsClient.evidenceStream`). Only emitted when the command was
 * started with `sandbox_run_command`'s `stream_output: true`. `seq` is
 * counted per stream: stdout and stderr each start at 0.
 */
export interface CommandOutputChunk {
  execution_id: string;
  stream: OutputStream;
  seq: number;
  data: number[];
}

export type WireFileKind = 'file' | 'directory' | 'symlink' | 'other';

/** Metadata for one filesystem entry, as returned by the `file_op` verb. */
export interface WireFileStat {
  path: string;
  name: string;
  kind: WireFileKind;
  size: number;
  mode?: number;
  modified_unix_ms?: number;
  symlink_target?: string;
}

export interface WireSearchMatch {
  path: string;
  line?: number;
  text?: string;
}

export type FileOpErrorKind =
  | 'invalid_path'
  | 'permission_denied'
  | 'already_exists'
  | 'not_a_directory'
  | 'is_a_directory'
  | 'directory_not_empty'
  | 'too_large'
  | 'invalid_request'
  | 'io'
  | 'unknown_checkpoint'
  | 'unknown_watch';

export type WireFileWatchEventKind = 'created' | 'modified' | 'deleted';

/** One change a watch observed. `seq` increases by one per event within a watch. */
export interface WireFileWatchEvent {
  seq: number;
  kind: WireFileWatchEventKind;
  path: string;
  is_dir: boolean;
}

/** One structured file operation. Byte payloads (`data`) are base64 strings. */
export type FileOp =
  | { op: 'list'; path: string; recursive?: boolean; max_entries?: number }
  | { op: 'stat'; path: string }
  | { op: 'read'; path: string; offset?: number; length?: number; max_bytes?: number }
  | {
      op: 'write';
      path: string;
      data: string;
      offset?: number;
      truncate?: boolean;
      mode?: number;
      create_parents?: boolean;
    }
  | { op: 'edit'; path: string; find: string; replace: string; expected_occurrences?: number }
  | {
      op: 'search';
      path: string;
      glob?: string;
      pattern?: string;
      regex?: boolean;
      case_insensitive?: boolean;
      max_results?: number;
      max_file_bytes?: number;
    }
  | { op: 'remove'; path: string; recursive?: boolean }
  | { op: 'mkdir'; path: string; parents?: boolean }
  | { op: 'rename'; from: string; to: string; overwrite?: boolean }
  | { op: 'archive_get'; path: string; max_bytes?: number }
  | { op: 'archive_put'; path: string; data: string }
  | { op: 'checkpoint'; path: string; hash?: boolean }
  | { op: 'changes'; since?: string; path?: string }
  | { op: 'watch_start'; path: string; recursive?: boolean }
  | { op: 'watch_poll'; watch_id: string; after_seq?: number; wait_ms?: number }
  | { op: 'watch_stop'; watch_id: string }
  | { op: 'checksum'; path: string; offset?: number; length?: number };

export interface FileOpRequest {
  lease_id: LeaseId;
  op: FileOp;
  as_user?: string;
}

export type FileOpResponse =
  | { outcome: 'listing'; entries: WireFileStat[]; truncated: boolean }
  | { outcome: 'stat'; stat: WireFileStat }
  | { outcome: 'content'; data: string; offset: number; total_size: number; eof: boolean; truncated: boolean }
  | { outcome: 'written'; bytes: number; size: number }
  | { outcome: 'edited'; occurrences: number; size: number }
  | { outcome: 'edit_rejected'; occurrences: number; expected: number }
  | { outcome: 'search_results'; matches: WireSearchMatch[]; truncated: boolean; files_scanned: number }
  | { outcome: 'removed'; existed: boolean }
  | { outcome: 'created'; already_existed: boolean }
  | { outcome: 'renamed' }
  | { outcome: 'archive'; data: string; entries: number; bytes: number }
  | { outcome: 'extracted'; entries: number; bytes: number }
  | { outcome: 'checkpointed'; checkpoint_id: string; entries: number }
  | {
      outcome: 'changes';
      since: string;
      added: string[];
      modified: string[];
      deleted: string[];
      truncated: boolean;
    }
  | { outcome: 'watch_started'; watch_id: string }
  | { outcome: 'watch_events'; events: WireFileWatchEvent[]; next_seq: number; overflowed: boolean }
  | { outcome: 'watch_stopped'; existed: boolean }
  | { outcome: 'checksum'; sha256: string; bytes: number; total_size: number }
  | { outcome: 'not_found'; path: string }
  | { outcome: 'failed'; kind: FileOpErrorKind; message: string }
  | { outcome: 'unsupported'; message: string };

/** Cpu and memory figures a provider reported; each is absent when it cannot. */
export interface ResourceGauges {
  cpu_nanos_total?: number;
  memory_bytes?: number;
  memory_peak_bytes?: number;
  memory_limit_bytes?: number;
}

/**
 * Usage the broker recorded for one lease. A released lease reports its final
 * figures (`status` is `released` and `ended_at` is set).
 */
export interface LeaseUsage {
  lease_id: string;
  tenant: string;
  status: LeaseStatus;
  created_at: WireSystemTime;
  ended_at?: WireSystemTime;
  last_activity_at: WireSystemTime;
  commands_run: number;
  command_output_bytes: number;
  file_bytes_in: number;
  file_bytes_out: number;
  /** Seconds since creation, excluding time spent paused. */
  running_seconds: number;
  paused_seconds: number;
  /** Absent from brokers that predate suspend. */
  suspended_seconds?: number;
  /** Bytes the suspend artifact takes now; absent unless suspended. */
  suspend_artifact_bytes?: number;
  /** Present only when the provider reports gauges and answered. */
  gauges?: ResourceGauges;
}

/** Live leases allowed per value of one tag key, with the current counts. */
export interface TagLimits {
  tag_key: string;
  max_active_leases: number;
  /** Live leases per tag value; values with none are omitted. */
  in_use: Record<string, number>;
}

/** The tenant's broker-side limits next to what it has consumed. */
export interface AccountLimits {
  active_leases: number;
  max_active_leases: number;
  /** Cpu seconds reserved this billing month and the monthly cap. */
  cpu_seconds_used: number;
  cpu_seconds_cap: number;
  /** Wall seconds reserved this billing month and the monthly cap. */
  wall_seconds_used: number;
  wall_seconds_cap: number;
  max_cpu_seconds_per_request: number;
  max_memory_mib_per_request: number;
  /** Present when the tenant has a per-tag live lease cap. */
  per_tag?: TagLimits;
}

/**
 * Counters summed over a tenant's live leases and the released leases whose
 * final figures the broker still keeps.
 */
export interface TenantUsage {
  live_leases: number;
  /** Released leases folded into the totals. Absent from older brokers. */
  ended_leases?: number;
  commands_run: number;
  command_output_bytes: number;
  file_bytes_in: number;
  file_bytes_out: number;
  running_seconds: number;
  paused_seconds: number;
  /** Absent from brokers that predate suspend. */
  suspended_seconds?: number;
}

export interface TerminalOpenRequest {
  lease_id: LeaseId;
  cols: number;
  rows: number;
  /** Program and arguments; empty runs the sandbox's default shell. */
  cmd?: string[];
  env?: Record<string, string>;
  cwd?: string;
}

/** Output of an interactive terminal. `data` is base64 of the raw PTY bytes. */
export type TerminalOutputEvent =
  | { type: 'data'; seq: number; data: string }
  | { type: 'exit'; seq: number; exit_code?: number }
  /**
   * Sent on a reattach when the worker no longer holds the output the reader missed. Output
   * before `from_seq` is gone: reset the screen and treat `from_seq - 1` as the high-water mark.
   */
  | { type: 'resync'; from_seq: number };

export const MAX_TERMINAL_INPUT_BYTES = 64 * 1024;
