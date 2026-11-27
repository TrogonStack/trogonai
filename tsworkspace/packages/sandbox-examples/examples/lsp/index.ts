import { Sandbox, SandboxWsClient, tenantId } from '@trogonai/sandbox-ws-client';

const client = await SandboxWsClient.connect({
  gatewayHttpUrl: process.env.SANDBOX_GATEWAY_HTTP_URL!,
  gatewayWsUrl: process.env.SANDBOX_GATEWAY_WS_URL!,
  apiKey: process.env.SANDBOX_API_KEY!,
});

try {
  const sandbox = await Sandbox.create(client, {
    tenant: tenantId(process.env.SANDBOX_TENANT!),
    image: {
      kind: 'registry',
      reference: { registry: 'docker.io', repository: 'library/node', tag: '22-bookworm-slim' },
    },
    providerPref: ['local_docker'],
    networkPolicy: 'allow-all',
  });

  const projectDir = '/workspace/project';
  const source = `export function greet(name: string): string {
  return \`Hello, \${name}!\`;
}

const message = greet('World');
message.
`;

  try {
    await sandbox.fs.mkdir(projectDir);
    const installing = await sandbox.runCommand({
      cmd: 'sh',
      args: ['-c', 'npm install -g typescript-language-server && npm install typescript@5.9.3'],
      cwd: projectDir,
      detached: true,
      timeoutSeconds: 300,
    });
    const install = await installing.wait();
    if (install.exitCode !== 0) {
      throw new Error(`installing the language server failed: ${install.stderr()}`);
    }
    await sandbox.writeFiles([{ path: `${projectDir}/main.ts`, content: source }]);

    const lsp = sandbox.createLspServer('typescript', projectDir);
    await lsp.start();
    try {
      await lsp.didOpen('main.ts', source);

      const symbols = await lsp.documentSymbols('main.ts');
      console.log(
        'Symbols:',
        symbols?.map((symbol) => symbol.name),
      );

      const completions = await lsp.completions('main.ts', { line: 5, character: 8 });
      console.log(
        'Completions:',
        completions.slice(0, 10).map((item) => item.label),
      );
    } finally {
      await lsp.stop();
    }
  } finally {
    await sandbox.delete();
  }
} finally {
  client.close();
}
