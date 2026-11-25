import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { JsonRpcConnection, LspResponseError, LspTimeoutError } from '../src/lsp/index.ts';
import type { JsonRpcMessage } from '../src/lsp/index.ts';

describe('JsonRpcConnection', () => {
  let sent: JsonRpcMessage[];
  let conn: JsonRpcConnection;

  beforeEach(() => {
    vi.useFakeTimers();
    sent = [];
    conn = new JsonRpcConnection({
      send: async (m) => {
        sent.push(m);
      },
      requestTimeoutMs: 1000,
    });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('numbers requests from 1 and correlates out-of-order responses', async () => {
    const a = conn.request('a');
    const b = conn.request('b');
    expect(sent.map((m) => (m as { id: number }).id)).toEqual([1, 2]);
    conn.handleBody(JSON.stringify({ jsonrpc: '2.0', id: 2, result: 'B' }));
    conn.handleBody(JSON.stringify({ jsonrpc: '2.0', id: 1, result: 'A' }));
    await expect(a).resolves.toBe('A');
    await expect(b).resolves.toBe('B');
  });

  it('rejects with LspResponseError on an error response', async () => {
    const p = conn.request('a');
    conn.handleBody(JSON.stringify({ jsonrpc: '2.0', id: 1, error: { code: -32000, message: 'bad', data: 7 } }));
    await expect(p).rejects.toMatchObject({ rpcCode: -32000, message: 'bad', data: 7 });
    await expect(p).rejects.toBeInstanceOf(LspResponseError);
  });

  it('times out, sends $/cancelRequest and ignores the late response', async () => {
    const p = conn.request('slow');
    const assertion = expect(p).rejects.toBeInstanceOf(LspTimeoutError);
    await vi.advanceTimersByTimeAsync(1000);
    await assertion;
    expect(sent[1]).toEqual({ jsonrpc: '2.0', method: '$/cancelRequest', params: { id: 1 } });
    conn.handleBody(JSON.stringify({ jsonrpc: '2.0', id: 1, result: 1 }));
  });

  it('honours a per-request timeout', async () => {
    const p = conn.request('x', undefined, { timeoutMs: 50 });
    const assertion = expect(p).rejects.toBeInstanceOf(LspTimeoutError);
    await vi.advanceTimersByTimeAsync(50);
    await assertion;
  });

  it('rejects pending requests on close', async () => {
    const p = conn.request('x');
    conn.close(new Error('gone'));
    await expect(p).rejects.toThrow('gone');
  });

  it('delivers notifications and supports unsubscribe', () => {
    const seen: unknown[] = [];
    const off = conn.onNotification('n', (params) => seen.push(params));
    conn.handleBody(JSON.stringify({ jsonrpc: '2.0', method: 'n', params: 1 }));
    off();
    conn.handleBody(JSON.stringify({ jsonrpc: '2.0', method: 'n', params: 2 }));
    expect(seen).toEqual([1]);
  });

  it('answers server requests with fixed replies', () => {
    conn.handleBody(
      JSON.stringify({
        jsonrpc: '2.0',
        id: 'c',
        method: 'workspace/configuration',
        params: { items: [{}, { section: 's' }] },
      }),
    );
    conn.handleBody(JSON.stringify({ jsonrpc: '2.0', id: 5, method: 'client/registerCapability', params: {} }));
    conn.handleBody(JSON.stringify({ jsonrpc: '2.0', id: 6, method: 'window/workDoneProgress/create', params: {} }));
    conn.handleBody(JSON.stringify({ jsonrpc: '2.0', id: 7, method: 'what/ever' }));
    expect(sent).toEqual([
      { jsonrpc: '2.0', id: 'c', result: [null, null] },
      { jsonrpc: '2.0', id: 5, result: null },
      { jsonrpc: '2.0', id: 6, result: null },
      { jsonrpc: '2.0', id: 7, error: { code: -32601, message: 'method not found: what/ever' } },
    ]);
  });

  it('uses caller configuration for workspace/configuration', () => {
    const configured = new JsonRpcConnection({
      send: async (m) => {
        sent.push(m);
      },
      configuration: (item) => (item.section === 'python' ? { a: 1 } : undefined),
    });
    configured.handleBody(
      JSON.stringify({
        jsonrpc: '2.0',
        id: 1,
        method: 'workspace/configuration',
        params: { items: [{ section: 'python' }, { section: 'x' }] },
      }),
    );
    expect(sent[0]).toEqual({ jsonrpc: '2.0', id: 1, result: [{ a: 1 }, null] });
  });

  it('rejects invalid JSON bodies', () => {
    expect(() => conn.handleBody('{nope')).toThrow(/not valid JSON/);
  });
});
