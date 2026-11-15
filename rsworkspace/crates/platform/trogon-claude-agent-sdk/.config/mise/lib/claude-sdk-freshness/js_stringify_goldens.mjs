#!/usr/bin/env node
// Records what `JSON.stringify` prints for a fixed corpus, so the Rust `js_stringify` is checked against the
// engine instead of against a reading of the specification.
//
// usage: node js_stringify_goldens.mjs <out-file> [--check]
//
// `numbers` pairs the bit pattern of a finite double (16 hex digits) with `JSON.stringify(value)`. `documents`
// pairs JSON source text with `JSON.stringify(JSON.parse(text))`, which covers token spellings, key order,
// duplicate keys and string escapes. The corpus is drawn from a seeded generator, so the output only changes when
// this script or the engine's number printing does.
import fs from 'node:fs';

const [out, ...flags] = process.argv.slice(2);
if (!out) {
  process.stderr.write('usage: js_stringify_goldens.mjs OUT_FILE [--check]\n');
  process.exit(2);
}

let seed = 0x9e3779b9;
function next32() {
  seed = (seed + 0x6d2b79f5) >>> 0;
  let t = seed;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return (t ^ (t >>> 14)) >>> 0;
}
const below = (n) => next32() % n;
const pick = (items) => items[below(items.length)];

const view = new DataView(new ArrayBuffer(8));
const bitsOf = (value) => {
  view.setFloat64(0, value);
  return view.getBigUint64(0);
};
const fromBits = (bits) => {
  view.setBigUint64(0, BigInt.asUintN(64, bits));
  return view.getFloat64(0);
};
const hex = (value) => bitsOf(value).toString(16).padStart(16, '0');
const neighbors = (value) => {
  if (value === 0) return [value];
  const bits = bitsOf(value);
  return [fromBits(bits - 1n), value, fromBits(bits + 1n)];
};

const numbers = new Map();
// The sign only prepends a minus, so one value in four is recorded negated as well.
const add = (value) => {
  for (const item of below(4) === 0 ? [value, -value] : [value]) {
    if (Number.isFinite(item) && !numbers.has(hex(item))) numbers.set(hex(item), JSON.stringify(item));
  }
};

// Zero, both subnormal ends, the normal boundary and the largest double.
for (const value of [0, 5e-324, 1e-323, 2.225073858507201e-308, 2.2250738585072014e-308, Number.MAX_VALUE]) {
  neighbors(value).forEach(add);
}
for (const value of [-0, -5e-324, -Number.MAX_VALUE]) numbers.set(hex(value), JSON.stringify(value));
// Every power of ten and its neighbors: the exponent and fixed-notation thresholds sit on these.
for (let exponent = -324; exponent <= 308; exponent += 1) neighbors(Number(`1e${exponent}`)).forEach(add);
// Every power of two; the neighbors where integers stop being exact and where 64-bit integers end.
for (let exponent = -1074; exponent <= 1023; exponent += 1) {
  const value = 2 ** exponent;
  if (exponent >= -24 && exponent <= 72) neighbors(value).forEach(add);
  else add(value);
}
// Values with few significant digits on both sides of the thresholds JavaScript switches notation at.
for (const exponent of [-8, -7, -6, -5, -4, 15, 16, 17, 18, 19, 20, 21, 22, 23]) {
  for (let mantissa = 1; mantissa <= 25; mantissa += 1) {
    add(Number(`${mantissa}e${exponent}`));
    add(Number(`${mantissa / 10}e${exponent}`));
  }
}
// Integers past 2^53 written in full, as a settings file holds them.
for (let count = 0; count < 300; count += 1) {
  const digits = 16 + below(8);
  let text = String(1 + below(9));
  for (let place = 1; place < digits; place += 1) text += String(below(10));
  add(Number(text));
}
// Random bit patterns: every exponent range, mostly non-round values.
for (let count = 0; count < 2000; count += 1) {
  const value = fromBits((BigInt(next32()) << 32n) | BigInt(next32()));
  if (Number.isFinite(value)) add(value);
}
// Short decimals at random scales, where the shortest digits are few and the rounding tie rules matter.
for (let count = 0; count < 1000; count += 1) {
  const digits = 1 + below(17);
  let text = String(1 + below(9));
  for (let place = 1; place < digits; place += 1) text += String(below(10));
  add(Number(`${text}e${below(80) - 40}`));
}

const handWritten = [
  '0', '-0', '0.0', '-0.0', '1E5', '1e-7', '-1e-7', '1e-400', '-0e10', '0.000001', '0.0000015', '0.00001',
  '123e-2', '9007199254740993', '-9007199254740993', '18446744073709551615', '18446744073709551616',
  '-9223372036854775808', '-9223372036854775809', '-18446744073709551617', '99999999999999999999',
  '999999999999999999999', '1000000000000000000000', '123456789012345678901234567890', '1.7976931348623157e308',
  '[]', '{}', '[[],{},[{}]]', ' { "a" : [ 1 , 2 ] } ', 'true', 'false', 'null', '""',
  '{"b":1,"a":2,"1":3,"0":4}',
  '{"a":1,"b":2,"a":3}',
  '{"2":"x","1":"y","2":"z"}',
  '{"4294967294":1,"4294967295":2,"4294967296":3,"0":4}',
  '{"4294967295":2,"4294967294":1,"0":4}',
  '{"b":1,"4294967296":2,"4294967295":3,"4294967294":4,"9007199254740991":5}',
  '{"01":1,"1":2,"-1":3,"1.0":4,"":5," 1":6,"1 ":7,"10":8,"9":9}',
  '{"9007199254740993":1,"100":2,"a":{"3":1,"b":2,"1":3}}',
  '["\\u0000\\u0001\\u001f\\u007f\\u0080\\u2028\\u2029\\ufeff"]',
  '"\\"\\\\\\/\\b\\f\\n\\r\\t"',
  '"\\ud83d\\ude00 é 日本 \\u00e9"',
  '{"\\u0000":"\\u0000","\\n":"\\t","\\"":"\\\\"}',
  '{"a":[1.0,2.50,-0.0,1e21,1e+21,1E-7,0.1e1,10e-1]}',
];

const keyPool = ['a', 'b', 'model', '0', '1', '2', '10', '01', '-1', '4294967294', '4294967295', 'é', '\n', '"', ''];
const stringPool = ['', 'a', '"', '\\', '/', '\u0000', '\u0008', '\u000b', '\u001f', '\u007f', ' ', 'é', '日本', '😀'];
function token() {
  const sign = below(4) === 0 ? '-' : '';
  const intDigits = 1 + below(24);
  let text = String(below(10) === 0 ? 0 : 1 + below(9));
  if (text !== '0') for (let place = 1; place < intDigits; place += 1) text += String(below(10));
  if (below(3) === 0) {
    text += '.';
    for (let place = 0, digits = 1 + below(12); place < digits; place += 1) text += String(below(10));
  }
  if (below(3) === 0) text += `${pick(['e', 'E'])}${pick(['', '+', '-'])}${below(30)}`;
  return sign + text;
}
function text(value) {
  return JSON.stringify(value);
}
function document(depth) {
  const kind = depth >= 4 ? below(4) : below(6);
  if (kind === 0) return token();
  if (kind === 1) return text(Array.from({ length: below(3) + 1 }, () => pick(stringPool)).join(''));
  if (kind === 2) return pick(['true', 'false', 'null']);
  if (kind === 3) return token();
  if (kind === 4) return `[${Array.from({ length: below(5) }, () => document(depth + 1)).join(',')}]`;
  const entries = Array.from({ length: below(6) }, () => `${text(pick(keyPool))}:${document(depth + 1)}`);
  return `{${entries.join(',')}}`;
}
const generated = Array.from({ length: 400 }, () => document(0));

const documents = [...handWritten, ...generated].map((source) => [source, JSON.stringify(JSON.parse(source))]);
const lines = (pairs) => pairs.map((pair) => `    ${JSON.stringify(pair)}`).join(',\n');
const body = `{\n  "numbers": [\n${lines([...numbers])}\n  ],\n  "documents": [\n${lines(documents)}\n  ]\n}\n`;

if (flags.includes('--check')) {
  if (fs.readFileSync(out, 'utf8') !== body) {
    process.stderr.write(`${out} is stale: run js_stringify_goldens.mjs without --check\n`);
    process.exit(1);
  }
} else {
  fs.writeFileSync(out, body);
}
