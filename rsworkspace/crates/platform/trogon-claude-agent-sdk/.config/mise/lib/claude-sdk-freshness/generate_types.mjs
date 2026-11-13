#!/usr/bin/env node
// Deterministic Rust type generator for the pinned upstream TypeScript declarations.
//
// It reads published `.d.ts` files with the same pinned TypeScript compiler the freshness
// extractor uses and emits serde types with the upstream wire shape:
//   * every object type keeps undeclared keys in a flattened `extra` map and remembers the key
//     order it was decoded with, so decoding then encoding is lossless and byte-identical;
//   * typed maps are insertion ordered;
//   * string-literal unions become enums whose `Unknown(String)` fallback round-trips;
//   * `field?: T` (omitted), `field: T | null` (null) and `field?: T | null` are distinct;
//   * unions of other shapes pick the member that round-trips exactly with the fewest undeclared
//     keys (earliest declared on ties), otherwise keep the raw JSON value in their own
//     `Unknown` variant;
//   * nothing is validated.
//
// Usage: generate_types.mjs --typescript-package DIR --config types.json --out-root DIR [--check]
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import ts from 'typescript';

class GenerateError extends Error {}

function fail(message) {
  throw new GenerateError(message);
}

// ---------------------------------------------------------------------------------------------
// Naming
// ---------------------------------------------------------------------------------------------

const RUST_KEYWORDS = new Set([
  'as', 'break', 'const', 'continue', 'else', 'enum', 'extern', 'false', 'fn', 'for', 'if', 'impl', 'in', 'let',
  'loop', 'match', 'mod', 'move', 'mut', 'pub', 'ref', 'return', 'static', 'struct', 'trait', 'true', 'type',
  'unsafe', 'use', 'where', 'while', 'async', 'await', 'dyn', 'abstract', 'become', 'box', 'do', 'final', 'macro',
  'override', 'priv', 'typeof', 'unsized', 'virtual', 'yield', 'try', 'gen',
]);
const UNRAWABLE = new Set(['self', 'Self', 'super', 'crate']);

function words(text) {
  return text
    .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
    .replace(/([A-Z]+)([A-Z][a-z])/g, '$1 $2')
    .split(/[^A-Za-z0-9]+/)
    .filter(Boolean);
}

function pascal(text) {
  const joined = words(text).map(word => word[0].toUpperCase() + word.slice(1).toLowerCase()).join('');
  if (joined === '') fail(`Cannot derive a Rust type name from ${JSON.stringify(text)}`);
  return /^[0-9]/.test(joined) ? `V${joined}` : joined;
}

function snake(text) {
  const prefix = text.startsWith('-') ? 'dash_' : '';
  const joined = words(text).map(word => word.toLowerCase()).join('_');
  if (joined === '') fail(`Cannot derive a Rust field name from ${JSON.stringify(text)}`);
  const name = `${prefix}${/^[0-9]/.test(joined) ? `n_${joined}` : joined}`;
  if (UNRAWABLE.has(name)) return `${name}_`;
  return RUST_KEYWORDS.has(name) ? `r#${name}` : name;
}

function rustString(value) {
  return JSON.stringify(value);
}

function docLines(node, indent) {
  const docs = node.jsDoc;
  if (!docs || docs.length === 0) return '';
  const text = ts.getTextOfJSDocComment(docs[docs.length - 1].comment);
  if (!text) return '';
  const lines = [];
  for (const raw of text.split('\n')) {
    const line = raw.trim().replace(/\u2014/g, '-').replace(/\u2013/g, '-');
    if (line.startsWith('```')) continue;
    const escaped = line.replace(/\\/g, '\\\\').replace(/\[/g, '\\[').replace(/\]/g, '\\]').replace(/</g, '\\<').replace(/>/g, '\\>');
    lines.push(escaped === '' ? `${indent}///` : `${indent}/// ${escaped}`);
  }
  while (lines.length && lines[lines.length - 1] === `${indent}///`) lines.pop();
  return lines.length ? `${lines.join('\n')}\n` : '';
}

// ---------------------------------------------------------------------------------------------
// TypeScript to intermediate representation
// ---------------------------------------------------------------------------------------------

function key(ir) {
  return JSON.stringify(ir, (name, value) => (name === 'doc' ? undefined : value));
}

function unionOf(members) {
  const flat = [];
  const visit = member => (member.k === 'union' ? member.members.forEach(visit) : flat.push(member));
  members.forEach(visit);
  let nullable = false;
  const literals = [];
  const others = [];
  const seen = new Set();
  const push = member => {
    const id = key(member);
    if (!seen.has(id)) {
      seen.add(id);
      others.push(member);
    }
  };
  for (const member of flat) {
    if (member.k === 'null') nullable = true;
    else if (member.k === 'nullable') {
      nullable = true;
      if (member.inner.k === 'strenum') literals.push(...member.inner.values);
      else push(member.inner);
    } else if (member.k === 'strenum') literals.push(...member.values);
    else push(member);
  }
  if (others.some(member => member.k === 'unknown')) return { k: 'unknown' };
  if (others.some(member => member.k === 'string')) {
    literals.length = 0;
  }
  if (literals.length) push({ k: 'strenum', values: [...new Set(literals)] });
  let result;
  if (others.length === 0) return { k: 'null' };
  if (others.length === 1) result = others[0];
  else result = { k: 'union', members: others };
  return nullable ? { k: 'nullable', inner: result } : result;
}

function parseObject(members, source) {
  const props = [];
  let index = null;
  for (const member of members) {
    if (ts.isPropertySignature(member)) {
      if (!member.type) fail(`Property without a type at ${source.fileName}`);
      const name = ts.isIdentifier(member.name) || ts.isStringLiteral(member.name) ? member.name.text : fail(`Computed property name ${member.name.getText(source)}`);
      props.push({ name, optional: Boolean(member.questionToken), type: parseType(member.type, source), doc: docLines(member, '    ') });
    } else if (ts.isIndexSignatureDeclaration(member)) {
      if (member.parameters.length !== 1 || member.parameters[0].type?.kind !== ts.SyntaxKind.StringKeyword) fail('Only string index signatures are supported');
      if (index) fail('Duplicate index signature');
      index = parseType(member.type, source);
    } else {
      fail(`Unsupported member ${ts.SyntaxKind[member.kind]} in ${source.fileName}`);
    }
  }
  return { k: 'object', props, index };
}

function parseType(node, source) {
  if (ts.isParenthesizedTypeNode(node)) return parseType(node.type, source);
  switch (node.kind) {
    case ts.SyntaxKind.StringKeyword: return { k: 'string' };
    case ts.SyntaxKind.NumberKeyword: return { k: 'number' };
    case ts.SyntaxKind.BooleanKeyword: return { k: 'bool' };
    case ts.SyntaxKind.UnknownKeyword:
    case ts.SyntaxKind.AnyKeyword: return { k: 'unknown' };
    default: break;
  }
  if (ts.isLiteralTypeNode(node)) {
    const literal = node.literal;
    if (ts.isStringLiteral(literal)) return { k: 'strenum', values: [literal.text] };
    if (ts.isNumericLiteral(literal)) return { k: 'number' };
    if (literal.kind === ts.SyntaxKind.TrueKeyword || literal.kind === ts.SyntaxKind.FalseKeyword) return { k: 'bool' };
    if (literal.kind === ts.SyntaxKind.NullKeyword) return { k: 'null' };
    fail(`Unsupported literal type ${node.getText(source)}`);
  }
  if (ts.isArrayTypeNode(node)) return { k: 'array', el: parseType(node.elementType, source) };
  if (ts.isTupleTypeNode(node)) {
    const elements = node.elements.map(element => {
      if (ts.isRestTypeNode(element)) {
        const inner = parseType(element.type, source);
        if (inner.k !== 'array') fail('Rest element must be an array');
        return inner.el;
      }
      if (ts.isOptionalTypeNode(element)) return parseType(element.type, source);
      if (ts.isNamedTupleMember(element)) fail('Named tuple members are unsupported');
      return parseType(element, source);
    });
    const ids = new Set(elements.map(key));
    if (ids.size !== 1) fail(`Heterogeneous tuple ${node.getText(source)}`);
    return { k: 'array', el: elements[0] };
  }
  if (ts.isTypeLiteralNode(node)) return parseObject(node.members, source);
  if (ts.isUnionTypeNode(node)) return unionOf(node.types.map(type => parseType(type, source)));
  if (ts.isTypeReferenceNode(node)) {
    const name = node.typeName.getText(source);
    if (name === 'Record' && node.typeArguments?.length === 2 && node.typeArguments[0].kind === ts.SyntaxKind.StringKeyword) {
      return { k: 'map', val: parseType(node.typeArguments[1], source) };
    }
    if (name === 'Array' && node.typeArguments?.length === 1) return { k: 'array', el: parseType(node.typeArguments[0], source) };
    if (node.typeArguments) fail(`Generic reference ${node.getText(source)} is unsupported`);
    return { k: 'ref', name };
  }
  return fail(`Unsupported type ${ts.SyntaxKind[node.kind]}: ${node.getText(source)}`);
}

function simplifyObject(ir) {
  // `{ [k: string]: V }` with no properties is a plain map.
  if (ir.k === 'object' && ir.props.length === 0 && ir.index) return { k: 'map', val: ir.index };
  return ir;
}

function normalize(ir) {
  switch (ir.k) {
    case 'array': return { k: 'array', el: normalize(ir.el) };
    case 'map': return { k: 'map', val: normalize(ir.val) };
    case 'nullable': return { k: 'nullable', inner: normalize(ir.inner) };
    case 'object': return simplifyObject({
      k: 'object',
      props: ir.props.map(prop => ({ ...prop, type: normalize(prop.type) })),
      index: ir.index ? normalize(ir.index) : null,
    });
    case 'union': return unionOf(ir.members.map(normalize));
    default: return ir;
  }
}

// ---------------------------------------------------------------------------------------------
// Emission
// ---------------------------------------------------------------------------------------------

class Emitter {
  constructor(declarations, source) {
    this.declarations = declarations;
    this.source = source;
    this.claimed = new Set();
    this.chunks = [];
    this.pending = [];
    this.emitted = new Set();
    this.omit = new Map();
    this.externs = new Map();
  }

  claim(name) {
    if (this.claimed.has(name)) fail(`Generated Rust type name ${name} is not unique`);
    this.claimed.add(name);
  }

  request(name) {
    if (this.externs.has(name)) return;
    if (!this.declarations.has(name)) fail(`Reference to ${name}, which is not declared in the source file`);
    if (!this.emitted.has(name)) {
      this.emitted.add(name);
      this.pending.push(name);
    }
  }

  declaration(name) {
    const node = this.declarations.get(name);
    if (ts.isInterfaceDeclaration(node)) {
      if (node.heritageClauses?.length) fail(`Interface ${name} extends another type`);
      if (node.typeParameters?.length) fail(`Interface ${name} is generic`);
      return normalize(parseObject(node.members, this.source));
    }
    if (node.typeParameters?.length) fail(`Alias ${name} is generic`);
    const ir = normalize(parseType(node.type, this.source));
    const omitted = this.omit.get(name);
    if (!omitted) return ir;
    if (ir.k !== 'union') fail(`omit_members target ${name} is not a union`);
    const members = ir.members.filter(member => !(member.k === 'ref' && omitted.includes(member.name)));
    return members.length === 1 ? members[0] : { k: 'union', members };
  }

  // Emits the named root; aliases of non-aggregate types become Rust type aliases.
  root(name) {
    const ir = this.declaration(name);
    this.claim(name);
    const doc = docLines(this.declarations.get(name), '');
    switch (ir.k) {
      case 'object': this.struct(name, ir, doc); break;
      case 'union': this.union(name, ir, doc); break;
      case 'strenum': this.strenum(name, ir, doc); break;
      default: {
        const type = this.type(ir, name);
        this.chunks.push(`${doc}pub type ${name} = ${type};\n`);
      }
    }
  }

  drain() {
    while (this.pending.length) this.root(this.pending.shift());
  }

  type(ir, name) {
    switch (ir.k) {
      case 'string': return 'String';
      case 'number': return 'serde_json::Number';
      case 'bool': return 'bool';
      case 'unknown': return 'serde_json::Value';
      case 'null': return 'Option<serde_json::Value>';
      case 'ref': this.request(ir.name); return this.externs.get(ir.name) ?? ir.name;
      case 'array': return `Vec<${this.type(ir.el, `${name}Item`)}>`;
      case 'map': return ir.val.k === 'unknown'
        ? 'serde_json::Map<String, serde_json::Value>'
        : `indexmap::IndexMap<String, ${this.type(ir.val, `${name}Value`)}>`;
      case 'nullable': return `Option<${this.type(ir.inner, name)}>`;
      case 'object': this.claim(name); this.struct(name, ir, ''); return name;
      case 'strenum': this.claim(name); this.strenum(name, ir, ''); return name;
      case 'union': this.claim(name); this.union(name, ir, ''); return name;
      default: return fail(`Unhandled type ${ir.k}`);
    }
  }

  struct(name, ir, doc) {
    const used = new Set();
    const fields = [];
    for (const prop of ir.props) {
      const field = snake(prop.name);
      if (used.has(field)) fail(`Field name ${field} collides in ${name}`);
      used.add(field);
      const attrs = [];
      let inner = prop.type;
      let nullable = false;
      if (inner.k === 'nullable') {
        nullable = true;
        inner = inner.inner;
      }
      const rust = this.type(inner, `${name}${pascal(prop.name)}`);
      let type;
      if (prop.optional && nullable) {
        type = `Option<Option<${rust}>>`;
        attrs.push('default', 'skip_serializing_if = "Option::is_none"', 'with = "super::support::double_option"');
      } else if (prop.optional) {
        type = `Option<${rust}>`;
        attrs.push('default', 'skip_serializing_if = "Option::is_none"');
        if (inner.k === 'unknown') attrs.push('deserialize_with = "super::support::optional_value"');
      } else if (nullable) {
        type = `Option<${rust}>`;
        attrs.push('default');
      } else {
        type = rust;
        if (inner.k === 'unknown') attrs.push('default');
      }
      const rename = field.replace(/^r#/, '') === prop.name ? [] : [`rename = ${rustString(prop.name)}`];
      const all = [...rename, ...attrs];
      fields.push({
        doc: prop.doc,
        serde: all.length ? `    #[serde(${all.join(', ')})]\n` : '',
        decl: `    pub ${field}: ${type},`,
      });
    }
    for (const reserved of ['extra', 'key_order']) {
      if (used.has(reserved)) fail(`Field name ${reserved} collides in ${name}`);
    }
    const extraType = ir.index && ir.index.k !== 'unknown'
      ? `indexmap::IndexMap<String, ${this.type(ir.index, `${name}Extra`)}>`
      : 'serde_json::Map<String, serde_json::Value>';
    const extraDoc = '    /// Keys the pinned upstream declarations do not list, in the order they were decoded.\n';
    const orderDoc = '    /// The key order this value was decoded with; encoding reproduces it.\n';
    const wire = this.claimWire(name);
    const plain = fields.map(field => `${field.doc}${field.decl}`);
    plain.push(`${extraDoc}    pub extra: ${extraType},`, `${orderDoc}    pub key_order: super::support::KeyOrder,`);
    const mirror = fields.map(field => `${field.serde}${field.decl}`);
    mirror.push(`    #[serde(flatten)]\n    pub extra: ${extraType},`, '    #[serde(skip)]\n    pub key_order: super::support::KeyOrder,');
    const known = ir.props.map(prop => rustString(prop.name)).join(', ');
    const out = [];
    out.push(`${doc}#[derive(Clone, Debug, PartialEq, Eq)]\npub struct ${name} {\n${plain.join('\n')}\n}\n`);
    out.push(`#[derive(Serialize, Deserialize)]\n#[serde(remote = "${name}")]\nstruct ${wire} {\n${mirror.join('\n')}\n}\n`);
    out.push(`impl ${name} {`, '    /// The keys the pinned upstream declarations list for this object.', `    pub const KNOWN_KEYS: &'static [&'static str] = &[${known}];`, '}', '');
    out.push(`impl Serialize for ${name} {`, '    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {', `        super::support::ordered(${wire}::serialize(self, serde_json::value::Serializer), &self.key_order, serializer)`, '    }', '}', '');
    out.push(`impl<'de> Deserialize<'de> for ${name} {`, '    fn deserialize<D: serde::Deserializer<\'de>>(deserializer: D) -> Result<Self, D::Error> {', '        let value = serde_json::Value::deserialize(deserializer)?;', `        let mut decoded = ${wire}::deserialize(value.clone()).map_err(serde::de::Error::custom)?;`, '        decoded.key_order = super::support::KeyOrder::of(&value);', '        Ok(decoded)', '    }', '}', '');
    this.chunks.push(out.join('\n'));
  }

  claimWire(name) {
    const wire = `${name}Wire`;
    this.claim(wire);
    return wire;
  }

  strenum(name, ir, doc) {
    const variants = ir.values.map(value => ({ value, variant: pascal(value) }));
    const names = new Set(['Unknown']);
    for (const { variant, value } of variants) {
      if (names.has(variant)) fail(`Enum variant ${variant} collides in ${name} (${JSON.stringify(value)})`);
      names.add(variant);
    }
    const out = [];
    out.push(`${doc}#[derive(Clone, Debug, PartialEq, Eq, Hash)]\npub enum ${name} {`);
    for (const { variant, value } of variants) out.push(`    /// \`${rustString(value).slice(1, -1).replace(/`/g, '')}\`\n    ${variant},`);
    out.push('    /// A value the pinned upstream declarations do not list; it round-trips unchanged.', '    Unknown(String),', '}', '');
    out.push(`impl ${name} {`, '    pub fn as_str(&self) -> &str {', '        match self {');
    for (const { variant, value } of variants) out.push(`            Self::${variant} => ${rustString(value)},`);
    out.push('            Self::Unknown(value) => value,', '        }', '    }', '}', '');
    out.push(`impl Serialize for ${name} {`, '    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {', '        serializer.serialize_str(self.as_str())', '    }', '}', '');
    out.push(`impl<'de> Deserialize<'de> for ${name} {`, '    fn deserialize<D: serde::Deserializer<\'de>>(deserializer: D) -> Result<Self, D::Error> {', '        let value = String::deserialize(deserializer)?;', '        Ok(match value.as_str() {');
    for (const { variant, value } of variants) out.push(`            ${rustString(value)} => Self::${variant},`);
    out.push('            _ => Self::Unknown(value),', '        })', '    }', '}', '');
    this.chunks.push(out.join('\n'));
  }

  union(name, ir, doc) {
    const objects = ir.members.filter(member => member.k === 'object');
    const discriminator = this.discriminator(objects);
    const variants = [];
    const names = new Set(['Unknown']);
    let objectIndex = 0;
    for (const member of ir.members) {
      let base;
      switch (member.k) {
        case 'string': base = 'Text'; break;
        case 'number': base = 'Number'; break;
        case 'bool': base = 'Bool'; break;
        case 'unknown': base = 'Value'; break;
        case 'array': base = 'List'; break;
        case 'map': base = 'Map'; break;
        case 'strenum': base = 'Literal'; break;
        case 'ref': base = member.name; break;
        case 'object': {
          objectIndex += 1;
          if (discriminator) {
            const prop = member.props.find(candidate => candidate.name === discriminator);
            base = prop.type.values.map(pascal).join('Or');
          } else {
            base = `Variant${objectIndex}`;
          }
          break;
        }
        default: fail(`Unsupported union member ${member.k} in ${name}`);
      }
      let variant = base;
      for (let suffix = 2; names.has(variant); suffix += 1) variant = `${base}${suffix}`;
      names.add(variant);
      let kind = 'plain';
      if (member.k === 'object') kind = 'struct';
      else if (member.k === 'map') kind = 'map';
      else if (member.k === 'ref') {
        const target = this.declaration(member.name).k;
        if (target === 'object') kind = 'struct';
        else if (target === 'union') kind = 'union';
        else if (target === 'map') kind = 'map';
      }
      variants.push({ variant, kind, type: this.type(member, `${name}${variant}`) });
    }
    const out = [];
    out.push(`${doc}#[derive(Clone, Debug, PartialEq, Eq)]\npub enum ${name} {`);
    for (const { variant, type } of variants) out.push(`    ${variant}(${type}),`);
    out.push('    /// A value matching none of the pinned upstream shapes exactly; it round-trips unchanged.', '    Unknown(serde_json::Value),', '}', '');
    out.push(`impl Serialize for ${name} {`, '    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {', '        match self {');
    for (const { variant } of variants) out.push(`            Self::${variant}(inner) => inner.serialize(serializer),`);
    out.push('            Self::Unknown(inner) => inner.serialize(serializer),', '        }', '    }', '}', '');
    out.push(`impl ${name} {`, '    /// Decodes `value` into the declared member that reproduces it exactly with the fewest', '    /// undeclared keys, earliest declared on ties, or `None` when no member matches.', '    pub fn best_match(value: &serde_json::Value) -> Option<(usize, Self)> {', '        let mut best: Option<(usize, Self)> = None;');
    for (const { variant, type, kind } of variants) {
      let candidate;
      if (kind === 'struct') candidate = `super::support::exact::<${type}>(value).map(|inner| (super::support::unknown_keys(value, ${type}::KNOWN_KEYS), Self::${variant}(inner)))`;
      else if (kind === 'union') candidate = `${type}::best_match(value).map(|(score, inner)| (score, Self::${variant}(inner)))`;
      else if (kind === 'map') candidate = `super::support::exact::<${type}>(value).map(|inner| (super::support::unknown_keys(value, &[]), Self::${variant}(inner)))`;
      else candidate = `super::support::exact::<${type}>(value).map(|inner| (0, Self::${variant}(inner)))`;
      out.push(`        super::support::keep_best(&mut best, ${candidate});`);
    }
    out.push('        best', '    }', '}', '');
    out.push(`impl<'de> Deserialize<'de> for ${name} {`, '    fn deserialize<D: serde::Deserializer<\'de>>(deserializer: D) -> Result<Self, D::Error> {', '        let value = serde_json::Value::deserialize(deserializer)?;', '        Ok(match Self::best_match(&value) {', '            Some((_, matched)) => matched,', '            None => Self::Unknown(value),', '        })', '    }', '}', '');
    this.chunks.push(out.join('\n'));
  }

  // A property every object member declares as disjoint string literals names the variants.
  discriminator(objects) {
    if (objects.length < 2) return null;
    for (const prop of objects[0].props) {
      if (prop.type.k !== 'strenum') continue;
      const seen = new Set();
      let ok = true;
      for (const object of objects) {
        const other = object.props.find(candidate => candidate.name === prop.name);
        if (!other || other.optional || other.type.k !== 'strenum') { ok = false; break; }
        for (const value of other.type.values) {
          if (seen.has(value)) ok = false;
          seen.add(value);
        }
        if (!ok) break;
      }
      if (ok) return prop.name;
    }
    return null;
  }
}

function declarationsOf(sourceFile) {
  const declarations = new Map();
  for (const statement of sourceFile.statements) {
    if (ts.isInterfaceDeclaration(statement) || ts.isTypeAliasDeclaration(statement)) declarations.set(statement.name.text, statement);
  }
  return declarations;
}

export function generate({ packageDir, target, label }) {
  const file = path.join(packageDir, target.source);
  if (!fs.existsSync(file)) fail(`Missing declaration file ${target.source}`);
  const text = fs.readFileSync(file, 'utf8');
  const sourceFile = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true);
  const declarations = declarationsOf(sourceFile);
  const emitter = new Emitter(declarations, sourceFile);
  for (const [name, omitted] of Object.entries(target.omit_members ?? {})) {
    if (!declarations.has(name)) fail(`omit_members names ${name}, which is not declared`);
    emitter.omit.set(name, omitted);
  }
  for (const [name, rustPath] of Object.entries(target.extern ?? {})) {
    if (!declarations.has(name)) fail(`extern names ${name}, which is not declared`);
    emitter.externs.set(name, rustPath);
  }
  for (const name of target.roots) {
    if (!declarations.has(name)) fail(`Root ${name} is not declared in ${target.source}`);
    if (emitter.emitted.has(name)) continue;
    emitter.emitted.add(name);
    emitter.root(name);
    emitter.drain();
  }
  const header = [
    `// @generated by .config/mise/lib/claude-sdk-freshness/generate_types.mjs from ${label} ${target.source}.`,
    '// Do not edit by hand; run `mise run claude-sdk:types` from the SDK crate to regenerate.',
    '',
    'use serde::{Deserialize, Serialize};',
    '',
  ].join('\n');
  return `${header}\n${emitter.chunks.join('\n')}`;
}

function parseArgs(argv) {
  const options = { check: false };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === '--check') options.check = true;
    else if (['--typescript-package', '--config', '--out-root'].includes(arg)) {
      options[arg.slice(2).replace(/-/g, '_')] = argv[++index];
    } else fail(`Unknown argument ${arg}`);
  }
  for (const required of ['typescript_package', 'config', 'out_root']) {
    if (!options[required]) fail(`Missing --${required.replace(/_/g, '-')}`);
  }
  return options;
}

function main(argv) {
  const options = parseArgs(argv);
  const config = JSON.parse(fs.readFileSync(options.config, 'utf8'));
  const manifest = JSON.parse(fs.readFileSync(path.join(options.typescript_package, 'package.json'), 'utf8'));
  if (manifest.name !== config.package) fail(`Expected package ${config.package}, found ${manifest.name}`);
  if (manifest.version !== config.version) fail(`Expected ${config.package} ${config.version}, found ${manifest.version}`);
  const label = `${manifest.name} ${manifest.version}`;
  const drift = [];
  for (const target of config.targets) {
    const output = generate({ packageDir: options.typescript_package, target, label });
    const destination = path.resolve(options.out_root, target.output);
    if (options.check) {
      const current = fs.existsSync(destination) ? fs.readFileSync(destination, 'utf8') : null;
      if (current !== output) drift.push(target.output);
    } else {
      fs.mkdirSync(path.dirname(destination), { recursive: true });
      fs.writeFileSync(destination, output);
    }
  }
  if (drift.length) {
    process.stderr.write(`Generated types differ from the pinned upstream output: ${drift.join(', ')}\nRun \`mise run claude-sdk:types\` and review the diff.\n`);
    return 1;
  }
  return 0;
}

if (import.meta.url === new URL(`file://${process.argv[1]}`).href) {
  try {
    process.exitCode = main(process.argv.slice(2));
  } catch (error) {
    if (!(error instanceof GenerateError)) throw error;
    process.stderr.write(`generate_types: ${error.message}\n`);
    process.exitCode = 2;
  }
}
