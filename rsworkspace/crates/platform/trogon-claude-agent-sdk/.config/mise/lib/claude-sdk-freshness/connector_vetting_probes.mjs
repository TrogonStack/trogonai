#!/usr/bin/env node
// Records what the pinned bundle's connector folder check answers when its path vetting walks a stubbed host.
//
// usage: node connector_vetting_probes.mjs --package <dir> --node-lib <dir> --out <file> [--check]
//
// The real functions are cut out of the pinned sdk.mjs by name and evaluated in a sandbox whose `process.platform`,
// working directory, environment, launch directory, home directory and file system are injected, so every operating
// system branch runs the same way on any host. The `path` module the bundle imports is node's own lib/path.js for the
// node release a probe names, loaded from `--node-lib` (files named path-<version>.js), because its win32 handling
// changed between releases. Each probe records the answers and the ordered host reads the upstream code made; the Rust
// tests replay both. `--check` fails when the recorded file is not what the bundle produces now.
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { corpus } from './connector_vetting_corpus.mjs';
import { NODE_RELEASES } from './node_releases.mjs';

const args = process.argv.slice(2);
const option = (name) => {
  const index = args.indexOf(name);
  return index < 0 ? undefined : args[index + 1];
};
const pkg = option('--package');
const nodeLib = option('--node-lib');
const out = option('--out');
if (!pkg || !nodeLib || !out) {
  process.stderr.write('usage: connector_vetting_probes.mjs --package DIR --node-lib DIR --out FILE [--check]\n');
  process.exit(2);
}
const bundle = fs.readFileSync(path.join(pkg, 'sdk.mjs'), 'utf8');

const nodePathSource = Object.fromEntries(
  NODE_RELEASES.map((release) => [release, fs.readFileSync(path.join(nodeLib, `path-${release}.js`), 'utf8')]),
);

// Finds the end of the block that opens at `open`, skipping strings, template literals and regular expressions.
function blockEnd(text, open) {
  let depth = 0;
  let previous = '';
  for (let index = open; index < text.length; index++) {
    const character = text[index];
    if (character === '"' || character === "'") {
      index++;
      while (text[index] !== character) index += text[index] === '\\' ? 2 : 1;
      previous = character;
      continue;
    }
    if (character === '`') {
      index++;
      let nesting = 0;
      while (!(text[index] === '`' && nesting === 0)) {
        if (text[index] === '\\') index += 2;
        else if (text[index] === '$' && text[index + 1] === '{') {
          nesting++;
          index += 2;
        } else if (text[index] === '}' && nesting > 0) {
          nesting--;
          index++;
        } else index++;
      }
      previous = character;
      continue;
    }
    if (character === '/' && /[(,=:[!&|?{};]/.test(previous) && text[index + 1] !== '/' && text[index + 1] !== '*') {
      index++;
      let inClass = false;
      while (inClass || text[index] !== '/') {
        if (text[index] === '\\') index++;
        else if (text[index] === '[') inClass = true;
        else if (text[index] === ']') inClass = false;
        index++;
      }
      previous = '/';
      continue;
    }
    if (character === '{') depth++;
    else if (character === '}') {
      depth--;
      if (depth === 0) return index + 1;
    }
    if (!/\s/.test(character)) previous = character;
  }
  throw new Error('unbalanced block');
}

function functionSource(name) {
  const marks = [`function ${name}(`, `function*${name}(`];
  const found = marks.flatMap((mark) => {
    const hits = [];
    for (let at = bundle.indexOf(mark); at >= 0; at = bundle.indexOf(mark, at + 1)) hits.push(at);
    return hits;
  });
  if (found.length !== 1) throw new Error(`expected one definition of ${name}, found ${found.length}`);
  const start = found[0];
  // The body opens after the parameter list, which may itself destructure an object.
  return bundle.slice(start, blockEnd(bundle, bundle.indexOf('){', start) + 1));
}

// Text the sandbox prelude restates by hand; each must still be in the bundle exactly as written here.
function assertBundleHolds(literal) {
  if (!bundle.includes(literal)) throw new Error(`the bundle no longer holds: ${literal}`);
}

// Functions the connector check runs, and everything they call that is not a host effect.
const FUNCTIONS = [
  'aM', 'bd', 'Qee', 'Il', 'qee', 'tte', 'nM', 'ha', 'go', 'vI', 'dd', 'wI', 'zI', 'Nb', 'Rr', 'Kr', 'Pn', 'Wm', 'nee',
  'Or', 'Yn', 'vl', 'xl', 'Vb', 'ud', 'jm', 'Wb', 'MQ', 'FQ', 'Bb', 'BI', 'XI', 'HQ', 'cd', 'at', 'sg', 'rM', 'MC',
  'ag', 'oM', '$I', 'OI', 'Jo', 'Gb', 'es', 'Xo', 'Qo', 'ig', 'sM', 'IC', 'Bee', 'jee', 'Gee', 'Wee', 'LC', 'cg', 'Nm',
  'ae', 'tt', 'Jee', 'Xee', 'ete', 'Vee', 'n7e', 't7e', 'nE', 'rXe', 'uP', 'H5', 'im', 'X8e', 'Q8e', 'U5', 'we',
  'Lx', 'jI', 'kYe', '_4',
];
[
  'Dl=process.platform==="win32"?/[\\\\/]+/:/\\/+/;Kee=process.platform==="win32"?Bee:jee;',
  'yd="ERR_SYMLINK_TO_NETWORK_PATH"',
  'Wr="\\x00unverified-ancestry"',
  'Sd="\\x00opaque-link-text"',
  'NQ=6,KI=1024,GI=4096',
  'MI=/^[\\\\/]\\?\\?[\\\\/]/',
  'J8e="connector-downloads"',
  'var fJ=/[\\u200c-\\u200f\\u202a-\\u202e\\u206a-\\u206f\\ufeff]/,nXe=new RegExp(fJ,"g")',
  'sm="/mnt/user-data/uploads"',
  'z8e=eE(QS(sm),$8e)',
  '$8e="/connector-downloads"',
  'var e7e=/^(download-[0-9a-f]{8}(-\\d+)?(\\.[^./\\\\]*)?|\\.download-[0-9a-f]{8}(-\\d+)?(\\.[^./\\\\]*)?\\.(from\\.json|part))$/;',
  'import*as Ne from"path"',
  'import*as zb from"path"',
  'resolve as Y8e}from"node:path"',
  'var{dirname:QS,isAbsolute:Zqt,join:eE,normalize:Jqt,relative:Xqt,sep:Qqt}=F8e',
  'import{posix as F8e}from"path"',
  'XE=(()=>{if(typeof process>"u"||typeof process.cwd!=="function")return null;try{return process.cwd()}catch{return null}})()',
  'TI={readAutoMaster:()=>LQ("/etc/auto_master","utf8"),homeIsOwnMount:()=>{let e;try{e=AI("/home")}catch(t){if(at(t))return!0;throw t}return e.dev!==AI("/Users").dev}}',
  'readFileSync as LQ',
  'statSync as AI',
  'import{homedir as FI}from"os"',
  'lstatSync as G8e',
  'openSync as W8e',
  'readSync as V8e',
  'closeSync as K8e',
  'function F5(){return LP.O_RDONLY|j8e()}function j8e(){if(ut()==="windows")return 0;return LP.O_NOFOLLOW|LP.O_NONBLOCK}',
  'UC={cwd(){return process.cwd()}',
  'return Ce.lstatSync(e)',
  'return Ce.readlinkSync(e)',
  'return Ce.readdirSync(e,{withFileTypes:!0})',
  'let t=Ce.openSync(e,Ce.constants.O_DIRECTORY|Ce.constants.O_NOFOLLOW);Ce.closeSync(t)',
  'var S4={unreadableAncestry:"unverified",surfaceNetworkRaw:!0,literalLinkText:"opaque"};',
  'realpathSync as OYe}from"fs"',
  'join as Mx,',
  'function*E4(e,t,n,r){let o=Mx(e,...t),',
  'let i=yield*Lx(o),a=yield*Lx(e);if(i==="gone"||a==="gone")return null;if(a==="unverified"||"network"in a||i==="unverified")return{path:o,folder:e,kind:"unverified"};',
].forEach(assertBundleHolds);

const PRELUDE = `
var yd="ERR_SYMLINK_TO_NETWORK_PATH", Wr="\\x00unverified-ancestry", Sd="\\x00opaque-link-text";
var NQ=6, KI=1024, GI=4096;
var MI=/^[\\\\/]\\?\\?[\\\\/]/;
var J8e="connector-downloads";
var fJ=/[\\u200c-\\u200f\\u202a-\\u202e\\u206a-\\u206f\\ufeff]/, nXe=new RegExp(fJ,"g");
var e7e=/^(download-[0-9a-f]{8}(-\\d+)?(\\.[^./\\\\]*)?|\\.download-[0-9a-f]{8}(-\\d+)?(\\.[^./\\\\]*)?\\.(from\\.json|part))$/;
var Ne=pathModule, zb=pathModule, Y8e=pathModule.resolve;
var {dirname:QS,join:eE}=pathModule.posix;
var sm="/mnt/user-data/uploads", $8e="/connector-downloads", z8e=eE(QS(sm),$8e);
var $m=new Map, zm=new Map, tE=new Map, Ub={value:void 0};
var TI={readAutoMaster:()=>LQ("/etc/auto_master","utf8"),homeIsOwnMount:()=>{let e;try{e=AI("/home")}catch(t){if(at(t))return!0;throw t}return e.dev!==AI("/Users").dev}};
var Dl=process.platform==="win32"?/[\\\\/]+/:/\\/+/;
var UC=hostUC;
var S4={unreadableAncestry:"unverified",surfaceNetworkRaw:!0,literalLinkText:"opaque"};
var E4=function*(e){return yield*Lx(e)};
`;
const KEE = 'var Kee=process.platform==="win32"?Bee:jee;';
const VQ_START = bundle.indexOf(';VQ=[');
const VQ = bundle.slice(VQ_START + 4, bundle.indexOf('];MI=', VQ_START) + 1);
const FUNCTION_SOURCE = FUNCTIONS.map(functionSource).join('\n');

// Evaluates node's lib/path.js for `release` against the sandbox process, with the primordials it destructures.
function loadNodePath(release, sandboxProcess) {
  const primordials = new Proxy(
    {},
    {
      get(_, name) {
        const match = /^(Array|String|Function)Prototype(\w+)$/.exec(name);
        if (!match) throw new Error(`unexpected primordial ${String(name)}`);
        const method = { Array, String, Function }[match[1]].prototype[match[2][0].toLowerCase() + match[2].slice(1)];
        return (receiver, ...rest) => method.apply(receiver, rest);
      },
    },
  );
  const internal = {
    'internal/constants': {
      CHAR_UPPERCASE_A: 65,
      CHAR_LOWERCASE_A: 97,
      CHAR_UPPERCASE_Z: 90,
      CHAR_LOWERCASE_Z: 122,
      CHAR_DOT: 46,
      CHAR_FORWARD_SLASH: 47,
      CHAR_BACKWARD_SLASH: 92,
      CHAR_COLON: 58,
      CHAR_QUESTION_MARK: 63,
    },
    'internal/validators': {
      validateString(value, name) {
        if (typeof value !== 'string') throw new TypeError(`${name} must be a string`);
      },
      validateObject() {},
    },
    'internal/util': {
      isWindows: sandboxProcess.platform === 'win32',
      isMacOS: sandboxProcess.platform === 'darwin',
      getLazy: (load) => () => load(),
      emitExperimentalWarning() {},
    },
  };
  const module = { exports: {} };
  const require = (name) => {
    if (!(name in internal)) throw new Error(`unexpected require ${name}`);
    return internal[name];
  };
  new Function('module', 'require', 'primordials', 'process', nodePathSource[release])(
    module,
    require,
    primordials,
    sandboxProcess,
  );
  return module.exports;
}

function sandbox(probe) {
  const trace = [];
  const raise = (code) => Object.assign(new Error(code), { code });
  const platform = probe.platform ?? 'linux';
  // Windows accepts either separator and either case of drive letter, so the fake file system keys every entry by its
  // backslash spelling with an upper case drive letter there. The host reads keep the spelling the bundle asked for.
  const fsKey = (key) =>
    platform === 'win32' ? key.replace(/\//g, '\\').replace(/^[a-z]:/, (drive) => drive.toUpperCase()) : key;
  const keyed = (entries) => Object.fromEntries(Object.entries(entries ?? {}).map(([key, value]) => [fsKey(key), value]));
  const tree = keyed(probe.fs);
  const forcedOpendir = keyed(probe.opendir);
  const forcedReadlink = keyed(probe.readlink);
  const forcedReaddir = keyed(probe.readdir);
  const forcedRealpath = keyed(probe.realpath);
  const parent = (key) => key.replace(/[\\/][^\\/]*$/, '');
  // A missing entry beneath a file is not a folder (ENOTDIR); any other missing entry does not exist (ENOENT).
  const missing = (key) => {
    for (let above = parent(fsKey(key)), below = fsKey(key); above !== below; below = above, above = parent(above)) {
      if (tree[above]?.kind === 'file') return raise('ENOTDIR');
    }
    return raise('ENOENT');
  };
  const environment = probe.env ?? {};
  const findVariable = (key) =>
    platform === 'win32'
      ? Object.keys(environment).find((name) => name.toUpperCase() === key.toUpperCase())
      : Object.hasOwn(environment, key)
        ? key
        : undefined;
  const env = new Proxy(
    {},
    {
      get(_, key) {
        if (typeof key !== 'string') return undefined;
        trace.push(['env', key]);
        const name = findVariable(key);
        return name === undefined ? undefined : environment[name];
      },
    },
  );
  const sandboxProcess = {
    platform,
    env,
    cwd() {
      trace.push(['cwd']);
      if (probe.cwd === null || probe.cwd === undefined) throw raise('ENOENT');
      return probe.cwd;
    },
  };
  const lstat = (key) => {
    trace.push(['lstat', key]);
    const found = tree[fsKey(key)];
    if (found === undefined) throw missing(key);
    if (found.errno) throw raise(found.errno);
    return {
      isSymbolicLink: () => found.kind === 'symlink',
      isFile: () => found.kind === 'file',
      isDirectory: () => found.kind === 'dir',
      size: found.size ?? Buffer.byteLength(found.content ?? ''),
    };
  };
  const hostUC = {
    cwd: () => sandboxProcess.cwd(),
    lstatSync: lstat,
    openDirNoFollowSync(key) {
      trace.push(['opendir', key]);
      const forced = forcedOpendir[fsKey(key)];
      if (forced === 'ok') return;
      if (forced) throw raise(forced);
      const found = tree[fsKey(key)];
      if (found === undefined) throw missing(key);
      if (found.errno) throw raise(found.errno);
      if (found.kind === 'dir') return;
      throw raise(found.kind === 'symlink' ? 'ELOOP' : 'ENOTDIR');
    },
    readlinkSync(key) {
      trace.push(['readlink', key]);
      const forced = forcedReadlink[fsKey(key)];
      if (forced) throw raise(forced);
      const found = tree[fsKey(key)];
      if (found === undefined) throw missing(key);
      if (found.kind !== 'symlink') throw raise('EINVAL');
      return found.target;
    },
    readdirSync(key) {
      trace.push(['readdir', key]);
      // Folders are listed from the tree; a probe forces any other listing (including one through a link) with `readdir`.
      const folder = fsKey(key);
      const listed = forcedReaddir[folder];
      if (typeof listed === 'string') throw raise(listed);
      if (listed === undefined) {
        const found = tree[folder];
        if (found === undefined) throw missing(key);
        if (found.errno) throw raise(found.errno);
        if (found.kind !== 'dir') throw raise('ENOTDIR');
      }
      const names =
        listed ??
        Object.keys(tree)
          .filter((candidate) => candidate !== folder && parent(candidate) === folder)
          .map((candidate) => candidate.slice(folder.length + 1));
      return names.map((name) => ({ name }));
    },
  };
  let openFile;
  const context = vm.createContext({
    process: sandboxProcess,
    Buffer,
    hostUC,
    XE: probe.launch ?? null,
    LQ(key) {
      trace.push(['readfile', key]);
      const text = (probe.files ?? {})[key];
      if (text === undefined) throw raise('ENOENT');
      if (text.errno) throw raise(text.errno);
      return text;
    },
    AI(key) {
      trace.push(['stat', key]);
      const found = (probe.stat ?? {})[key];
      if (found === undefined) throw raise('ENOENT');
      if (typeof found === 'string') throw raise(found);
      return { dev: found.dev };
    },
    FI() {
      trace.push(['homedir']);
      if (probe.homedir === null || probe.homedir === undefined) throw raise('ENOENT');
      return probe.homedir;
    },
    G8e: lstat,
    W8e(key) {
      trace.push(['open', key]);
      const found = tree[fsKey(key)];
      if (found === undefined) throw missing(key);
      if (found.openErrno) throw raise(found.openErrno);
      openFile = Buffer.from(found.content ?? '');
      return 3;
    },
    // `realpathSync.native` answers the path itself unless a probe forces another answer with `realpath`.
    OYe: {
      native(key) {
        trace.push(['realpath', key]);
        const forced = forcedRealpath[fsKey(key)];
        if (forced !== undefined) {
          if (forced.errno) throw raise(forced.errno);
          return forced.path;
        }
        const found = tree[fsKey(key)];
        if (found === undefined) throw missing(key);
        if (found.errno) throw raise(found.errno);
        return key;
      },
    },
    F5: () => 0,
    V8e: (_, buffer, offset, length) => openFile.copy(buffer, offset, 0, length),
    K8e: () => {},
    pathModule: loadNodePath(probe.node ?? NODE_RELEASES[0], sandboxProcess),
  });
  vm.runInContext(`${PRELUDE}\n${KEE}\n${FUNCTION_SOURCE}\nvar VQ=${VQ};`, context);
  return { context, trace };
}

const errnoOf = (error) => (error && typeof error.code === 'string' ? error.code : null);

function answer(context, probe, input) {
  switch (probe.kind) {
    case 'vet':
      try {
        const vetted = context.bd(context.UC, input);
        return { result: 'ok', absentAt: vetted.absentAt ?? null, linksResolvedPath: vetted.linksResolvedPath ?? null };
      } catch (error) {
        return { result: 'refused', reason: error.reason ?? null, code: errnoOf(error) };
      }
    case 'folder':
      return attempt(() => ({ ignored: context.n7e(input) }));
    case 'path':
      return attempt(() => ({ ignored: context.nE(input) }));
    case 'name':
      return { connector: context.H5(input), normalized: context.rXe(input, true), names: context.im(input) };
    // The managed settings write check's `_4` driver and `kYe` host answering `Lx` for one path.
    case 'locate':
      return context._4(input);
    default:
      throw new Error(`unknown probe kind ${probe.kind}`);
  }
}

// What the bundle throws (for example when the working directory is gone) is part of the answer.
function attempt(question) {
  try {
    return question();
  } catch (error) {
    return { thrown: { code: errnoOf(error), reason: error.reason ?? null } };
  }
}

// A pure call on node's path module: `flavor` picks `posix` or `win32`, `fn` the method, `args` its arguments.
function pathCall(context, probe) {
  const module = context.pathModule[probe.flavor];
  try {
    const value = module[probe.fn](...probe.args);
    return { value };
  } catch (error) {
    return { error: errnoOf(error) };
  }
}

// Bundle functions a `call` probe may run directly, so each helper's answer is pinned on its own.
const CALLABLE = new Set([
  'Kee', 'IC', 'Bee', 'jee', 'ig', 'sM', 'Wee', 'Gee', 'LC', 'cg', 'Pn', 'Wm', 'nee', 'Or', 'Yn', 'vl', 'go', 'dd',
  'ud', 'Rr', 'Kr', 'Xo', 'Qo', 'es', 'OI', '$I', 'Jo', 'Gb', 'MC', 'sg', 'rM', 'ag', 'oM', 'Wb', 'MQ', 'FQ', 'Bb',
  'XI', 'HQ', 'BI', 'Qee', 'nM', 'tte',
]);

// Arguments spelled `{"host": true}` stand for the host object the bundle passes (`we()`).
function callFunction(context, probe) {
  if (!CALLABLE.has(probe.fn)) throw new Error(`${probe.name}: ${probe.fn} is not callable`);
  const values = probe.args.map((value) => (value && value.host === true ? context.UC : value));
  try {
    const value = context[probe.fn](...values);
    return value === undefined ? { undefined: true } : { value };
  } catch (error) {
    return { thrown: { code: errnoOf(error), reason: error.reason ?? null } };
  }
}

function run(probe) {
  // JSON has no undefined, so an argument left undefined would be recorded as null; probes omit such arguments.
  if ((probe.args ?? []).some((value) => value === undefined)) throw new Error(`${probe.name}: undefined argument`);
  const { context, trace } = sandbox(probe);
  let expect;
  if (probe.kind === 'pathfn') expect = pathCall(context, probe);
  else if (probe.kind === 'call') expect = callFunction(context, probe);
  else expect = probe.paths.map((input) => answer(context, probe, input));
  return { ...probe, expect, calls: trace };
}

const probes = corpus().map(run);
const names = new Set();
for (const probe of probes) {
  if (names.has(probe.name)) throw new Error(`duplicate probe name ${probe.name}`);
  names.add(probe.name);
}
const text = `{"source":"sdk.mjs","nodeReleases":${JSON.stringify(NODE_RELEASES)},"probes":[\n${probes
  .map((probe) => JSON.stringify(probe))
  .join(',\n')}\n]}\n`;
if (args.includes('--check')) {
  if (!fs.existsSync(out) || fs.readFileSync(out, 'utf8') !== text) {
    process.stderr.write(`${out} is stale; regenerate it with this script\n`);
    process.exit(1);
  }
} else {
  fs.mkdirSync(path.dirname(out), { recursive: true });
  fs.writeFileSync(out, text);
}
