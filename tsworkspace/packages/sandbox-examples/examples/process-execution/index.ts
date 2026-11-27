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
    image: { kind: 'registry', reference: { registry: 'docker.io', repository: 'library/python', tag: '3.12-slim' } },
  });

  try {
    const code = await sandbox.runCommand('python3', ['-c', 'print("Hello World!")']);
    if (code.exitCode !== 0) {
      console.error(`Error: ${code.exitCode} ${code.stderr()}`);
    } else {
      console.log(code.stdout());
    }

    const exec = await client.runCommand(sandbox.sandboxId, ['sh', '-c', 'echo "Hello World from exec!"'], {
      cwd: '/tmp',
      timeoutSeconds: 10,
    });
    if (exec.outcome === 'completed') {
      if (exec.exit_code !== 0) {
        console.error(`Error: ${exec.exit_code}`);
      } else {
        console.log(new TextDecoder().decode(Uint8Array.from(exec.stdout)));
      }
    }
  } finally {
    await sandbox.delete();
  }
} finally {
  client.close();
}
