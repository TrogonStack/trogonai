import { parseArgs } from 'node:util';

import { createSandboxTools } from '@trogonai/sandbox-agent-tools';
import { toAiSdkTools } from '@trogonai/sandbox-agent-tools/ai-sdk';
import { MemorySandboxClient, Sandbox, SandboxWsClient, scrubSecrets, tenantId } from '@trogonai/sandbox-ws-client';
import type { SandboxClientApi } from '@trogonai/sandbox-ws-client';
import { generateText, stepCountIs, tool } from 'ai';
import type { ToolSet } from 'ai';
import { z } from 'zod';

import { logger } from './logger.ts';
import { withFakeRepo } from './memory-repo.ts';
import { pickModel } from './model.ts';

const argumentsSchema = z.object({ task: z.string().trim().min(1), repo: z.string().min(1) });
const providerSchema = z.object({
  SANDBOX_PROVIDER: z.enum(['local_docker', 'memory']).default('local_docker'),
  SANDBOX_TENANT: z.string().min(1).default('demo'),
});
const gatewaySchema = z.object({
  SANDBOX_GATEWAY_HTTP_URL: z.string().url(),
  SANDBOX_GATEWAY_WS_URL: z.string().url(),
  SANDBOX_API_KEY: z.string().min(1),
});
const commandResultSchema = z.object({ exitCode: z.number().int(), stderr: z.string(), timedOut: z.boolean() });
const statusSchema = z.object({
  clean: z.boolean(),
  entries: z.array(z.object({ index: z.string(), worktree: z.string(), path: z.string() })),
});
const diffSchema = z.object({ diff: z.string() });

function printable(message: string): string {
  let text = message;
  for (const name of ['SANDBOX_API_KEY', 'SANDBOX_EXAMPLE_MODEL_API_KEY', 'GIT_TOKEN']) {
    const secret = process.env[name];
    if (secret) text = text.replaceAll(secret, '[REDACTED]');
  }
  return scrubSecrets(text);
}

async function main(): Promise<void> {
  const { values, positionals } = parseArgs({ allowPositionals: true, options: { repo: { type: 'string' } } });
  const parsed = argumentsSchema.safeParse({ task: positionals[0], repo: values.repo });
  if (!parsed.success) {
    logger.error('usage: pnpm coding-agent "<task>" --repo <git url>');
    process.exitCode = 2;
    return;
  }
  const config = providerSchema.parse(process.env);
  const memory = config.SANDBOX_PROVIDER === 'memory';
  const gateway = memory ? undefined : gatewaySchema.parse(process.env);
  const model = pickModel();
  const client: SandboxClientApi =
    gateway === undefined
      ? withFakeRepo(new MemorySandboxClient())
      : await SandboxWsClient.connect({
          gatewayHttpUrl: gateway.SANDBOX_GATEWAY_HTTP_URL,
          gatewayWsUrl: gateway.SANDBOX_GATEWAY_WS_URL,
          apiKey: gateway.SANDBOX_API_KEY,
        });
  try {
    const sandbox = await Sandbox.create(client, {
      tenant: tenantId(config.SANDBOX_TENANT),
      providerPref: [memory ? 'mock' : 'local_docker'],
      image: {
        kind: 'registry',
        reference: { registry: 'docker.io', repository: 'library/buildpack-deps', tag: 'bookworm-scm' },
      },
      networkPolicy: 'allow-all',
    });
    try {
      const workspace = '/tmp/sandbox-coding-' + crypto.randomUUID();
      const repoDir = workspace + '/repo';
      await sandbox.mkDir(workspace);
      await sandbox.git.clone({ url: parsed.data.repo, path: repoDir, depth: 50, token: process.env.GIT_TOKEN });
      logger.info(`Cloned into ${repoDir}`);
      const tools = createSandboxTools(sandbox, {
        cwd: repoDir,
        allow: ['run_command', 'read_file', 'write_file', 'edit_file', 'find_files'],
      });
      const aiTools: ToolSet = Object.fromEntries(
        Object.entries(toAiSdkTools(tools)).map(([name, definition]) => [
          name,
          tool<unknown, string>({
            ...definition!,
            inputSchema: definition!.inputSchema,
            outputSchema: z.string(),
            execute: (input: unknown) => {
              logger.info(`> ${name}`);
              return definition!.execute(input);
            },
          }),
        ]),
      );
      const result = await generateText({
        model,
        tools: aiTools,
        stopWhen: stepCountIs(20),
        system:
          `You are a coding agent working in a git checkout at ${repoDir}. ` +
          'Use the tools to inspect and edit files. Do not commit or push. Finish with a one-paragraph summary.',
        prompt: parsed.data.task,
      });
      process.stdout.write(printable(z.string().parse(result.text)) + '\n');
      const added = await sandbox.runCommand({ cmd: 'git', args: ['add', '--intent-to-add', '.'], cwd: repoDir });
      const add = commandResultSchema.parse({
        exitCode: added.exitCode,
        stderr: added.stderr(),
        timedOut: added.timedOut,
      });
      if (add.exitCode !== 0 || add.timedOut)
        throw new Error('Unable to include new files in the final diff: ' + add.stderr);
      const status = statusSchema.parse(await sandbox.git.status({ cwd: repoDir }));
      process.stdout.write('Changed files:\n');
      for (const entry of status.entries)
        process.stdout.write(printable(`  ${(entry.index + entry.worktree).trim() || '?'} ${entry.path}`) + '\n');
      if (status.clean) process.stdout.write('  (none)\n');
      process.stdout.write(printable(diffSchema.parse(await sandbox.git.diff({ cwd: repoDir })).diff) + '\n');
    } finally {
      await sandbox.delete();
    }
  } finally {
    if (client instanceof SandboxWsClient) client.close();
  }
}

await main().catch((error: unknown) => {
  logger.error(printable(error instanceof Error ? error.message : String(error)));
  process.exitCode = 1;
});
