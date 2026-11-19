import type { SandboxClientApi } from './client-api.ts';
import type { LeaseId } from './ids.ts';
import { Command, CommandFinished } from './sandbox.ts';
import type { PollCommandResponse, ReadFileResponse, SandboxFileWrite, WriteFilesResponse } from './wire.ts';

function resolvePath(homeDir: string, path: string): string {
  return path.startsWith('/') ? path : `${homeDir}/${path}`;
}

export class SandboxUser {
  readonly username: string;
  readonly homeDir: string;

  constructor(
    private readonly client: SandboxClientApi,
    private readonly leaseId: LeaseId,
    username: string,
  ) {
    this.username = username;
    this.homeDir = `/home/${username}`;
  }

  runCommand(
    command: string[],
    opts: { cwd?: string; env?: Record<string, string>; timeoutSeconds?: number; detached: true },
  ): Promise<Command>;
  runCommand(
    command: string[],
    opts?: { cwd?: string; env?: Record<string, string>; timeoutSeconds?: number; detached?: false },
  ): Promise<CommandFinished>;
  async runCommand(
    command: string[],
    opts?: { cwd?: string; env?: Record<string, string>; timeoutSeconds?: number; detached?: boolean },
  ): Promise<Command | CommandFinished> {
    const response = await this.client.runCommand(this.leaseId, command, {
      cwd: opts?.cwd ?? this.homeDir,
      env: opts?.env,
      timeoutSeconds: opts?.timeoutSeconds,
      detached: opts?.detached,
      runAsUser: this.username,
    });
    return Command.fromRunCommandResponse(this.client, this.leaseId, response);
  }

  async pollCommand(executionId: string): Promise<PollCommandResponse> {
    return this.client.pollCommand(this.leaseId, executionId);
  }

  async writeFiles(files: SandboxFileWrite[]): Promise<WriteFilesResponse> {
    const resolved = files.map((file) => ({ ...file, path: resolvePath(this.homeDir, file.path) }));
    return this.client.writeFiles(this.leaseId, resolved, { asUser: this.username });
  }

  async readFile(path: string): Promise<ReadFileResponse> {
    return this.client.readFile(this.leaseId, resolvePath(this.homeDir, path), { asUser: this.username });
  }

  // Sugar: there is no dedicated mkdir wire verb, so this shells out to `mkdir -p`.
  async mkDir(path: string): Promise<void> {
    const result = await this.runCommand(['mkdir', '-p', resolvePath(this.homeDir, path)]);
    if (result.exitCode !== 0) {
      throw new Error(`mkDir failed with exit code ${result.exitCode}: ${result.stderr()}`);
    }
  }

  async addToGroup(groupname: string): Promise<void> {
    await this.client.addUserToGroup(this.leaseId, this.username, groupname);
  }

  async removeFromGroup(groupname: string): Promise<void> {
    await this.client.removeUserFromGroup(this.leaseId, this.username, groupname);
  }
}
