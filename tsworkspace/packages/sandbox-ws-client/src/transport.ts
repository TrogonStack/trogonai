import { WebSocket as NodeWebSocket } from 'ws';

export function getWebSocketImpl(): typeof WebSocket {
  if (typeof globalThis.WebSocket !== 'undefined') {
    return globalThis.WebSocket;
  }
  return NodeWebSocket as unknown as typeof WebSocket;
}

export function openWebSocket(url: string, protocols: string[]): WebSocket {
  const Impl = getWebSocketImpl();
  return new Impl(url, protocols);
}
