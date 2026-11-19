import { SandboxWsError } from './errors.ts';
import type { NetworkPolicy } from './wire.ts';

/** Sugar for the common case: egress only to these hosts, everything else denied. */
export interface NetworkAllowList {
  /**
   * Domain patterns the sandbox may reach, in the wire's `DomainPattern`
   * form: an exact host (`registry.npmjs.org`), a wildcard that matches any
   * strict subdomain (`*.pythonhosted.org`), optionally with a `:port`.
   */
  allow: string[];
}

/**
 * Builds the `user_defined` policy for a plain allow list: one pass-through
 * rule per host, no IP ranges, deny by default. An empty list allows nothing
 * and maps to `deny_all`. Entries are trimmed and de-duplicated in order.
 * Throws `SandboxWsError` (`invalid_request`) for an entry that is blank,
 * holds whitespace, or looks like a URL (`https://host/path`) rather than a
 * host pattern, since the broker would reject it later with less context.
 */
export function allowListPolicy(allow: readonly string[]): NetworkPolicy {
  const hosts: string[] = [];
  for (const entry of allow) {
    const host = entry.trim();
    if (host === '' || /\s/.test(host) || host.includes('://') || host.includes('/')) {
      throw new SandboxWsError(
        'invalid_request',
        `invalid network allow entry ${JSON.stringify(entry)}: expected a host pattern such as "registry.npmjs.org" or "*.example.com:443"`,
      );
    }
    if (!hosts.includes(host)) {
      hosts.push(host);
    }
  }
  if (hosts.length === 0) {
    return 'deny_all';
  }
  return {
    user_defined: {
      rules: hosts.map((domain) => ({ domain, action: 'pass_through' as const })),
      allow_ranges: [],
      deny_ranges: [],
    },
  };
}
