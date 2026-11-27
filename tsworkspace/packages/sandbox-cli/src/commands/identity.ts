import { leaseId } from '@trogonai/sandbox-ws-client';
import { Command } from 'commander';

import { connectClient, type GlobalOptions } from '../context.js';

export function registerUserCommand(program: Command): void {
  const user = program.command('user').description('Manage sandbox Linux users');

  user
    .command('create <lease-id> <username>')
    .description('Create a Linux user inside a sandbox')
    .action(async (leaseIdArg: string, username: string, _opts: unknown, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const created = await client.createUser(leaseId(leaseIdArg), username);
        process.stdout.write(`created user "${created.username}" (home: ${created.homeDir})\n`);
      } finally {
        client.close();
      }
    });
}

export function registerGroupCommand(program: Command): void {
  const group = program.command('group').description('Manage sandbox Linux groups');

  group
    .command('create <lease-id> <group-name>')
    .description('Create a Linux group inside a sandbox')
    .action(async (leaseIdArg: string, groupName: string, _opts: unknown, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const { sharedDir } = await client.createGroup(leaseId(leaseIdArg), groupName);
        process.stdout.write(`created group "${groupName}" (shared dir: ${sharedDir})\n`);
      } finally {
        client.close();
      }
    });

  group
    .command('add <lease-id> <user> <group>')
    .description('Add a user to a group')
    .action(async (leaseIdArg: string, username: string, groupName: string, _opts: unknown, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        await client.addUserToGroup(leaseId(leaseIdArg), username, groupName);
        process.stdout.write(`added "${username}" to group "${groupName}"\n`);
      } finally {
        client.close();
      }
    });

  group
    .command('remove <lease-id> <user> <group>')
    .description('Remove a user from a group')
    .action(async (leaseIdArg: string, username: string, groupName: string, _opts: unknown, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        await client.removeUserFromGroup(leaseId(leaseIdArg), username, groupName);
        process.stdout.write(`removed "${username}" from group "${groupName}"\n`);
      } finally {
        client.close();
      }
    });
}
