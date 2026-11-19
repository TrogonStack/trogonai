import { lspRequestId } from '../ids.ts';
import type { LspRequestId } from '../ids.ts';
import { LspProtocolError, LspResponseError, LspTimeoutError } from './errors.ts';
import type {
  ConfigurationItem,
  JsonRpcId,
  JsonRpcMessage,
  JsonRpcNotificationMessage,
  JsonRpcRequestMessage,
  JsonRpcResponseMessage,
} from './types.ts';

const DEFAULT_REQUEST_TIMEOUT_MS = 30_000;
const METHOD_NOT_FOUND = -32601;

export type NotificationHandler = (params: unknown) => void;

export interface JsonRpcConnectionOptions {
  send: (message: JsonRpcMessage) => Promise<void>;
  requestTimeoutMs?: number;
  /** Answers `workspace/configuration` items. Defaults to `null` for every item. */
  configuration?: (item: ConfigurationItem) => unknown;
}

interface PendingRequest {
  method: string;
  resolve: (value: unknown) => void;
  reject: (reason: unknown) => void;
  timer: ReturnType<typeof setTimeout>;
}

export class JsonRpcConnection {
  private nextId = 0;
  private readonly pending = new Map<LspRequestId, PendingRequest>();
  private readonly handlers = new Map<string, Set<NotificationHandler>>();
  private readonly send: (message: JsonRpcMessage) => Promise<void>;
  private readonly requestTimeoutMs: number;
  private readonly configuration?: (item: ConfigurationItem) => unknown;

  constructor(opts: JsonRpcConnectionOptions) {
    this.send = opts.send;
    this.requestTimeoutMs = opts.requestTimeoutMs ?? DEFAULT_REQUEST_TIMEOUT_MS;
    this.configuration = opts.configuration;
  }

  request<T = unknown>(method: string, params?: unknown, opts?: { timeoutMs?: number }): Promise<T> {
    const id = lspRequestId(++this.nextId);
    const timeoutMs = opts?.timeoutMs ?? this.requestTimeoutMs;
    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        this.send({ jsonrpc: '2.0', method: '$/cancelRequest', params: { id } }).catch(() => undefined);
        reject(new LspTimeoutError(method, timeoutMs));
      }, timeoutMs);
      this.pending.set(id, { method, resolve: resolve as (value: unknown) => void, reject, timer });
      this.send({ jsonrpc: '2.0', id, method, params }).catch((err) => {
        if (this.pending.delete(id)) {
          clearTimeout(timer);
          reject(err);
        }
      });
    });
  }

  notify(method: string, params?: unknown): Promise<void> {
    return this.send({ jsonrpc: '2.0', method, params });
  }

  onNotification(method: string, handler: NotificationHandler): () => void {
    let set = this.handlers.get(method);
    if (!set) {
      set = new Set();
      this.handlers.set(method, set);
    }
    set.add(handler);
    return () => {
      set.delete(handler);
    };
  }

  /** Feeds one decoded JSON body from the framer. */
  handleBody(body: string): void {
    let message: JsonRpcMessage;
    try {
      message = JSON.parse(body) as JsonRpcMessage;
    } catch {
      throw new LspProtocolError('LSP message body is not valid JSON');
    }
    if (typeof message !== 'object' || message === null) {
      throw new LspProtocolError('LSP message body is not an object');
    }
    if ('method' in message && typeof message.method === 'string') {
      if ('id' in message && message.id !== undefined && message.id !== null) {
        this.replyToServerRequest(message as JsonRpcRequestMessage);
      } else {
        this.dispatchNotification(message as JsonRpcNotificationMessage);
      }
      return;
    }
    this.handleResponse(message as JsonRpcResponseMessage);
  }

  /** Rejects every in-flight request, e.g. when the server stream ends. */
  close(reason: Error): void {
    for (const [id, pending] of this.pending) {
      clearTimeout(pending.timer);
      this.pending.delete(id);
      pending.reject(reason);
    }
  }

  private handleResponse(message: JsonRpcResponseMessage): void {
    if (typeof message.id !== 'number') {
      return;
    }
    const id = lspRequestId(message.id);
    const pending = this.pending.get(id);
    if (!pending) {
      return;
    }
    this.pending.delete(id);
    clearTimeout(pending.timer);
    if (message.error) {
      pending.reject(new LspResponseError(message.error.code, message.error.message, message.error.data));
    } else {
      pending.resolve(message.result ?? null);
    }
  }

  private dispatchNotification(message: JsonRpcNotificationMessage): void {
    for (const handler of this.handlers.get(message.method) ?? []) {
      try {
        handler(message.params);
      } catch {
        // a throwing listener must not break the output pump
      }
    }
  }

  private replyToServerRequest(message: JsonRpcRequestMessage): void {
    const reply = (response: Omit<JsonRpcResponseMessage, 'jsonrpc' | 'id'>) => {
      const id: JsonRpcId = message.id;
      this.send({ jsonrpc: '2.0', id, ...response }).catch(() => undefined);
    };
    switch (message.method) {
      case 'workspace/configuration': {
        const items = (message.params as { items?: ConfigurationItem[] } | undefined)?.items ?? [];
        reply({ result: items.map((item) => this.configuration?.(item) ?? null) });
        return;
      }
      case 'client/registerCapability':
      case 'window/workDoneProgress/create':
        reply({ result: null });
        return;
      default:
        reply({ error: { code: METHOD_NOT_FOUND, message: `method not found: ${message.method}` } });
    }
  }
}
