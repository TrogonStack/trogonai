import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { test } from 'node:test';
import ts from 'typescript';
import { extract } from '../upstream_typescript.mjs';

const source = `throw new Error('declarations must not execute');
export type PermissionMode = 'default' | 'plan';
export type SettingSource = 'user' | 'project';
export type Options = {
  model?: string;
  readonly env: Record<string, string>;
  callback?: (input: { value: string }) => Promise<boolean>;
};`;
const read = (text = source) => {
  const bytes = Buffer.from(text);
  return extract(bytes, createHash('sha256').update(bytes).digest('hex'));
};
const rejects = (text, code = 'unsupported-format') =>
  assert.throws(() => read(text), error => error.code === code && typeof error.selector === 'string');

test('uses exact pinned compiler and retains fields, modifiers, and literal aliases', () => {
  assert.equal(ts.version, '6.0.3');
  const result = read();
  assert.deepEqual(result['Options.model'], { type: 'string', optional: true, readonly: false });
  assert.deepEqual(result['Options.env'], { type: 'Record<string, string>', optional: false, readonly: true });
  assert.match(result['Options.callback'].type, /Promise<boolean>/);
  assert.deepEqual(result.PermissionMode.literals, ['default', 'plan']);
  assert.deepEqual(result['SettingSource.user'], { kind: 'literal' });
});

test('rejects a source hash mismatch and oversized input before parsing', () => {
  for (const bytes of [Buffer.from(source), Buffer.alloc(2 * 1024 * 1024 + 1)]) {
    assert.throws(() => extract(bytes, '0'.repeat(64)), error => error.code === 'source-pin-mismatch');
  }
});

test('rejects invalid UTF-8 and invalid syntax as typed extraction failures', () => {
  const bytes = Buffer.from([0xff]);
  assert.throws(() => extract(bytes, createHash('sha256').update(bytes).digest('hex')),
    error => error.code === 'upstream-extraction-failed');
  rejects('export type Options = {', 'upstream-extraction-failed');
});

test('rejects missing, duplicate, nonexported, and unsupported selected declarations', () => {
  for (const text of [
    source.replace("export type SettingSource = 'user' | 'project';", ''),
    source + "\nexport type PermissionMode = 'again';",
    source.replace('export type Options', 'type Options'),
    source.replace('model?: string;', 'method(): string;'),
    source.replace('model?: string;', '[key: string]: string;'),
    source.replace('model?: string;', 'model?: string; model: number;'),
    source.replace("'default' | 'plan'", "1 | 'plan'"),
    source.replace("'default' | 'plan'", "'plan' | 'plan'"),
  ]) rejects(text);
});

test('rejects competing selected type declarations, generic aliases, and initializers', () => {
  for (const text of [
    source + '\nexport interface Options { hidden: number }',
    source + '\nexport class PermissionMode {}',
    source.replace('type Options =', 'type Options<T> ='),
    source.replace('model?: string;', 'model?: string = "default";'),
  ]) rejects(text);
});
