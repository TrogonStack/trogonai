import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';

export interface ProfileConfig {
  gatewayHttp?: string;
  gatewayWs?: string;
  apiKey?: string;
  tenant?: string;
  sshGateway?: string;
}

export interface ConfigFile {
  profiles: Record<string, ProfileConfig>;
  activeProfile?: string;
}

export const DEFAULT_PROFILE_NAME = 'default';

/**
 * Resolves the config file path from an explicit environment snapshot, so
 * tests can point this at a temp directory via `SANDBOX_CLI_CONFIG_HOME`
 * instead of touching the real home directory.
 *
 * Resolution order: `env.SANDBOX_CLI_CONFIG_HOME` (test override), then
 * `env.XDG_CONFIG_HOME`, then `~/.config`. In every case the file lives at
 * `<base>/trogon-sandbox/config.json`.
 */
export function resolveConfigPath(env: NodeJS.ProcessEnv = process.env): string {
  const override = env.SANDBOX_CLI_CONFIG_HOME;
  if (override) {
    return path.join(override, 'trogon-sandbox', 'config.json');
  }
  const xdgHome = env.XDG_CONFIG_HOME;
  if (xdgHome) {
    return path.join(xdgHome, 'trogon-sandbox', 'config.json');
  }
  return path.join(os.homedir(), '.config', 'trogon-sandbox', 'config.json');
}

export function readConfig(configPath: string): ConfigFile {
  if (!fs.existsSync(configPath)) {
    return { profiles: {} };
  }
  const raw = fs.readFileSync(configPath, 'utf8');
  const parsed = JSON.parse(raw) as ConfigFile;
  return { profiles: parsed.profiles ?? {}, activeProfile: parsed.activeProfile };
}

export function writeConfig(configPath: string, config: ConfigFile): void {
  const dir = path.dirname(configPath);
  fs.mkdirSync(dir, { recursive: true });
  const json = JSON.stringify(config, null, 2);
  fs.writeFileSync(configPath, json, { mode: 0o600 });
  fs.chmodSync(configPath, 0o600);
}

export function getProfile(config: ConfigFile, profileName: string): ProfileConfig {
  return config.profiles[profileName] ?? {};
}

export function setProfile(config: ConfigFile, profileName: string, profile: ProfileConfig): ConfigFile {
  return {
    ...config,
    profiles: { ...config.profiles, [profileName]: profile },
  };
}

export function removeProfile(config: ConfigFile, profileName: string): ConfigFile {
  const profiles = { ...config.profiles };
  delete profiles[profileName];
  return { ...config, profiles };
}
