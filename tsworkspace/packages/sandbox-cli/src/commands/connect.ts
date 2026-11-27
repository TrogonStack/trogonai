import { spawn } from 'node:child_process';

import { Command } from 'commander';

import { resolveConnection, type GlobalOptions } from '../context.js';

interface ConnectOptions extends GlobalOptions {
  sshGateway?: string;
}

function parseHostPort(hostArg: string): { host: string; port: number } {
  const colon = hostArg.lastIndexOf(':');
  if (colon <= 0) {
    return { host: hostArg, port: 22 };
  }
  const maybePort = Number(hostArg.slice(colon + 1));
  if (!Number.isFinite(maybePort)) {
    return { host: hostArg, port: 22 };
  }
  return { host: hostArg.slice(0, colon), port: maybePort };
}

export function registerConnectCommand(program: Command): void {
  program
    .command('connect <lease-id>')
    .description(
      'Open an interactive shell into a sandbox lease via trogon-sandbox-ssh-gateway. There is no native TTY support ' +
        'in the WS protocol, so this shells out to the local `ssh` binary. The SSH username convention is ' +
        '<lease-id>@gw (trogon-sandbox-ssh-gateway parses everything before the first "@" as the lease id); the ' +
        'bearer API key must be pasted as the SSH password when prompted. Requires --ssh-gateway <host[:port]> ' +
        'or the sshGateway config key (default port 22); there is no fixed default host.',
    )
    .option('--ssh-gateway <host[:port]>', 'trogon-sandbox-ssh-gateway host, optionally with :port (default port 22)')
    .action(async (leaseIdArg: string, opts: ConnectOptions, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions & ConnectOptions;
      const connection = resolveConnection(global);
      const hostArg = opts.sshGateway ?? connection.sshGateway;
      if (!hostArg) {
        throw new Error(
          'missing SSH gateway host: pass --ssh-gateway <host[:port]> or run `sandbox config set sshGateway <host[:port]>`',
        );
      }
      const { host, port } = parseHostPort(hostArg);
      const sshUser = `${leaseIdArg}@gw`;
      const sshTarget = `${sshUser}@${host}`;
      const args = ['-p', String(port), sshTarget];

      process.stdout.write(`bearer API key must be pasted as the SSH password when prompted\n`);
      process.stdout.write(`$ ssh ${args.join(' ')}\n`);

      const child = spawn('ssh', args, { stdio: 'inherit' });
      await new Promise<void>((resolve, reject) => {
        child.on('error', (err: NodeJS.ErrnoException) => {
          if (err.code === 'ENOENT') {
            process.stdout.write(
              `ssh binary not found on PATH. Run the command above manually once ssh is available, ` +
                `or install an SSH client and retry.\n`,
            );
            resolve();
            return;
          }
          reject(err);
        });
        child.on('exit', (code) => {
          process.exitCode = code ?? 0;
          resolve();
        });
      });
    });
}
