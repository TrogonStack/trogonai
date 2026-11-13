// The probes connector_vetting_probes.mjs records. Each names the host it runs on (platform, node release, working
// directory, environment, launch directory, home directory, file system) and the inputs to ask about; the recorder
// adds what the pinned bundle answered and the host reads it made.

import { NODE_RELEASES } from './node_releases.mjs';

// A file system holding `paths` as folders, together with every folder above them up to the root they hang from.
function folders(...paths) {
  const tree = {};
  for (const full of paths) {
    const windows = /^[A-Za-z]:|^\\\\/.test(full);
    const separator = windows ? '\\' : '/';
    const parts = full.split(/[\\/]/);
    if (windows && /^[A-Za-z]:$/.test(parts[0])) {
      tree[`${parts[0]}\\`] = { kind: 'dir' };
      for (let index = 1; index < parts.length; index++) {
        if (parts[index] === '') continue;
        tree[parts.slice(0, index + 1).join(separator)] = { kind: 'dir' };
      }
    } else if (windows) {
      for (let index = 4; index < parts.length; index++) tree[parts.slice(0, index + 1).join(separator)] = { kind: 'dir' };
    } else {
      tree['/'] = { kind: 'dir' };
      for (let index = 1; index < parts.length; index++) {
        if (parts[index] === '') continue;
        tree[parts.slice(0, index + 1).join('/')] = { kind: 'dir' };
      }
    }
  }
  return tree;
}

const link = (target) => ({ kind: 'symlink', target });
const file = (content, extra = {}) => ({ kind: 'file', content, ...extra });

function pathFunctionProbes() {
  const probes = [];
  const posixInputs = [
    '', '.', '..', '/', '//', '///', '/a/b', 'a/b', 'a//b/', '/a/./b/../c', '../../a', 'a/../..', '/..', '/../a',
    '/a/b/c.d.e', '.hidden', '/a/.hidden', 'a.', '/a/b/', '..a', 'a..', '/a/...', 'é/ü/文件', 'a\\b', '//a//b//',
    '/net/host/share', '/dev/fd/3/../x',
  ];
  const win32Inputs = [
    '', '.', '..', '\\', '/', '\\\\', '\\\\\\', '//', 'C:', 'c:', 'C:\\', 'C:/', 'C:a', 'C:a\\..\\..', 'c:/a/../b',
    'C:\\a\\.\\b\\..\\c', 'D:..\\x', '\\x', '/x', 'a\\b', 'a/b/', '\\\\server\\share', '\\\\server\\share\\',
    '\\\\server\\share\\x\\..\\..', '//server/share/x', '\\\\server', '\\\\server\\', '\\\\a\\b\\c', '\\\\?\\C:\\x',
    '\\\\?\\UNC\\srv\\sh\\x', '\\\\.\\COM1', '\\\\.\\C:\\x', '\\\\?\\COM1:', '\\\\?\\COM1:\\x', '//./x', '//?/x',
    '\\\\.\\x\\y', '\\\\?\\x', 'CON', 'CON:', 'con:x', 'CONx', 'COM1:', 'COM1', 'com9:', 'COM\u00b9:', 'LPT3:a',
    'LPT\u00b2:', 'NUL:', 'nul', 'AUX:\\x', 'PRN:/x', 'a/CON:/b', 'x\\..\\..\\y', '/??/C:/x', '\\??\\x', 'a/../??/x',
    '/./??/x', 'C:\\a. \\b ', 'é:x', '\\\\server\\share\\é', '///a//b///', 'C:\\a\\b.c.d', 'C:\\.git', 'C:\\a\\..',
    '\\\\wsl$\\Ubuntu\\home', '\\\\wsl.localhost\\Ubuntu\\x', 'C:\\Users\\me\\connector-downloads\\f',
  ];
  const singleArgument = ['normalize', 'dirname', 'isAbsolute', 'parse'];
  for (const node of NODE_RELEASES) {
    for (const fn of singleArgument) {
      for (const input of posixInputs) {
        probes.push({ name: `pathfn ${node} posix ${fn} ${JSON.stringify(input)}`, kind: 'pathfn', node, platform: 'linux', cwd: '/w', flavor: 'posix', fn, args: [input] });
      }
      for (const input of win32Inputs) {
        probes.push({ name: `pathfn ${node} win32 ${fn} ${JSON.stringify(input)}`, kind: 'pathfn', node, platform: 'win32', cwd: 'C:\\w', flavor: 'win32', fn, args: [input] });
      }
    }
    const posixJoins = [
      [], [''], ['', ''], ['a'], ['/a', 'b'], ['a', '/b'], ['a', '..', '..'], ['/', '..'], ['', 'a'], ['a', ''],
      ['/a/', '/b/'], ['.', '.'], ['/net', 'host', '..', 'x'], ['/w', 'connector-downloads', '.gitignore'],
    ];
    const win32Joins = [
      [], [''], ['', ''], ['a'], ['C:\\a', 'b'], ['C:', 'b'], ['//a', 'q'], ['\\\\a', 'b'], ['\\\\', 'a', 'b'],
      ['//', '/a'], ['\\', '\\\\a\\b'], ['a', 'CON:', 'b'], ['C:\\x', 'COM1:'], ['CON:', 'x'], ['c:', 'con:'],
      ['C:\\', '..', 'x'], ['\\\\srv\\sh', '..', '..'], ['C:\\a. ', 'b'], ['\\\\?\\C:', 'x'], ['//?/', 'x'],
    ];
    for (const args of posixJoins) {
      probes.push({ name: `pathfn ${node} posix join ${JSON.stringify(args)}`, kind: 'pathfn', node, platform: 'linux', cwd: '/w', flavor: 'posix', fn: 'join', args });
    }
    for (const args of win32Joins) {
      probes.push({ name: `pathfn ${node} win32 join ${JSON.stringify(args)}`, kind: 'pathfn', node, platform: 'win32', cwd: 'C:\\w', flavor: 'win32', fn: 'join', args });
    }
    const posixResolves = [
      { cwd: '/w', args: [] },
      { cwd: '/w', args: ['.'] },
      { cwd: '/w', args: [''] },
      { cwd: '/w//x/', args: [] },
      { cwd: '/w/./x/..', args: ['.'] },
      { cwd: '/w//x/', args: [''] },
      { cwd: 'relative', args: [] },
      { cwd: '/w', args: ['a', 'b'] },
      { cwd: '/w', args: ['/x', 'y'] },
      { cwd: '/w', args: ['a', '/b', 'c'] },
      { cwd: '/w', args: ['/a/../..'] },
      { cwd: '/w', args: ['../../..'] },
      { cwd: null, args: ['a'] },
      { cwd: null, args: ['/a'] },
      { cwd: null, args: [] },
      { cwd: '/w', args: ['', '/a', ''] },
      { cwd: '/', args: ['..'] },
    ];
    for (const { cwd, args } of posixResolves) {
      probes.push({ name: `pathfn ${node} posix resolve ${JSON.stringify(cwd)} ${JSON.stringify(args)}`, kind: 'pathfn', node, platform: 'linux', cwd, flavor: 'posix', fn: 'resolve', args });
    }
    const win32Resolves = [
      { cwd: 'C:\\w', args: [] },
      { cwd: 'C:\\w', args: ['.'] },
      { cwd: 'C:\\w', args: [''] },
      { cwd: 'C:/w//x/', args: [] },
      { cwd: 'C:/w//x/', args: ['.'] },
      { cwd: '\\\\srv\\sh\\w', args: [] },
      { cwd: '\\\\srv\\sh\\w', args: ['..\\..\\x'] },
      { cwd: '\\\\srv\\sh\\w', args: ['\\x'] },
      { cwd: '/w', args: [] },
      { cwd: 'C:\\w', args: ['a', 'b'] },
      { cwd: 'C:\\w', args: ['\\x'] },
      { cwd: 'C:\\w', args: ['/x', 'y'] },
      { cwd: 'C:\\w', args: ['D:x'], env: { '=D:': 'D:\\d' } },
      { cwd: 'C:\\w', args: ['d:x'], env: { '=D:': 'D:\\d' } },
      { cwd: 'C:\\w', args: ['D:x'] },
      { cwd: 'C:\\w', args: ['D:x'], env: { '=D:': 'E:\\elsewhere' } },
      { cwd: 'C:\\w', args: ['D:'], env: { '=D:': 'D:\\d\\' } },
      { cwd: 'C:\\w', args: ['c:x'] },
      { cwd: 'C:\\w', args: ['C:\\a', 'D:b'] },
      { cwd: 'C:\\w', args: ['\\\\srv\\sh', 'x'] },
      { cwd: 'C:\\w', args: ['\\\\srv\\sh\\', '..'] },
      { cwd: 'C:\\w', args: ['\\\\?\\C:\\x', 'y'] },
      { cwd: 'C:\\w', args: ['\\\\.\\COM1'] },
      { cwd: 'C:\\w', args: ['\\\\?\\COM1:', 'x'] },
      { cwd: 'C:\\w', args: ['CON'] },
      { cwd: 'C:\\w', args: ['CON:'] },
      { cwd: 'C:\\w', args: ['COM1:x'] },
      { cwd: 'C:\\w', args: ['//./x'] },
      { cwd: 'C:\\w', args: ['//?/x'] },
      { cwd: 'C:\\w', args: ['a', '..', '..', '..'] },
      { cwd: null, args: ['a'] },
      { cwd: null, args: ['C:\\a'] },
      { cwd: null, args: ['\\a'] },
      { cwd: null, args: [] },
      { cwd: 'C:\\w', args: ['C:\\Users\\me\\..\\..\\..\\x'] },
      { cwd: 'C:\\w', args: ['\\\\wsl$\\Ubuntu\\home'] },
    ];
    for (const { cwd, args, env } of win32Resolves) {
      probes.push({ name: `pathfn ${node} win32 resolve ${JSON.stringify(cwd)} ${JSON.stringify(args)} ${JSON.stringify(env ?? {})}`, kind: 'pathfn', node, platform: 'win32', cwd, env, flavor: 'win32', fn: 'resolve', args });
    }
    // win32 calls made on a non-Windows host take node's other branch for the working directory.
    probes.push({ name: `pathfn ${node} win32 resolve on linux`, kind: 'pathfn', node, platform: 'linux', cwd: '/w/x', flavor: 'win32', fn: 'resolve', args: [] });
    probes.push({ name: `pathfn ${node} win32 resolve dot on linux`, kind: 'pathfn', node, platform: 'linux', cwd: '/w/x', flavor: 'win32', fn: 'resolve', args: ['.'] });
    probes.push({ name: `pathfn ${node} posix resolve on win32`, kind: 'pathfn', node, platform: 'win32', cwd: 'C:\\w\\x', flavor: 'posix', fn: 'resolve', args: [] });
    probes.push({ name: `pathfn ${node} posix resolve relative on win32`, kind: 'pathfn', node, platform: 'win32', cwd: 'C:\\w\\x', flavor: 'posix', fn: 'resolve', args: ['a'] });
  }
  return probes;
}

function nameProbes() {
  const inputs = [
    'connector-downloads', 'Connector-Downloads', 'CONNECTOR-DOWNLOADS', 'connector-downloads.', 'connector-downloads ',
    'connector-downloads. . ', 'connector-downloads:stream', 'connector-downloads::$DATA', 'conn\u200cector-downloads',
    'connector-downloads\ufeff', '\u202aconnector-downloads', 'connector-download\u017f', 'connector-downloads\u0131',
    'connector-downloadsx', 'xconnector-downloads', 'connector downloads', '', '.', '...', ':', ' ', '\u200c',
    'connector-downloads:', ':connector-downloads', 'connector-downloads.:x', 'İ', 'connector-downloadS:a:b',
  ];
  const probes = inputs.map((input) => ({ name: `name ${JSON.stringify(input)}`, kind: 'name', platform: 'linux', paths: [input] }));
  const pathsNamingFolder = [
    '/a/connector-downloads/b', '/a/connector-downloads', '/a/connector-downloads/', 'C:\\x\\Connector-Downloads',
    'C:\\x\\Connector-Downloads\\y', '/a/connector-downloadsx/b', 'connector-downloads', '/a/b',
  ];
  probes.push({ name: 'name paths', kind: 'name', platform: 'linux', paths: pathsNamingFolder });
  return probes;
}

// Helper functions run on their own, on every platform where they branch.
function callProbes() {
  const probes = [];
  // Each runs in a working directory of its platform unless the probe names another (null when it is gone).
  const add = (fn, args, extra = {}) => probes.push({ name: `call ${fn} ${extra.platform ?? 'linux'} ${extra.node ?? ''} ${JSON.stringify(args)} ${extra.tag ?? ''}`.replace(/ +/g, ' ').trim(), kind: 'call', platform: 'linux', cwd: extra.platform === 'win32' ? 'C:\\w' : '/w', fn, args, ...extra });
  const samples = [
    '/', '/a/b', '/net', '/net/', '/NET', '/net/host', '/net/host/x', '/Net/Host/x', '/net/../net/h', '/network',
    '/Network/Servers', '/Network/Servers/h', '/network/servers/h/x', '/network/h', '/nEtwork/x/../y',
    '/n\u200cet/host', '/network\ufeff/x', '/.vol/1/2', '/.VOL', '/a/.file/x', '/.nofollow/x', '/.resolve/1/x',
    '/x/../.vol/1', '/.volx/1', '//net/host', '\\\\server\\share\\x', '//server/share', '\\\\?\\C:\\x',
    '\\\\?\\UNC\\srv\\sh\\x', '\\\\.\\C:\\x', '//./x', '\\??\\x', '/??/x', 'a/../??/x', '/./??/x', 'x??', '\\\\wsl$\\U\\x',
    '//wsl.localhost/U/x', '\\\\wsl$\\.\\x', '\\\\wsl$\\..\\x', '\\\\wsl$\\ . \\x', '\\\\WSL$\\u', '\\\\wsl$\\',
    'C:\\a\\..\\b', 'C:\\a\\...\\b', '/a/. /b', '/a/.. /b', '/a/...', 'a. ', '.', '..', '', 'C:x', '\\x',
    '/home/bob', '/System/Volumes/Data/home/bob', '/system/volumes/data/Users/x', '/System/Volumes/Data/usr/local/x',
    '/System/Volumes/Data/usr/x', '/System/Volumes/Data', '/dev/fd/3', '/dev/fd', '\\\\?\\COM1:', 'CON:??/x',
  ];
  for (const platform of ['linux', 'darwin', 'win32']) {
    for (const fn of ['Pn', 'Wm', 'nee', 'Or', 'Yn', 'vl', 'go', 'dd', 'ud', 'Rr', 'Kr', 'Xo', 'Qo', 'es', 'OI', 'Jo', 'Gb', 'Kee', 'IC', 'Bee', 'jee', 'XI']) {
      for (const sample of samples) add(fn, [sample], { platform });
    }
  }
  for (const node of NODE_RELEASES.slice(1)) {
    for (const platform of ['linux', 'win32']) {
      for (const sample of ['CON:??/x', 'a/CON:/??/x', 'COM1:/??/x', '\\\\?\\COM1:\\??\\x', 'CONx']) {
        add('Wm', [sample], { node, platform });
        add('nee', [sample], { node, platform });
      }
    }
  }
  const linkTexts = [
    'target', '../x', 'a/../b', '..', '../../a', '/abs', 'C:\\abs', 'C:rel', '\\rooted', '/rooted', 'a\\..\\b', 'x\uFFFD',
    '\\\\?\\C:\\x', '\\\\?\\UNC\\s\\h', '\\\\?\\x', '//?/c:/x', 'a.\\b', 'a \\b', 'a\\b.', './a', '.\\a', '...', 'a/./b',
    '\\\\srv\\sh', '..\\..', '..a', '..\\x',
  ];
  for (const platform of ['linux', 'win32']) for (const text of linkTexts) add('Wee', [text], { platform });
  for (const [e, t] of [
    ['/a', '/a/b'], ['/a/b', '/a/b'], ['/a/b', '/a'], ['/', '/a'], ['/a', '/ab'], ['C:\\a', 'C:\\a\\b'], ['C:\\a', 'D:\\a\\b'],
    ['c:\\a', 'C:\\a\\b'], ['\\\\s\\h\\a', '\\\\s\\h\\a\\b'], ['/a/../a', '/a/b'], ['a', 'a/b'],
  ]) {
    add('LC', [e, t], { platform: 'linux' });
    add('LC', [e, t], { platform: 'win32' });
  }
  add('LC', ['a', 'a/b'], { cwd: null, tag: 'cwd gone' });
  add('LC', ['a', 'a/b'], { platform: 'win32', cwd: null, tag: 'cwd gone' });
  for (const [e, t] of [['/a', '/a'], ['/a/b', '/a'], ['/ab', '/a'], ['/a/', '/a//'], ['/', '/'], ['/a', '/'], ['/dev/fd/3', '/dev/fd']]) add('MC', [e, t]);
  for (const [e, t] of [
    ['\\\\srv\\sh\\x', '\\\\SRV\\other'], ['\\\\srv\\sh\\x', '\\\\other\\sh'], ['//srv/sh', '\\\\?\\UNC\\srv\\x'],
    ['\\\\?\\C:\\x', '\\\\srv'], ['\\\\srv\\..\\x', '\\\\srv'], ['/a', '\\\\srv'], ['\\??\\x', '\\\\srv'],
  ]) add('$I', [e, t], { platform: 'win32' });
  for (const [e, t] of [
    ['/a/b', []], ['/a/b', ['c']], ['/a/...', ['c']], ['/a/...', []], ['/a', ['.', 'x']], ['/a', ['..', 'x']],
    ['/net/../net/h/./x', []], ['/network/../tmp', ['x']], ['\\\\s\\h\\.\\x', []],
  ]) {
    add('ig', [e, t]);
    add('ig', [e, t], { platform: 'win32' });
  }
  for (const [e, anchors] of [
    ['/net/h/x', ['/network/h/x']], ['/net/h/x', null], ['/Network/Servers/h/x', null], ['/net/h/./x', null], ['/a', null],
  ]) {
    // Anchors the bundle leaves undefined are omitted, since JSON would record them as null.
    add('sM', anchors ? [e, anchors] : [e]);
    add('sM', anchors ? [e, anchors] : [e], { platform: 'darwin', files: {}, stat: { '/home': { dev: 1 }, '/Users': { dev: 1 } } });
  }
  // The macOS automount checks: what /etc/auto_master maps, whether /home is its own mount, and the home directory.
  const automount = [
    { tag: 'master maps home', files: { '/etc/auto_master': '# comment\n/home auto_home -nobrowse\n' } },
    { tag: 'master maps home after comment', files: { '/etc/auto_master': '/net -hosts # /home x\n  /HOME\tauto_home\n' } },
    { tag: 'master home alone', files: { '/etc/auto_master': '/home\n' } },
    { tag: 'master without home same dev', files: { '/etc/auto_master': '/net -hosts\n' }, stat: { '/home': { dev: 5 }, '/Users': { dev: 5 } } },
    { tag: 'master without home other dev', files: { '/etc/auto_master': '/net -hosts\n' }, stat: { '/home': { dev: 6 }, '/Users': { dev: 5 } } },
    { tag: 'no master no home', stat: {} },
    { tag: 'no master home same dev', stat: { '/home': { dev: 2 }, '/Users': { dev: 2 } } },
    { tag: 'master unreadable', files: { '/etc/auto_master': { errno: 'EACCES' } }, stat: { '/home': { dev: 2 }, '/Users': { dev: 2 } } },
    { tag: 'home unreadable', stat: { '/home': 'EACCES', '/Users': { dev: 2 } } },
    { tag: 'users missing', stat: { '/home': { dev: 2 } } },
  ];
  for (const host of automount) {
    const extra = { platform: 'darwin', homedir: '/Users/me', ...host };
    add('BI', [], extra);
    for (const [e, t] of [
      ['/network/a/b/c', '/network/a/b/c'], ['/network/a', '/network/a/b/c'], ['/network/a/b/c', '/network/a/x/c'],
      ['/net/h/../../home/bob', '/net/h/s/l'], ['/net/h/../../System/Volumes/Data/home/me', '/net/h/s'],
      ['/network/a/b/../../../home/bob', '/network/a/b'], ['/home/bob', '/network/a/b/c'], ['/net/h', '/net/h/s'],
    ]) {
      add('MQ', [e, t], extra);
      add('Wb', [e, t], extra);
    }
    for (const e of ['/net/h/../../home/bob', '/net/h/../../home/me', '/network', '/a/b', '/x/../home/bob/../../home/al', '/.vol/1']) {
      add('FQ', [e], extra);
    }
  }
  for (const homedir of ['/home/bob', '/Users/bob', '/System/Volumes/Data/home/bob', '/home', null]) {
    add('HQ', [], { platform: 'darwin', homedir, tag: `home ${homedir}` });
    add('FQ', ['/net/h/../../home/bob'], { platform: 'darwin', homedir, files: { '/etc/auto_master': '/home auto_home\n' }, tag: `home ${homedir}` });
  }
  for (const [e, ancestry] of [
    ['/net/host/x', ['/network/a/b']], ['/network/a', ['/network/a/b']], ['/network/a/b/c', ['/network/a/b']],
    ['/net/host', ['/net/host/x']], ['/a', ['/network/a']],
  ]) {
    for (const fn of ['sg', 'rM', 'ag', 'oM']) add(fn, [e, ancestry], { platform: 'darwin', homedir: '/Users/me', files: { '/etc/auto_master': '/home auto_home\n' } });
    for (const fn of ['sg', 'rM', 'ag']) add(fn, [e, ancestry], { platform: 'linux' });
  }
  add('ag', ['/net/h']);
  add('oM', ['\\\\s\\h'], { platform: 'win32' });
  // The ancestry the launch directory shares with the working directory.
  for (const [launch, cwd, env, platform] of [
    ['/home/u/proj', '/home/u/proj/sub', {}, 'linux'],
    ['/home/u/proj', '/home/u/other', {}, 'linux'],
    ['/home/u/proj', '/srv', {}, 'linux'],
    ['/home/u/proj/', '/home/u/proj//', {}, 'linux'],
    ['/home/u/proj', '/home/u/proj', { CLAUDE_CODE_SESSION_KIND: 'bg' }, 'linux'],
    ['/home/u/proj', '/home/u/proj', { CLAUDE_CODE_SESSION_KIND: ' bg\n' }, 'linux'],
    ['/home/u/proj', '/home/u/proj', { CLAUDE_CODE_SESSION_KIND: 'BG' }, 'linux'],
    ['/home/u/proj', '/home/u/proj', { CLAUDE_CODE_SESSION_KIND: 'fg' }, 'linux'],
    [null, '/home/u/proj', {}, 'linux'],
    ['relative/dir', '/w', {}, 'linux'],
    ['/home/u/proj', null, {}, 'linux'],
    ['C:\\Users\\me\\proj', 'C:\\Users\\me\\proj\\sub', {}, 'win32'],
    ['C:\\Users\\me\\proj', 'c:\\users\\me', {}, 'win32'],
    ['C:\\Users\\me\\proj', 'D:\\Users\\me', {}, 'win32'],
    ['\\\\srv\\sh\\a\\b', '\\\\srv\\sh\\a\\c', {}, 'win32'],
    ['C:\\Users\\me', 'C:\\Users\\me', { claude_code_session_kind: 'bg' }, 'win32'],
  ]) {
    add('cg', [{ host: true }], { platform, launch, cwd, env, tag: `launch ${launch} cwd ${cwd} ${JSON.stringify(env)}` });
  }
  for (const [error, links, dotted, trailing] of [
    [{ errno: 'ENAMETOOLONG' }, false, false, false], [{ errno: 'ENAMETOOLONG' }, true, false, false],
    [{ errno: 'ENAMETOOLONG' }, false, true, false], [{ errno: 'ENAMETOOLONG' }, false, false, true],
    [{ errno: 'ENOENT' }, true, true, true], [{ errno: 'ENOTDIR' }, false, false, false], [{ errno: 'EACCES' }, false, false, false],
    [{ errno: null }, false, false, false],
  ]) {
    add('nM', [error, { unreadableAncestry: 'unverified' }, links, dotted, trailing]);
  }
  add('nM', [{ errno: 'EACCES' }, {}, false, false, false]);
  // Branches the samples above leave alone: an empty UNC server, a `wsl` server that is not a share, a share landing
  // that stays on its anchor or moves to another share, and automount checks off macOS, on relative paths, on a dot
  // namespace, and deeper than the automounter looks.
  add('OI', ['\\\\?\\UNC\\']);
  add('Or', ['\\\\wslx\\a']);
  for (const [e, anchors] of [['/net/h/x', ['/net/h/y']], ['/net/h/../h2/x', null], ['/net/h/../../a', null]]) {
    add('sM', anchors ? [e, anchors] : [e]);
  }
  const darwinHome = { platform: 'darwin', homedir: '/Users/me', files: { '/etc/auto_master': '/home auto_home\n' } };
  add('Wb', ['/.vol/1', '/network/a/b/c'], darwinHome);
  add('MQ', ['/.vol/1', '/network/a/b/c'], darwinHome);
  add('MQ', ['/.vol/1', 'network/a/b'], darwinHome);
  add('FQ', ['/network/a']);
  add('FQ', ['network/a'], darwinHome);
  add('FQ', ['/a/b/c/d/e/f/g/h'], darwinHome);
  add('FQ', ['/a/b/c/d/e/home/x'], darwinHome);
  return probes;
}

// The path vetting walk itself (`bd`), on each platform.
function vetProbes() {
  const probes = [];
  const add = (name, probe) => probes.push({ name: `vet ${name}`, kind: 'vet', platform: 'linux', cwd: '/w', ...probe });
  const tree = folders('/w/a/b', '/w/c', '/srv/x');

  add('plain paths', { fs: { ...tree, '/w/f': file('x') }, paths: ['/w/a/b', '/w/a/b/missing/deeper', '/w/f/under', 'a/b', '../srv/x', '/', '/w/a/./b/../b', 'rel/missing'] });
  add('relative with dots', { fs: tree, paths: ['a/./b', './a', '.', '', '..', 'a/b/..'] });
  add('working directory gone', { cwd: null, fs: tree, paths: ['a', '/w/a'] });
  add('nfc on darwin', { platform: 'darwin', fs: { ...folders('/w/cafe\u0301'), '/w/l': link('/w/cafe\u0301') }, paths: ['/w/cafe\u0301', '/w/l', '/w/l/x'] });
  add('nfc not on linux', { fs: { ...folders('/w/cafe\u0301'), '/w/l': link('/w/cafe\u0301') }, paths: ['/w/l'] });

  // Links: relative, absolute, chained, looping, too long, and with text that cannot be trusted.
  const links = {
    ...tree,
    '/w/rel': link('a/b'),
    '/w/up': link('../srv/x'),
    '/w/abs': link('/srv/x'),
    '/w/dangling': link('/w/nothing/here'),
    '/w/self': link('self'),
    '/w/loop1': link('loop2'),
    '/w/loop2': link('loop1'),
    '/w/dots': link('./a/../a/./b'),
    '/w/undecodable': link('a\uFFFDb'),
    '/w/tofile': link('/w/f'),
    '/w/f': file('x'),
    '/w/net': link('/net/host/share'),
    '/w/netdots': link('/net/../net/host/x'),
    '/w/network': link('/network/../tmp'),
    '/w/networkdeep': link('/network/srv/share/x'),
    '/w/vol': link('/.vol/1/2'),
    '/w/netroot': link('/net'),
    '/w/netrel': link('../net'),
    '/w/dotty': link('a/.../b'),
    '/w/spaced': link('a/. /b'),
    '/w/twoslash': link('//server/share/x'),
    '/w/tilde': link('~/x'),
    '/w/readlinkfails': link('a'),
    '/w/a/inner': link('../c'),
    '/w/hop1': link('hop2/x'),
    '/w/hop2': link('/w/a'),
    '/w/wsldot': link('//wsl$/U/./x'),
  };
  add('links', {
    fs: links,
    readlink: { '/w/readlinkfails': 'EIO' },
    paths: [
      '/w/rel', '/w/rel/x', '/w/up', '/w/abs/y', '/w/dangling', '/w/self', '/w/self/x', '/w/loop1', '/w/dots', '/w/undecodable',
      '/w/tofile/x', '/w/net', '/w/net/deeper', '/w/netdots', '/w/network', '/w/networkdeep/y', '/w/vol', '/w/netroot',
      '/w/netroot/x', '/w/netrel', '/w/dotty', '/w/spaced', '/w/twoslash', '/w/twoslash/y', '/w/tilde', '/w/readlinkfails',
      '/w/a/inner/x', '/w/hop1', '/w/netrel/host/x', '/w/wsldot/y',
    ],
  });
  add('links on darwin', { platform: 'darwin', homedir: '/Users/me', files: { '/etc/auto_master': '/home auto_home\n' }, fs: links, paths: ['/w/net', '/w/netdots', '/w/vol', '/w/networkdeep/y', '/w/netroot'] });
  const chain = { ...tree };
  for (let index = 0; index < 70; index++) chain[`/w/chain${index}`] = link(`chain${index + 1}`);
  chain['/w/chain70'] = { kind: 'dir' };
  add('long link chain', { fs: chain, paths: ['/w/chain0', '/w/chain6', '/w/chain7'] });
  add('long link chain on win32', { platform: 'win32', cwd: 'C:\\w', fs: Object.fromEntries(Object.entries(chain).map(([key, value]) => [`C:${key.replace(/\//g, '\\')}`, value])), paths: ['C:\\w\\chain0', 'C:\\w\\chain7'] });

  // An unreadable component: posix retries with a no-follow directory open; Windows has no such probe.
  const locked = { ...folders('/w/locked/in', '/home/u/proj/sub'), '/w/locked': { kind: 'dir', errno: 'EACCES' }, '/home': { kind: 'dir', errno: 'EACCES' } };
  add('unreadable component opens as a folder', { fs: locked, opendir: { '/w/locked': 'ok' }, paths: ['/w/locked/in', '/w/locked/none'] });
  add('unreadable component does not open', { fs: locked, opendir: { '/w/locked': 'EACCES' }, paths: ['/w/locked/in'] });
  for (const errno of ['EPERM', 'EIO', 'ELOOP', 'EBUSY', 'ENAMETOOLONG', 'ENOTDIR', 'ENOENT', 'EINVAL']) {
    add(`lstat ${errno}`, { fs: { ...tree, '/w/odd': { kind: 'dir', errno } }, paths: ['/w/odd', '/w/odd/x', '/w/odd/x/y'] });
    add(`lstat ${errno} opendir ok`, { fs: { ...tree, '/w/odd': { kind: 'dir', errno } }, opendir: { '/w/odd': 'ok' }, paths: ['/w/odd/x'] });
    add(`lstat ${errno} on win32`, { platform: 'win32', cwd: 'C:\\w', fs: { 'C:\\': { kind: 'dir' }, 'C:\\w': { kind: 'dir' }, 'C:\\w\\odd': { kind: 'dir', errno } }, paths: ['C:\\w\\odd\\x'] });
  }
  add('opendir after link', { fs: { ...locked, '/w/l': link('/w/locked') }, opendir: { '/w/locked': 'ok' }, paths: ['/w/l/in'] });
  // The no-follow open of an unreadable link fails with ELOOP after lstat already failed.
  add('opendir is a link', { fs: { ...tree, '/w/odd': { kind: 'symlink', errno: 'EACCES', target: 'a' } }, opendir: { '/w/odd': 'ELOOP' }, paths: ['/w/odd/x'] });

  // The launch directory's ancestry may be unreadable without failing the check.
  const launch = { launch: '/home/u/proj', cwd: '/home/u/proj/sub', fs: locked };
  add('launch ancestry unreadable', { ...launch, opendir: { '/home': 'EACCES' }, paths: ['/home/u/proj/sub/x', 'x', '/home/u/other', '/home/u/proj/sub/../../u/./proj'] });
  add('launch ancestry eperm', { ...launch, fs: { ...locked, '/home': { kind: 'dir', errno: 'EPERM' } }, opendir: { '/home': 'EPERM' }, paths: ['/home/u/proj/sub/x'] });
  add('launch ancestry dotted', { ...launch, opendir: { '/home': 'EACCES' }, paths: ['/home/./u/proj', '/home/u/../u/proj', '/home/.../x', '/home/. /x'] });
  add('launch ancestry in background session', { ...launch, env: { CLAUDE_CODE_SESSION_KIND: 'bg' }, opendir: { '/home': 'EACCES' }, paths: ['/home/u/proj/sub/x'] });
  add('launch ancestry unknown', { ...launch, launch: null, opendir: { '/home': 'EACCES' }, paths: ['/home/u/proj/sub/x'] });
  add('launch ancestry after opendir', { ...launch, opendir: { '/home': 'ok' }, fs: { ...locked, '/home/u': { kind: 'dir', errno: 'EACCES' } }, paths: ['/home/u/proj/sub/x'] });
  add('launch ancestry two levels', { ...launch, fs: { ...locked, '/home/u': { kind: 'dir', errno: 'EACCES' } }, opendir: { '/home': 'EACCES', '/home/u': 'EACCES' }, paths: ['/home/u/proj/x', '/home/u/proj', '/home/u'] });
  add('launch ancestry on win32', {
    platform: 'win32', launch: 'C:\\Users\\me\\proj', cwd: 'C:\\Users\\me\\proj\\sub',
    fs: { 'C:\\': { kind: 'dir' }, 'C:\\Users': { kind: 'dir', errno: 'EPERM' }, 'C:\\Users\\me': { kind: 'dir', errno: 'EACCES' }, 'C:\\Users\\me\\proj': { kind: 'dir' }, 'C:\\Users\\me\\proj\\sub': { kind: 'dir' } },
    paths: ['C:\\Users\\me\\proj\\sub\\x', 'c:\\users\\me\\proj\\x', 'C:\\Users\\other\\x', 'D:\\Users\\me\\proj\\x'],
  });

  // Paths spelled as network locations.
  const network = { ...folders('/net/host/share/dir', '/network/srv/share/dir', '/Network/Servers/h/s'), '/net/host/share/l': link('/net/host/../../home/bob'), '/net/host/share/m': link('/net/other/x'), '/net/host/share/n': link('/net/host/share/dir'), '/net/host/share/nr': link('/net'), '/network/srv/share/up': link('/network/srv') };
  for (const platform of ['linux', 'darwin']) {
    add(`network paths on ${platform}`, {
      platform, homedir: '/Users/me', files: { '/etc/auto_master': '/home auto_home\n' }, fs: network,
      opendir: { '/net/host/share/locked': 'EACCES' },
      paths: [
        '/net/host/share/dir/x', '/net/host/share/l', '/net/host/share/m', '/net/host/share/n/y', '/network/srv/share/dir/x',
        '/network/srv', '/network', '/net', '/net/host', '/Network/Servers/h/s/x', '/NET/HOST/share/dir', '/w/../net/host/share',
        '/network/srv/share/../../other', '/.vol/1/2', '/a/.nofollow/x', '/.resolve/1/2', '/net/host/share/nr/host/share',
        '/network/srv/share/up/share',
      ],
    });
  }
  add('automount without home map', { platform: 'darwin', homedir: '/Users/me', files: { '/etc/auto_master': '/net -hosts\n' }, stat: { '/home': { dev: 3 }, '/Users': { dev: 3 } }, fs: network, paths: ['/net/host/share/l', '/net/host/share/m', '/net/host/share/l'] });
  add('automount unreadable master', { platform: 'darwin', homedir: '/Users/me', files: { '/etc/auto_master': { errno: 'EACCES' } }, fs: network, paths: ['/net/host/share/l', '/net/host/share/l'] });
  add('automount own home', { platform: 'darwin', homedir: '/home/bob', files: { '/etc/auto_master': '/home auto_home\n' }, fs: network, paths: ['/net/host/share/l'] });

  // macOS reads /dev/fd entries as already open descriptors.
  const devfd = { ...folders('/dev/fd/3/x', '/w/x'), '/dev/fd/4': link('/w'), '/w/fd': link('/dev/fd/3/./../x') };
  for (const platform of ['darwin', 'linux']) {
    add(`dev fd on ${platform}`, { platform, fs: devfd, paths: ['/dev/fd/3/../x', '/dev/fd/3/./../x', '/dev/fd/3/x', '/dev/fd', '/dev/fd/4/x', '/dev/fdx/a', '/dev/fd/3/./.', '/dev/fd/3/.././../w', '/dev/./fd/3/../x', '/dev/fd/3/../.', '/w/fd'] });
  }

  // Windows spellings.
  const windows = {
    'C:\\': { kind: 'dir' },
    'C:\\w': { kind: 'dir' },
    'C:\\w\\a': { kind: 'dir' },
    'C:\\w\\a\\b': { kind: 'dir' },
    'C:\\w\\l': link('C:\\w\\a'),
    'C:\\w\\rel': link('a\\b'),
    'C:\\w\\up': link('..\\w\\a'),
    'C:\\w\\dotname': link('a.\\b'),
    'C:\\w\\rooted': link('\\w\\a'),
    'C:\\w\\driverel': link('C:a'),
    'C:\\w\\unc': link('\\\\srv\\sh\\x'),
    'C:\\w\\verbatim': link('\\\\?\\C:\\w\\a'),
    'C:\\w\\verbatimunc': link('\\\\?\\UNC\\srv\\sh'),
    'C:\\w\\device': link('\\\\?\\Volume{x}\\a'),
    'C:\\w\\undecodable': link('a\uFFFD'),
    'C:\\w\\wsl': link('\\\\wsl$\\Ubuntu\\home'),
    'C:\\w\\other': link('D:\\x'),
    'D:\\': { kind: 'dir' },
    'D:\\d': { kind: 'dir' },
    'D:\\d\\x': { kind: 'dir' },
    '\\\\srv\\sh\\x': { kind: 'dir' },
    '\\\\srv\\sh\\x\\l': link('\\\\srv\\sh\\x'),
    '\\\\srv\\sh\\x\\far': link('\\\\other\\sh'),
    '\\\\srv\\sh\\x\\server': link('\\\\srv'),
    '\\\\wsl$\\Ubuntu\\home': { kind: 'dir' },
    '\\\\wsl$\\Ubuntu\\home\\me': { kind: 'dir' },
    '\\\\wsl.localhost\\Ubuntu\\x': { kind: 'dir' },
  };
  for (const node of NODE_RELEASES) {
    add(`windows paths ${node}`, {
      platform: 'win32', node, cwd: 'C:\\w', env: { '=D:': 'D:\\d' }, fs: windows,
      paths: [
        'C:\\w\\a\\b', 'c:/w/a/b', 'C:\\w\\a\\missing\\x', 'a\\b', 'D:x', 'd:x\\y', '\\w\\a', 'C:\\w\\l\\b', 'C:\\w\\rel',
        'C:\\w\\up', 'C:\\w\\dotname', 'C:\\w\\rooted', 'C:\\w\\driverel', 'C:\\w\\unc', 'C:\\w\\verbatim', 'C:\\w\\verbatimunc',
        'C:\\w\\device', 'C:\\w\\undecodable', 'C:\\w\\wsl', 'C:\\w\\wsl\\me', 'C:\\w\\other', '\\\\srv\\sh\\x\\y',
        '\\\\SRV\\sh\\x\\l', '\\\\srv\\sh\\x\\far', '\\\\srv\\sh\\x\\server', '//srv/sh/x', '\\\\?\\C:\\w\\a', '\\\\?\\UNC\\srv\\sh\\x', '\\\\.\\C:\\w',
        '\\??\\C:\\w', '//./C:/w', '\\\\?\\Volume{x}\\a', '\\\\wsl$\\Ubuntu\\home\\me', '\\\\wsl.localhost\\Ubuntu\\x\\y',
        '\\\\wsl$\\.\\x', '\\\\wsl$\\', 'C:\\w\\a. \\b', 'C:\\w\\a\\..\\..\\..', 'C:\\w\\CON', 'C:\\w\\CON:x', 'CON:', 'COM1:\\x',
        'C:\\w\\??\\x', 'C:\\w\\a\\...\\b', '\\\\srv\\sh', '\\\\srv',
      ],
    });
  }
  add('wsl and unc on linux', { fs: { ...tree, '/wsl$/U/x': { kind: 'dir' } }, paths: ['//wsl$/U/x', '\\\\wsl$\\U\\x', '//srv/sh/x', '/??/x', '/a/../??/x', '/w/??/x'] });
  // Relative links at the root that climb into `/network` and onto `/net` itself.
  add('links into network roots', { fs: { '/': { kind: 'dir' }, '/l': link('network'), '/m': link('net'), '/net': { kind: 'dir' } }, paths: ['/l/srv', '/m'] });

  // Names too long to examine.
  const longName = 'n'.repeat(300);
  add('name too long', {
    fs: { ...tree, [`/w/${longName}`]: { kind: 'dir', errno: 'ENAMETOOLONG' }, [`/w/${longName}.`]: { kind: 'dir', errno: 'ENAMETOOLONG' }, [`/w/${longName} `]: { kind: 'dir', errno: 'ENAMETOOLONG' }, '/w/tolong': link(longName), [`/w/${longName}/...`]: { kind: 'dir' } },
    paths: [`/w/${longName}`, `/w/${longName}/x`, `/w/${longName}.`, `/w/${longName} `, '/w/tolong', `/w/${longName}/.../x`, `/w/${longName}/x/. /y`],
  });
  add('name too long on own host', { fs: { ...network, [`/net/host/${longName}`]: { kind: 'dir', errno: 'ENAMETOOLONG' } }, paths: [`/net/host/${longName}`] });
  add('own host errors are tolerated', { fs: { ...network, '/net/host/share/bad': { kind: 'dir', errno: 'EACCES' } }, opendir: { '/net/host/share/bad': 'EACCES' }, paths: ['/net/host/share/bad/x'] });
  return probes;
}

// Whether a folder named connector-downloads is marked ignored, directly (`n7e`) and from any path (`nE`).
function connectorProbes() {
  const probes = [];
  const add = (kind, name, probe) => probes.push({ name: `${kind} ${name}`, kind, platform: 'linux', cwd: '/w', ...probe });
  const marked = (content, extra = {}) => ({ ...folders('/w/connector-downloads/sub'), '/w/connector-downloads/.gitignore': file(content, extra) });
  for (const [label, content] of [['star', '*'], ['star newline', '*\n'], ['star crlf', '*\r\n'], ['star space', '* '], ['empty', ''], ['two stars', '**'], ['newline star', '\n*'], ['bom star', '\ufeff*'], ['star cr', '*\r']]) {
    add('folder', `gitignore ${label}`, { fs: marked(content), paths: ['/w/connector-downloads/.gitignore'] });
  }
  add('folder', 'gitignore reported larger', { fs: marked('*', { size: 4 }), paths: ['/w/connector-downloads/.gitignore'] });
  add('folder', 'gitignore unreadable', { fs: marked('*', { openErrno: 'EACCES' }), paths: ['/w/connector-downloads/.gitignore'] });
  add('folder', 'gitignore is a link', { fs: { ...folders('/w/connector-downloads'), '/w/connector-downloads/.gitignore': link('/w/star'), '/w/star': file('*') }, paths: ['/w/connector-downloads/.gitignore'] });
  add('folder', 'gitignore is a folder', { fs: folders('/w/connector-downloads/.gitignore'), paths: ['/w/connector-downloads/.gitignore'] });
  add('folder', 'gitignore missing', { fs: folders('/w/connector-downloads'), paths: ['/w/connector-downloads/.gitignore'] });
  add('folder', 'folder missing', { fs: folders('/w'), paths: ['/w/connector-downloads/.gitignore'] });
  add('folder', 'folder unlistable', { fs: marked('*'), readdir: { '/w/connector-downloads': 'EACCES' }, paths: ['/w/connector-downloads/.gitignore'] });
  for (const name of ['download-0123abcd', 'download-0123abcd-2', 'download-0123abcd.pdf', 'download-0123abcd-17.tar', '.download-0123abcd.part', '.download-0123abcd.pdf.part', '.download-0123abcd-3.from.json', 'download-0123ABCD', 'download-0123abc', 'download-0123abcd.a.b', 'xdownload-0123abcd', '.download-0123abcd', 'download-0123abcd.']) {
    add('folder', `temporary ${name}`, { fs: folders('/w/connector-downloads'), readdir: { '/w/connector-downloads': ['notes.txt', name] }, paths: ['/w/connector-downloads/.gitignore'] });
  }
  add('folder', 'unvettable path', { fs: { ...folders('/w'), '/w/connector-downloads': link('/net/host/x') }, paths: ['/w/connector-downloads/.gitignore'] });
  // The fake file system does not follow links on its own, so the listing and the marker under the link are spelled out.
  add('folder', 'through a link', { fs: { ...marked('*'), '/w/l': link('connector-downloads'), '/w/l/.gitignore': file('*') }, readdir: { '/w/l': ['.gitignore'] }, paths: ['/w/l/.gitignore'] });

  add('path', 'marked folders', {
    fs: { ...marked('*'), ...folders('/w/plain/connector-downloads/x', '/w/Connector-Downloads.'), '/w/Connector-Downloads./.gitignore': file('*\n') },
    paths: [
      '/w/connector-downloads/sub/file', '/w/connector-downloads', '/w/connector-downloads/sub/file', 'connector-downloads/x',
      '/w/plain/connector-downloads/x', '/w/Connector-Downloads./y', '/w/a/../connector-downloads/x', '/w/plain/x',
      '/mnt/user-data/connector-downloads/x', '/mnt/user-data/uploads/connector-downloads/x', '/w/connector-downloads:x/y',
      '/w//connector-downloads//', '../w/connector-downloads/q',
    ],
  });
  add('path', 'nested connector folders', { fs: { ...folders('/w/connector-downloads/connector-downloads/x'), '/w/connector-downloads/connector-downloads/.gitignore': file('*') }, paths: ['/w/connector-downloads/connector-downloads/x'] });
  add('path', 'working directory gone', { cwd: null, fs: marked('*'), paths: ['connector-downloads/x', '/w/connector-downloads/x'] });
  add('path', 'on win32', {
    platform: 'win32', cwd: 'C:\\w', env: { '=D:': 'D:\\d' },
    fs: { 'C:\\': { kind: 'dir' }, 'C:\\w': { kind: 'dir' }, 'C:\\w\\connector-downloads': { kind: 'dir' }, 'C:\\w\\connector-downloads\\.gitignore': file('*') },
    readdir: { 'C:/w/connector-downloads': ['.gitignore'], '//srv/sh/connector-downloads': ['download-0123abcd'] },
    paths: ['C:\\w\\connector-downloads\\x', 'c:/w/Connector-Downloads/x', 'connector-downloads\\x', 'D:connector-downloads\\x', '\\\\srv\\sh\\connector-downloads\\x', 'C:\\w\\connector-downloads::$DATA\\x'],
  });
  add('path', 'on darwin', { platform: 'darwin', fs: marked('*'), paths: ['/w/connector-downloads/x', '/W/Connector-Downloads/x'] });
  const many = {};
  const manyPaths = [];
  for (let index = 0; index < 66; index++) {
    Object.assign(many, folders(`/w/c${index}/connector-downloads`));
    manyPaths.push(`/w/c${index}/connector-downloads/x`);
  }
  manyPaths.push('/w/c0/connector-downloads/x', '/w/c65/connector-downloads/x');
  add('path', 'cache fills', { fs: many, paths: manyPaths });
  return probes;
}

export function corpus() {
  return [...pathFunctionProbes(), ...nameProbes(), ...callProbes(), ...vetProbes(), ...connectorProbes()];
}
