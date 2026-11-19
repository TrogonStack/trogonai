import { describe, expect, it } from 'vitest';

import {
  MAX_REDACT_VALUES,
  MemorySandboxClient,
  REDACTION_MARKER,
  Sandbox,
  SandboxWsError,
  redactText,
  redactValuesSchema,
  tenantId,
} from '../src/index.ts';

const SECRET = 's3cr3t-token-value';

async function create() {
  const client = new MemorySandboxClient({
    commands: { leak: ({ env }) => ({ exitCode: 0, stdout: `token=${env.TOKEN}\n`, stderr: `again ${env.TOKEN}` }) },
  });
  const sandbox = await Sandbox.create(client, { tenant: tenantId('acme'), timeoutMs: 60_000 });
  return { client, sandbox };
}

describe('redactText', () => {
  it('replaces exact values, longest first, and ignores short ones', () => {
    expect(redactText(`a ${SECRET} b`, [SECRET])).toBe(`a ${REDACTION_MARKER} b`);
    expect(redactText('abc abc', ['abc'])).toBe('abc abc');
    expect(redactText('prefix-longer-value', ['prefix', 'prefix-longer-value'])).toBe(REDACTION_MARKER);
  });

  it('does not catch an encoded copy of a value', () => {
    const encoded = Buffer.from(SECRET).toString('base64');
    expect(redactText(encoded, [SECRET])).toBe(encoded);
  });
});

describe('redactValuesSchema', () => {
  it('rejects too many values', () => {
    const values = Array.from({ length: MAX_REDACT_VALUES + 1 }, (_, i) => `value-${i}-xxxx`);
    expect(redactValuesSchema.safeParse(values).success).toBe(false);
  });
});

describe('MemorySandboxClient redaction', () => {
  it('scrubs foreground output', async () => {
    const { sandbox } = await create();
    const result = await sandbox.runCommand({ cmd: 'leak', env: { TOKEN: SECRET }, redactValues: [SECRET] });
    expect(result.stdout()).toBe(`token=${REDACTION_MARKER}\n`);
    expect(result.stderr()).toBe(`again ${REDACTION_MARKER}`);
  });

  it('leaves output alone when no values are registered', async () => {
    const { sandbox } = await create();
    const result = await sandbox.runCommand({ cmd: 'leak', env: { TOKEN: SECRET } });
    expect(result.stdout()).toContain(SECRET);
  });

  it('rejects an oversized value list before dispatch', async () => {
    const { sandbox } = await create();
    const values = Array.from({ length: MAX_REDACT_VALUES + 1 }, (_, i) => `value-${i}-xxxx`);
    await expect(sandbox.runCommand({ cmd: 'leak', redactValues: values })).rejects.toBeInstanceOf(SandboxWsError);
  });
});
