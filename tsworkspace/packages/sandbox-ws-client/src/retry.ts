import { READ_ONLY_TOOLS } from './error-classes.ts';
import { SandboxWsError } from './errors.ts';

/**
 * Tools `SandboxWsClient` may repeat after a retryable failure: every
 * read-only tool, plus the mutations that are safe to apply twice.
 * `sandbox_lease` carries an idempotency key, so a repeat returns the same
 * lease; `sandbox_reap_leases` and `sandbox_update_tags` converge on the same
 * end state. `run_command`, `write_stdin`, `write_files`, `file_op`,
 * `kill_*`, `stop`/`delete`/`pause`/`resume`, `fork` and the user and drive
 * verbs are never repeated, because a second application is observable.
 * The exception is a `run_command` carrying a `launch_key`, which the broker
 * replays instead of running again; `SandboxWsClient.runCommand` always sends
 * one and retries it.
 * Subscriptions are excluded as they register state on the connection.
 */
export const IDEMPOTENT_TOOLS: ReadonlySet<string> = new Set([
  ...[...READ_ONLY_TOOLS].filter((tool) => !tool.startsWith('sandbox_subscribe_')),
  'sandbox_lease',
  'sandbox_reap_leases',
  'sandbox_update_tags',
]);

export interface RetryOptions {
  /** Total attempts including the first, at least 1. Defaults to 3. */
  maxAttempts?: number;
  /** Backoff before the second attempt; doubles each time. Defaults to 100. */
  baseDelayMs?: number;
  /** Upper bound of the backoff (before jitter). Defaults to 2000. */
  maxDelayMs?: number;
  /**
   * Longest server-requested wait (`Retry-After`) that is honored. A longer
   * hint means the server wants the caller away for a while, so the error is
   * raised instead of sleeping. Defaults to 30000.
   */
  maxRetryAfterMs?: number;
  /** Randomize each backoff within [0, delay] ("full jitter"). Defaults to true. */
  jitter?: boolean;
  /** Source of randomness in [0, 1); for tests. Defaults to `Math.random`. */
  random?: () => number;
  /** Sleep implementation; for tests. */
  sleep?: (ms: number) => Promise<void>;
  /** Called before each wait with the failure that caused the retry. */
  onRetry?: (info: { tool: string; attempt: number; delayMs: number; error: SandboxWsError }) => void;
}

export interface ResolvedRetryPolicy {
  maxAttempts: number;
  baseDelayMs: number;
  maxDelayMs: number;
  maxRetryAfterMs: number;
  jitter: boolean;
  random: () => number;
  sleep: (ms: number) => Promise<void>;
  onRetry?: RetryOptions['onRetry'];
}

export const DEFAULT_RETRY: Readonly<
  Pick<ResolvedRetryPolicy, 'maxAttempts' | 'baseDelayMs' | 'maxDelayMs' | 'maxRetryAfterMs' | 'jitter'>
> = Object.freeze({
  maxAttempts: 3,
  baseDelayMs: 100,
  maxDelayMs: 2000,
  maxRetryAfterMs: 30_000,
  jitter: true,
});

const defaultSleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Fills defaults. `false` disables retries, which is `maxAttempts: 1`;
 * `undefined` means the defaults.
 */
export function resolveRetryPolicy(opts: RetryOptions | false | undefined): ResolvedRetryPolicy {
  const given: RetryOptions = opts === false ? { maxAttempts: 1 } : (opts ?? {});
  const maxAttempts = given.maxAttempts ?? DEFAULT_RETRY.maxAttempts;
  if (!Number.isInteger(maxAttempts) || maxAttempts < 1) {
    throw new RangeError('retry.maxAttempts must be an integer of at least 1');
  }
  for (const key of ['baseDelayMs', 'maxDelayMs', 'maxRetryAfterMs'] as const) {
    const value = given[key];
    if (value !== undefined && (!Number.isFinite(value) || value < 0)) {
      throw new RangeError(`retry.${key} must be a non-negative number`);
    }
  }
  return {
    maxAttempts,
    baseDelayMs: given.baseDelayMs ?? DEFAULT_RETRY.baseDelayMs,
    maxDelayMs: given.maxDelayMs ?? DEFAULT_RETRY.maxDelayMs,
    maxRetryAfterMs: given.maxRetryAfterMs ?? DEFAULT_RETRY.maxRetryAfterMs,
    jitter: given.jitter ?? DEFAULT_RETRY.jitter,
    random: given.random ?? Math.random,
    sleep: given.sleep ?? defaultSleep,
    onRetry: given.onRetry,
  };
}

/**
 * Wait before retry number `attempt` (1 for the first retry): exponential
 * backoff capped at `maxDelayMs`, jittered, and never shorter than the
 * server's `Retry-After` hint.
 */
export function retryDelayMs(policy: ResolvedRetryPolicy, attempt: number, retryAfterMs?: number): number {
  const ceiling = Math.min(policy.maxDelayMs, policy.baseDelayMs * 2 ** (attempt - 1));
  const backoff = policy.jitter ? Math.floor(policy.random() * (ceiling + 1)) : ceiling;
  return Math.max(backoff, retryAfterMs ?? 0);
}

/**
 * Runs `attemptFn`, repeating it for an idempotent tool while it fails with a
 * retryable `SandboxWsError`. `idempotent` overrides the per-tool default for a
 * call whose payload makes it safe to repeat, such as a keyed `run_command`. Other tools, non-retryable errors, non-library
 * errors and a closed connection (`canRetry` false) fail on the first throw.
 */
export async function withRetry<T>(
  tool: string,
  policy: ResolvedRetryPolicy,
  canRetry: () => boolean,
  attemptFn: () => Promise<T>,
  idempotent: boolean = IDEMPOTENT_TOOLS.has(tool),
): Promise<T> {
  if (policy.maxAttempts <= 1 || !idempotent) {
    return attemptFn();
  }
  for (let attempt = 1; ; attempt += 1) {
    try {
      return await attemptFn();
    } catch (err) {
      if (
        !(err instanceof SandboxWsError) ||
        !err.retryable ||
        attempt >= policy.maxAttempts ||
        !canRetry() ||
        (err.retryAfterMs !== undefined && err.retryAfterMs > policy.maxRetryAfterMs)
      ) {
        throw err;
      }
      const delayMs = retryDelayMs(policy, attempt, err.retryAfterMs);
      policy.onRetry?.({ tool, attempt, delayMs, error: err });
      await policy.sleep(delayMs);
      if (!canRetry()) {
        throw err;
      }
    }
  }
}
