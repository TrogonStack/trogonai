import type { NextConfig } from 'next';

const config: NextConfig = {
  // The sandbox client opens a WebSocket from Node; keep it out of the server bundle.
  serverExternalPackages: ['@trogonai/sandbox-ws-client', '@trogonai/sandbox-agent-tools', 'ws'],
};

export default config;
