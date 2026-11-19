import { SandboxWsError } from './errors.ts';

/**
 * Optional features whose support varies by provider. A provider that lacks
 * one answers the matching verb with a typed `unsupported` reply; naming the
 * features lets callers branch before the round trip.
 */
export const SANDBOX_FEATURES = [
  'fork',
  'pause_resume',
  'write_stdin',
  'restart_command',
  'read_logs',
  'update_network_policy',
  'sandbox_domain',
  'users',
  'groups',
  'drives',
  'evidence_streaming',
  'snapshots',
  'change_tracking',
  'file_watch',
  'dir_transfer',
  'port_preview',
  'expose_after_create',
  'file_ops',
  'file_search',
  'file_archive',
  'terminal',
  'named_processes',
  'graceful_stop',
  'command_cancellation',
  'command_launch_cancellation',
  'persistent_runtime',
  'browser',
  'output_streaming',
  'usage_gauges',
  'native_auto_stop',
  'native_passthrough',
] as const;

export type SandboxFeature = (typeof SANDBOX_FEATURES)[number];

const KNOWN_FEATURES: ReadonlySet<string> = new Set(SANDBOX_FEATURES);

/**
 * What a sandbox's provider supports, as far as the broker told the client.
 * Unknown feature names from a newer broker are kept out of the typed surface
 * but visible through `raw`.
 */
export class SandboxCapabilities<F extends SandboxFeature = SandboxFeature> {
  /** Every name the broker sent, including ones this client version does not know. */
  readonly raw: ReadonlySet<string>;

  private constructor(raw: ReadonlySet<string>) {
    this.raw = raw;
  }

  static from<G extends SandboxFeature>(supported: Iterable<G | (string & {})>): SandboxCapabilities<G> {
    return new SandboxCapabilities<G>(new Set(supported));
  }

  /** The known features present. */
  get features(): F[] {
    return [...this.raw].filter((name): name is F => KNOWN_FEATURES.has(name));
  }

  has<G extends SandboxFeature>(feature: G): this is SandboxCapabilities<F | G> {
    return this.raw.has(feature);
  }

  /** Throws a non-retryable `unsupported` error naming the feature unless it is present. */
  require<G extends SandboxFeature>(feature: G): asserts this is SandboxCapabilities<F | G> {
    if (!this.raw.has(feature)) {
      throw new SandboxWsError('unsupported', `the provider does not support ${feature}`);
    }
  }
}
