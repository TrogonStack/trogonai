import { createHash } from 'node:crypto';
import ts from 'typescript';

export class GateError extends Error {
  constructor(code, selector) {
    super(`${code}: ${selector}`);
    this.name = 'GateError';
    this.code = code;
    this.selector = selector;
  }
}

const unsupported = selector => { throw new GateError('unsupported-format', selector); };

export function extract(contents, expectedHash) {
  if (ts.version !== '6.0.3') unsupported('typescript-compiler');
  if (contents.length > 2 * 1024 * 1024 || createHash('sha256').update(contents).digest('hex') !== expectedHash) {
    throw new GateError('source-pin-mismatch', 'typescript-declarations');
  }
  let text;
  try {
    text = new TextDecoder('utf-8', { fatal: true }).decode(contents);
  } catch (error) {
    throw new GateError('upstream-extraction-failed', 'typescript-declarations');
  }
  const source = ts.createSourceFile('sdk.d.ts', text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  if (source.parseDiagnostics.length) throw new GateError('upstream-extraction-failed', 'typescript-declarations');
  const printer = ts.createPrinter({ removeComments: true, newLine: ts.NewLineKind.LineFeed });
  const result = {};
  const declarations = new Map();
  for (const statement of source.statements) {
    const selected = ['Options', 'PermissionMode', 'SettingSource'].includes(statement.name?.text);
    if (selected && !ts.isTypeAliasDeclaration(statement)) unsupported(statement.name.text);
    if (!ts.isTypeAliasDeclaration(statement)) continue;
    const name = statement.name.text;
    if (!['Options', 'PermissionMode', 'SettingSource'].includes(name)) continue;
    if (declarations.has(name) || statement.typeParameters?.length) unsupported(name);
    if (!statement.modifiers?.some(m => m.kind === ts.SyntaxKind.ExportKeyword)) unsupported(name);
    declarations.set(name, statement.type);
  }
  if (declarations.size !== 3) unsupported('selected-typescript-declarations');
  const options = declarations.get('Options');
  if (!ts.isTypeLiteralNode(options)) unsupported('Options');
  result.Options = { kind: 'type-alias' };
  for (const member of options.members) {
    if (!ts.isPropertySignature(member) || !ts.isIdentifier(member.name) || !member.type || member.initializer) unsupported('Options.member');
    const key = `Options.${member.name.text}`;
    if (Object.hasOwn(result, key)) unsupported(key);
    result[key] = {
      type: printer.printNode(ts.EmitHint.Unspecified, member.type, source).trim(),
      optional: Boolean(member.questionToken),
      readonly: Boolean(member.modifiers?.some(m => m.kind === ts.SyntaxKind.ReadonlyKeyword)),
    };
  }
  for (const name of ['PermissionMode', 'SettingSource']) {
    const union = declarations.get(name);
    if (!ts.isUnionTypeNode(union) || !union.types.every(t => ts.isLiteralTypeNode(t) && ts.isStringLiteral(t.literal))) unsupported(name);
    const literals = union.types.map(t => t.literal.text).sort();
    if (new Set(literals).size !== literals.length) unsupported(name);
    result[name] = { literals };
    for (const value of literals) result[`${name}.${value}`] = { kind: 'literal' };
  }
  return result;
}
