import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { runCli } from '../src/cli.js';

describe('config profile round-trip', () => {
  let tmpDir: string;
  let realHomeConfig: string;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'sandbox-cli-config-'));
    process.env.SANDBOX_CLI_CONFIG_HOME = tmpDir;
    realHomeConfig = path.join(os.homedir(), '.config', 'trogon-sandbox', 'config.json');
    vi.spyOn(process.stdout, 'write').mockImplementation(() => true);
    vi.spyOn(process.stderr, 'write').mockImplementation(() => true);
  });

  afterEach(() => {
    delete process.env.SANDBOX_CLI_CONFIG_HOME;
    fs.rmSync(tmpDir, { recursive: true, force: true });
    vi.restoreAllMocks();
  });

  it('writes login, reads/sets config, and removes the profile on logout, without touching the real home dir', async () => {
    const configPath = path.join(tmpDir, 'trogon-sandbox', 'config.json');
    const realConfigExistedBefore = fs.existsSync(realHomeConfig);
    const realConfigContentBefore = realConfigExistedBefore ? fs.readFileSync(realHomeConfig, 'utf8') : null;

    const loginExit = await runCli([
      'node',
      'sandbox',
      'login',
      '--gateway-http',
      'http://gw',
      '--gateway-ws',
      'ws://gw',
      '--api-key',
      'secret-key',
      '--tenant',
      'tenant-a',
    ]);
    expect(loginExit).toBe(0);
    expect(fs.existsSync(configPath)).toBe(true);

    const written = JSON.parse(fs.readFileSync(configPath, 'utf8'));
    expect(written.profiles.default).toMatchObject({
      gatewayHttp: 'http://gw',
      gatewayWs: 'ws://gw',
      apiKey: 'secret-key',
      tenant: 'tenant-a',
    });

    const mode = fs.statSync(configPath).mode & 0o777;
    expect(mode).toBe(0o600);

    const setExit = await runCli(['node', 'sandbox', 'config', 'set', 'sshGateway', 'ssh.example.com:2222']);
    expect(setExit).toBe(0);

    const stdoutSpy = vi.spyOn(process.stdout, 'write').mockImplementation(() => true);
    const getExit = await runCli(['node', 'sandbox', 'config', 'get', 'sshGateway']);
    expect(getExit).toBe(0);
    const printed = stdoutSpy.mock.calls.map((call: unknown[]) => call[0]).join('');
    expect(printed).toContain('ssh.example.com:2222');

    const logoutExit = await runCli(['node', 'sandbox', 'logout']);
    expect(logoutExit).toBe(0);
    const afterLogout = JSON.parse(fs.readFileSync(configPath, 'utf8'));
    expect(afterLogout.profiles.default).toBeUndefined();

    const realConfigExistedAfter = fs.existsSync(realHomeConfig);
    const realConfigContentAfter = realConfigExistedAfter ? fs.readFileSync(realHomeConfig, 'utf8') : null;
    expect(realConfigExistedAfter).toBe(realConfigExistedBefore);
    expect(realConfigContentAfter).toBe(realConfigContentBefore);
  });

  it('supports multiple named profiles independently', async () => {
    await runCli([
      'node',
      'sandbox',
      '--profile',
      'staging',
      'login',
      '--gateway-http',
      'http://staging',
      '--gateway-ws',
      'ws://staging',
      '--api-key',
      'staging-key',
    ]);
    await runCli([
      'node',
      'sandbox',
      '--profile',
      'prod',
      'login',
      '--gateway-http',
      'http://prod',
      '--gateway-ws',
      'ws://prod',
      '--api-key',
      'prod-key',
    ]);

    const configPath = path.join(tmpDir, 'trogon-sandbox', 'config.json');
    const written = JSON.parse(fs.readFileSync(configPath, 'utf8'));
    expect(written.profiles.staging.apiKey).toBe('staging-key');
    expect(written.profiles.prod.apiKey).toBe('prod-key');
  });
});
