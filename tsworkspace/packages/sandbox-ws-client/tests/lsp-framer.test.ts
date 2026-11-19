import { describe, expect, it } from 'vitest';

import { encodeMessage, LspFramer, LspProtocolError } from '../src/lsp/index.ts';

const enc = new TextEncoder();

function frame(body: string, header = 'Content-Length'): Uint8Array {
  const bytes = enc.encode(body);
  return enc.encode(`${header}: ${bytes.length}\r\n\r\n${body}`);
}

function join(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let offset = 0;
  for (const p of parts) {
    out.set(p, offset);
    offset += p.length;
  }
  return out;
}

describe('LspFramer', () => {
  it('encodes with a byte length', () => {
    const bytes = encodeMessage({ a: 'é' });
    expect(new TextDecoder().decode(bytes)).toBe('Content-Length: 10\r\n\r\n{"a":"é"}');
  });

  it('reassembles a message split at every byte boundary', () => {
    const bytes = frame('{"k":"héllo €"}');
    for (let cut = 1; cut < bytes.length; cut++) {
      const framer = new LspFramer();
      expect(framer.push(bytes.subarray(0, cut))).toEqual([]);
      expect(framer.push(bytes.subarray(cut))).toEqual(['{"k":"héllo €"}']);
    }
  });

  it('returns several messages from one chunk and keeps the partial tail', () => {
    const framer = new LspFramer();
    const tail = frame('{"c":3}');
    expect(framer.push(join(frame('{"a":1}'), frame('{"b":2}'), tail.subarray(0, 5)))).toEqual(['{"a":1}', '{"b":2}']);
    expect(framer.push(tail.subarray(5))).toEqual(['{"c":3}']);
  });

  it('matches headers case-insensitively and ignores other headers', () => {
    const framer = new LspFramer();
    const body = '{"x":1}';
    const raw = enc.encode(
      `content-type: application/vscode-jsonrpc; charset=utf-8\r\nCONTENT-LENGTH: ${body.length}\r\n\r\n${body}`,
    );
    expect(framer.push(raw)).toEqual([body]);
  });

  it('rejects a missing or malformed Content-Length', () => {
    expect(() => new LspFramer().push(enc.encode('X-Other: 1\r\n\r\n{}'))).toThrow(LspProtocolError);
    expect(() => new LspFramer().push(enc.encode('Content-Length: abc\r\n\r\n{}'))).toThrow(LspProtocolError);
  });

  it('rejects an unbounded header section', () => {
    expect(() => new LspFramer().push(new Uint8Array(9 * 1024).fill(65))).toThrow(LspProtocolError);
  });

  it('rejects invalid UTF-8 in a body', () => {
    const raw = join(enc.encode('Content-Length: 1\r\n\r\n'), Uint8Array.from([0xff]));
    expect(() => new LspFramer().push(raw)).toThrow(LspProtocolError);
  });
});
