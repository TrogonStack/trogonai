import type { OciImageRef } from '@trogonai/sandbox-ws-client';

/**
 * Parses a repeatable `k=v` flag into a `Record<string, string>`. Duplicate
 * keys are rejected rather than silently overwritten or silently deduped,
 * since a silently-dropped duplicate is a worse surprise for a CLI user than
 * a clear error telling them to fix their command line.
 */
export function parseKeyValueFlags(values: string[], flagName: string): Record<string, string> {
  const result: Record<string, string> = {};
  for (const raw of values) {
    const eq = raw.indexOf('=');
    if (eq <= 0) {
      throw new Error(`invalid ${flagName} value "${raw}": expected format key=value`);
    }
    const key = raw.slice(0, eq);
    const value = raw.slice(eq + 1);
    if (Object.prototype.hasOwnProperty.call(result, key)) {
      throw new Error(`duplicate ${flagName} key "${key}"`);
    }
    result[key] = value;
  }
  return result;
}

export const MAX_TAGS = 5;

export function validateTagCount(tags: Record<string, string>): void {
  const count = Object.keys(tags).length;
  if (count > MAX_TAGS) {
    throw new Error(`too many tags: got ${count}, max is ${MAX_TAGS}`);
  }
}

export interface RemotePath {
  leaseId: string;
  path: string;
}

/**
 * Docker-style `cp` path syntax: `<lease-id>:/path` denotes a remote path,
 * anything else is local. Ambiguity rule (POSIX-only, no Windows drive
 * letters to worry about): a string is treated as remote only if it matches
 * `/^[^:]+:.+/` AND the part before the first colon does not start with `/`
 * or `.`. That excludes paths like `./foo:bar` or `/abs:path` (which contain
 * a colon but are clearly local paths) while accepting `my-lease:/workspace`.
 */
const REMOTE_PATH_PATTERN = /^([^:]+):(.+)$/;

export function parseCopyPath(raw: string): RemotePath | null {
  const match = REMOTE_PATH_PATTERN.exec(raw);
  if (!match) {
    return null;
  }
  const [, prefix, rest] = match;
  if (prefix.startsWith('/') || prefix.startsWith('.')) {
    return null;
  }
  return { leaseId: prefix, path: rest };
}

const DEFAULT_REGISTRY = 'docker.io';

/**
 * Parses an OCI image reference of the form `[registry/]repository[:tag]`.
 * A prefix is treated as a registry only if it looks like one (contains a
 * dot, a colon for a port, or is literally `localhost`); otherwise the whole
 * string is the repository and the registry defaults to `docker.io`, mirroring
 * how `docker pull alpine` resolves against Docker Hub.
 */
export function parseImageReference(raw: string): OciImageRef {
  let remainder = raw;
  let registry = DEFAULT_REGISTRY;

  const firstSlash = remainder.indexOf('/');
  if (firstSlash > 0) {
    const candidate = remainder.slice(0, firstSlash);
    if (candidate.includes('.') || candidate.includes(':') || candidate === 'localhost') {
      registry = candidate;
      remainder = remainder.slice(firstSlash + 1);
    }
  }

  const atIndex = remainder.indexOf('@');
  if (atIndex >= 0) {
    const repository = remainder.slice(0, atIndex);
    const digest = remainder.slice(atIndex + 1);
    if (!repository) {
      throw new Error(`invalid image reference "${raw}": missing repository`);
    }
    return { registry, repository, digest: digest as OciImageRef['digest'] };
  }

  const lastColon = remainder.lastIndexOf(':');
  if (lastColon > 0) {
    const repository = remainder.slice(0, lastColon);
    const tag = remainder.slice(lastColon + 1);
    return { registry, repository, tag };
  }

  if (!remainder) {
    throw new Error(`invalid image reference "${raw}": missing repository`);
  }
  return { registry, repository: remainder, tag: 'latest' };
}
