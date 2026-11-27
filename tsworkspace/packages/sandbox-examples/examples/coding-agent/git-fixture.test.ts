import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { promisify } from 'node:util';

import { createGitFixture } from './test-support/git-fixture.ts';

test('the owned Git HTTP fixture supports the real shallow clone and denies writes', { timeout: 30_000 }, async () => {
  const fixture = await createGitFixture();
  const destination = await mkdtemp(join(tmpdir(), 'sandbox-coding-clone-'));
  try {
    await promisify(execFile)('git', ['clone', '--depth', '50', fixture.hostUrl, join(destination, 'repo')], {
      timeout: 10_000,
    });
    assert.equal(await readFile(join(destination, 'repo', 'greeting.txt'), 'utf8'), 'Hello, there!\n');
    assert.equal((await fetch(fixture.hostUrl + '/info/refs?service=git-receive-pack')).status, 403);
    assert.equal(
      (await fetch(fixture.hostUrl + '/git-receive-pack', { method: 'POST', body: 'deny write' })).status,
      403,
    );
  } finally {
    try {
      await fixture.close();
    } finally {
      await rm(destination, { recursive: true, force: true });
    }
  }
  await assert.rejects(() => fetch(fixture.hostUrl));
});
