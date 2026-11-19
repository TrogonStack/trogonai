import { LspProtocolError } from './errors.ts';

const HEADER_TERMINATOR = [0x0d, 0x0a, 0x0d, 0x0a];
const MAX_HEADER_BYTES = 8 * 1024;
const MAX_BODY_BYTES = 64 * 1024 * 1024;

const encoder = new TextEncoder();

export function encodeMessage(message: unknown): Uint8Array {
  const body = encoder.encode(JSON.stringify(message));
  const header = encoder.encode(`Content-Length: ${body.length}\r\n\r\n`);
  const out = new Uint8Array(header.length + body.length);
  out.set(header, 0);
  out.set(body, header.length);
  return out;
}

/**
 * Byte-based `Content-Length` framing for LSP/JSON-RPC streams. Lengths count
 * bytes, so bodies are only decoded once complete and multi-byte UTF-8 split
 * across chunks is safe.
 */
export class LspFramer {
  private buffer: Uint8Array = new Uint8Array(0);
  private readonly decoder = new TextDecoder('utf-8', { fatal: true });

  /** Returns the raw JSON body of every message completed by this chunk. */
  push(chunk: Uint8Array): string[] {
    this.buffer = concat(this.buffer, chunk);
    const bodies: string[] = [];
    for (;;) {
      const headerEnd = indexOfTerminator(this.buffer);
      if (headerEnd < 0) {
        if (this.buffer.length > MAX_HEADER_BYTES) {
          throw new LspProtocolError('LSP header section exceeds limit');
        }
        break;
      }
      const length = parseContentLength(this.decode(this.buffer.subarray(0, headerEnd)));
      const bodyStart = headerEnd + HEADER_TERMINATOR.length;
      if (this.buffer.length < bodyStart + length) {
        break;
      }
      bodies.push(this.decode(this.buffer.subarray(bodyStart, bodyStart + length)));
      this.buffer = this.buffer.subarray(bodyStart + length);
    }
    return bodies;
  }

  private decode(bytes: Uint8Array): string {
    try {
      return this.decoder.decode(bytes);
    } catch {
      throw new LspProtocolError('LSP message is not valid UTF-8');
    }
  }
}

function concat(a: Uint8Array, b: Uint8Array): Uint8Array {
  if (a.length === 0) {
    return b;
  }
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}

function indexOfTerminator(buffer: Uint8Array): number {
  for (let i = 0; i + HEADER_TERMINATOR.length <= buffer.length; i++) {
    if (HEADER_TERMINATOR.every((byte, offset) => buffer[i + offset] === byte)) {
      return i;
    }
  }
  return -1;
}

function parseContentLength(header: string): number {
  for (const line of header.split('\r\n')) {
    const colon = line.indexOf(':');
    if (colon < 0 || line.slice(0, colon).trim().toLowerCase() !== 'content-length') {
      continue;
    }
    const value = line.slice(colon + 1).trim();
    if (!/^\d+$/.test(value)) {
      throw new LspProtocolError(`invalid Content-Length: ${value}`);
    }
    const length = Number(value);
    if (length > MAX_BODY_BYTES) {
      throw new LspProtocolError(`Content-Length ${length} exceeds limit`);
    }
    return length;
  }
  throw new LspProtocolError('LSP message is missing Content-Length');
}
