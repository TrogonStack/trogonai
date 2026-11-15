#!/usr/bin/env node
// Records what the pinned `extractFromBunfs.js` does on a real file system, so the Rust `extract_from_bunfs` is
// checked against the upstream code instead of against a reading of it.
//
// usage: node extract_from_bunfs_goldens.mjs <extractFromBunfs.js> <out-file>
//
// Every case builds a fixture tree under a fresh temporary root, runs the upstream function (twice when `calls`
// says so) with `process.platform`, `process.getuid` and `process.pid` replaced where the case says so, and
// records the returned paths, the warnings, and the state it left behind. Machine-specific text is replaced with
// placeholders: `{root}` (the fixture root), `{uid}` (the owner of the extraction root), `{fake_uid}` (the
// replaced `process.getuid` of the wrong-owner case), `{hash}`, `{pid}` and `{random}`. The recorder also dumps
// the `path.posix` and `path.win32` join results the Rust path code must reproduce, and libuv's description for
// every errno name the warning text can carry.
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { getSystemErrorMap } from 'node:util';
import { pathToFileURL } from 'node:url';

const [source, out] = process.argv.slice(2);
if (!source || !out) {
  process.stderr.write('usage: extract_from_bunfs_goldens.mjs EXTRACT_JS OUT_FILE\n');
  process.exit(2);
}

const { extractFromBunfs } = await import(pathToFileURL(path.resolve(source)).href);
const realUid = process.getuid();
const fakeUid = realUid + 1;
const realGetuid = process.getuid;
const realPlatform = process.platform;
const PID = 4242;

const content = 'claude-binary-bytes\n';
const hash16 = createHash('sha256').update(content).digest('hex').slice(0, 16);
const BIG = 'abcdefgh'.repeat(((1 << 20) / 8) * 2) + 'tail';
const bigHash16 = createHash('sha256').update(BIG).digest('hex').slice(0, 16);
const BIG_LATE = `${BIG.slice(0, -4)}tall`;
// The two megabyte bodies are recorded by name; the Rust test rebuilds them.
const shorten = (text) => (text === BIG ? '@big' : text === BIG_LATE ? '@big_late' : text);

function mockProcess({ platform, uid }) {
  Object.defineProperty(process, 'platform', { value: platform ?? realPlatform, configurable: true });
  Object.defineProperty(process, 'pid', { value: PID, configurable: true });
  if (uid === 'none') {
    process.getuid = undefined;
  } else if (uid === 'fake') {
    process.getuid = () => fakeUid;
  } else {
    process.getuid = realGetuid;
  }
}

function template(text, root, hash) {
  return text
    .replaceAll(root, '{root}')
    .replaceAll(`claude-agent-sdk-${hash}`, 'claude-agent-sdk-{hash}')
    .replaceAll(`claude-agent-sdk-${realUid}`, 'claude-agent-sdk-{uid}')
    .replaceAll(`claude-agent-sdk-${fakeUid}`, 'claude-agent-sdk-{fake_uid}')
    .replaceAll(`uid ${realUid}, expected ${fakeUid}`, 'uid {uid}, expected {fake_uid}')
    .replace(new RegExp(`\\.tmp\\.${PID}\\.[0-9a-f]{16}`, 'g'), '.tmp.{pid}.{random}');
}

function resolve(text, root, hash) {
  return text
    .replaceAll('{root}', root)
    .replaceAll('{hash}', hash)
    .replaceAll('{uid}', String(realUid));
}

function describeTree(root, hash, targets) {
  const files = {};
  for (const target of targets) {
    const file = resolve(target, root, hash);
    try {
      const stat = fs.lstatSync(file);
      files[target] = {
        kind: stat.isSymbolicLink() ? 'symlink' : stat.isDirectory() ? 'directory' : 'file',
        mode: stat.mode & 0o777,
        nlink: stat.nlink,
        ...(stat.isFile() ? { content: shorten(fs.readFileSync(file, 'utf8')) } : {}),
        ...(stat.isDirectory() ? { listing: fs.readdirSync(file).sort() } : {}),
      };
    } catch (error) {
      files[target] = { kind: 'missing', code: error.code };
    }
  }
  return files;
}

function run(spec) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'extract-golden-'));
  try {
    const hash = spec.hash ?? hash16;
    const name = spec.name ?? 'claude';
    const body = spec.content ?? content;
    const embedded = spec.embedded ?? `{root}/$bunfs/root/${name}`;
    if (spec.embedded === undefined) {
      fs.mkdirSync(path.join(root, '$bunfs', 'root'), { recursive: true });
      fs.writeFileSync(path.join(root, '$bunfs', 'root', name), body);
    }
    for (const step of spec.setup ?? []) {
      const target = resolve(step.path, root, hash);
      if (step.op === 'mkdir') {
        fs.mkdirSync(target, { recursive: true });
        fs.chmodSync(target, step.mode ?? 0o700);
      } else if (step.op === 'write') {
        fs.mkdirSync(path.dirname(target), { recursive: true });
        fs.writeFileSync(target, step.content);
      } else if (step.op === 'symlink') {
        fs.mkdirSync(path.dirname(target), { recursive: true });
        fs.symlinkSync(resolve(step.target, root, hash), target);
      } else if (step.op === 'link') {
        fs.linkSync(resolve(step.target, root, hash), target);
      }
    }
    const saved = {};
    for (const [key, value] of Object.entries(spec.env ?? {})) {
      saved[key] = process.env[key];
      process.env[key] = resolve(value, root, hash);
    }
    const warnings = [];
    const realWarn = console.warn;
    console.warn = (message) => warnings.push(message);
    mockProcess(spec);
    const returns = [];
    try {
      for (let call = 0; call < (spec.calls ?? 1); call += 1) {
        returns.push(extractFromBunfs(resolve(embedded, root, hash)));
      }
    } finally {
      console.warn = realWarn;
      mockProcess({});
      for (const [key, value] of Object.entries(saved)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
      }
    }
    const observed = describeTree(root, hash, spec.inspect ?? []);
    return {
      id: spec.id,
      platform: spec.platform ?? 'linux',
      uid: spec.uid ?? 'real',
      name,
      content: shorten(body),
      embedded,
      setup: (spec.setup ?? []).map((step) => (step.content === undefined ? step : { ...step, content: shorten(step.content) })),
      env: spec.env ?? {},
      calls: spec.calls ?? 1,
      hash,
      returns: returns.map((value) => template(value, root, hash)),
      warnings: warnings.map((value) => template(value, root, hash)),
      files: JSON.parse(template(JSON.stringify(observed), root, hash)),
    };
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
}

const base = '{root}/tmp';
const rootDir = `${base}/claude-agent-sdk-{uid}`;
const hashDir = `${rootDir}/claude-agent-sdk-{hash}`;
const extracted = `${hashDir}/claude`;
const env = { CLAUDE_CODE_TMPDIR: base };
const windowsRoot = `${base}/claude-agent-sdk`;
const windowsHashDir = `${windowsRoot}/claude-agent-sdk-{hash}`;
const windowsExtracted = `${windowsHashDir}/claude`;

const cases = [
  { id: 'fresh_extraction', env, inspect: [rootDir, hashDir, extracted] },
  { id: 'second_call_reuses_the_copy', env, calls: 2, inspect: [hashDir, extracted] },
  {
    id: 'tampered_copy_with_the_same_length_is_replaced',
    env,
    setup: [{ op: 'write', path: extracted, content: content.replace('a', 'b') }],
    inspect: [hashDir, extracted],
  },
  {
    id: 'tampered_copy_with_another_length_is_replaced',
    env,
    setup: [{ op: 'write', path: extracted, content: 'short' }],
    inspect: [hashDir, extracted],
  },
  {
    id: 'hard_linked_copy_is_replaced',
    env,
    setup: [
      { op: 'write', path: '{root}/elsewhere', content },
      { op: 'mkdir', path: hashDir },
      { op: 'link', path: extracted, target: '{root}/elsewhere' },
    ],
    inspect: [hashDir, extracted, '{root}/elsewhere'],
  },
  {
    id: 'symlinked_copy_is_replaced',
    env,
    setup: [
      { op: 'write', path: '{root}/elsewhere', content },
      { op: 'mkdir', path: hashDir },
      { op: 'symlink', path: extracted, target: '{root}/elsewhere' },
    ],
    inspect: [hashDir, extracted, '{root}/elsewhere'],
  },
  {
    id: 'loose_extraction_root_is_tightened',
    env,
    setup: [{ op: 'mkdir', path: rootDir, mode: 0o755 }],
    inspect: [rootDir, extracted],
  },
  {
    id: 'embedded_name_is_kept',
    env,
    name: 'claude.exe',
    inspect: [`${hashDir}/claude.exe`],
  },
  {
    id: 'content_over_one_chunk_is_compared_in_full',
    env,
    content: BIG,
    hash: bigHash16,
    calls: 2,
    inspect: [hashDir],
  },
  {
    id: 'content_over_one_chunk_with_a_late_difference_is_replaced',
    env,
    content: BIG,
    hash: bigHash16,
    setup: [{ op: 'write', path: extracted, content: BIG_LATE }],
    inspect: [hashDir],
  },
  { id: 'missing_embedded_file', env, embedded: '/$bunfs/root/definitely-missing' },
  { id: 'missing_embedded_file_with_the_windows_marker', env, embedded: 'B:\\~BUN\\root\\claude.exe' },
  {
    id: 'embedded_directory',
    env,
    embedded: '{root}/$bunfs',
    setup: [{ op: 'mkdir', path: '{root}/$bunfs' }],
  },
  {
    id: 'temp_directory_is_a_file',
    env,
    setup: [{ op: 'write', path: base, content: 'file' }],
  },
  {
    id: 'extraction_root_is_a_symlink',
    env,
    setup: [
      { op: 'mkdir', path: '{root}/real-root' },
      { op: 'symlink', path: rootDir, target: '{root}/real-root' },
    ],
  },
  { id: 'extraction_root_owned_by_another_uid', env, uid: 'fake' },
  {
    id: 'hash_directory_is_a_symlink',
    env,
    setup: [
      { op: 'mkdir', path: '{root}/real-hash' },
      { op: 'mkdir', path: rootDir },
      { op: 'symlink', path: hashDir, target: '{root}/real-hash' },
    ],
  },
  {
    id: 'rename_onto_a_directory',
    env,
    setup: [{ op: 'write', path: `${extracted}/inner`, content: 'x' }],
    inspect: [hashDir],
  },
  {
    id: 'windows_platform_extracts_without_uid',
    env,
    platform: 'win32',
    uid: 'none',
    inspect: [windowsRoot, windowsHashDir, windowsExtracted],
  },
  {
    id: 'windows_platform_reuses_the_copy',
    env,
    platform: 'win32',
    uid: 'none',
    calls: 2,
    inspect: [windowsExtracted],
  },
  {
    id: 'windows_platform_replaces_a_tampered_copy',
    env,
    platform: 'win32',
    uid: 'none',
    setup: [{ op: 'write', path: windowsExtracted, content: 'short' }],
    inspect: [windowsExtracted],
  },
  {
    id: 'windows_platform_replaces_a_symlinked_copy',
    env,
    platform: 'win32',
    uid: 'none',
    setup: [
      { op: 'write', path: '{root}/elsewhere', content },
      { op: 'mkdir', path: windowsHashDir },
      { op: 'symlink', path: windowsExtracted, target: '{root}/elsewhere' },
    ],
    inspect: [windowsExtracted, '{root}/elsewhere'],
  },
  {
    id: 'windows_platform_with_a_uid_keeps_the_plain_root_name',
    env,
    platform: 'win32',
    inspect: [windowsRoot, windowsExtracted],
  },
  {
    id: 'windows_platform_ignores_a_temp_directory_that_is_a_file',
    env,
    platform: 'win32',
    uid: 'none',
    setup: [{ op: 'write', path: base, content: 'file' }],
  },
  {
    id: 'os_temp_directory_on_linux',
    env: { CLAUDE_CODE_TMPDIR: '', TMPDIR: base },
    platform: 'linux',
    inspect: [extracted],
  },
  { id: 'passes_a_plain_path_through', embedded: '/usr/local/bin/claude' },
];

const results = cases.map(run);

const joinInputs = [
  [],
  ['x'],
  ['/a/..'],
  ['a/..', '/'],
  ['a/..'],
  ['/'],
  ['\\'],
  ['C:'],
  ['//server//share//dir'],
  ['\\\\server\\share\\'],
  ['\\\\server\\\\'],
  [''],
  ['/tmp', 'claude-agent-sdk-501'],
  ['/tmp/', 'claude-agent-sdk-501'],
  ['/a//b/', 'c'],
  ['/a/./b/../c', 'd'],
  ['a', '..', '..', 'b'],
  ['/..', 'x'],
  ['', 'x'],
  ['x', ''],
  ['.', '.'],
  ['a/', ''],
  ['C:\\Users\\me\\AppData\\Local\\Temp', 'claude-agent-sdk'],
  ['C:/Temp/', 'claude-agent-sdk', 'x'],
  ['C:', 'x'],
  ['C:\\', '..', 'x'],
  ['\\\\server\\share\\dir', 'x'],
  ['\\\\server\\share', 'x'],
  ['\\\\server', 'x'],
  ['//server/share/', '..', 'y'],
  ['\\\\', 'x'],
  ['\\\\\\a', 'b'],
  ['\\a\\b', '..', '..', '..', 'c'],
  ['a\\b', '..', '..', '..', 'c'],
  ['C:a', '..', '..', 'b'],
  ['/', 'x'],
  ['\\', 'x'],
  ['x/y/', ''],
  ['x\\y\\', ''],
];
const joins = joinInputs.map((parts) => ({
  parts,
  posix: path.posix.join(...parts),
  win32: path.win32.join(...parts),
}));

const wanted = [
  'EPERM',
  'ENOENT',
  'EACCES',
  'EEXIST',
  'EBUSY',
  'ENOTDIR',
  'EISDIR',
  'ENOTEMPTY',
  'ELOOP',
  'ENOSPC',
  'EROFS',
  'EMFILE',
  'EXDEV',
  'ENAMETOOLONG',
  'EIO',
  'ENOMEM',
  'EINVAL',
];
const errors = {};
for (const [, [name, message]] of getSystemErrorMap()) {
  if (wanted.includes(name)) errors[name] = message;
}

const document = { source: path.basename(source), hash: hash16, big_hash: bigHash16, cases: results, joins, errors };
fs.mkdirSync(path.dirname(path.resolve(out)), { recursive: true });
fs.writeFileSync(out, `${JSON.stringify(document, null, 2).replaceAll('\u2014', '\\u2014')}\n`);
