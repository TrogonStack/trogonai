import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { test } from 'node:test';

import { WORKDIR } from './sandbox.ts';

test('the app workspace is writable under the Docker worker non-root defaults', () => {
  const probe = [
    'import { mkdir, writeFile } from "node:fs/promises";',
    'const path = process.argv[1];',
    'await mkdir(path, { recursive: true });',
    'await writeFile(path + "/app-probe.txt", "app-workspace");',
  ].join('\n');
  const name = 'sandbox-app-workspace-' + crypto.randomUUID();
  let primaryFailure: unknown;
  try {
    const result = spawnSync(
      'docker',
      [
        'run',
        '--name',
        name,
        '--rm',
        '--network',
        'none',
        '--user',
        '65532:65532',
        '--cap-drop',
        'ALL',
        '--security-opt',
        'no-new-privileges',
        '--memory',
        '256m',
        '--pids-limit',
        '64',
        'node:22-bookworm-slim',
        'node',
        '--input-type=module',
        '-e',
        probe,
        WORKDIR,
      ],
      { encoding: 'utf8', timeout: 20_000 },
    );
    assert.equal(result.error, undefined);
    assert.equal(result.status, 0, result.stderr);
  } catch (error) {
    primaryFailure = error;
    throw error;
  } finally {
    const cleanup = spawnSync('docker', ['rm', '--force', name], { encoding: 'utf8', timeout: 10_000 });
    const removed = cleanup.error === undefined && (cleanup.status === 0 || /No such container/.test(cleanup.stderr));
    if (primaryFailure === undefined) assert.ok(removed, 'the named probe container must be removed');
  }
});
