import { spawnSync } from 'node:child_process';

import { describe, expect, it } from 'vitest';

import { tenantId } from '../src/ids.ts';
import { MemorySandboxClient } from '../src/memory.ts';
import { Sandbox } from '../src/sandbox.ts';
import { raw, shellQuote, shellTemplate, shellValue } from '../src/shell.ts';

const sh = (strings: TemplateStringsArray, ...values: unknown[]) => shellTemplate(strings, ...values);

/** Runs `printf '%s\0' <words>` through a real sh and returns the words it received. */
function words(script: string): string[] {
  const result = spawnSync('sh', ['-c', `printf '%s\\0' ${script}`], { encoding: 'utf8' });
  expect(result.status).toBe(0);
  const parts = result.stdout.split('\0');
  parts.pop();
  return parts;
}

describe('shellQuote', () => {
  it('leaves plain words bare', () => {
    expect(shellQuote('abc')).toBe('abc');
    expect(shellQuote('/usr/local/bin/x-1.2_3')).toBe('/usr/local/bin/x-1.2_3');
    expect(shellQuote('a@b%c+d:e,f')).toBe('a@b%c+d:e,f');
  });

  it('quotes the empty string', () => {
    expect(shellQuote('')).toBe("''");
  });

  it('quotes spaces and shell metacharacters', () => {
    expect(shellQuote('a b')).toBe("'a b'");
    expect(shellQuote('$(rm -rf /)')).toBe("'$(rm -rf /)'");
    expect(shellQuote('`id`')).toBe("'`id`'");
    expect(shellQuote('a;b|c&d')).toBe("'a;b|c&d'");
    expect(shellQuote('*.txt')).toBe("'*.txt'");
    expect(shellQuote('~root')).toBe("'~root'");
    expect(shellQuote('a=b')).toBe("'a=b'");
    expect(shellQuote('#c')).toBe("'#c'");
    expect(shellQuote('!x')).toBe("'!x'");
  });

  it('escapes single quotes', () => {
    expect(shellQuote("it's")).toBe(`'it'\\''s'`);
    expect(shellQuote("'")).toBe(`''\\'''`);
    expect(shellQuote("''")).toBe(`''\\'''\\'''`);
  });

  it('keeps double quotes and backslashes literal', () => {
    expect(shellQuote('say "hi"')).toBe(`'say "hi"'`);
    expect(shellQuote('a\\nb')).toBe(`'a\\nb'`);
  });

  it('keeps newlines literal inside the quotes', () => {
    expect(shellQuote('a\nb')).toBe("'a\nb'");
    expect(shellQuote('\n')).toBe("'\n'");
    expect(shellQuote('a\r\nb')).toBe("'a\r\nb'");
  });

  it('rejects NUL bytes', () => {
    expect(() => shellQuote('a\0b')).toThrow(TypeError);
  });

  it('round-trips hostile strings through a real sh', () => {
    const samples = [
      '',
      ' ',
      'plain',
      'with space',
      "it's",
      '"double"',
      'mix \' and "',
      '$HOME',
      '${HOME}',
      '$(echo pwned)',
      '`echo pwned`',
      'a;echo pwned',
      'a && echo pwned',
      'a | cat',
      'a > /dev/null',
      '*',
      '?',
      '[a-z]',
      '~',
      '-n',
      '--flag=value',
      '\\',
      '\\n',
      'line1\nline2',
      '\n\n',
      '\t',
      'unicode é中😀',
      '#not-a-comment',
      '!history',
      '{a,b}',
      'a=b',
    ];
    const script = samples.map((s) => shellQuote(s)).join(' ');
    expect(words(script)).toEqual(samples);
  });
});

describe('shellValue', () => {
  it('renders numbers, bigints and booleans', () => {
    expect(shellValue(42)).toBe('42');
    expect(shellValue(-1.5)).toBe('-1.5');
    expect(shellValue(10n)).toBe('10');
    expect(shellValue(true)).toBe('true');
  });

  it('rejects non-finite numbers', () => {
    expect(() => shellValue(Number.NaN)).toThrow(TypeError);
    expect(() => shellValue(Number.POSITIVE_INFINITY)).toThrow(TypeError);
  });

  it('rejects null, undefined, objects and functions', () => {
    expect(() => shellValue(null)).toThrow(/null/);
    expect(() => shellValue(undefined)).toThrow(/undefined/);
    expect(() => shellValue({})).toThrow(/object/);
    expect(() => shellValue(() => 1)).toThrow(/function/);
  });

  it('expands arrays to one quoted word per element', () => {
    expect(shellValue(['a', 'b c', ''])).toBe("a 'b c' ''");
    expect(shellValue([])).toBe('');
    expect(shellValue([['a', 'b'], 'c d'])).toBe("a b 'c d'");
    expect(shellValue(['x', 1, true])).toBe('x 1 true');
  });

  it('passes raw text through unquoted, also inside arrays', () => {
    expect(shellValue(raw('| wc -l'))).toBe('| wc -l');
    expect(shellValue(['a', raw('>'), 'b'])).toBe('a > b');
  });
});

describe('shellTemplate', () => {
  it('quotes every interpolation and keeps literal text', () => {
    const dir = 'my dir; rm -rf /';
    expect(sh`ls -la ${dir}`).toBe("ls -la 'my dir; rm -rf /'");
  });

  it('handles no interpolations and adjacent ones', () => {
    expect(sh`echo hi`).toBe('echo hi');
    expect(sh`${'a'}${'b c'}`).toBe("a'b c'");
  });

  it('handles an interpolation at the very start and end', () => {
    expect(sh`${'x y'}`).toBe("'x y'");
    expect(sh`echo ${''}`).toBe("echo ''");
  });

  it('yields the exact words a real sh receives', () => {
    const evil = ['a b', "c'd", '$(id)', '', 'e\nf', '*'];
    expect(words(sh`${evil}`)).toEqual(evil);
    const flag = '--name=x y';
    expect(words(sh`${flag} ${['--', '-x']}`)).toEqual(['--name=x y', '--', '-x']);
  });

  it('does not let a value escape into a second command', () => {
    const attack = "'; echo pwned; echo '";
    const result = spawnSync('sh', ['-c', sh`echo ${attack}`], { encoding: 'utf8' });
    expect(result.stdout).toBe(`${attack}\n`);
  });
});

describe('Sandbox.$', () => {
  it('runs sh -c with the quoted script', async () => {
    const seen: string[][] = [];
    const client = new MemorySandboxClient({
      commands: {
        sh: ({ args }) => {
          seen.push(args);
          return { exitCode: 0, stdout: 'ok\n' };
        },
      },
    });
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
    const result = await sandbox.$`ls ${['a b', "c'd"]}`;
    expect(seen).toEqual([['-c', "ls 'a b' 'c'\\''d'"]]);
    expect(result.text()).toBe('ok');
  });

  it('shellWith applies run params', async () => {
    let cwd = '';
    const client = new MemorySandboxClient({
      commands: {
        sh: (ctx) => {
          cwd = ctx.cwd;
          return { exitCode: 3 };
        },
      },
    });
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
    const result = await sandbox.shellWith({ cwd: '/work' })`false`;
    expect(cwd).toBe('/work');
    expect(result.ok).toBe(false);
    expect(() => result.throwIfFailed()).toThrow(/code 3/);
  });

  it('rejects before running when a value cannot be quoted', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
    await expect(sandbox.$`echo ${undefined}`).rejects.toBeInstanceOf(TypeError);
  });
});
