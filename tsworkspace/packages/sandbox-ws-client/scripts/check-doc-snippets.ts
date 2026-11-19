import { readFileSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import ts from 'typescript';

const packageDirectory = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const virtualDirectory = join(packageDirectory, '.doc-snippets');
const documents = ['README.md', 'docs/how-to/restore-managed-servers.md'];
const languages = new Set(['ts', 'tsx', 'typescript']);
const prelude = `import { Sandbox, SandboxWsClient } from '../src/index.ts';
import type { LeaseId, SandboxSpec, TenantId } from '../src/index.ts';
declare const client: SandboxWsClient;
void Sandbox;
declare const wsClient: SandboxWsClient;
declare const sandbox: Sandbox;
declare const tenant: TenantId;
declare const leaseId: LeaseId;
declare const image: string;
declare const spec: SandboxSpec;
declare function defaultSpecFor(image: string): SandboxSpec;
export {};
async function __snippet() {
`;
const preludeLines = prelude.split('\n').length - 1;

interface Block {
  file: string;
  line: number;
  code: string;
  noCheck: boolean;
}

function extractBlocks(file: string): Block[] {
  const blocks: Block[] = [];
  let open: { info: string; start: number; body: string[] } | undefined;
  readFileSync(file, 'utf8')
    .split('\n')
    .forEach((text, index) => {
      if (open === undefined) {
        const match = /^```(\S*)\s*(.*)$/.exec(text);
        if (match) open = { info: `${match[1]} ${match[2]}`.trim(), start: index + 2, body: [] };
        return;
      }
      if (/^```\s*$/.test(text)) {
        const [language = '', ...attributes] = open.info.split(/\s+/);
        if (languages.has(language.split(',')[0] ?? '')) {
          blocks.push({
            file,
            line: open.start,
            code: open.body.join('\n'),
            noCheck: attributes.includes('no-check') || language.includes('no-check'),
          });
        }
        open = undefined;
        return;
      }
      open.body.push(text);
    });
  return blocks;
}

const blocks = documents.flatMap((file) => extractBlocks(join(packageDirectory, file)));
const checked = blocks.filter((block) => !block.noCheck);
if (checked.length === 0) throw new Error('No SDK documentation examples were checked');
const configFile = ts.readConfigFile(join(packageDirectory, 'tsconfig.json'), ts.sys.readFile);
if (configFile.error) throw new Error(ts.flattenDiagnosticMessageText(configFile.error.messageText, '\n'));
const config = ts.parseJsonConfigFileContent(configFile.config, ts.sys, packageDirectory);
const sources = new Map<string, { block: Block; code: string; offset: number }>();
for (const [index, block] of checked.entries()) {
  const fragment = !/^\s*import\s/m.test(block.code);
  sources.set(join(virtualDirectory, `${index}.ts`), {
    block,
    code: fragment ? `${prelude}${block.code}\n}\nvoid __snippet;\n` : block.code,
    offset: fragment ? preludeLines : 0,
  });
}
const options: ts.CompilerOptions = {
  ...config.options,
  rootDir: packageDirectory,
  noEmit: true,
  declaration: false,
  declarationMap: false,
  sourceMap: false,
  allowImportingTsExtensions: true,
  types: ['node'],
  typeRoots: [join(packageDirectory, 'node_modules/@types')],
  paths: { '@trogonai/sandbox-ws-client': [join(packageDirectory, 'src/index.ts')] },
};
const host = ts.createCompilerHost(options);
const originalGetSourceFile = host.getSourceFile.bind(host);
const originalFileExists = host.fileExists.bind(host);
const originalReadFile = host.readFile.bind(host);
const originalDirectoryExists = host.directoryExists?.bind(host);
host.fileExists = (file) => sources.has(resolve(file)) || originalFileExists(file);
host.readFile = (file) => sources.get(resolve(file))?.code ?? originalReadFile(file);
host.directoryExists = (directory) =>
  resolve(directory) === virtualDirectory || (originalDirectoryExists?.(directory) ?? false);
host.getSourceFile = (file, languageVersion, onError, shouldCreateNewSourceFile) => {
  const source = sources.get(resolve(file));
  return source
    ? ts.createSourceFile(file, source.code, languageVersion, true)
    : originalGetSourceFile(file, languageVersion, onError, shouldCreateNewSourceFile);
};
const program = ts.createProgram({ rootNames: [...sources.keys()], options, host });
const diagnostics = [...config.errors, ...ts.getPreEmitDiagnostics(program)];
for (const diagnostic of diagnostics) {
  const message = ts.flattenDiagnosticMessageText(diagnostic.messageText, '\n');
  if (!diagnostic.file || diagnostic.start === undefined) {
    process.stderr.write(`TS${diagnostic.code}: ${message}\n`);
    continue;
  }
  const position = diagnostic.file.getLineAndCharacterOfPosition(diagnostic.start);
  const source = sources.get(resolve(diagnostic.file.fileName));
  const file = source?.block.file ?? diagnostic.file.fileName;
  const line = source ? source.block.line + position.line - source.offset : position.line + 1;
  process.stderr.write(`${relative(packageDirectory, file)}:${line}:${position.character + 1}: ${message}\n`);
}
process.stdout.write(
  `SDK doc snippets: ${checked.length} checked, ${blocks.length - checked.length} marked no-check\n`,
);
if (diagnostics.length > 0) process.exitCode = 1;
