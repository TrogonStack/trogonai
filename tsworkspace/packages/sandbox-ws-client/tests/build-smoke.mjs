import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import { MemorySandboxClient, Sandbox, tenantId } from '@trogonai/sandbox-ws-client';
import ts from 'typescript';

test('the emitted package imports and preserves the public lifecycle contract', async () => {
  const client = new MemorySandboxClient();
  const name = 'compiled-consumer';
  const sandbox = await Sandbox.create(client, {
    tenant: tenantId('consumer'),
    timeoutMs: 60_000,
    name,
  });
  try {
    await sandbox.writeFiles([{ path: '/work/greeting.txt', content: 'hello' }]);
    assert.equal(await sandbox.readFile({ path: '/work/greeting.txt' }), 'hello');
  } finally {
    await sandbox.delete();
  }
  assert.equal(await Sandbox.deleteByName(client, name), false);
});

test('the emitted declarations preserve every public API type fixture', () => {
  const packageDirectory = resolve(dirname(fileURLToPath(import.meta.url)), '..');
  const fixtures = new Map(
    readdirSync('tests/types')
      .filter((name) => name.endsWith('.ts'))
      .map((name) => [
        join(packageDirectory, 'tests/types', name),
        readFileSync(join('tests/types', name), 'utf8').replaceAll('../../src/index.ts', '@trogonai/sandbox-ws-client'),
      ]),
  );
  assert.ok(fixtures.size > 0);
  const options = {
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.NodeNext,
    moduleResolution: ts.ModuleResolutionKind.NodeNext,
    strict: true,
    skipLibCheck: true,
    noEmit: true,
    types: ['node'],
    typeRoots: [join(packageDirectory, 'node_modules/@types')],
  };
  const host = ts.createCompilerHost(options);
  const originalGetSourceFile = host.getSourceFile.bind(host);
  host.getSourceFile = (file, languageVersion, onError, shouldCreateNewSourceFile) => {
    const source = fixtures.get(resolve(file));
    return source === undefined
      ? originalGetSourceFile(file, languageVersion, onError, shouldCreateNewSourceFile)
      : ts.createSourceFile(file, source, languageVersion, true);
  };
  const program = ts.createProgram({ rootNames: [...fixtures.keys()], options, host });
  assert.deepEqual(
    ts
      .getPreEmitDiagnostics(program)
      .map((diagnostic) => ts.flattenDiagnosticMessageText(diagnostic.messageText, '\n')),
    [],
  );
});
