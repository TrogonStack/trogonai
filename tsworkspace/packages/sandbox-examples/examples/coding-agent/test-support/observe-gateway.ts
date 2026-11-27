import { SandboxWsClient } from '@trogonai/sandbox-ws-client';

const connect = SandboxWsClient.connect.bind(SandboxWsClient);
SandboxWsClient.connect = async (...args: Parameters<typeof connect>) => {
  const client = await connect(...args);
  const lease = client.lease.bind(client);
  client.lease = async (...request: Parameters<typeof lease>) => {
    const grant = await lease(...request);
    process.send?.({ type: 'grant', leaseId: grant.lease_id });
    return grant;
  };
  return client;
};
