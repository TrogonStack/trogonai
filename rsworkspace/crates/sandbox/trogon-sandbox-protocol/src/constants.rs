use std::time::Duration;

pub(crate) const MAX_RECEIPT_BYTES: usize = 64 * 1024;
pub(crate) const HAS_EPOCH_COLUMN: &str =
    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('attempts') WHERE name='epoch')";
pub(crate) const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";
pub(crate) const HAS_LEASE: &str = "SELECT EXISTS(SELECT 1 FROM attempts WHERE lease_id=?1)";
/// Verbs the broker sends to the worker holding a lease, as the fourth token of
/// `sandbox.provider.<provider>.<verb>.<lease>`.
pub(crate) const LEASE_VERBS: &[&str] = &[
    "cancel",
    "cancel_owned",
    "checkpoint_running",
    "network_policy_update",
    "run_command",
    "get_domain",
    "native_handle",
    "ssh_access",
    "pause",
    "resume",
    "rollback",
    "suspend",
    "wake",
    "resource_usage",
    "kill_command",
    "cancel_command_launch",
    "stop_command",
    "restart_command",
    "write_stdin",
    "terminal",
    "write_files",
    "read_file",
    "file_op",
    "poll_command",
    "list_processes",
    "wait_ready",
    "create_user",
    "create_group",
    "modify_group_membership",
];
pub(crate) const MAX_READINESS_LEASE_LEN: usize = 1024;
/// How often a detached command's watcher asks the agent whether it exited.
pub const DETACHED_EXIT_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Consecutive failed polls after which the watcher stops and reports the
/// exit as unknown. The agent going quiet this long means the sandbox is gone,
/// and a follower is better told so than left waiting.
pub const DETACHED_EXIT_POLL_FAILURES: u32 = 20;
pub const CONVENTIONAL_AWS_ACCESS_KEY_ID: &str = "AWS_ACCESS_KEY_ID";
pub const CONVENTIONAL_AWS_SECRET_ACCESS_KEY: &str = "AWS_SECRET_ACCESS_KEY";
pub const CONVENTIONAL_AWS_SESSION_TOKEN: &str = "AWS_SESSION_TOKEN";
/// The NATS KV bucket the control plane commits each lease's credential policy to, and the credential broker and
/// workers read it from.
pub const COMMITTED_POLICY_BUCKET: &str = "sandbox-committed-credential-policies";
/// JSON field the broker adds to a signed dispatch.
pub const AUTH_FIELD: &str = "dispatch_auth";
/// Operation name of a lease launch dispatch.
pub const OP_LEASE_DISPATCH: &str = "lease_dispatch";
/// Header the broker adds to a signed per-lease request.
pub const AUTH_HEADER: &str = "Sandbox-Dispatch-Auth";
/// Environment variable naming the file that holds the broker's base64 signing seed.
pub const ENV_SIGNING_KEY_FILE: &str = "SANDBOX_DISPATCH_SIGNING_KEY_FILE";
/// Environment variable holding the broker's base64 signing seed.
pub const ENV_SIGNING_KEY: &str = "SANDBOX_DISPATCH_SIGNING_KEY";
/// Environment variable naming the signing key.
pub const ENV_SIGNING_KEY_ID: &str = "SANDBOX_DISPATCH_SIGNING_KEY_ID";
/// Environment variable holding the signed envelope lifetime in seconds.
pub const ENV_SIGNING_TTL_SECS: &str = "SANDBOX_DISPATCH_TTL_SECS";
/// Environment variable naming the file that holds the workers' public keys.
pub const ENV_PUBLIC_KEYS_FILE: &str = "SANDBOX_DISPATCH_PUBLIC_KEYS_FILE";
/// Environment variable holding the workers' public keys.
pub const ENV_PUBLIC_KEYS: &str = "SANDBOX_DISPATCH_PUBLIC_KEYS";
/// Environment variable holding the verification [`EnforcementMode`](crate::EnforcementMode).
pub const ENV_VERIFY_MODE: &str = "SANDBOX_DISPATCH_VERIFY";
/// How the worker treats a request whose lease epoch is older than the held one. Defaults to `warn` when unset.
pub const ENV_FENCE_EPOCH: &str = "SANDBOX_FENCE_EPOCH";
/// File that keeps the lease-to-tenant fence across a worker restart.
pub const ENV_FENCE_FILE: &str = "SANDBOX_DISPATCH_FENCE_FILE";
pub(crate) const FENCE_FILE_NAME: &str = "dispatch-fence.jsonl";
pub(crate) const DEFAULT_TTL: Duration = Duration::from_secs(300);
pub(crate) const CLOCK_SKEW: Duration = Duration::from_secs(30);
pub(crate) const SIGNING_DOMAIN: &str = "sandbox-dispatch-auth-v1";
/// Header `nats_transfer` sets when the body is a descriptor of a larger one.
pub(crate) const TRANSFER_HEADER: &str = "Sandbox-Transfer";
pub(crate) const RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);
pub(crate) const CAPACITY: usize = 100_000;
/// Journal lines beyond the live entries that trigger a rewrite on open.
pub(crate) const COMPACT_SLACK: usize = 1024;
pub(crate) const MAX_NONCES: usize = 100_000;
/// What an operator is told about an allowed-domain entry that is refused.
pub const ALLOWED_DOMAIN_ENTRY_MESSAGE: &str = "Invalid domain pattern. Must be a valid domain (e.g., \"example.com\"), a wildcard (e.g., \"*.example.com\"), or a bracketed IPv6 literal (e.g., \"[::1]\", \"[2001:db8::1]:443\"). Overly broad patterns like \"*.com\" or \"*\" are not allowed for security reasons. An optional \":port\" suffix (1-65535) restricts the entry to that port.";
/// What an operator is told about a denied-domain entry that is refused.
pub const DENIED_DOMAIN_ENTRY_MESSAGE: &str = "Invalid domain pattern. Must be a valid domain (e.g., \"example.com\"), a wildcard (e.g., \"*.example.com\"), or a bracketed IPv6 literal (e.g., \"[::1]\", \"[2001:db8::1]:443\"). Overly broad patterns like \"*.com\" or \"*\" are not allowed for security reasons. In deniedDomains a bare \"*\" (deny-all) is also accepted, and an optional \":port\" suffix (1-65535) restricts the entry to that port.";
/// Domain separator hashed before the encoded definition.
pub(crate) const DOMAIN: &[u8] = b"trogon.sandbox.environment.v1\n";
pub(crate) const DASH: u16 = b'-' as u16;
pub(crate) const SLASH: u16 = b'/' as u16;
pub(crate) const STAR_UNIT: u16 = b'*' as u16;
pub(crate) const OPEN: u16 = b'[' as u16;
pub(crate) const CLOSE: u16 = b']' as u16;
pub(crate) const QUESTION: u16 = b'?' as u16;
/// Most bytes of one stream the guest returns; beyond it the middle is cut in
/// the guest and reported as dropped.
pub const GUEST_STREAM_LIMIT: usize = 16 * 1024 * 1024;
/// What the guest script prints in place of the exit status when the deadline
/// stopped the command. GNU `timeout` reports a stop as 124 or 137; busybox
/// `timeout` execs the command in its own process and reports whatever the
/// signal leaves (143 for TERM, 137 for KILL). A command can return any of
/// those itself, so the script does not read them: the shell that runs the
/// command traps TERM and writes a marker file, and the script reports the
/// marker. That shell also writes an "armed" file once the trap is in place, so
/// a TERM that lands before the trap (the command never started) still reads
/// as a timeout rather than as 143.
pub(crate) const TIMED_OUT_REPLY: &str = "timeout";
/// Signals every process whose environment carries a marker. Invoke it as
/// `sh -c <script> sh KEY=VALUE <SIGNAL>`; it exits 0 when at least one
/// process was signaled. Children inherit the marker, so the whole process
/// tree of a command started with it in its environment is covered. The scan
/// repeats after a short pause, signaling only processes it has not signaled
/// yet, so a child forked after one pass listed `/proc`, or by a signal
/// handler, is caught by the next instead of being orphaned with the
/// command's output still open.
pub const SIGNAL_BY_MARKER_SCRIPT: &str = r#"marker="$1"; sig="$2"; self=$$; found=1; seen=" "; pass=0
while [ "$pass" -lt 5 ]; do
  new=1
  for environ in /proc/[0-9]*/environ; do
    pid=${environ#/proc/}; pid=${pid%/environ}
    [ "$pid" = "$self" ] && continue
    case "$seen" in *" $pid "*) continue ;; esac
    if tr '\0' '\n' < "$environ" 2>/dev/null | grep -qxF "$marker"; then
      kill "-$sig" "$pid" 2>/dev/null && { found=0; new=0; seen="$seen$pid "; }
    fi
  done
  [ "$new" = 0 ] || break
  pass=$((pass+1))
  sleep 0.1 2>/dev/null || sleep 1
done
exit $found"#;
/// How often workers publish a heartbeat unless they have a reason to differ.
pub const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
/// The port an omitted port means under `InjectScopeVersion::V2`.
pub const DEFAULT_INJECT_PORT: u16 = 443;
pub(crate) const FAKE_EXPIRY: u64 = 9_999_999_999;
/// Signature segment of every fake token.
pub(crate) const FAKE_SIGNATURE: &str = "ZmFrZS1zaWduYXR1cmU";
pub(crate) const ESCAPE: u8 = b'=';
pub(crate) const SEPARATOR: char = '_';
/// How long one connection attempt may take.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const TRANSFER_VERSION: &str = "chunks-v1";
/// The largest payload a transfer carries, in encoded bytes.
pub const MAX_TRANSFER_BYTES: usize = 128 * 1024 * 1024;
pub(crate) const TRANSFER_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const MAX_CHUNK_BYTES: usize = 64 * 1024;
pub(crate) const METADATA_RESERVE: usize = 1024;
/// The tunnel cap used when `NetworkOptions` sets none.
pub const DEFAULT_TUNNEL_CAP: u32 = 256;
/// Per-stream retention applied when a request carries no `max_output_bytes`.
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 1024 * 1024;
/// Encoding room a command reply reserves for both streams' truncation markers: a marker is under 64 bytes, and the
/// number-array encoding costs up to four bytes per byte.
pub const COMMAND_REPLY_MARKER_ROOM: usize = 2 * 64 * 4;
/// What replaces every redacted occurrence.
pub const REDACTION_MARKER: &str = "[REDACTED]";
/// Shortest registered value, in bytes, that is redacted. Shorter values are dropped when a `RedactionSet` is built.
pub const MIN_REDACTABLE_LEN: usize = 6;
/// What every sentinel starts with.
pub const SENTINEL_PREFIX: &str = "fake_value_";
pub(crate) const PADDING_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_-";
pub(crate) const ROOT: &str = "snapshots";
pub(crate) const HEX_SEGMENT: &str = "tenant-hex";
pub const LEASE_REQUEST: &str = "sandbox.lease.request";
/// Client-facing request to fork a running lease into a new, independent one.
pub const LEASE_FORK_REQUEST: &str = "sandbox.lease.fork";
/// Wildcard subscription for all per-lease release notifications.
pub const LEASE_RELEASED_FILTER: &str = "sandbox.lease.released.>";
pub const SNAPSHOT_PUBLISHED: &str = "sandbox.snapshot.published";
pub const SNAPSHOT_DELETED: &str = "sandbox.snapshot.deleted";
pub const SNAPSHOT_ACCOUNTING_REQUEST: &str = "sandbox.snapshot.account.published";
pub const SNAPSHOT_LIVE_KV_BUCKET: &str = "sandbox-snapshot-live-v1";
pub const SNAPSHOT_ARTIFACT_KV_BUCKET: &str = "sandbox-snapshot-accounting-v1";
pub const SNAPSHOT_PUBLICATION_RECEIPT_KV_BUCKET: &str = "sandbox-snapshot-publication-v1";
/// JetStream KV bucket holding live snapshot digest records (logical namespace
/// `sandbox.broker.kv.snapshot.{digest}`).
pub const SNAPSHOT_KV_BUCKET: &str = "sandbox-broker-kv-snapshot";
/// JetStream KV bucket for tenant-scoped prewarm policy and controller state.
///
/// Logical path: `sandbox.broker.kv.prewarm.{tenant}.{digest}`.
pub const PREWARM_KV_BUCKET: &str = "sandbox-broker-kv-prewarm";
/// Broker subscription for every `lease_cancel` subject.
pub const LEASE_CANCEL_FILTER: &str = "sandbox.lease.cancel.*";
/// Wildcard subscription for `lease_tags_update` requests.
pub const LEASE_TAGS_UPDATE_FILTER: &str = "sandbox.lease.tags.update.*";
/// Wildcard subscription for `lease_network_policy_update` requests.
pub const LEASE_NETWORK_POLICY_UPDATE_FILTER: &str = "sandbox.lease.network_policy_update.*";
/// Client-facing request/reply to look up one durable operation by id (see
/// [`trogon_sandbox_contract::GetOperationRequest`]). Tenant scoped through the
/// request body: another tenant's operation id answers `NotFound`.
pub const OPERATION_GET_REQUEST: &str = "sandbox.operation.get";
pub const OPERATION_CANCEL_REQUEST: &str = "sandbox.operation.cancel";
pub const OPERATION_CAPABILITIES_REQUEST: &str = "sandbox.operation.capabilities";
pub const DRIVE_CREATE_REQUEST: &str = "sandbox.drive.create";
pub const DRIVE_LIST_REQUEST: &str = "sandbox.drive.list";
pub const DRIVE_DELETE_REQUEST: &str = "sandbox.drive.delete";
/// Tenant-facing request to list provider capability profiles known to the
/// broker's worker registry (one profile per provider with a live heartbeat).
pub const CAPABILITY_LIST_REQUEST: &str = "sandbox.capability.list";
/// Client-facing request/reply to look up a single snapshot by digest (see
/// [`trogon_sandbox_contract::SnapshotGetRequest`]). Not scoped by lease id: snapshots are
/// tenant-owned, not lease-owned.
pub const SNAPSHOT_GET_REQUEST: &str = "sandbox.snapshot.get";
/// Client-facing request/reply to delete a snapshot by digest (see
/// [`trogon_sandbox_contract::SnapshotDeleteRequest`]).
pub const SNAPSHOT_DELETE_REQUEST: &str = "sandbox.snapshot.delete";
pub const NATIVE_SNAPSHOT_DELETE_REQUEST: &str = "sandbox.native_snapshot.delete";
/// Client-facing request/reply to walk a snapshot digest's lineage tree (see
/// [`trogon_sandbox_contract::SnapshotTreeRequest`]).
pub const SNAPSHOT_TREE_REQUEST: &str = "sandbox.snapshot.tree";
/// Client-facing request/reply to point a named snapshot tag at a digest (see
/// [`trogon_sandbox_contract::SetSnapshotTagRequest`]).
pub const SNAPSHOT_TAG_SET_REQUEST: &str = "sandbox.snapshot.tag.set";
/// Client-facing request/reply to remove a named snapshot tag (see
/// [`trogon_sandbox_contract::DeleteSnapshotTagRequest`]).
pub const SNAPSHOT_TAG_DELETE_REQUEST: &str = "sandbox.snapshot.tag.delete";
/// Client-facing request/reply to list a named snapshot tag's history (see
/// [`trogon_sandbox_contract::SnapshotTagHistoryRequest`]).
pub const SNAPSHOT_TAG_HISTORY_REQUEST: &str = "sandbox.snapshot.tag.history";
/// Client-facing request/reply to permanently delete a sandbox by its
/// tenant-scoped `name`, including when it has no active lease (see
/// [`trogon_sandbox_contract::DeleteNamedSandboxRequest`]). Not scoped by lease id: unlike
/// `lease_delete`, this must still find the sandbox after its lease is gone.
pub const DELETE_NAMED_SANDBOX_REQUEST: &str = "sandbox.named.delete";
/// Client-facing request/reply to look up a lease by its tenant-scoped `name`
/// (see [`trogon_sandbox_contract::GetLeaseByNameRequest`]). Not scoped by lease id since
/// the lookup is by name.
pub const LEASE_GET_BY_NAME_REQUEST: &str = "sandbox.lease.get_by_name";
/// Client-facing request/reply to list a tenant's leases, paginated by an
/// opaque cursor (see [`trogon_sandbox_contract::ListLeasesRequest`]).
pub const LEASE_LIST_REQUEST: &str = "sandbox.lease.list";
/// Client-facing request/reply to release a tenant's leases by tag, age or
/// idleness, with a dry-run mode (see [`trogon_sandbox_contract::ReapLeasesRequest`]).
pub const LEASE_REAP_REQUEST: &str = "sandbox.lease.reap";
/// Wildcard subscription for `lease_extend_timeout` requests.
pub const LEASE_EXTEND_TIMEOUT_FILTER: &str = "sandbox.lease.extend_timeout.*";
/// Wildcard subscription for `lease_delete` requests.
pub const LEASE_DELETE_FILTER: &str = "sandbox.lease.delete.*";
/// Wildcard subscription for `lease_stop` requests.
pub const LEASE_STOP_FILTER: &str = "sandbox.lease.stop.*";
/// Wildcard subscription for `lease_pause` requests.
pub const LEASE_PAUSE_FILTER: &str = "sandbox.lease.pause.*";
/// Wildcard subscription for `lease_snapshot` requests.
pub const LEASE_SNAPSHOT_FILTER: &str = "sandbox.lease.snapshot.*";
/// Wildcard subscription for `lease_resume` requests.
pub const LEASE_RESUME_FILTER: &str = "sandbox.lease.resume.*";
/// Wildcard subscription for `lease_suspend` requests.
pub const LEASE_SUSPEND_FILTER: &str = "sandbox.lease.suspend.*";
/// Wildcard subscription for `lease_rollback` requests.
pub const LEASE_ROLLBACK_FILTER: &str = "sandbox.lease.rollback.*";
/// Wildcard subscription for `lease_wake` requests.
pub const LEASE_WAKE_FILTER: &str = "sandbox.lease.wake.*";
/// Wildcard subscription for `lease_usage` requests.
pub const LEASE_USAGE_FILTER: &str = "sandbox.lease.usage.*";
/// Client-facing request for a tenant's usage summed over its live leases (see
/// [`trogon_sandbox_contract::GetTenantUsageRequest`]).
pub const TENANT_USAGE_REQUEST: &str = "sandbox.tenant.usage";
/// Client-facing request for a tenant's limits and current consumption (see
/// [`trogon_sandbox_contract::GetAccountLimitsRequest`]).
pub const TENANT_LIMITS_REQUEST: &str = "sandbox.tenant.limits";
/// Wildcard subscription for `lease_run_command` requests.
pub const LEASE_RUN_COMMAND_FILTER: &str = "sandbox.lease.run_command.*";
pub const LEASE_CANCEL_COMMAND_LAUNCH_FILTER: &str = "sandbox.lease.cancel_command_launch.*";
/// Wildcard subscription for `lease_kill_command` requests.
pub const LEASE_KILL_COMMAND_FILTER: &str = "sandbox.lease.kill_command.*";
/// Wildcard subscription for `lease_stop_command` requests.
pub const LEASE_STOP_COMMAND_FILTER: &str = "sandbox.lease.stop_command.*";
/// Wildcard subscription for `lease_restart_command` requests.
pub const LEASE_RESTART_COMMAND_FILTER: &str = "sandbox.lease.restart_command.*";
/// Wildcard subscription for `lease_read_logs` requests.
pub const LEASE_READ_LOGS_FILTER: &str = "sandbox.lease.read_logs.*";
/// Wildcard subscription for `lease_write_stdin` requests.
pub const LEASE_WRITE_STDIN_FILTER: &str = "sandbox.lease.write_stdin.*";
/// Wildcard subscription for `lease_terminal` requests.
pub const LEASE_TERMINAL_FILTER: &str = "sandbox.lease.terminal.*";
/// Wildcard subscription for `worker_lease_activity` signals.
pub const WORKER_LEASE_ACTIVITY_FILTER: &str = "sandbox.worker.activity.*";
/// Wildcard subscription for `worker_authority_check` requests.
pub const WORKER_AUTHORITY_FILTER: &str = "sandbox.worker.authority.*";
/// Wildcard subscription for `lease_get_domain` requests.
pub const LEASE_GET_DOMAIN_FILTER: &str = "sandbox.lease.get_domain.*";
/// Wildcard subscription for `lease_native_handle` requests.
pub const LEASE_NATIVE_HANDLE_FILTER: &str = "sandbox.lease.native_handle.*";
/// Wildcard subscription for `lease_ssh_access` requests.
pub const LEASE_SSH_ACCESS_FILTER: &str = "sandbox.lease.ssh_access.*";
/// Wildcard subscription for `lease_poll_command` requests.
pub const LEASE_POLL_COMMAND_FILTER: &str = "sandbox.lease.poll_command.*";
/// Wildcard subscription for `lease_list_processes` requests.
pub const LEASE_LIST_PROCESSES_FILTER: &str = "sandbox.lease.list_processes.*";
/// Wildcard subscription for `lease_wait_ready` requests.
pub const LEASE_WAIT_READY_FILTER: &str = "sandbox.lease.wait_ready.*";
/// Wildcard subscription for `lease_wait_sandbox_ready` requests.
pub const LEASE_WAIT_SANDBOX_READY_FILTER: &str = "sandbox.lease.wait_sandbox_ready.*";
/// Wildcard subscription for `lease_write_files` requests.
pub const LEASE_WRITE_FILES_FILTER: &str = "sandbox.lease.write_files.*";
/// Wildcard subscription for `lease_read_file` requests.
pub const LEASE_READ_FILE_FILTER: &str = "sandbox.lease.read_file.*";
/// Wildcard subscription for `lease_file_op` requests.
pub const LEASE_FILE_OP_FILTER: &str = "sandbox.lease.file_op.*";
/// Wildcard subscription for `lease_create_user` requests.
pub const LEASE_CREATE_USER_FILTER: &str = "sandbox.lease.create_user.*";
/// Wildcard subscription for `lease_create_group` requests.
pub const LEASE_CREATE_GROUP_FILTER: &str = "sandbox.lease.create_group.*";
/// Wildcard subscription for `lease_modify_group_membership` requests.
pub const LEASE_MODIFY_GROUP_MEMBERSHIP_FILTER: &str = "sandbox.lease.modify_group_membership.*";
/// Wildcard subscription for `egress_decide` requests.
pub const EGRESS_DECIDE_FILTER: &str = "sandbox.egress.decide.*";
/// Wildcard subscription for `egress_request_decide` requests.
pub const EGRESS_REQUEST_DECIDE_FILTER: &str = "sandbox.egress.request.*";
/// Wildcard subscription for `egress_violation` events.
pub const EGRESS_VIOLATION_FILTER: &str = "sandbox.egress.violation.*";
/// Wildcard subscription for `fs_violation` events.
pub const FS_VIOLATION_FILTER: &str = "sandbox.fs.violation.*";
/// Subject prefix of the credential audit events. The JetStream stream that
/// stores them is named by `credential_audit_stream_name`, because stream
/// names may not contain `.`.
pub const CREDENTIAL_AUDIT_STREAM: &str = "sandbox.audit.credentials";
pub const SNAPSHOT_TAG_AUDIT_STREAM: &str = "sandbox.audit.snapshot-tag";
/// JetStream KV bucket for tenant snapshot tag records.
///
/// Logical path per tag: `sandbox.broker.kv.snapshot.tag.{tenant}.{name}`.
pub const SNAPSHOT_TAG_KV_BUCKET: &str = "sandbox-snapshot-tag";
/// JetStream KV bucket recording digest resolved from a named tag at lease dispatch.
///
/// Logical path: `sandbox.broker.kv.lease.{lease_id}.resolved_digest`.
pub const LEASE_RESOLVED_DIGEST_KV_BUCKET: &str = "sandbox-lease-resolved-digest";
/// JetStream KV bucket mapping a tenant-scoped lease `name` to the lease ID
/// currently holding it, so a later request with the same name can reconnect
/// to (or resume from a snapshot tagged for) the same logical sandbox.
///
/// Logical path: `sandbox.broker.kv.lease.name.{tenant}.{name}`.
pub const LEASE_NAME_INDEX_KV_BUCKET: &str = "sandbox-lease-name-index";
/// JetStream KV bucket recording the last time each snapshot digest was used
/// to resolve a lease, so retention age can reset on use rather than only on
/// tag reassignment.
pub const SNAPSHOT_LAST_USED_KV_BUCKET: &str = "sandbox-snapshot-last-used";
/// JetStream KV bucket recording which tenant took each provider-native
/// snapshot, so a restore is limited to the tenant's own.
pub const NATIVE_SNAPSHOT_KV_BUCKET: &str = "sandbox-native-snapshots";
/// JetStream KV bucket recording whether a lease's snapshot capture succeeded
/// or failed, so failed captures are queryable per-lease instead of only
/// visible via logs and aggregate metrics.
///
/// Logical path: `sandbox.broker.kv.lease.{lease_id}.capture_status`.
pub const SNAPSHOT_CAPTURE_STATUS_KV_BUCKET: &str = "sandbox-snapshot-capture-status";
/// JetStream KV bucket for tenant drive records.
///
/// Logical path per drive: `sandbox.broker.kv.drive.{tenant}.{name}`.
pub const DRIVE_KV_BUCKET: &str = "sandbox-drive";
/// JetStream KV bucket for persistent sandbox entity records.
///
/// Key per entity: `e.{hex(tenant)}.{ulid}`.
pub const ENTITY_KV_BUCKET: &str = "sandbox-entities";
/// JetStream stream recording entity lifecycle events, capped per subject.
pub const ENTITY_EVENT_STREAM: &str = "sandbox-entity-events";
/// Subject prefix of entity events: `sandbox.entity.event.{hex(tenant)}.{ulid}`.
pub const ENTITY_EVENT_SUBJECT_PREFIX: &str = "sandbox.entity.event";
/// Client-facing request/reply to read one entity by id or name (see
/// [`trogon_sandbox_contract::GetEntityRequest`]). Not scoped by lease id: an entity
/// outlives its leases.
pub const ENTITY_GET_REQUEST: &str = "sandbox.entity.get";
/// Client-facing request/reply to list a tenant's entities (see
/// [`trogon_sandbox_contract::ListEntitiesRequest`]).
pub const ENTITY_LIST_REQUEST: &str = "sandbox.entity.list";
/// Client-facing request/reply to delete an entity (see
/// [`trogon_sandbox_contract::DeleteEntityRequest`]).
pub const ENTITY_DELETE_REQUEST: &str = "sandbox.entity.delete";
/// JetStream KV bucket mapping (tenant, name) to an entity id.
///
/// Key per name: `n.{hex(tenant)}.{hex(name)}`.
pub const ENTITY_NAME_INDEX_KV_BUCKET: &str = "sandbox-entity-names";
/// JetStream KV bucket for per-lease usage counters, keyed by lease id. Kept
/// beside the active-lease bucket so every broker replica reads the same
/// figures, and retained after release so a released lease reports its final
/// figures until the bucket ages them out.
pub const LEASE_USAGE_KV_BUCKET: &str = "sandbox-lease-usage";
/// JetStream KV bucket holding a marker for every delete-by-id whose purge has
/// not finished, keyed by lease id. The marker outlives the active-lease
/// record, so a retried delete and the broker's reconciler can still find the
/// name whose snapshot tag must go.
pub const LEASE_PENDING_PURGE_KV_BUCKET: &str = "sandbox-lease-pending-purge";
/// JetStream KV bucket holding a marker for every lease release that removed,
/// or is about to remove, the active-lease record and has not finished
/// refunding quota and announcing, keyed by lease id. Kept apart from the
/// pending-purge bucket so a release that is not a purge is never mistaken for
/// one, which would remove state a plain release keeps on purpose.
pub const LEASE_PENDING_RELEASE_KV_BUCKET: &str = "sandbox-lease-pending-release";
pub const CREDENTIAL_TOKEN_BUCKET: &str = "sandbox_credentials_kv_token";
/// The documented default ladder of Substrate pools.
pub const DEFAULT_SUBSTRATE_SIZE_CLASSES: &str = "small=2048,medium=8192,large=32768";
/// Largest chunk a worker publishes as one output frame, in bytes before base64, so a frame stays well under the
/// message payload limit.
pub const MAX_TERMINAL_OUTPUT_CHUNK_BYTES: usize = 16 * 1024;
/// Most terminals a worker keeps open for one lease.
pub const MAX_TERMINALS_PER_LEASE: usize = 8;
/// The environment variable a worker reads to size each terminal's replay ring, in bytes.
///
/// Unset or unparsable means `TerminalReplayRing::DEFAULT_CAPACITY`, `0` turns replay off, and larger values are
/// clamped to `TerminalReplayRing::MAX_CAPACITY`.
pub const TERMINAL_REPLAY_BYTES_ENV: &str = "SANDBOX_TERMINAL_REPLAY_BYTES";
/// The trust-store environment variable of each common TLS client.
///
/// Several tools ignore `SSL_CERT_FILE`, and some are pre-set to a system bundle in the parent environment, so each
/// one is set explicitly.
pub const CA_TRUST_ENV_VARS: [&str; 11] = [
    "SSL_CERT_FILE",
    "NODE_EXTRA_CA_CERTS",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "GIT_SSL_CAINFO",
    "PIP_CERT",
    "AWS_CA_BUNDLE",
    "CARGO_HTTP_CAINFO",
    "DENO_CERT",
    "CLOUDSDK_CORE_CUSTOM_CA_CERTS_FILE",
    "NIX_SSL_CERT_FILE",
];
/// How a worker reacts to leases the broker does not list as its own.
pub const ENV_WORKER_AUTHORITY: &str = "SANDBOX_WORKER_AUTHORITY";
/// Most lease ids one request may carry; a worker splits larger sets.
pub const MAX_LEASES_PER_REQUEST: usize = 10_000;
/// Time between authority checks.
pub const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_secs(15);
/// Failed checks for this long make every held lease suspect.
pub const DEFAULT_PARTITION_AFTER: Duration = Duration::from_secs(45);
/// Time between freezing a suspect lease and purging it.
pub const DEFAULT_GRACE: Duration = Duration::from_secs(60);
/// A lease younger than this is never frozen on the broker's say-so.
pub const MIN_LEASE_AGE: Duration = Duration::from_secs(30);
/// Consecutive checks in which the broker must disown a lease.
pub const CONFIRMATIONS: u32 = 2;
/// How long one authority request may take.
pub(crate) const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const MAX_LEGACY_BYTES: u64 = 64 * 1024 * 1024;
