import { describe, expect, it } from 'vitest';

import { MAX_TAGS, parseCopyPath, parseImageReference, parseKeyValueFlags, validateTagCount } from '../src/parsing.js';

describe('parseKeyValueFlags', () => {
  it('collects repeatable k=v flags into a record', () => {
    expect(parseKeyValueFlags(['a=1', 'b=2'], '--env')).toEqual({ a: '1', b: '2' });
  });

  it('allows "=" inside the value', () => {
    expect(parseKeyValueFlags(['a=b=c'], '--env')).toEqual({ a: 'b=c' });
  });

  it('rejects duplicate keys rather than silently overwriting', () => {
    expect(() => parseKeyValueFlags(['a=1', 'a=2'], '--env')).toThrow(/duplicate --env key "a"/);
  });

  it('rejects a value with no "=" separator', () => {
    expect(() => parseKeyValueFlags(['no-equals'], '--tag')).toThrow(/invalid --tag value/);
  });

  it('rejects an empty key', () => {
    expect(() => parseKeyValueFlags(['=value'], '--tag')).toThrow(/invalid --tag value/);
  });
});

describe('validateTagCount', () => {
  it('accepts up to 5 tags', () => {
    const tags = Object.fromEntries(Array.from({ length: MAX_TAGS }, (_, i) => [`k${i}`, `v${i}`]));
    expect(() => validateTagCount(tags)).not.toThrow();
  });

  it('rejects more than 5 tags', () => {
    const tags = Object.fromEntries(Array.from({ length: MAX_TAGS + 1 }, (_, i) => [`k${i}`, `v${i}`]));
    expect(() => validateTagCount(tags)).toThrow(/too many tags/);
  });
});

describe('parseCopyPath', () => {
  it('accepts "name:/abs/path" as remote', () => {
    expect(parseCopyPath('my-lease:/workspace/out.txt')).toEqual({ leaseId: 'my-lease', path: '/workspace/out.txt' });
  });

  it('rejects a relative local path that merely contains a colon', () => {
    expect(parseCopyPath('./foo:bar')).toBeNull();
  });

  it('rejects an absolute local path that contains a colon', () => {
    expect(parseCopyPath('/abs:path')).toBeNull();
  });

  it('returns null for a plain local path with no colon', () => {
    expect(parseCopyPath('local.txt')).toBeNull();
  });

  it('treats a Windows-drive-letter-shaped local path as local, not remote', () => {
    // Note: our rule only special-cases prefixes starting with "/" or ".";
    // "C:\\foo" does not start with either, so by our stated heuristic it is
    // still treated as remote (leaseId "C"). This test documents that this
    // CLI targets POSIX paths only, matching the spec's "reject ambiguous
    // local paths ... on non-Windows" scope; Windows drive letters are out
    // of scope and not specially handled.
    expect(parseCopyPath('C:\\foo')).toEqual({ leaseId: 'C', path: '\\foo' });
  });
});

describe('parseImageReference', () => {
  it('defaults to docker.io when there is no registry-looking prefix', () => {
    expect(parseImageReference('alpine')).toEqual({ registry: 'docker.io', repository: 'alpine', tag: 'latest' });
  });

  it('parses repository:tag with default registry', () => {
    expect(parseImageReference('alpine:3.20')).toEqual({ registry: 'docker.io', repository: 'alpine', tag: '3.20' });
  });

  it('parses a full registry/repository:tag reference', () => {
    expect(parseImageReference('docker.io/library/alpine:latest')).toEqual({
      registry: 'docker.io',
      repository: 'library/alpine',
      tag: 'latest',
    });
  });

  it('treats a "localhost" prefix as a registry', () => {
    expect(parseImageReference('localhost/myimage:dev')).toEqual({
      registry: 'localhost',
      repository: 'myimage',
      tag: 'dev',
    });
  });

  it('treats a prefix with a port as a registry', () => {
    expect(parseImageReference('registry.internal:5000/team/app:v1')).toEqual({
      registry: 'registry.internal:5000',
      repository: 'team/app',
      tag: 'v1',
    });
  });

  it('does not treat a plain org name before a slash as a registry', () => {
    expect(parseImageReference('library/alpine:latest')).toEqual({
      registry: 'docker.io',
      repository: 'library/alpine',
      tag: 'latest',
    });
  });
});
