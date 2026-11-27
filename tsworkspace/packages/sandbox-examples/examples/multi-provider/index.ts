import { MemorySandboxClient, Sandbox, SandboxWsClient, tenantId } from '@trogonai/sandbox-ws-client';
import type { ProviderKind, SandboxClientApi } from '@trogonai/sandbox-ws-client';

// `memory` runs in process. Any other value is a wire provider kind the gateway routes to.
const provider = process.env.SANDBOX_PROVIDER ?? 'memory';

const client: SandboxClientApi =
  provider === 'memory'
    ? new MemorySandboxClient()
    : await SandboxWsClient.connect({
        gatewayHttpUrl: process.env.SANDBOX_GATEWAY_HTTP_URL!,
        gatewayWsUrl: process.env.SANDBOX_GATEWAY_WS_URL!,
        apiKey: process.env.SANDBOX_API_KEY!,
      });

try {
  const sandbox = await Sandbox.create(client, {
    tenant: tenantId(process.env.SANDBOX_TENANT ?? 'demo'),
    providerPref: provider === 'memory' ? undefined : [provider as ProviderKind],
  });
  console.log(`Backend: ${provider}, served by ${sandbox.provider}`);

  try {
    await sandbox.writeFiles([{ path: '/tmp/hello.txt', content: 'Hello from any backend!\n' }]);
    console.log('File content:', await sandbox.readFile({ path: '/tmp/hello.txt' }));

    const echoed = await sandbox.runCommand('echo', ['ran a command']);
    console.log(`Command (exit ${echoed.exitCode}):`, echoed.stdout().trim());

    if (sandbox.supports('output_streaming')) {
      // A detached command started with streamOutput delivers chunks while it runs.
      const command = await sandbox.runCommand({
        cmd: 'sh',
        args: ['-c', 'for i in 1 2 3; do echo "tick $i"; sleep 1; done'],
        detached: true,
        streamOutput: true,
      });
      const decoder = new TextDecoder();
      // The chunk feed only ends when the connection closes, so it keeps running
      // in the background while we wait for the exit.
      void (async () => {
        for await (const chunk of command.stream()) {
          process.stdout.write(`[${chunk.stream}] ${decoder.decode(chunk.data)}`);
        }
      })().catch(() => {});
      const finished = await command.wait();
      await new Promise((resolve) => setTimeout(resolve, 250));
      console.log(`Streamed command exited with ${finished.exitCode}`);
    } else {
      console.log('Streaming: skipped, this provider does not report output_streaming');
    }

    if (sandbox.supports('pause_resume')) {
      const reapAt = await sandbox.pause();
      console.log(`Paused until at most ${reapAt.toISOString()}`);
      await sandbox.resume();
      console.log('Resumed, file still there:', await sandbox.readFile({ path: '/tmp/hello.txt' }));
    } else {
      console.log('Pause and resume: skipped, this provider does not report pause_resume');
    }
  } finally {
    await sandbox.delete();
  }
} finally {
  if (client instanceof SandboxWsClient) {
    client.close();
  }
}
