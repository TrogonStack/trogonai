import { SandboxWsClient, tenantId, type TenantId } from '@trogonai/sandbox-ws-client';

import { DEFAULT_PROFILE_NAME, getProfile, readConfig, resolveConfigPath, type ProfileConfig } from './config.js';

export interface GlobalOptions {
  profile?: string;
  gatewayHttp?: string;
  gatewayWs?: string;
  apiKey?: string;
  tenant?: string;
  json?: boolean;
}

export interface ResolvedConnection {
  gatewayHttp: string;
  gatewayWs: string;
  apiKey: string;
  tenant?: string;
  sshGateway?: string;
}

function resolveProfile(global: GlobalOptions): ProfileConfig {
  const configPath = resolveConfigPath();
  const config = readConfig(configPath);
  const profileName = global.profile ?? DEFAULT_PROFILE_NAME;
  return getProfile(config, profileName);
}

export function resolveConnection(global: GlobalOptions): ResolvedConnection {
  const profile = resolveProfile(global);
  const gatewayHttp = global.gatewayHttp ?? profile.gatewayHttp;
  const gatewayWs = global.gatewayWs ?? profile.gatewayWs;
  const apiKey = global.apiKey ?? profile.apiKey;
  const tenant = global.tenant ?? profile.tenant;

  if (!gatewayHttp) {
    throw new Error('missing gateway HTTP URL: pass --gateway-http or run `sandbox login`');
  }
  if (!gatewayWs) {
    throw new Error('missing gateway WS URL: pass --gateway-ws or run `sandbox login`');
  }
  if (!apiKey) {
    throw new Error('missing API key: pass --api-key or run `sandbox login`');
  }

  return { gatewayHttp, gatewayWs, apiKey, tenant, sshGateway: profile.sshGateway };
}

export async function connectClient(global: GlobalOptions): Promise<{ client: SandboxWsClient; tenant?: TenantId }> {
  const connection = resolveConnection(global);
  const client = await SandboxWsClient.connect({
    gatewayHttpUrl: connection.gatewayHttp,
    gatewayWsUrl: connection.gatewayWs,
    apiKey: connection.apiKey,
  });
  return { client, tenant: connection.tenant ? tenantId(connection.tenant) : undefined };
}

export function requireTenant(tenant: TenantId | undefined): TenantId {
  if (!tenant) {
    throw new Error(
      'missing tenant: pass --tenant or set it via `sandbox config set tenant <t>` / `sandbox login --tenant`',
    );
  }
  return tenant;
}
