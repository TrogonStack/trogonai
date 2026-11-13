#!/usr/bin/env node
// Synthesizes upstream-shaped JSON fixtures from the pinned declarations.
//
// For every root a target lists under `fixtures.roots` it writes samples that set every declared
// field. Sample `k` takes member `k mod n` of every union it meets, so enough samples visit every
// member of every union. Objects with an index signature also carry one undeclared key, except
// the root itself. `fixtures.families` splits a root that is a large interface into per-family
// samples (`{ name: [key, ...] }`); keys no family names go to the family called `other`.
// The Rust round-trip tests feed these samples through the generated types.
//
// Usage: synthesize_fixtures.mjs --typescript-package DIR --config types.json --out-root DIR [--check]
// `--check` writes nothing and exits 1 when a checked-in fixture file differs.
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import ts from 'typescript';

const options = { check: false };
const argv = process.argv.slice(2);
for (let index = 0; index < argv.length; index += 1) {
  const arg = argv[index];
  if (arg === '--check') options.check = true;
  else if (['--typescript-package', '--config', '--out-root'].includes(arg)) options[arg.slice(2).replace(/-/g, '_')] = argv[++index];
  else throw new Error(`Unknown argument ${arg}`);
}
for (const required of ['typescript_package', 'config', 'out_root']) {
  if (!options[required]) throw new Error(`Missing --${required.replace(/_/g, '-')}`);
}

const config = JSON.parse(fs.readFileSync(options.config, 'utf8'));
let declarations;
let sourceFile;
let foreign;

// Samples for the wire shapes a target's `foreign` map gives to names the source does not declare.
const FOREIGN = { string: () => 'text', object: () => ({ foreign: 'object' }), unknown: () => ({ any: ['thing', null] }) };

const isNull = type => ts.isLiteralTypeNode(type) && type.literal.kind === ts.SyntaxKind.NullKeyword;

function sample(node, variant) {
  if (ts.isParenthesizedTypeNode(node)) return sample(node.type, variant);
  switch (node.kind) {
    case ts.SyntaxKind.StringKeyword: return 'text';
    case ts.SyntaxKind.NumberKeyword: return variant % 2 ? 2.5 : 3;
    case ts.SyntaxKind.BooleanKeyword: return variant % 2 === 0;
    case ts.SyntaxKind.UnknownKeyword:
    case ts.SyntaxKind.AnyKeyword: return { any: ['thing', null] };
    default: break;
  }
  if (ts.isLiteralTypeNode(node)) {
    const literal = node.literal;
    if (ts.isStringLiteral(literal)) return literal.text;
    if (ts.isNumericLiteral(literal)) return Number(literal.text);
    if (literal.kind === ts.SyntaxKind.TrueKeyword) return true;
    if (literal.kind === ts.SyntaxKind.FalseKeyword) return false;
    return null;
  }
  if (ts.isArrayTypeNode(node)) return [sample(node.elementType, variant)];
  if (ts.isTupleTypeNode(node)) return node.elements.map(element => sample(ts.isRestTypeNode(element) ? element.type.elementType : element, variant)).slice(0, 1);
  if (ts.isUnionTypeNode(node)) {
    const members = node.types.filter(type => !isNull(type));
    return sample(members[variant % members.length], variant);
  }
  if (ts.isTypeLiteralNode(node)) return object(node.members, variant, false);
  if (ts.isIntersectionTypeNode(node)) return Object.assign({}, ...node.types.map(type => sample(type, variant)));
  if (ts.isIndexedAccessTypeNode(node)) {
    const owner = node.objectType.typeName?.getText(sourceFile);
    // A property of a type from another package samples like the generator types it: any JSON.
    if (foreign.has(owner)) return { any: ['thing', null] };
    return sample(property(declarations.get(owner), node.indexType.literal.text), variant);
  }
  if (ts.isTypeReferenceNode(node)) {
    const name = node.typeName.getText(sourceFile);
    if (name === 'Record') return { key: sample(node.typeArguments[1], variant) };
    if (foreign.has(name)) return FOREIGN[foreign.get(name)]();
    return declared(name, variant);
  }
  throw new Error(`Unsupported type ${ts.SyntaxKind[node.kind]}`);
}

function object(members, variant, top, keep) {
  const out = {};
  let index = null;
  for (const member of members) {
    if (ts.isPropertySignature(member)) {
      const name = member.name.text;
      if (!keep || keep.has(name)) out[name] = sample(member.type, variant);
    } else if (ts.isIndexSignatureDeclaration(member)) {
      index = member.type;
    }
  }
  if (index && !top) out.extra_key = sample(index, variant);
  return out;
}

function property(declaration, name) {
  const members = ts.isInterfaceDeclaration(declaration) ? declaration.members : declaration.type.members;
  const member = members?.find(candidate => ts.isPropertySignature(candidate) && candidate.name.text === name);
  if (!member) throw new Error(`Indexed access to ${name}, which ${declaration.name.text} does not declare`);
  return member.type;
}

function declared(name, variant, top, keep) {
  const node = declarations.get(name);
  return ts.isInterfaceDeclaration(node) ? object(node.members, variant, top, keep) : sample(node.type, variant);
}

function unionWidth(node) {
  let widest = 1;
  const visit = child => {
    if (ts.isUnionTypeNode(child)) widest = Math.max(widest, child.types.length);
    ts.forEachChild(child, visit);
  };
  visit(node);
  return widest;
}

function synthesize(fixtures) {
  const families = new Map(Object.entries(fixtures.families ?? {}).map(([name, keys]) => [name, new Set(keys)]));
  const result = {};
  for (const root of fixtures.roots) {
    const node = declarations.get(root);
    const width = unionWidth(node);
    const groups = [];
    if (families.size && ts.isInterfaceDeclaration(node)) {
      const claimed = new Set();
      for (const [name, keys] of families) {
        keys.forEach(key => claimed.add(key));
        groups.push([name, keys]);
      }
      const rest = new Set(
        node.members
          .filter(member => ts.isPropertySignature(member))
          .map(member => member.name.text)
          .filter(key => !claimed.has(key)),
      );
      if (rest.size) groups.push(['other', rest]);
    } else {
      groups.push([root, null]);
    }
    for (const [name, keys] of groups) {
      const samples = [];
      const seen = new Set();
      for (let variant = 0; variant < width; variant += 1) {
        const value = declared(root, variant, true, keys);
        const text = JSON.stringify(value);
        if (!seen.has(text)) {
          seen.add(text);
          samples.push(value);
        }
      }
      result[name] = samples;
    }
  }
  return `${JSON.stringify(result, null, 2)}\n`;
}

const drift = [];
for (const target of config.targets) {
  if (!target.fixtures) continue;
  const file = path.join(options.typescript_package, target.source);
  sourceFile = ts.createSourceFile(file, fs.readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true);
  declarations = new Map();
  foreign = new Map(Object.entries(target.foreign ?? {}));
  for (const statement of sourceFile.statements) {
    if (ts.isInterfaceDeclaration(statement) || ts.isTypeAliasDeclaration(statement)) declarations.set(statement.name.text, statement);
  }
  const destination = path.resolve(options.out_root, target.fixtures.output);
  const output = synthesize(target.fixtures);
  if (options.check) {
    if (!fs.existsSync(destination) || fs.readFileSync(destination, 'utf8') !== output) drift.push(target.fixtures.output);
  } else {
    fs.mkdirSync(path.dirname(destination), { recursive: true });
    fs.writeFileSync(destination, output);
  }
}
if (drift.length) {
  process.stderr.write(`Synthesized fixtures differ from the pinned upstream output: ${drift.join(', ')}\nRun \`mise run claude-sdk:types\` and review the diff.\n`);
  process.exitCode = 1;
}
