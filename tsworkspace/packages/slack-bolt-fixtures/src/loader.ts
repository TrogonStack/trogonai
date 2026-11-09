import { existsSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';
import vm from 'node:vm';

import ts from 'typescript';

export type ModuleExports = Record<string, unknown>;

interface ModuleRecord {
  exports: ModuleExports;
}

export interface LoaderOptions {
  externals: Readonly<Record<string, unknown>>;
  overrides: ReadonlyMap<string, ModuleExports>;
}

const NATIVE_DEPENDENCIES: ReadonlySet<string> = new Set(['raw-body', 'tsscmp']);

const COMPILER_OPTIONS: ts.CompilerOptions = {
  module: ts.ModuleKind.CommonJS,
  target: ts.ScriptTarget.ES2022,
  esModuleInterop: true,
};

function transpile(source: string, fileName: string): string {
  return ts.transpileModule(source, { compilerOptions: COMPILER_OPTIONS, fileName }).outputText;
}

function run(
  javascript: string,
  fileName: string,
  parameterNames: readonly string[],
  parameterValues: readonly unknown[],
): void {
  const wrapper = vm.runInThisContext(`(function (${parameterNames.join(', ')}) {${javascript}\n})`, {
    filename: fileName,
  }) as (...args: unknown[]) => void;
  wrapper(...parameterValues);
}

export class TypeScriptLoader {
  readonly #options: LoaderOptions;
  readonly #cache = new Map<string, ModuleRecord>();
  readonly #nativeRequire = createRequire(import.meta.url);

  constructor(options: LoaderOptions) {
    this.#options = options;
  }

  load(file: string): ModuleExports {
    const resolved = path.resolve(file);
    const override = this.#options.overrides.get(resolved);
    if (override !== undefined) {
      return override;
    }
    const cached = this.#cache.get(resolved);
    if (cached !== undefined) {
      return cached.exports;
    }

    const record: ModuleRecord = { exports: {} };
    this.#cache.set(resolved, record);
    const javascript = transpile(readFileSync(resolved, 'utf8'), resolved);
    run(
      javascript,
      resolved,
      ['exports', 'require', 'module', '__filename', '__dirname'],
      [record.exports, this.#requireFrom(resolved), record, resolved, path.dirname(resolved)],
    );
    return record.exports;
  }

  #requireFrom(importer: string): (specifier: string) => unknown {
    return (specifier) => {
      if (specifier.startsWith('.')) {
        return this.load(resolveRelative(path.dirname(importer), specifier));
      }
      const external = this.#options.externals[specifier];
      if (external !== undefined) {
        return external;
      }
      if (specifier.startsWith('node:') || NATIVE_DEPENDENCIES.has(specifier)) {
        return this.#nativeRequire(specifier);
      }
      throw new Error(`Unexpected import "${specifier}" from ${importer}; add a stub or an override`);
    };
  }
}

function resolveRelative(directory: string, specifier: string): string {
  const base = path.resolve(directory, specifier);
  const candidates = [base, `${base}.ts`, path.join(base, 'index.ts')];
  const found = candidates.find((candidate) => candidate.endsWith('.ts') && existsSync(candidate));
  if (found === undefined) {
    throw new Error(`Cannot resolve "${specifier}" from ${directory}`);
  }
  return found;
}

export function extractBlock(source: string, start: RegExp, opener: RegExp): string {
  const startMatch = start.exec(source);
  if (startMatch === null) {
    throw new Error(`Block start ${start} not found`);
  }
  const fromStart = source.slice(startMatch.index);
  const openerMatch = opener.exec(fromStart);
  if (openerMatch === null) {
    throw new Error(`Block opener ${opener} not found after ${start}`);
  }

  let depth = 0;
  for (let index = openerMatch.index + openerMatch[0].length - 1; index < fromStart.length; index += 1) {
    const character = fromStart[index];
    if (character === '{') {
      depth += 1;
    } else if (character === '}') {
      depth -= 1;
      if (depth === 0) {
        return fromStart.slice(0, index + 1);
      }
    }
  }
  throw new Error(`Block starting at ${start} is not balanced`);
}

export function evaluateSnippet(code: string, scope: Readonly<Record<string, unknown>>): unknown {
  const record: ModuleRecord = { exports: {} };
  const names = Object.keys(scope);
  run(
    transpile(code, 'snippet.ts'),
    'snippet.ts',
    ['module', 'exports', ...names],
    [record, record.exports, ...names.map((name) => scope[name])],
  );
  return record.exports;
}
