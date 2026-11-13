import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { pathToFileURL } from 'node:url';
import ts from 'typescript';

const printer = ts.createPrinter({ removeComments: true, newLine: ts.NewLineKind.LineFeed });

function canonical(node, source) {
  const transformed = ts.transform(node, [context => {
    const visit = current => ts.isStringLiteral(current)
      ? ts.factory.createStringLiteral(current.text)
      : ts.visitEachChild(current, visit, context);
    return current => ts.visitNode(current, visit);
  }]);
  try {
    return printer.printNode(ts.EmitHint.Unspecified, transformed.transformed[0], source).trim();
  } finally {
    transformed.dispose();
  }
}

function typePaths(value) {
  if (typeof value === 'string') return value.endsWith('.d.ts') ? [value] : [];
  if (Array.isArray(value)) return value.flatMap(typePaths);
  if (value && typeof value === 'object') return Object.values(value).flatMap(typePaths);
  return [];
}

function privateMember(node) {
  return (node.name && ts.isPrivateIdentifier(node.name))
    || node.modifiers?.some(modifier => modifier.kind === ts.SyntaxKind.PrivateKeyword);
}

function declarationName(node, source) {
  if (ts.isConstructorDeclaration(node) || ts.isConstructSignatureDeclaration(node)) return 'constructor';
  if (ts.isCallSignatureDeclaration(node)) return '$call';
  if (ts.isIndexSignatureDeclaration(node)) return '$index';
  if (!node.name) return `$${ts.SyntaxKind[node.kind]}`;
  return ts.isIdentifier(node.name) || ts.isStringLiteral(node.name) || ts.isNumericLiteral(node.name)
    ? node.name.text
    : canonical(node.name, source);
}

function header(node) {
  if (ts.isClassDeclaration(node)) {
    return ts.factory.updateClassDeclaration(node, node.modifiers, node.name, node.typeParameters, node.heritageClauses, []);
  }
  if (ts.isInterfaceDeclaration(node)) {
    return ts.factory.updateInterfaceDeclaration(node, node.modifiers, node.name, node.typeParameters, node.heritageClauses, []);
  }
  return node;
}

function typeMembers(node) {
  if (ts.isTypeLiteralNode(node) || ts.isClassDeclaration(node) || ts.isInterfaceDeclaration(node) || ts.isEnumDeclaration(node)) return [...node.members];
  if (ts.isTypeAliasDeclaration(node)) return typeMembers(node.type);
  if (ts.isIntersectionTypeNode(node) || ts.isUnionTypeNode(node)) return node.types.flatMap(typeMembers);
  if (ts.isParenthesizedTypeNode(node)) return typeMembers(node.type);
  return [];
}

function kind(node) {
  if (ts.isClassDeclaration(node)) return 'class';
  if (ts.isInterfaceDeclaration(node)) return 'interface';
  if (ts.isTypeAliasDeclaration(node)) return 'type-alias';
  if (ts.isEnumDeclaration(node)) return 'enum';
  if (ts.isEnumMember(node)) return 'enum-member';
  if (ts.isFunctionDeclaration(node)) return 'function';
  if (ts.isMethodDeclaration(node) || ts.isMethodSignature(node)) return 'method';
  if (ts.isPropertyDeclaration(node) || ts.isPropertySignature(node)) return 'field';
  if (ts.isVariableDeclaration(node)) return 'value';
  if (ts.isConstructorDeclaration(node) || ts.isConstructSignatureDeclaration(node)) return 'constructor';
  if (ts.isCallSignatureDeclaration(node)) return 'call-signature';
  if (ts.isIndexSignatureDeclaration(node)) return 'index-signature';
  if (ts.isGetAccessorDeclaration(node) || ts.isSetAccessorDeclaration(node)) return 'accessor';
  if (ts.isModuleDeclaration(node)) return 'namespace';
  return `syntax:${ts.SyntaxKind[node.kind]}`;
}

export function extract(packageDirectory) {
  const root = fs.realpathSync(path.resolve(packageDirectory));
  const inspectDirectory = directory => {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      if (entry.isSymbolicLink()) throw new Error('symbolic links are not supported in declaration packages');
      if (entry.isDirectory()) inspectDirectory(path.join(directory, entry.name));
    }
  };
  inspectDirectory(root);
  let manifest;
  try {
    manifest = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
  } catch {
    throw new Error('cannot read package.json');
  }
  const entries = new Map();
  const exports = manifest.exports;
  if (exports && typeof exports === 'object' && !Array.isArray(exports) && Object.keys(exports).some(name => name.startsWith('.'))) {
    for (const [name, value] of Object.entries(exports)) {
      entries.set(name, [...new Set(typePaths(value))]);
    }
  } else if (exports) {
    entries.set('.', [...new Set(typePaths(exports))]);
  }
  const primary = manifest.types ?? manifest.typings ?? (fs.existsSync(path.join(root, 'sdk.d.ts')) ? 'sdk.d.ts' : undefined);
  if (primary) entries.set('.', [...new Set([...(entries.get('.') ?? []), primary])]);
  if (!entries.has('.') || entries.get('.').length === 0) throw new Error('package has no root declaration entrypoint');
  const local = filename => {
    const relative = path.relative(root, path.resolve(filename));
    return relative !== '..' && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative);
  };
  const relative = filename => path.relative(root, filename).split(path.sep).join('/');
  const files = [...new Set([...entries.values()].flat().map(filename => path.resolve(root, filename)))];
  for (const filename of files) {
    if (!local(filename) || !fs.existsSync(filename) || !fs.statSync(filename).isFile()) throw new Error('missing or invalid declaration entrypoint');
  }
  const options = { target: ts.ScriptTarget.Latest, module: ts.ModuleKind.NodeNext, moduleResolution: ts.ModuleResolutionKind.NodeNext, skipLibCheck: true, noEmit: true, noLib: true, types: [] };
  const host = ts.createCompilerHost(options);
  const readFile = host.readFile.bind(host);
  const fileExists = host.fileExists.bind(host);
  const directoryExists = host.directoryExists.bind(host);
  host.readFile = filename => local(filename) ? readFile(filename) : undefined;
  host.fileExists = filename => local(filename) && fileExists(filename);
  host.directoryExists = filename => local(filename) && directoryExists(filename);
  const program = ts.createProgram(files, options, host);
  const checker = program.getTypeChecker();
  const symbols = {};
  const queued = [];
  const exported = new Set();
  const visited = new Set();

  function sourceRecord(node) {
    const source = node.getSourceFile();
    return { path: relative(source.fileName), line: source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1 };
  }

  function resolve(symbol) {
    if (!(symbol.flags & ts.SymbolFlags.Alias)) return symbol;
    const target = checker.getAliasedSymbol(symbol);
    if (target.declarations?.length) return target;
    const declaration = symbol.declarations?.[0];
    let cursor = declaration;
    while (cursor && !ts.isExportDeclaration(cursor) && !ts.isImportDeclaration(cursor)) cursor = cursor.parent;
    const specifier = cursor?.moduleSpecifier;
    if (specifier && ts.isStringLiteral(specifier) && !specifier.text.startsWith('.')) return null;
    throw new Error(`unresolved local export: ${symbol.name}`);
  }

  function add(name, declarations) {
    if (declarations.length === 0) throw new Error(`symbol has no declarations: ${name}`);
    const shape = { declarations: declarations.map(node => canonical(header(node), node.getSourceFile())) };
    const optional = declarations.map(node => Boolean(node.questionToken));
    if (declarations.some(node => ts.isPropertySignature(node) || ts.isPropertyDeclaration(node) || ts.isParameter(node))) shape.optional = optional;
    const kinds = [...new Set(declarations.map(kind))];
    const record = { kind: kinds.length === 1 ? kinds[0] : 'merged', shape, source: sourceRecord(declarations[0]) };
    if (symbols[name] && JSON.stringify(symbols[name]) !== JSON.stringify(record)) throw new Error(`conflicting symbol: ${name}`);
    symbols[name] = record;
    const children = new Map();
    for (const declaration of declarations) {
      for (const child of typeMembers(declaration)) {
        if (privateMember(child)) continue;
        const childName = declarationName(child, child.getSourceFile());
        if (!children.has(childName)) children.set(childName, []);
        children.get(childName).push(child);
      }
      if (ts.isModuleDeclaration(declaration)) {
        const namespace = checker.getSymbolAtLocation(declaration.name);
        for (const member of namespace ? checker.getExportsOfModule(namespace) : []) {
          const target = resolve(member);
          if (target) add(`${name}.${member.name}`, target.declarations.filter(node => local(node.getSourceFile().fileName)));
        }
      }
      references(declaration);
    }
    for (const [childName, childrenDeclarations] of children) add(`${name}.${childName}`, childrenDeclarations);
  }

  function references(node) {
    const visit = current => {
      if (ts.isIdentifier(current)) {
        const symbol = checker.getSymbolAtLocation(current);
        const target = symbol && resolve(symbol);
        if (target && !exported.has(target) && !visited.has(target)) {
          const declarations = target.declarations?.filter(declaration => local(declaration.getSourceFile().fileName) && !ts.isSourceFile(declaration));
          const top = declarations?.filter(declaration => {
            let parent = declaration.parent;
            if (ts.isVariableDeclarationList(parent)) parent = parent.parent;
            return ts.isSourceFile(parent) || (ts.isVariableStatement(parent) && ts.isSourceFile(parent.parent));
          });
          if (top?.length) {
            visited.add(target);
            const moduleName = relative(top[0].getSourceFile().fileName).replace(/\.d\.(?:ts|mts|cts)$/, '').replaceAll('/', '.');
            queued.push([`${moduleName}.${target.name}`, top]);
          }
        }
      }
      ts.forEachChild(current, visit);
    };
    visit(node);
  }

  for (const source of program.getSourceFiles()) {
    if (!local(source.fileName)) continue;
    if (source.parseDiagnostics.length) throw new Error(`invalid declaration syntax: ${relative(source.fileName)}`);
    const imports = [];
    for (const statement of source.statements) {
      if ((ts.isImportDeclaration(statement) || ts.isExportDeclaration(statement)) && statement.moduleSpecifier && ts.isStringLiteral(statement.moduleSpecifier)) {
        const specifier = statement.moduleSpecifier.text;
        if (specifier.startsWith('.')) {
          const resolution = ts.resolveModuleName(specifier, source.fileName, options, host).resolvedModule;
          if (!resolution || !local(resolution.resolvedFileName)) throw new Error(`unresolved local declaration module: ${specifier}`);
        }
        imports.push(canonical(statement, source));
      }
    }
    symbols[`modules.${relative(source.fileName)}.imports`] = { kind: 'module-imports', shape: { declarations: imports }, source: { path: relative(source.fileName), line: 1 } };
  }

  const roots = [];
  for (const [entry, filenames] of [...entries].sort(([left], [right]) => left.localeCompare(right))) {
    const prefix = entry === '.' ? '' : `${entry.replace(/^\.\//, '').replaceAll('/', '.')}.`;
    for (const filename of filenames) {
      const source = program.getSourceFile(path.resolve(root, filename));
      const moduleSymbol = source && checker.getSymbolAtLocation(source);
      if (!moduleSymbol) throw new Error(`entrypoint is not an external module: ${filename}`);
      for (const symbol of checker.getExportsOfModule(moduleSymbol)) {
        const target = resolve(symbol);
        if (target) exported.add(target);
        roots.push([`${prefix}${symbol.name}`, symbol, target]);
      }
    }
  }
  if (roots.length === 0) throw new Error('declaration entrypoints export no symbols');
  for (const [name, symbol, target] of roots) {
    if (!target) {
      const declaration = symbol.declarations[0];
      symbols[name] = { kind: 'external-reference', shape: { declaration: canonical(declaration.parent, declaration.getSourceFile()) }, source: sourceRecord(declaration) };
    } else {
      const declarations = target.declarations.filter(declaration => local(declaration.getSourceFile().fileName));
      if (declarations.length === 0) {
        symbols[name] = { kind: 'external-reference', shape: { name: target.name, declaration: canonical(symbol.declarations[0], symbol.declarations[0].getSourceFile()) }, source: sourceRecord(symbol.declarations[0]) };
      } else {
        add(name, declarations);
      }
    }
  }
  while (queued.length) {
    const [name, declarations] = queued.shift();
    add(name, declarations);
  }
  symbols['package.exports'] = { kind: 'package-exports', shape: { exports: exports ?? null, types: manifest.types ?? null, typings: manifest.typings ?? null }, source: { path: 'package.json', line: 1 } };
  return { schema_version: 1, language: 'typescript', symbols: Object.fromEntries(Object.entries(symbols).sort(([left], [right]) => left.localeCompare(right))) };
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  if (process.argv.length !== 3) {
    console.error('Usage: node extract_typescript.mjs <published-package-directory>');
    process.exitCode = 1;
  } else {
    try {
      process.stdout.write(`${JSON.stringify(extract(process.argv[2]), null, 2)}\n`);
    } catch (error) {
      console.error(`TypeScript API extraction failed: ${error.message}`);
      process.exitCode = 1;
    }
  }
}
