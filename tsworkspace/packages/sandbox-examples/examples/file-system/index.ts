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

  async function sh(script: string): Promise<string> {
    const result = await sandbox.runCommand('sh', ['-c', script]);
    if (result.exitCode !== 0) {
      throw new Error(`${script} failed with exit code ${result.exitCode}: ${result.stderr()}`);
    }
    return result.stdout();
  }

  try {
    const rootDir = (await sh('pwd')).trim();
    console.log('Files:', await sandbox.fs.ls(rootDir));

    const newDir = `${rootDir}/new-dir`;
    await sandbox.fs.mkdir(newDir);
    await sandbox.fs.chmod(newDir, 0o755);

    const filePath = `${newDir}/data.txt`;
    await sandbox.writeFiles([{ path: filePath, content: 'Hello, World!' }]);

    console.log('Matches:', await sh(`grep -rln 'World!' ${rootDir}`));

    await sh(`sed -i 's/Hello, World!/Goodbye, World!/' ${filePath}`);
    console.log('File content:', await sandbox.readFile({ path: filePath }));

    await sandbox.fs.chmod(filePath, 0o777);
    console.log('File info:', await sandbox.fs.stat(filePath));

    const newFilePath = `${rootDir}/moved-data.txt`;
    await sandbox.fs.mv(filePath, newFilePath);
    console.log('Search results:', await sh(`find ${rootDir} -name moved-data.txt`));

    await sandbox.fs.rm(newFilePath);
  } finally {
    await sandbox.delete();
  }
} finally {
  client.close();
}
