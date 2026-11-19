/**
 * Caller-facing classification of broker and gateway errors.
 *
 * Mirrors `sandbox-core/src/error_class.rs`. The gateway also sends
 * `retryable`, `outcome_unknown` and `hint` on every error frame, and those
 * win over these tables; the tables cover older gateways and errors raised
 * inside this client. Each LeaseError entry sits on one line so the Rust test
 * that compares the two tables can read them.
 */

export interface ErrorClass {
  /** Repeating the request after a backoff may succeed. */
  retryable: boolean;
  /** The request may already have been applied; retry only with the same idempotency key or after checking state. */
  outcomeUnknown: boolean;
  hint: string;
}

const permanent = (hint: string): ErrorClass => ({ retryable: false, outcomeUnknown: false, hint });
const transient = (hint: string): ErrorClass => ({ retryable: true, outcomeUnknown: false, hint });
const unknownOutcome = (hint: string): ErrorClass => ({ retryable: true, outcomeUnknown: true, hint });

/** Every `LeaseError` kind the broker can send. A new Rust variant must be added here. */
export const LEASE_ERROR_KINDS = [
  'quota_exceeded',
  'no_provider_available',
  'invalid_spec',
  'invalid_network_policy',
  'backend',
  'unauthorized',
  'no_worker_satisfies_capabilities',
  'no_worker_can_restore_snapshot',
  'too_many_active_leases',
  'too_many_tagged_leases',
  'snapshot_quota_exceeded',
  'snapshot_tag_not_found',
  'snapshot_unsupported',
  'snapshot_signature_invalid',
  'prewarm_quota_exceeded',
  'image_not_ready',
  'source_lease_not_found',
  'source_lease_not_running',
  'checkpoint_running_unsupported',
  'checkpoint_running_failed',
  'snapshots_disabled',
  'lease_not_found',
  'lease_not_running',
  'invalid_tags',
  'would_exceed_max_lifetime',
  'provider_launch_failed',
  'provider_rejected_dispatch',
  'file_too_large_for_transport',
  'command_output_too_large_for_transport',
  'files_too_large_for_transport',
  'name_conflict',
  'native_passthrough_not_permitted',
  'invalid_native_options',
  'no_worker_in_region',
  'operation_conflict',
  'operation_not_found',
  'operation_canceled',
  'suspend_unsupported',
  'lease_waking',
  'suspend_artifact_lost',
  'entity_unavailable',
  'entity_not_enabled',
] as const;

export type LeaseErrorKind = (typeof LEASE_ERROR_KINDS)[number];

export const LEASE_ERROR_CLASSES: Record<LeaseErrorKind, ErrorClass> = {
  quota_exceeded: permanent('the tenant resource quota is used up; release leases or raise the quota'),
  no_provider_available: transient('no worker is registered for the requested providers; retry once one is online'),
  invalid_spec: permanent('the sandbox spec is invalid; fix the request'),
  invalid_network_policy: permanent('the network policy is invalid; fix the request'),
  backend: transient('a broker backend call failed; retry with the same idempotency key'),
  unauthorized: permanent('the credentials are not allowed to do this'),
  no_worker_satisfies_capabilities: permanent(
    'no worker declares the required capabilities; relax the requirements or pick another provider',
  ),
  no_worker_can_restore_snapshot: permanent(
    'no worker can restore this snapshot; choose a provider that supports its format',
  ),
  too_many_active_leases: permanent('the tenant is at its active lease cap; release a lease before creating another'),
  too_many_tagged_leases: permanent(
    'this user (tag value) is at its active lease cap; release one of its leases first',
  ),
  snapshot_quota_exceeded: permanent('the tenant snapshot quota is used up; delete a snapshot'),
  snapshot_tag_not_found: permanent('the snapshot tag does not exist; check its name'),
  snapshot_unsupported: permanent('the provider does not support snapshots for this lease; use a provider that does'),
  snapshot_signature_invalid: permanent('the snapshot signature failed verification; the artifact is untrusted'),
  prewarm_quota_exceeded: permanent('the tenant prewarm quota is used up'),
  image_not_ready: transient('the image is still being prepared; retry after a short wait'),
  source_lease_not_found: permanent('the source lease does not exist or already ended'),
  source_lease_not_running: permanent('the source lease is not running; resume or recreate it'),
  checkpoint_running_unsupported: permanent('the provider cannot checkpoint a running lease; stop it first'),
  checkpoint_running_failed: transient('checkpointing the running lease failed; retry'),
  snapshots_disabled: permanent('the lease was created with snapshots off; no disk capture of it is allowed'),
  lease_not_found: permanent('the lease does not exist or already ended'),
  lease_not_running: permanent('the lease is not running; resume or recreate it'),
  invalid_tags: permanent('the tags are invalid; fix keys and values'),
  would_exceed_max_lifetime: permanent('the extension would pass the maximum lease lifetime; ask for less'),
  provider_launch_failed: transient('the provider failed to start the sandbox; retry, possibly on another provider'),
  provider_rejected_dispatch: transient('the provider rejected the dispatch; retry, possibly on another provider'),
  file_too_large_for_transport: permanent('the file exceeds the transport payload limit; use chunked transfer'),
  command_output_too_large_for_transport: permanent(
    'the command output exceeds the transport payload limit; stream it or write it to a file',
  ),
  files_too_large_for_transport: permanent('the files exceed the transport payload limit; split the batch'),
  name_conflict: transient(
    'another request is creating a sandbox under this name; retry with the same idempotency key',
  ),
  native_passthrough_not_permitted: permanent(
    'the tenant policy does not allow vendor passthrough; drop `native` or ask an admin to enable it',
  ),
  invalid_native_options: permanent(
    'the worker rejected the native options; use only the keys its documentation lists',
  ),
  no_worker_in_region: transient(
    'no worker serves the requested regions right now; retry later or add fallback regions',
  ),
  operation_conflict: permanent(
    'the operation id belongs to another request or the lease is held by another operation',
  ),
  operation_not_found: permanent('the operation does not exist for this tenant'),
  operation_canceled: permanent('the broker confirmed that the operation was canceled'),
  suspend_unsupported: permanent(
    'the provider cannot suspend to that level; ask for auto or pick a provider that offers it',
  ),
  lease_waking: transient('the lease is waking; retry after the delay in retry_after_ms'),
  suspend_artifact_lost: permanent('the suspend artifact is gone, so the lease cannot wake; recreate the sandbox'),
  entity_unavailable: transient('the sandbox entity store failed; retry with the same idempotency key'),
  entity_not_enabled: permanent(
    'the broker has sandbox entities off for this operation; ask an operator to set SANDBOX_ENTITY_MODE',
  ),
};

/** Every `DriveError` kind. */
export const DRIVE_ERROR_CLASSES: Record<string, ErrorClass> = {
  drive_not_found: permanent('the drive does not exist'),
  drive_already_exists: permanent('a drive with this name already exists'),
  drive_attached: permanent('the drive is attached to another lease; release that lease first'),
  drive_size_exceeded: permanent('the requested drive size is over the limit'),
  too_many_drives: permanent('too many drives for one lease; mount fewer'),
  drive_mount_path_overlap: permanent('two drive mounts overlap; use distinct paths'),
  invalid_drive_mount_path: permanent('the drive mount path is invalid'),
  storage_backend: transient('the drive storage backend failed; retry'),
  unauthorized: permanent('the credentials are not allowed to do this'),
};

/** Every `TerminalError` kind. */
export const TERMINAL_ERROR_CLASSES: Record<string, ErrorClass> = {
  not_found: permanent('no open terminal has this id; it was closed, exited or never opened'),
  id_in_use: permanent('the terminal id is open on another lease; pick a new id'),
  too_many_terminals: permanent('the lease has too many open terminals; close one first'),
  invalid_request: permanent('the terminal request is invalid; fix the request'),
  spawn_failed: transient('the terminal process could not start; retry'),
  exited: permanent("the terminal's process has exited; close it and open a new one"),
};

/** Every `FileOpErrorKind`. */
export const FILE_OP_ERROR_CLASSES: Record<string, ErrorClass> = {
  invalid_path: permanent('the path is malformed or escapes the sandbox filesystem'),
  permission_denied: permanent('the sandbox user may not access this path'),
  already_exists: permanent('the path already exists'),
  not_a_directory: permanent('a path component is not a directory'),
  is_a_directory: permanent('the path is a directory'),
  directory_not_empty: permanent('the directory is not empty'),
  too_large: permanent('the request or result exceeds a size limit'),
  invalid_request: permanent('the request is malformed'),
  io: transient('the filesystem reported an I/O error; retry'),
  unknown_checkpoint: permanent('no such checkpoint on this lease (it may have been evicted); take a new one'),
  unknown_watch: permanent('no such watch on this lease (it may have expired); start a new one'),
};

/** Classification by gateway error `code`, for errors that carry no finer `kind`. */
export const CODE_CLASSES: Record<string, ErrorClass> = {
  lease_denied: permanent('the broker denied the lease request'),
  tenant_mismatch: permanent('the lease belongs to another tenant'),
  timeout: unknownOutcome(
    'no reply before the deadline; the request may have been applied, so retry with the same idempotency key or check state first',
  ),
  transport: unknownOutcome(
    'the connection failed; the request may have been applied, so retry with the same idempotency key or check state first',
  ),
  drive_denied: permanent('the broker denied the drive operation'),
  unsupported: permanent('the provider does not support this operation; use a provider that does'),
  file_op_failed: permanent('the file operation failed'),
  not_found: permanent('the path or resource does not exist'),
  edit_rejected: permanent('the edit target did not match the expected occurrence count; re-read the file'),
  backend: transient('a gateway backend call failed; retry'),
  internal: permanent('the gateway failed internally'),
  invalid_payload: permanent('the request payload is invalid; fix the request'),
  invalid_response: permanent('the gateway reply did not match the expected shape'),
  terminal_failed: permanent('the terminal operation failed'),
  stdin_closed: permanent('stdin is closed'),
  stdin_stalled: permanent('stdin made no progress'),
  lsp_protocol: permanent('the language server broke the protocol'),
  lsp_timeout: unknownOutcome('the language server did not answer in time; it may still apply the request'),
};

export const UNKNOWN_CODE_CLASS: ErrorClass = permanent('unrecognized error; do not retry blindly');

/**
 * Tools that only read, so a timeout or lost connection on them cannot have
 * changed anything and `outcomeUnknown` is false.
 */
export const READ_ONLY_TOOLS: ReadonlySet<string> = new Set([
  'sandbox_get_domain',
  'sandbox_get_lease_by_name',
  'sandbox_list_capabilities',
  'sandbox_list_drives',
  'sandbox_list_leases',
  'sandbox_list_processes',
  'sandbox_native_handle',
  'sandbox_list_snapshots',
  'sandbox_list_tags',
  'sandbox_poll_command',
  'sandbox_read_file',
  'sandbox_snapshot_get',
  'sandbox_snapshot_tag_history',
  'sandbox_snapshot_tree',
  'sandbox_subscribe_dashboard',
  'sandbox_subscribe_evidence',
  'sandbox_wait_ready',
  'sandbox_wait_released',
  'sandbox_wait_sandbox_ready',
]);

export interface ErrorClassInput {
  code: string;
  /** Finer variant name sent by the gateway (`LeaseError` kind and the like). */
  kind?: string;
  retryable?: boolean;
  outcomeUnknown?: boolean;
  hint?: string;
  /** The tool the failed request called, used to tell reads from writes. */
  tool?: string;
}

/** Resolves the classification for an error: wire fields first, then kind, then code. */
export function classify(input: ErrorClassInput): ErrorClass {
  const byKind =
    input.kind === undefined
      ? undefined
      : ((LEASE_ERROR_CLASSES as Record<string, ErrorClass | undefined>)[input.kind] ??
        (input.code === 'drive_denied' ? DRIVE_ERROR_CLASSES[input.kind] : undefined) ??
        (input.code === 'file_op_failed' ? FILE_OP_ERROR_CLASSES[input.kind] : undefined) ??
        (input.code === 'terminal_failed' ? TERMINAL_ERROR_CLASSES[input.kind] : undefined));
  const base = byKind ?? CODE_CLASSES[input.code] ?? UNKNOWN_CODE_CLASS;
  const readOnly = input.tool !== undefined && READ_ONLY_TOOLS.has(input.tool);
  const outcomeUnknown = input.outcomeUnknown ?? base.outcomeUnknown;
  return {
    retryable: input.retryable ?? base.retryable,
    outcomeUnknown: readOnly ? false : outcomeUnknown,
    hint: input.hint ?? base.hint,
  };
}
