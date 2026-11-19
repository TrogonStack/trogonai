import { classify } from './error-classes.ts';
import { scrubCause, scrubSecrets } from './scrub.ts';

export interface RestoreCleanupReceipt {
  stoppedExecutionIds: readonly string[];
  incompleteExecutionIds: readonly string[];
}

export interface CommandCleanupReceipt {
  executionId?: string;
  cleanupPending: boolean;
}

/** Extra context an error can carry. Unset classification fields come from the tables in `error-classes.ts`. */
export interface SandboxWsErrorDetails {
  retryable?: boolean;
  outcomeUnknown?: boolean;
  hint?: string;
  /** Finer variant name from the gateway, such as a `LeaseError` kind (`quota_exceeded`). */
  variant?: string;
  provider?: string;
  leaseId?: string;
  executionId?: string;
  launchKey?: string;
  restoreCleanup?: RestoreCleanupReceipt;
  commandCleanup?: CommandCleanupReceipt;
  /** The tool the failed request called. */
  tool?: string;
  /** Milliseconds the server asked the caller to wait before repeating (`Retry-After`). */
  retryAfterMs?: number;
  cause?: unknown;
}

/**
 * Base class of every error the client raises. The message and any `cause` are
 * scrubbed of credentials on construction (see `scrubSecrets`), so a log line
 * that prints an error cannot carry a token across a tenant boundary.
 */
export class SandboxWsError extends Error {
  readonly code: string;
  /** Repeating the request after a backoff may succeed. */
  readonly retryable: boolean;
  /**
   * The request may already have been applied (a timeout or lost connection on
   * a mutating call). Retry only with the same idempotency key or after
   * checking state.
   */
  readonly outcomeUnknown: boolean;
  /** One-line remediation advice. */
  readonly hint: string;
  readonly variant?: string;
  readonly provider?: string;
  readonly leaseId?: string;
  readonly executionId?: string;
  readonly launchKey?: string;
  readonly restoreCleanup?: RestoreCleanupReceipt;
  readonly commandCleanup?: CommandCleanupReceipt;
  /** Milliseconds the server asked the caller to wait before repeating; retry helpers never wait less. */
  readonly retryAfterMs?: number;

  constructor(code: string, message: string, details: SandboxWsErrorDetails = {}) {
    super(scrubSecrets(message), details.cause === undefined ? undefined : { cause: scrubCause(details.cause) });
    this.name = 'SandboxWsError';
    this.code = code;
    const cls = classify({
      code,
      kind: details.variant,
      retryable: details.retryable,
      outcomeUnknown: details.outcomeUnknown,
      hint: details.hint,
      tool: details.tool,
    });
    this.retryable = cls.retryable;
    this.outcomeUnknown = cls.outcomeUnknown;
    this.hint = cls.hint;
    this.variant = details.variant;
    this.provider = details.provider;
    this.leaseId = details.leaseId;
    this.executionId = details.executionId;
    this.launchKey = details.launchKey;
    this.restoreCleanup = details.restoreCleanup;
    this.commandCleanup = details.commandCleanup;
    this.retryAfterMs = details.retryAfterMs;
  }
}

export class LeaseDeniedError extends SandboxWsError {
  constructor(message: string, details?: SandboxWsErrorDetails) {
    super('lease_denied', message, details);
    this.name = 'LeaseDeniedError';
  }
}

export class CommandCancellationError extends SandboxWsError {
  readonly cleanupPending: boolean;
  readonly preserved: boolean;

  constructor(
    leaseId: string,
    executionId: string | undefined,
    reason: unknown,
    cleanupPending: boolean,
    context: { launchKey?: string; preserved?: boolean } = {},
  ) {
    const preserved = context.preserved === true && !cleanupPending;
    super(
      preserved ? 'command_preserved' : cleanupPending ? 'cancellation_incomplete' : 'command_cancelled',
      preserved
        ? 'the canceled launch preserved an existing command'
        : cleanupPending
          ? 'remote command cancellation is not confirmed'
          : 'remote command cancellation is confirmed',
      {
        leaseId,
        executionId,
        launchKey: context.launchKey,
        outcomeUnknown: cleanupPending,
        retryable: false,
        cause: reason,
      },
    );
    this.name = 'CommandCancellationError';
    this.cleanupPending = cleanupPending;
    this.preserved = preserved;
  }
}

export class OperationCancellationError extends SandboxWsError {
  constructor(
    readonly operationId: string,
    readonly canceled: boolean,
    readonly cleanupPending: boolean,
    reason: unknown,
    leaseId?: string,
  ) {
    super(
      canceled
        ? 'operation_cancelled'
        : cleanupPending
          ? 'operation_cancellation_pending'
          : 'operation_already_completed',
      canceled
        ? 'operation cancellation is confirmed'
        : cleanupPending
          ? 'operation cancellation is not confirmed'
          : 'the operation completed before cancellation',
      { leaseId, cause: reason, outcomeUnknown: cleanupPending, retryable: false },
    );
    this.name = 'OperationCancellationError';
  }
}

/** The broker saw another request still creating a sandbox under this name. Transient: retry with the same idempotency key. */
export class NameConflictError extends LeaseDeniedError {
  constructor(message: string, details?: SandboxWsErrorDetails) {
    super(message, { variant: 'name_conflict', ...details });
    this.name = 'NameConflictError';
  }
}

export class TenantMismatchError extends SandboxWsError {
  constructor(message: string, details?: SandboxWsErrorDetails) {
    super('tenant_mismatch', message, details);
    this.name = 'TenantMismatchError';
  }
}

export class TimeoutError extends SandboxWsError {
  constructor(message: string, details?: SandboxWsErrorDetails) {
    super('timeout', message, details);
    this.name = 'TimeoutError';
  }
}

/** `sandbox.preview(port, { waitMs })` gave up because the port never answered. Safe to retry. */
export class PreviewNotReadyError extends TimeoutError {
  readonly port: number;
  readonly waitedMs: number;
  constructor(port: number, waitedMs: number, details?: SandboxWsErrorDetails) {
    super(`port ${port} did not answer within ${waitedMs} ms`, {
      retryable: true,
      variant: 'preview_not_ready',
      ...details,
    });
    this.name = 'PreviewNotReadyError';
    this.port = port;
    this.waitedMs = waitedMs;
  }
}

/** `Sandbox.connect` found no lease with this id for the session tenant. Permanent. */
export class SandboxNotFoundError extends SandboxWsError {
  constructor(sandboxId: string, details?: SandboxWsErrorDetails) {
    super('lease_not_found', `sandbox ${sandboxId} not found`, {
      variant: 'lease_not_found',
      leaseId: sandboxId,
      ...details,
    });
    this.name = 'SandboxNotFoundError';
  }
}

/** `CommandFinished.throwIfFailed` saw a non-zero exit or a timeout. Permanent: the command ran. */
export class CommandFailedError extends SandboxWsError {
  readonly exitCode: number;
  readonly stderr: string;
  readonly timedOut: boolean;
  constructor(exitCode: number, stderr: string, opts: { timedOut?: boolean; leaseId?: string } = {}) {
    const tail = stderr.trim().slice(-500);
    super(
      'command_failed',
      `${opts.timedOut ? 'command timed out' : `command exited with code ${exitCode}`}${tail === '' ? '' : `: ${tail}`}`,
      { leaseId: opts.leaseId },
    );
    this.name = 'CommandFailedError';
    this.exitCode = exitCode;
    this.stderr = stderr;
    this.timedOut = opts.timedOut ?? false;
  }
}

/** `Sandbox.startServer` saw the process exit before its port answered. Permanent: the command ran. */
export class ServerExitedError extends SandboxWsError {
  readonly exitCode: number;
  /** The last output the process wrote, stdout then stderr. */
  readonly output: string;
  constructor(exitCode: number, output: string, opts: { leaseId?: string } = {}) {
    const tail = output.trim();
    super(
      'command_failed',
      `server exited with code ${exitCode} before it was ready${tail === '' ? '' : `:\n${tail}`}`,
      { leaseId: opts.leaseId },
    );
    this.name = 'ServerExitedError';
    this.exitCode = exitCode;
    this.output = output;
  }
}

export class TransportError extends SandboxWsError {
  constructor(message: string, details?: SandboxWsErrorDetails) {
    super('transport', message, details);
    this.name = 'TransportError';
  }
}

export class StdinError extends SandboxWsError {
  constructor(code: string, message: string, details?: SandboxWsErrorDetails) {
    super(code, message, details);
    this.name = 'StdinError';
  }
}

/** The error fields the gateway adds to an error frame. All are optional so older gateways still parse. */
export interface WireErrorExtras {
  kind?: string;
  retryable?: boolean;
  outcome_unknown?: boolean;
  hint?: string;
  provider?: string;
  lease_id?: string;
  /** Server-requested wait before a retry, in milliseconds. */
  retry_after_ms?: number;
  /** Same hint in seconds, as an HTTP `Retry-After` carries it. `retry_after_ms` wins when both are set. */
  retry_after_seconds?: number;
}

export interface ErrorContext {
  /** The tool the failed request called. */
  tool?: string;
  /** The lease the request addressed, when the wire error does not name one. */
  leaseId?: string;
}

function retryAfterMsOf(extras: WireErrorExtras): number | undefined {
  const ms =
    extras.retry_after_ms ?? (extras.retry_after_seconds === undefined ? undefined : extras.retry_after_seconds * 1000);
  return typeof ms === 'number' && Number.isFinite(ms) && ms >= 0 ? ms : undefined;
}

export function errorFromCode(
  code: string,
  message: string,
  extras: WireErrorExtras = {},
  context: ErrorContext = {},
): SandboxWsError {
  const details: SandboxWsErrorDetails = {
    retryable: extras.retryable,
    outcomeUnknown: extras.outcome_unknown,
    hint: extras.hint,
    variant: extras.kind,
    provider: extras.provider,
    leaseId: extras.lease_id ?? context.leaseId,
    tool: context.tool,
    retryAfterMs: retryAfterMsOf(extras),
  };
  switch (code) {
    case 'lease_denied':
      if (details.variant === 'name_conflict') {
        return new NameConflictError(message, details);
      }
      return new LeaseDeniedError(message, details);
    case 'tenant_mismatch':
      return new TenantMismatchError(message, details);
    case 'timeout':
      return new TimeoutError(message, details);
    case 'transport':
      return new TransportError(message, details);
    default:
      return new SandboxWsError(code, message, details);
  }
}
