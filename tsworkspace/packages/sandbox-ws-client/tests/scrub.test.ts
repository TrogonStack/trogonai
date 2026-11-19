import { describe, expect, it } from 'vitest';

import { errorFromCode, REDACTED, SandboxWsError, scrubCause, scrubSecrets, TransportError } from '../src/index.ts';

describe('scrubSecrets', () => {
  it('removes URL credentials but keeps the host', () => {
    expect(scrubSecrets('connect to https://alice:hunter2@gw.example.com/v1 failed')).toBe(
      `connect to https://${REDACTED}@gw.example.com/v1 failed`,
    );
    expect(scrubSecrets('nats://s3cr3t@nats:4222')).toBe(`nats://${REDACTED}@nats:4222`);
  });

  it('removes bearer tokens in header and subprotocol form', () => {
    expect(scrubSecrets('rejected: Bearer abc123DEF456ghi')).toBe(`rejected: Bearer ${REDACTED}`);
    expect(scrubSecrets('protocol bearer.abc123DEF456ghi refused')).toBe(`protocol bearer.${REDACTED} refused`);
  });

  it('removes sensitive query parameters and keeps the others', () => {
    expect(scrubSecrets('GET https://h/p?a=1&token=abc&b=2&api_key=zzz')).toBe(
      `GET https://h/p?a=1&token=${REDACTED}&b=2&api_key=${REDACTED}`,
    );
    expect(scrubSecrets('wss://h/ws?X-Amz-Signature=deadbeef')).toBe(`wss://h/ws?X-Amz-Signature=${REDACTED}`);
  });

  it('removes API-key and authorization header values', () => {
    expect(scrubSecrets('headers: X-Sandbox-API-Key: sk_live_abc')).toBe(`headers: X-Sandbox-API-Key: ${REDACTED}`);
    expect(scrubSecrets('Authorization: Basic dXNlcjpwYXNz')).toBe(`Authorization: ${REDACTED}`);
    expect(scrubSecrets('x-api-key=abcdef')).toBe(`x-api-key=${REDACTED}`);
  });

  it('removes sensitive fields of a JSON error body', () => {
    expect(scrubSecrets('{"error":"bad","token":"abc\\"def","n":1}')).toBe(
      `{"error":"bad","token":"${REDACTED}","n":1}`,
    );
  });

  it('leaves ordinary text and prose alone', () => {
    for (const text of [
      'lease l-1 not found',
      'authorization: denied',
      'no such file or directory: /work/token.txt',
      'timed out after 30s at https://example.com/path?page=2',
    ]) {
      expect(scrubSecrets(text)).toBe(text);
    }
  });

  it('is deterministic and idempotent', () => {
    const input = 'https://u:p@h/x?token=abc Bearer abcdefgh12345 X-Api-Key: k1';
    const once = scrubSecrets(input);
    expect(scrubSecrets(input)).toBe(once);
    expect(scrubSecrets(once)).toBe(once);
    expect(once).not.toMatch(/abcdefgh12345|token=abc|u:p/);
  });
});

describe('scrubCause', () => {
  it('copies an error with a scrubbed message, stack, name and nested cause', () => {
    const inner = new Error('inner https://a:b@h/');
    const outer = new TypeError('fetch failed for https://h/?token=abc', { cause: inner });
    const copy = scrubCause(outer) as Error;
    expect(copy).not.toBe(outer);
    expect(copy.name).toBe('TypeError');
    expect(copy.message).toBe(`fetch failed for https://h/?token=${REDACTED}`);
    expect(copy.stack).not.toContain('token=abc');
    expect((copy.cause as Error).message).toBe(`inner https://${REDACTED}@h/`);
  });

  it('turns non-error causes into scrubbed text and passes nullish through', () => {
    expect(scrubCause('Bearer abcdefgh12345')).toBe(`Bearer ${REDACTED}`);
    expect(scrubCause(undefined)).toBeUndefined();
  });
});

describe('client error mapping', () => {
  it('scrubs a server-supplied message', () => {
    const err = errorFromCode('backend', 'pull failed: https://u:pw@registry.example.com/v2/');
    expect(err.message).toBe(`pull failed: https://${REDACTED}@registry.example.com/v2/`);
  });

  it('scrubs messages and causes on direct construction', () => {
    const err = new TransportError('failed Bearer abcdefgh12345', { cause: new Error('x?api_key=zzz') });
    expect(err.message).toBe(`failed Bearer ${REDACTED}`);
    expect((err.cause as Error).message).toBe(`x?api_key=${REDACTED}`);
    expect(new SandboxWsError('c', 'plain').message).toBe('plain');
  });
});
