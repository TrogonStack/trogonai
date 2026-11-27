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
    image: {
      kind: 'registry',
      reference: { registry: 'docker.io', repository: 'library/buildpack-deps', tag: 'bookworm-scm' },
    },
  });

  async function git(args: string[], cwd?: string): Promise<string> {
    const result = await sandbox.runCommand({ cmd: 'git', args, cwd });
    if (result.exitCode !== 0) {
      throw new Error(`git ${args.join(' ')} failed with exit code ${result.exitCode}: ${result.stderr()}`);
    }
    return result.stdout();
  }

  try {
    const rootDir = (await sandbox.runCommand('pwd')).stdout().trim();
    const projectDir = `${rootDir}/learn-typescript`;

    await git(['clone', '--branch', 'master', 'https://github.com/panaverse/learn-typescript', projectDir]);
    await git(['pull'], projectDir);
  } catch (error) {
    console.error('Error:', error);
    process.exitCode = 1;
  } finally {
    await sandbox.delete();
  }
} finally {
  client.close();
}
