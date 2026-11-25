#!/usr/bin/env node
// Records what the pinned TypeScript SDK's `SandboxNetworkConfigSchema`, `SandboxFilesystemConfigSchema` and
// `SandboxCredentialsConfigSchema` answer for a battery of documents, by running the real zod schemas.
//
// The factories live in the minified bundle, so the recorder copies the pinned package into a scratch directory,
// appends a line that exposes the three factories and calls `safeParse` on every battery entry. Each recorded entry
// holds the input as JSON text (`null` is `undefined`), whether the parse succeeded and the text `JSON.stringify`
// prints for the parsed data, which pins key order as well as values.
//
// Usage: sandbox_schema_battery.mjs --package DIR --out FILE [--check]
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const FACTORIES = { network: 'w6e', filesystem: 'v6e', credentials: 'k6e' };

const COMMON = [
  undefined, null, 1, 'x', true, [], ['a'], {}, { unknownKey: 1 },
];

export const BATTERY = {
  network: [
    ...COMMON,
    { allowedDomains: ['example.com'], deniedDomains: [] },
    { allowedDomains: [1] },
    { deniedDomains: 'example.com' },
    { strictAllowlist: true, allowManagedDomainsOnly: false },
    { strictAllowlist: 'yes' },
    { allowUnixSockets: ['/run/a.sock'], allowAllUnixSockets: true, allowLocalBinding: false },
    { allowUnixSockets: [null] },
    { allowMachLookup: ['com.apple.*', 'com.apple.x', '*'] },
    { allowMachLookup: ['a*b'] },
    { allowMachLookup: ['*a'] },
    { allowMachLookup: ['**'] },
    { allowMachLookup: [''] },
    { httpProxyPort: 8080, socksProxyPort: 1080 },
    { httpProxyPort: 1.5, socksProxyPort: -1 },
    { httpProxyPort: '8080' },
    { tlsTerminate: { caCertPath: '/a.pem', caKeyPath: '/a.key' } },
    { tlsTerminate: { caCertPath: '' } },
    { tlsTerminate: {} },
    { tlsTerminate: null },
    { allowedDomains: ['a'], extra: { nested: true }, httpProxyPort: 1 },
    { socksProxyPort: 2, httpProxyPort: 1, allowedDomains: [], deniedDomains: ['b'] },
  ],
  filesystem: [
    ...COMMON,
    { allowWrite: ['/tmp'], denyWrite: ['/etc'], denyRead: ['/root'], allowRead: ['/usr'] },
    { allowWrite: [1] },
    { denyRead: 'x' },
    { allowManagedReadPathsOnly: true, disabled: false },
    { disabled: 'no' },
    { disabled: true, extra: 1, allowRead: [] },
    { allowRead: ['/a'], allowWrite: ['/b'], extra: [1, 2] },
  ],
  credentials: [
    ...COMMON,
    { files: [] },
    { files: [{ path: '/a', mode: 'deny' }] },
    { files: [{ path: '/a', mode: 'mask' }] },
    { files: [{ path: '/a/', mode: 'mask' }] },
    { files: [{ path: '/a/', mode: 'deny' }] },
    { files: [{ path: '', mode: 'deny' }] },
    { files: [{ path: '/a', mode: 'other' }] },
    { files: [{ path: '/a', mode: 'deny', extract: 1, onExtractNoMatch: 'x', decode: 'x', maskClaims: [1], maskDuplicates: 'y', injectHosts: [1] }] },
    { files: [{ path: '/a', mode: 'deny', extract: 'a', onExtractNoMatch: 'warn', decode: 'jwt', maskClaims: ['a'], maskDuplicates: true, injectHosts: ['h'] }] },
    { files: [{ path: '/a', mode: 'mask', extract: 'a', onExtractNoMatch: 'error', decode: 'jwt', maskClaims: ['a'], maskDuplicates: true, injectHosts: ['h'] }] },
    { files: [{ path: '/a', mode: 'mask', extract: 'a' }] },
    { files: [{ path: '/a', mode: 'mask', extract: 'a', onExtractNoMatch: 'bogus' }] },
    { files: [{ path: '/a', mode: 'mask', maskClaims: [1] }] },
    { files: [{ mode: 'deny' }] },
    { files: [{ path: '/a' }] },
    { files: ['/a'] },
    { files: [null] },
    { files: 'x' },
    { files: [{ path: '/a', mode: 'deny', unknown: 1 }] },
    { files: [{ path: '/a', mode: 'deny' }, { path: '/b', mode: 'mask' }], extra: true },
  ],
};

const args = process.argv.slice(2);
const option = (name) => {
  const index = args.indexOf(name);
  return index === -1 ? undefined : args[index + 1];
};

async function main() {
  const pkg = option('--package');
  const out = option('--out');
  const check = args.includes('--check');
  if (!pkg || !out) {
    process.stderr.write('usage: sandbox_schema_battery.mjs --package DIR --out FILE [--check]\n');
    process.exit(2);
  }
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'sandbox-schema-'));
  try {
    fs.cpSync(pkg, scratch, { recursive: true });
    const bundle = path.join(scratch, 'sdk.mjs');
    const capture = Object.entries(FACTORIES).map(([name, minified]) => `${name}:${minified}`).join(',');
    fs.writeFileSync(bundle, `${fs.readFileSync(bundle, 'utf8')}\n;globalThis.__sandboxSchemas={${capture}};\n`);
    await import(pathToFileURL(bundle).href);
    const document = {};
    for (const [name, inputs] of Object.entries(BATTERY)) {
      const schema = globalThis.__sandboxSchemas[name]();
      document[name] = inputs.map((input) => {
        const result = schema.safeParse(input);
        return {
          input: input === undefined ? null : JSON.stringify(input),
          success: result.success,
          output: result.success && result.data !== undefined ? JSON.stringify(result.data) : null,
        };
      });
    }
    const rendered = `${JSON.stringify(document, null, 2)}\n`;
    if (check) {
      const existing = fs.existsSync(out) ? fs.readFileSync(out, 'utf8') : '';
      if (existing !== rendered) {
        process.stderr.write(`${out} differs from the pinned upstream sandbox schemas\n`);
        process.exitCode = 1;
      }
    } else {
      fs.writeFileSync(out, rendered);
    }
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true });
  }
}

await main();
