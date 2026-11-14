#!/usr/bin/env node
// Records what the pinned TypeScript package's bridge.mjs does that the Rust `createCodeSession` port has to match
// exactly: how Node's ICU classifies and case maps characters, which repository addresses the host and path rejectors
// refuse.
//
// usage: node bridge_create_session_goldens.mjs <path to bridge.mjs> <fixtures dir> <tables .rs file> [--check]
//
// Writes `unicode.json`, `urls.json` and `details.json` into the fixtures dir (one case per line) and
// the generated Rust range tables into the .rs file. Every table is swept from Node itself with the very regular
// expression sources the bridge uses, so the Rust side never depends on the Unicode version of a Rust crate.
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const [bridgeArg, fixturesArg, tablesArg, ...flags] = process.argv.slice(2);
if (!bridgeArg || !fixturesArg || !tablesArg) {
  process.stderr.write('usage: bridge_create_session_goldens.mjs BRIDGE_MJS FIXTURES_DIR TABLES_RS [--check]\n');
  process.exit(2);
}
const bridgePath = path.resolve(bridgeArg);
const fixturesDir = path.resolve(fixturesArg);
const tablesPath = path.resolve(tablesArg);
const check = flags.includes('--check');
const source = fs.readFileSync(bridgePath, 'utf8');

// The bridge builds its forbidden-character class and its leading-mark rule from these sources; fail loudly if a new
// release changes them, because the sweeps below must use the same ones.
const GR = '\\p{Cc}\\p{Cf}\\p{Cs}\\p{Co}\\p{Cn}\\u2028\\u2029\\p{Default_Ignorable_Code_Point}\\u2800';
for (const needle of ['Gr=`\\\\p{Cc}\\\\p{Cf}\\\\p{Cs}\\\\p{Co}\\\\p{Cn}\\\\u2028\\\\u2029${vk}`', 'vk="\\\\p{Default_Ignorable_Code_Point}\\\\u2800"', 'DX=new RegExp(`[^\\\\S ]|[${Gr}]`,"u")', 'RD=/(?<![^\\s\\p{P}])\\p{M}+/gu']) {
  if (!source.includes(needle)) throw new Error(`bridge.mjs no longer contains ${needle}`);
}

const scalar = (cp) => cp < 0xd800 || cp > 0xdfff;
function sweep(test) {
  const ranges = [];
  let start = -1;
  for (let cp = 0; cp <= 0x110000; cp++) {
    const hit = cp <= 0x10ffff && scalar(cp) && test(String.fromCodePoint(cp));
    if (hit && start < 0) start = cp;
    if (!hit && start >= 0) {
      ranges.push([start, cp - 1]);
      start = -1;
    }
  }
  return ranges;
}
const forbidden = new RegExp(`[^\\S ]|[${GR}]`, 'u');
const tables = {
  SPACE: sweep((c) => /\s/u.test(c)),
  FORBIDDEN: sweep((c) => forbidden.test(c)),
  MARK: sweep((c) => /\p{M}/u.test(c)),
  PUNCT: sweep((c) => /\p{P}/u.test(c)),
  CASED: sweep((c) => /\p{Cased}/u.test(c)),
  CASE_IGNORABLE: sweep((c) => /\p{Case_Ignorable}/u.test(c)),
};
for (let cp = 0; cp <= 0x10ffff; cp++) {
  if (scalar(cp)) {
    const c = String.fromCodePoint(cp);
    if (/\s/u.test(c) !== (c.trim() === '')) throw new Error(`trim and \\s disagree at U+${cp.toString(16)}`);
  }
}
const lower = [];
for (let cp = 0x80; cp <= 0x10ffff; cp++) {
  if (!scalar(cp)) continue;
  const c = String.fromCodePoint(cp);
  const mapped = c.toLowerCase();
  if (mapped !== c) lower.push([cp, mapped]);
}
for (let cp = 0; cp < 0x80; cp++) {
  const c = String.fromCharCode(cp);
  if (c.toLowerCase() !== (cp >= 65 && cp <= 90 ? String.fromCharCode(cp + 32) : c)) throw new Error('ascii lower');
}

const hex = (n) => `0x${n.toString(16)}`;
function rustRanges(name, ranges, doc) {
  return `/// ${doc}\npub(crate) const ${name}: &[(u32, u32)] = &[\n${ranges.map(([a, b]) => `    (${hex(a)}, ${hex(b)}),`).join('\n')}\n];\n`;
}
function rustMap(name, entries, doc) {
  const esc = (s) => [...s].map((ch) => `\\u{${ch.codePointAt(0).toString(16)}}`).join('');
  return `/// ${doc}\npub(crate) const ${name}: &[(u32, &str)] = &[\n${entries.map(([a, s]) => `    (${hex(a)}, "${esc(s)}"),`).join('\n')}\n];\n`;
}
const rust = [
  '//! Character tables swept from Node by `bridge_create_session_goldens.mjs`; do not edit by hand.',
  `//! Node ${process.versions.node}, ICU ${process.versions.icu}, Unicode ${process.versions.unicode}.`,
  '',
  rustRanges('SPACE', tables.SPACE, 'Characters matching the JavaScript `\\s` class (and trimmed by `String.prototype.trim`).'),
  rustRanges('FORBIDDEN', tables.FORBIDDEN, 'Characters the bridge refuses in an error detail: `[^\\S ]` or `\\p{Cc}\\p{Cf}\\p{Cs}\\p{Co}\\p{Cn}`, the line separators, default ignorables and U+2800.'),
  rustRanges('MARK', tables.MARK, 'Characters matching `\\p{M}`.'),
  rustRanges('PUNCT', tables.PUNCT, 'Characters matching `\\p{P}`.'),
  rustRanges('CASED', tables.CASED, 'Characters matching `\\p{Cased}`.'),
  rustRanges('CASE_IGNORABLE', tables.CASE_IGNORABLE, 'Characters matching `\\p{Case_Ignorable}`.'),
  rustMap('LOWER', lower, 'Non-ASCII characters whose `toLowerCase()` differs from themselves, without the final sigma context rule.'),
].join('\n');

const compact = (cases) => '[\n' + cases.map((c) => JSON.stringify(c)).join(',\n') + '\n]\n';
const outputs = new Map();
outputs.set(tablesPath, rust);

// ---- unicode.json: sampled boundaries, sigma context, normalization sweep ----
const unicode = [];
const sigmaTexts = ['\u03a3', 'A\u03a3', 'A\u03a3B', 'A\u03a3 ', 'A.\u03a3', 'A\u00ad\u03a3', '\u03a3A', 'A\u03a3\u03a3', 'ΑΣ', 'ΑΣΑ', 'ΑΣ.', 'Σ.Α', 'a\u0301\u03a3', 'A\u2019\u03a3', '1\u03a3', 'İ', 'K', 'ſ', 'ǅ', 'ẞ', 'ﬅ', 'ǰ', 'İSTANBUL', 'GITHUB.COM', 'Github.com'];
for (const text of sigmaTexts) unicode.push({ kind: 'lower', text, lower: text.toLowerCase() });
const bases = ['a', '\u1100', '\u1161', ' ', '\u0627', '\u0f40'];
for (let cp = 0x80; cp <= 0x10ffff; cp++) {
  if (!scalar(cp) || forbidden.test(String.fromCodePoint(cp))) continue;
  const c = String.fromCodePoint(cp);
  for (const base of bases) {
    const text = base + c;
    const nfc = text.normalize('NFC');
    if (nfc !== text) unicode.push({ kind: 'nfc', text, nfc });
  }
  const alone = c.normalize('NFC');
  if (alone !== c) unicode.push({ kind: 'nfc', text: c, nfc: alone });
}
outputs.set(path.join(fixturesDir, 'unicode.json'), compact(unicode));

// ---- urls.json and details.json: run the real createCodeSession ----
const urls = [];
const add = (u) => urls.push(u);
const hosts = ['github.com', 'GitHub.COM', 'www.github.com', 'gitlab.com', 'example.org', 'a.b.c.dev', '-x.com', 'x-.com', 'host', 'host.123', '1.2.3.4', '[::1]', '[::1]:22', 'g\u00efthub.com', 'xn--ithub-0qa.com', '\u96ea.com', 'github\u3002com', 'github\uff0ecom', '\u212Aitchen.com', '\u0130x.com', 'github.com.', 'github.com:22', 'github.com:99999', 'github.com:', ':22', 'github.com%2f', 'git%68ub.com', 'git\u00adhub.com', 'git\u200bhub.com', 'github.com\\x'];
const schemes = ['https', 'http', 'ssh', 'git', 'HTTPS', 'Ssh', 'file', 'ftp', 'ws', 'git+ssh', 'h\u212Atp'];
const users = ['', 'git@', 'user:pw@', '%40@', 'a%40evil.com@', 'evil.com%23@', 'evil.com%2f@', 'evil.com%3f@', 'evil.com%5c@', 'a\\b@', '%00@', '%E9%9B%AA@', '%E2%80%AE@', '%ZZ@', '%C3@', '\u00e9@', '[::1]@', 'x@y@', '@'];
const paths = ['acme/widgets', 'acme/widgets.git', 'acme/widgets/', 'acme/wid gets', 'a/b/c', 'ac\u00e9/w', 'acme/w\u212A', '%61cme/widgets', 'acme/..', './widgets', 'acme/widgets?x', 'acme/widgets#f'];
for (const scheme of ['https', 'ssh', 'git', 'http', 'ftp', 'file']) {
  for (const host of hosts) add(`${scheme}://${host}/acme/widgets`);
}
for (const scheme of schemes) {
  for (const user of users) add(`${scheme}://${user}github.com/acme/widgets`);
}
for (const user of users) for (const host of ['github.com', 'example.org', '[::1]', '[::1]:22', 'github.com:22']) add(`${user || 'git@'}${host}:acme/widgets.git`);
for (const scheme of ['https', 'ssh']) for (const p of paths) add(`${scheme}://github.com/${p}`);
for (const p of paths) add(p);
for (const prefix of ['', ' ', '\t', '\n', '\u0001', '\u0000', ' \u0001 ', '\u00a0', '\u3000', '\u2028', '\ufeff', '\u200b', '\\', '//', '/']) {
  add(`${prefix}https://github.com/acme/widgets`);
  add(`${prefix}git@github.com:acme/widgets`);
  add(`${prefix}acme/widgets`);
}
for (const text of ['https://github.com/acme/widgets\n', 'https://gi\nthub.com/acme/widgets', 'https://github.com/acme/widgets\u2028', 'https://github.com\\acme/widgets', 'https:\\\\github.com/acme/widgets', 'https:/\\github.com/acme/widgets', 'ssh://git@github.com\\@evil.com/acme/widgets', 'ssh://evil.com\\@github.com/acme/widgets', 'git@[::1]:acme/widgets', 'git@[::1]/acme/widgets', 'ssh://git@[::1]/acme/widgets', 'ssh://git@[::1]:22/acme/widgets', 'ssh://[::1]:22/acme/widgets', 'https://[::1]/acme/widgets', 'https://[fe80::1%25eth0]/acme/widgets', 'ssh://[g]/acme/widgets', 'git@[x]:acme/widgets', 'a@[::1]:b/c', 'file://[::1]/acme/widgets', 'git@github.com:acme/widgets@[::1]', 'git@github.com:[::1]/widgets', 'git@github.com:acme/[x]', 'ssh://github.com:22:22/acme/widgets', 'ssh://github.com:-1/acme/widgets', 'ssh://x@[::1]:65536/acme/widgets', 'ssh://x@github.com:0/acme/widgets', 'ssh://x@github.com: 22/acme/widgets', 'C:\\acme\\widgets', 'c:/acme/widgets', 'x:y/acme', 'git@host.org:acme/widgets:extra', 'https://github.com/acme/widgets://x', 'ssh://%C3%A9@github.com/acme/widgets', 'ssh://%E2%84%AA@github.com/acme/widgets', 'ssh://\u00e9%40github.com@github.com/acme/widgets', 'ssh://a%2540b@github.com/acme/widgets', 'ssh://a%3Ab@github.com/acme/widgets', 'ssh://a%25@github.com/acme/widgets', 'ssh://%EF%BC%8F@github.com/acme/widgets', 'ssh://%EF%BC%A0github.com/acme/widgets', 'ssh://x%EF%BC%8Fy@github.com/acme/widgets', 'ssh://%E3%80%82@github.com/acme/widgets', 'ssh://%ED%A0%80@github.com/acme/widgets', 'ssh://%F4%90%80%80@github.com/acme/widgets', 'ssh://%C0%AF@github.com/acme/widgets', 'ssh://%0A@github.com/acme/widgets', 'ssh://%09github.com/acme/widgets']) add(text);
const uniqueUrls = [...new Set(urls)];

const startServer = () => new Promise((resolve) => {
  const state = { body: '', port: 0, status: 200, reply: '' };
  const server = http.createServer((req, res) => {
    const chunks = [];
    req.on('data', (chunk) => chunks.push(chunk));
    req.on('end', () => {
      state.body = Buffer.concat(chunks).toString('utf8');
      res.writeHead(state.status, { 'content-type': 'application/json' });
      res.end(state.reply);
    });
  });
  server.listen(0, '127.0.0.1', () => {
    state.port = server.address().port;
    resolve({ server, state });
  });
});

const cwdForRun = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'bridge-create-')));
process.chdir(cwdForRun);
delete process.env.CLAUDE_CODE_ENTRYPOINT;
const bridge = await import(pathToFileURL(bridgePath).href);
const { server, state } = await startServer();
const base = () => `http://127.0.0.1:${state.port}`;
state.reply = JSON.stringify({ session: { id: 'cse_abc' } });

const urlCases = [];
for (const url of uniqueUrls) {
  const result = await bridge.createCodeSession(base(), 't', 'T', 5000, undefined, { gitRepoUrl: url, branch: 'feat', defaultBranch: 'main' }, '/w');
  const config = JSON.parse(state.body).config;
  const entry = { url, ok: result === 'cse_abc', source: config.sources?.[0]?.url ?? null, repo: config.outcomes?.[0]?.git_info?.repo ?? null };
  urlCases.push(entry);
}
outputs.set(path.join(fixturesDir, 'urls.json'), compact(urlCases));

const detailMessages = [];
const detail = (m) => detailMessages.push(m);
for (const m of ['plain', 'caf\u00e9', 'cafe\u0301', 'e\u0301', '\u0301', 'a \u0301b', 'a.\u0301b', 'a\u0301\u0302', '\u212b', '\u00c5', 'A\u030a', 'x\u0345', '\u1100\u1161', '\u1100\u1161\u11a8', '\ud55c\u1100', '\u0627\u0653', '\u0f40\u0f71', '\u0958', '\u2126', '\u03a9', '\u1e9b\u0323', '\ufb01', '\u00b5', '\u0130', 'a\u0307\u0323', 'a\u0323\u0307', '\u30ab\u3099', '\u30ac']) detail(m);
for (const [a, b] of tables.FORBIDDEN) for (const cp of new Set([a, b, a + 1, b + 1])) if (cp <= 0x10ffff && scalar(cp)) detail(`a${String.fromCodePoint(cp)}b`);
for (const [a, b] of [...tables.MARK, ...tables.PUNCT, ...tables.SPACE]) for (const cp of new Set([a, b])) detail(`.${String.fromCodePoint(cp)}x`), detail(` ${String.fromCodePoint(cp)}x`), detail(`x${String.fromCodePoint(cp)}`), detail(String.fromCodePoint(cp) + 'x');
for (let cp = 0x80; cp <= 0x10ffff; cp += 0x101) if (scalar(cp)) detail(`m${String.fromCodePoint(cp)}`);
const uniqueDetails = [...new Set(detailMessages)];
state.status = 400;
const detailCases = [];
for (const message of uniqueDetails) {
  state.reply = JSON.stringify({ error: { message } });
  const result = await bridge.createCodeSession(base(), 't', 'T', 5000);
  detailCases.push({ message, detail: result && typeof result === 'object' ? (result.detail ?? null) : '<none>' });
}
outputs.set(path.join(fixturesDir, 'details.json'), compact(detailCases));
server.close();

fs.rmSync(cwdForRun, { recursive: true, force: true });

let stale = false;
for (const [file, text] of outputs) {
  if (check) {
    if (!fs.existsSync(file) || fs.readFileSync(file, 'utf8') !== text) {
      process.stderr.write(`${file} is stale\n`);
      stale = true;
    }
  } else {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, text);
  }
}
process.exit(stale ? 1 : 0);
