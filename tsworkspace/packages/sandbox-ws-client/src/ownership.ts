import type { SandboxTags } from './wire.ts';

/** Tag key every lease created through `Sandbox.create` carries unless opted out. */
export const OWNER_TAG_KEY = 'client';
/** Tag value that marks a lease as created by this SDK. */
export const OWNER_TAG_VALUE = 'sandbox-ws-client';

/** A lease carries at most this many tags (`SandboxTags::MAX_TAGS` in sandbox-core). */
export const MAX_LEASE_TAGS = 5;

/** The reserved ownership tag as a filter or tag set. */
export function ownershipTags(): SandboxTags {
  return { [OWNER_TAG_KEY]: OWNER_TAG_VALUE };
}

/**
 * Tags to send on create. The ownership tag consumes one of the five tag
 * slots, so it is added only when it fits: a caller who already uses all five
 * keeps their tags and the lease is created without ownership. A caller-set
 * `client` key always wins.
 */
export function withOwnershipTag(tags: SandboxTags | undefined, enabled = true): SandboxTags | undefined {
  if (!enabled) {
    return tags;
  }
  const base = tags ?? {};
  if (OWNER_TAG_KEY in base || Object.keys(base).length >= MAX_LEASE_TAGS) {
    return tags;
  }
  return { ...base, [OWNER_TAG_KEY]: OWNER_TAG_VALUE };
}

/**
 * Tag filter for list and cleanup. By default the ownership tag is merged
 * into the caller's filter so leases of other tools sharing the tenant are
 * left alone; `includeForeign: true` leaves the filter as given.
 */
export function ownershipFilter(tags: SandboxTags | undefined, includeForeign = false): SandboxTags | undefined {
  if (includeForeign) {
    return tags;
  }
  return { ...ownershipTags(), ...tags };
}
