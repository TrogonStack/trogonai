import { execFileSync, spawn } from 'node:child_process';
import type { ChildProcess } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import type { IncomingMessage } from 'node:http';
import { devNull, tmpdir } from 'node:os';
import { join } from 'node:path';

import { z } from 'zod';

const configSchema = z.object({
  dockerHost: z
    .string()
    .regex(/^[A-Za-z0-9.-]+$/)
    .default('host.docker.internal'),
});
const MAX_REQUEST = 1024 * 1024;
const MAX_RESPONSE = 2 * 1024 * 1024;

function gitEnvironment(): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...process.env };
  for (const name of Object.keys(env)) {
    if (name.startsWith('GIT_') || ['SANDBOX_API_KEY', 'SANDBOX_EXAMPLE_MODEL_API_KEY'].includes(name))
      delete env[name];
  }
  Object.assign(env, {
    GIT_CONFIG_NOSYSTEM: '1',
    GIT_CONFIG_GLOBAL: devNull,
    GIT_CONFIG_SYSTEM: devNull,
    GIT_TERMINAL_PROMPT: '0',
  });
  return env;
}

function stop(child: ChildProcess): void {
  if (child.exitCode !== null || child.signalCode !== null || child.pid === undefined) return;
  try {
    if (process.platform === 'win32') child.kill('SIGTERM');
    else process.kill(-child.pid, 'SIGTERM');
  } catch (error) {
    if (!(error instanceof Error && 'code' in error && error.code === 'ESRCH')) throw error;
  }
}

async function bodyOf(request: IncomingMessage): Promise<Buffer> {
  const chunks: Buffer[] = [];
  let length = 0;
  for await (const chunk of request) {
    const bytes = Buffer.from(chunk);
    length += bytes.length;
    if (length > MAX_REQUEST) throw new Error('Git fixture request exceeded its bound');
    chunks.push(bytes);
  }
  return Buffer.concat(chunks);
}

export async function createGitFixture(options: { dockerHost?: string } = {}) {
  const config = configSchema.parse(options);
  const directory = await mkdtemp(join(tmpdir(), 'sandbox-coding-fixture-'));
  const repository = join(directory, 'demo.git');
  const children = new Set<ChildProcess>();
  const env = gitEnvironment();
  const git = (args: string[], input?: string): string =>
    z
      .string()
      .parse(
        execFileSync('git', args, {
          env,
          encoding: 'utf8',
          input,
          timeout: 10_000,
          maxBuffer: MAX_RESPONSE,
          stdio: ['pipe', 'pipe', 'pipe'],
        }),
      )
      .trim();
  try {
    git(['init', '--bare', '--initial-branch=main', repository]);
    const blob = git(['--git-dir', repository, 'hash-object', '-w', '--stdin'], 'Hello, there!\n');
    const tree = git(['--git-dir', repository, 'mktree'], `100644 blob ${blob}\tgreeting.txt\n`);
    const commit = git(
      [
        '--git-dir',
        repository,
        '-c',
        'user.name=Sandbox fixture',
        '-c',
        'user.email=sandbox@localhost',
        '-c',
        'commit.gpgSign=false',
        'commit-tree',
        tree,
      ],
      'Seed coding fixture\n',
    );
    git(['--git-dir', repository, 'update-ref', 'refs/heads/main', commit]);
    git(['--git-dir', repository, 'config', 'http.receivepack', 'false']);
    const server = createServer(async (request, response) => {
      try {
        const url = new URL(request.url ?? '/', 'http://fixture.invalid');
        const method = z.enum(['GET', 'POST']).safeParse(request.method);
        const service = url.searchParams.get('service');
        if (
          !method.success ||
          !/^\/demo\.git\/(?:HEAD|info\/refs|git-upload-pack)$/.test(url.pathname) ||
          (service !== null && service !== 'git-upload-pack') ||
          (method.data === 'POST' && url.pathname !== '/demo.git/git-upload-pack')
        ) {
          response.writeHead(403).end('read-only Git fixture');
          return;
        }
        const body = await bodyOf(request);
        const output = await new Promise<Buffer>((resolve, reject) => {
          const child = spawn('git', ['http-backend'], {
            env: {
              ...env,
              GIT_PROJECT_ROOT: directory,
              GIT_HTTP_EXPORT_ALL: '1',
              PATH_INFO: url.pathname,
              QUERY_STRING: url.search.slice(1),
              REQUEST_METHOD: method.data,
              CONTENT_LENGTH: String(body.length),
              CONTENT_TYPE: String(request.headers['content-type'] ?? ''),
              SERVER_PROTOCOL: 'HTTP/1.1',
              REMOTE_ADDR: request.socket.remoteAddress ?? '127.0.0.1',
              HTTP_GIT_PROTOCOL: String(request.headers['git-protocol'] ?? ''),
            },
            stdio: ['pipe', 'pipe', 'pipe'],
            detached: process.platform !== 'win32',
          });
          children.add(child);
          let length = 0;
          let stderr = 0;
          const chunks: Buffer[] = [];
          const timeout = setTimeout(() => {
            stop(child);
            reject(new Error('Git fixture backend timed out'));
          }, 10_000);
          const abort = () => stop(child);
          request.once('aborted', abort);
          child.stdout.on('data', (chunk: Buffer) => {
            length += chunk.length;
            if (length > MAX_RESPONSE) {
              stop(child);
              reject(new Error('Git fixture response exceeded its bound'));
            } else chunks.push(chunk);
          });
          child.stderr.on('data', (chunk: Buffer) => {
            stderr += chunk.length;
            if (stderr > MAX_RESPONSE) stop(child);
          });
          child.once('error', reject);
          child.once('close', (code) => {
            clearTimeout(timeout);
            children.delete(child);
            request.off('aborted', abort);
            if (code === 0) resolve(Buffer.concat(chunks));
            else reject(new Error('Git fixture backend failed'));
          });
          child.stdin.on('error', () => {});
          child.stdin.end(body);
        });
        const boundary = output.indexOf('\r\n\r\n');
        if (boundary < 0) throw new Error('Git fixture returned invalid CGI headers');
        let status = 200;
        for (const line of output.subarray(0, boundary).toString('utf8').split('\r\n')) {
          const colon = line.indexOf(':');
          if (colon < 0) throw new Error('Git fixture returned an invalid header');
          const name = line.slice(0, colon);
          const value = line.slice(colon + 1).trim();
          if (name.toLowerCase() === 'status')
            status = z.number().int().min(100).max(599).parse(Number.parseInt(value, 10));
          else response.setHeader(name, value);
        }
        response.writeHead(status).end(output.subarray(boundary + 4));
      } catch {
        if (!response.headersSent) response.writeHead(500);
        response.end('Git fixture failed');
      }
    });
    server.requestTimeout = 15_000;
    server.headersTimeout = 10_000;
    await new Promise<void>((resolve, reject) => {
      server.once('error', reject);
      server.listen(0, '0.0.0.0', resolve);
    });
    const address = server.address();
    if (address === null || typeof address === 'string') throw new Error('Git fixture has no TCP listener');
    let closed = false;
    return {
      hostUrl: `http://127.0.0.1:${address.port}/demo.git`,
      dockerUrl: `http://${config.dockerHost}:${address.port}/demo.git`,
      async close(): Promise<void> {
        if (closed) return;
        closed = true;
        try {
          for (const child of children) stop(child);
          server.closeAllConnections();
          await new Promise<void>((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
        } finally {
          await rm(directory, { recursive: true, force: true });
        }
      },
    };
  } catch (error) {
    for (const child of children) stop(child);
    await rm(directory, { recursive: true, force: true });
    throw error;
  }
}
