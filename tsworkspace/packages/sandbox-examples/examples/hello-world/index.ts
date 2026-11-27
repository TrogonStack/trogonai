import { Sandbox, SandboxWsClient, tenantId } from '@trogonai/sandbox-ws-client';

const client = await SandboxWsClient.connect({
  gatewayHttpUrl: process.env.SANDBOX_GATEWAY_HTTP_URL!,
  gatewayWsUrl: process.env.SANDBOX_GATEWAY_WS_URL!,
  apiKey: process.env.SANDBOX_API_KEY!,
});

try {
  const sandbox = await Sandbox.create(client, {
    tenant: tenantId(process.env.SANDBOX_TENANT!),
    providerPref: ['local_docker'],
  });
  try {
    const response = await sandbox.runCommand('sh', ['-c', 'echo "Hello, World!"']);
    console.log(response.stdout());
  } finally {
    await sandbox.delete();
  }
} finally {
  client.close();
}
