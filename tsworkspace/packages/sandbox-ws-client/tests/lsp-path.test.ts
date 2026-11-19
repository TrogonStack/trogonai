import { describe, expect, it } from 'vitest';

import { basenamePosix, fileUri, isAbsolutePosix, resolvePosix } from '../src/lsp/index.ts';

describe('posix path helpers', () => {
  it('resolves relative paths against the base', () => {
    expect(resolvePosix('/work/app', 'src/a.ts')).toBe('/work/app/src/a.ts');
    expect(resolvePosix('/work/app', '.')).toBe('/work/app');
  });

  it('collapses dot segments and duplicate slashes', () => {
    expect(resolvePosix('/work//app/', './src//./a.ts')).toBe('/work/app/src/a.ts');
    expect(resolvePosix('/work/app', '../lib/x.ts')).toBe('/work/lib/x.ts');
    expect(resolvePosix('/work/app', '../../../../x')).toBe('/x');
  });

  it('keeps absolute paths and normalizes them', () => {
    expect(resolvePosix('/work', '/etc//a/../b')).toBe('/etc/b');
    expect(resolvePosix('/work', '/')).toBe('/');
  });

  it('requires an absolute base', () => {
    expect(() => resolvePosix('rel', 'x')).toThrow(/absolute/);
    expect(isAbsolutePosix('/a')).toBe(true);
    expect(isAbsolutePosix('a')).toBe(false);
  });

  it('computes basenames', () => {
    expect(basenamePosix('/a/b/')).toBe('b');
    expect(basenamePosix('/')).toBe('');
  });

  it('percent-encodes each segment in file URIs', () => {
    expect(fileUri('/a b/c#d/é.ts')).toBe('file:///a%20b/c%23d/%C3%A9.ts');
    expect(fileUri('/')).toBe('file:///');
  });
});
