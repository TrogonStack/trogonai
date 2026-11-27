import { Command } from 'commander';

import {
  DEFAULT_PROFILE_NAME,
  getProfile,
  readConfig,
  removeProfile,
  resolveConfigPath,
  setProfile,
  writeConfig,
} from '../config.js';
import type { GlobalOptions } from '../context.js';

/**
 * `login`/`logout` deliberately reuse the root program's `--gateway-http`,
 * `--gateway-ws`, `--api-key`, `--tenant`, and `--profile` flags instead of
 * redeclaring them locally with `requiredOption`. Commander resolves an
 * option by flag name across the whole command chain; defining the same
 * flag twice (once globally, once on the subcommand) makes the subcommand's
 * own required-option check run against an option instance the parser never
 * populates, so the command fails with "required option not specified" even
 * when the flag is passed. Reusing the global option and validating
 * presence manually here avoids that collision entirely.
 */
export function registerLoginCommand(program: Command): void {
  program
    .command('login')
    .description('Store gateway connection details and API key for a profile')
    .action((_opts: unknown, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      if (!global.gatewayHttp) {
        throw new Error('missing --gateway-http');
      }
      if (!global.gatewayWs) {
        throw new Error('missing --gateway-ws');
      }
      if (!global.apiKey) {
        throw new Error('missing --api-key');
      }
      const profileName = global.profile ?? DEFAULT_PROFILE_NAME;
      const configPath = resolveConfigPath();
      const config = readConfig(configPath);
      const existing = getProfile(config, profileName);
      const next = setProfile(config, profileName, {
        ...existing,
        gatewayHttp: global.gatewayHttp,
        gatewayWs: global.gatewayWs,
        apiKey: global.apiKey,
        tenant: global.tenant ?? existing.tenant,
      });
      writeConfig(configPath, next);
      process.stdout.write(`saved profile "${profileName}" to ${configPath}\n`);
    });
}

export function registerLogoutCommand(program: Command): void {
  program
    .command('logout')
    .description('Remove a stored profile')
    .action((_opts: unknown, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const profileName = global.profile ?? DEFAULT_PROFILE_NAME;
      const configPath = resolveConfigPath();
      const config = readConfig(configPath);
      const next = removeProfile(config, profileName);
      writeConfig(configPath, next);
      process.stdout.write(`removed profile "${profileName}"\n`);
    });
}

const CONFIGURABLE_KEYS = ['tenant', 'gatewayHttp', 'gatewayWs', 'sshGateway'] as const;
type ConfigurableKey = (typeof CONFIGURABLE_KEYS)[number];

function assertConfigurableKey(key: string): asserts key is ConfigurableKey {
  if (!(CONFIGURABLE_KEYS as readonly string[]).includes(key)) {
    throw new Error(`unknown config key "${key}": expected one of ${CONFIGURABLE_KEYS.join(', ')}`);
  }
}

export function registerConfigCommand(program: Command): void {
  const config = program.command('config').description('Read or write the active profile config');

  config
    .command('get <key>')
    .description(`Print a config value (${CONFIGURABLE_KEYS.join(', ')})`)
    .action((key: string, _opts: unknown, command: Command) => {
      assertConfigurableKey(key);
      const global = command.optsWithGlobals() as GlobalOptions;
      const profileName = global.profile ?? DEFAULT_PROFILE_NAME;
      const configPath = resolveConfigPath();
      const fileConfig = readConfig(configPath);
      const profile = getProfile(fileConfig, profileName);
      const value = profile[key];
      process.stdout.write(`${value ?? ''}\n`);
    });

  config
    .command('set <key> <value>')
    .description(`Set a config value (${CONFIGURABLE_KEYS.join(', ')})`)
    .action((key: string, value: string, _opts: unknown, command: Command) => {
      assertConfigurableKey(key);
      const global = command.optsWithGlobals() as GlobalOptions;
      const profileName = global.profile ?? DEFAULT_PROFILE_NAME;
      const configPath = resolveConfigPath();
      const fileConfig = readConfig(configPath);
      const existing = getProfile(fileConfig, profileName);
      const next = setProfile(fileConfig, profileName, { ...existing, [key]: value });
      writeConfig(configPath, next);
      process.stdout.write(`set ${key} for profile "${profileName}"\n`);
    });
}
