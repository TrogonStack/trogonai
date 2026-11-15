#!/usr/bin/env node
// Records `platform.json`: what the upstream bridge's path checks answer for each platform, with the host replaced by
// scripted answers. Each scenario is one line of probe inputs (the platform, what each path is, where links lead,
// the file system type of directories, the home directory) plus the output of the real functions, which a patched
// copy of the bundle exports without changing them.
//
// The process platform is overridden before the bundle loads, and `fs`/`os` calls are replaced by the scripted
// answers, so a recording made on one machine covers the others.
//
// Usage: node bridge_create_session_platform_goldens.mjs BRIDGE_MJS FIXTURES_DIR [--check]
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { syncBuiltinESMExports } from 'node:module';
import win32Path from 'node:path/win32';

const self = fileURLToPath(import.meta.url);
if (process.argv[2] === '--child') {
  await child(JSON.parse(process.argv[3]));
  process.exit(0);
}
const [bridgePath, fixturesDir, flag] = process.argv.slice(2);
if (!bridgePath || !fixturesDir) {
  process.stderr.write('usage: node bridge_create_session_platform_goldens.mjs BRIDGE_MJS FIXTURES_DIR [--check]\n');
  process.exit(2);
}
const check = flag === '--check';

const OLD_SESSION = 'function jo(){if(mw()?.originalCwd===void 0&&ft().project===null)return null;return kl()}';
const OLD_BLOCKED = 'function Kp(){return Ry.of(_e().host).blocked()}';
let source = fs.readFileSync(bridgePath, 'utf8');
for (const [old, replacement] of [
  [OLD_SESSION, 'function jo(){return process.env.PROBE_SESSION_CWD??null}'],
  [OLD_BLOCKED, 'function Kp(){return false}'],
]) {
  if (source.split(old).length !== 2) throw new Error(`bundle accessor not found exactly once: ${old}`);
  source = source.replace(old, () => replacement);
}
source += '\nexport{Wc as __walk,$o as __refuses,kO as __mac,Ys as __step,Bo as __host,hy as __autofs,OIe as __root,$p as __none,wy as __reach,Og as __mountCache,Sl as __share,Lg as __server,Ug as __device,jt as __dots,Cl as __nt,fn as __two,kU as __rooted,gIe as __rootRe,mIe as __splitRe,hIe as __endRe};\n';
const patchedDir = fs.mkdtempSync(path.join(os.tmpdir(), 'bridge-platform-bundle-'));
const patchedPath = path.join(patchedDir, 'bridge.mjs');
fs.writeFileSync(patchedPath, source);

const hookPath = path.join(path.dirname(self), 'bridge_create_session_win32_hook.mjs');
const scenarios = [];
const add = (name, platform, fn, args, probe = {}) => scenarios.push({ name, platform, fn, args, probe });
const BASE = '/r/repo/.git';
const links = (map) => ({ links: map });
const kinds = (map) => ({ kinds: map });

// Walking a path: kinds, links, shares, special mounts.
for (const platform of ['linux', 'darwin']) {
  const p = `${platform}_walk`;
  add(`${p}_plain`, platform, 'walk', [`${BASE}/HEAD`, BASE]);
  add(`${p}_relative_path`, platform, 'walk', ['refs/heads/main', BASE]);
  add(`${p}_slash_runs`, platform, 'walk', [`${BASE}//refs///heads/./x`, BASE]);
  add(`${p}_dot_dot`, platform, 'walk', [`${BASE}/refs/../HEAD`, BASE]);
  add(`${p}_absent_tail`, platform, 'walk', [`${BASE}/refs/x`, BASE], kinds({ [`${BASE}/refs`]: 'absent' }));
  add(`${p}_file_midway`, platform, 'walk', [`${BASE}/HEAD/x`, BASE], kinds({ [`${BASE}/HEAD`]: 'file' }));
  add(`${p}_special`, platform, 'walk', [`${BASE}/HEAD`, BASE], kinds({ [`${BASE}/HEAD`]: 'special' }));
  add(`${p}_lookup_fails`, platform, 'walk', [`${BASE}/HEAD`, BASE], kinds({ [`${BASE}/HEAD`]: 'other' }));
  add(`${p}_link_inside`, platform, 'walk', [`${BASE}/HEAD`, BASE], { ...kinds({ [`${BASE}/HEAD`]: 'symlink' }), ...links({ [`${BASE}/HEAD`]: 'refs/heads/x' }) });
  add(`${p}_link_absolute_with_tail`, platform, 'walk', [`${BASE}/refs/heads/x`, BASE], { ...kinds({ [`${BASE}/refs`]: 'symlink' }), ...links({ [`${BASE}/refs`]: '/r/elsewhere' }) });
  add(`${p}_link_parent`, platform, 'walk', [`${BASE}/refs/heads/x`, BASE], { ...kinds({ [`${BASE}/refs`]: 'symlink' }), ...links({ [`${BASE}/refs`]: '../shared/' }) });
  add(`${p}_link_unreadable`, platform, 'walk', [`${BASE}/HEAD`, BASE], { ...kinds({ [`${BASE}/HEAD`]: 'symlink' }), ...links({ [`${BASE}/HEAD`]: 'a\u0000b' }) });
  add(`${p}_link_missing_text`, platform, 'walk', [`${BASE}/HEAD`, BASE], kinds({ [`${BASE}/HEAD`]: 'symlink' }));
  add(`${p}_link_loop`, platform, 'walk', [`${BASE}/a`, BASE], { ...kinds({ [`${BASE}/a`]: 'symlink' }), ...links({ [`${BASE}/a`]: `${BASE}/a` }) });
  add(`${p}_link_to_network`, platform, 'walk', [`${BASE}/HEAD`, BASE], { ...kinds({ [`${BASE}/HEAD`]: 'symlink' }), ...links({ [`${BASE}/HEAD`]: '/network/h/s' }) });
  add(`${p}_link_to_share_syntax`, platform, 'walk', [`${BASE}/HEAD`, BASE], { ...kinds({ [`${BASE}/HEAD`]: 'symlink' }), ...links({ [`${BASE}/HEAD`]: '//h/s' }) });
  add(`${p}_link_to_vol`, platform, 'walk', [`${BASE}/HEAD`, BASE], { ...kinds({ [`${BASE}/HEAD`]: 'symlink' }), ...links({ [`${BASE}/HEAD`]: '/.vol/1/2' }) });
  add(`${p}_network`, platform, 'walk', ['/network/h/s/x', BASE]);
  add(`${p}_network_then_back`, platform, 'walk', ['/network/../x', BASE]);
  add(`${p}_network_mixed_case`, platform, 'walk', ['/NetWork/x', BASE]);
  add(`${p}_network_kelvin`, platform, 'walk', ['/networ\u212a/x', BASE]);
  add(`${p}_vol`, platform, 'walk', ['/.vol/1/x', BASE]);
  add(`${p}_vol_then_back`, platform, 'walk', ['/.vol/../x', BASE]);
  add(`${p}_file_mount_case`, platform, 'walk', ['/.FILE/x', BASE]);
  add(`${p}_nofollow_nested`, platform, 'walk', ['/a/.nofollow/x', BASE]);
  add(`${p}_resolve_root`, platform, 'walk', ['/.resolve', BASE]);
  add(`${p}_net_same_host`, platform, 'walk', ['/net/hosta/x', '/net/hosta/repo']);
  add(`${p}_net_same_host_case`, platform, 'walk', ['/net/HostA/x', '/net/hosta/repo']);
  add(`${p}_net_other_host`, platform, 'walk', ['/net/hostb/x', '/net/hosta/repo']);
  add(`${p}_net_from_plain_base`, platform, 'walk', ['/net/hosta/x', BASE]);
  add(`${p}_net_root_only`, platform, 'walk', ['/net', BASE]);
  add(`${p}_servers_same_host`, platform, 'walk', ['/network/servers/hosta/x', '/network/servers/hosta/repo']);
  add(`${p}_servers_other_host`, platform, 'walk', ['/Network/Servers/hostb/x', '/network/servers/hosta/repo']);
  add(`${p}_servers_ligature`, platform, 'walk', ['/net/hosta/x', '/net/hosta/repo'], {});
  add(`${p}_format_characters_in_share`, platform, 'walk', ['/n\u200cet/hosta/x', '/net/hosta/repo']);
  add(`${p}_home_other_user`, platform, 'walk', ['/home/other/x', '/home/me/repo'], { home: '/home/me' });
  add(`${p}_home_own_user`, platform, 'walk', ['/home/me/x', '/home/me/repo'], { home: '/home/me' });
  add(`${p}_home_no_home_dir`, platform, 'walk', ['/home/me/x', '/home/me/repo'], { home: null });
  add(`${p}_home_listed_in_master`, platform, 'walk', ['/home/me/x', '/home/me/repo'], { home: '/home/me', master: '# maps\n/home auto_home\n' });
  add(`${p}_home_commented_out`, platform, 'walk', ['/home/me/x', '/home/me/repo'], { home: '/home/me', master: '#/home auto_home\n/net -hosts\n' });
  add(`${p}_home_other_mount`, platform, 'walk', ['/home/me/x', '/home/me/repo'], { home: '/home/me', master: '', stats: { '/home': { dev: 1 }, '/Users': { dev: 2 } } });
  add(`${p}_home_same_mount`, platform, 'walk', ['/home/me/x', '/home/me/repo'], { home: '/home/me', master: '', stats: { '/home': { dev: 2 }, '/Users': { dev: 2 } } });
  add(`${p}_home_missing`, platform, 'walk', ['/home/me/x', '/home/me/repo'], { home: '/home/me', master: { code: 'ENOENT' }, stats: { '/home': { code: 'ENOENT' } } });
  add(`${p}_home_denied`, platform, 'walk', ['/home/me/x', '/home/me/repo'], { home: '/home/me', master: { code: 'EACCES' } });
  add(`${p}_home_users_unreadable`, platform, 'walk', ['/home/me/x', '/home/me/repo'], { home: '/home/me', master: '', stats: { '/home': { dev: 1 }, '/Users': { code: 'EACCES' } } });
  add(`${p}_home_stat_denied`, platform, 'walk', ['/home/me/x', '/home/me/repo'], { home: '/home/me', master: '', stats: { '/home': { code: 'EACCES' } } });
  add(`${p}_data_volume_home`, platform, 'walk', ['/System/Volumes/Data/home/other/x', BASE], { home: '/home/me' });
  add(`${p}_data_volume_network`, platform, 'walk', ['/System/Volumes/Data/network/h/s/x', BASE]);
  add(`${p}_data_volume_nested_root`, platform, 'walk', ['/system/volumes/DATA/usr/local/x', BASE]);
  add(`${p}_data_volume_unknown_root`, platform, 'walk', ['/System/Volumes/Data/nothing/x', BASE]);
  add(`${p}_data_volume_ligature`, platform, 'walk', ['/sy\ufb05em/volumes/data/home/other', BASE], { home: '/home/me' });
  add(`${p}_home_data_volume_home_dir`, platform, 'walk', ['/home/me/x', BASE], { home: '/System/Volumes/Data/home/me' });
  add(`${p}_home_deep`, platform, 'walk', ['/home/me/a/b/c/d/e/f/g', BASE], { home: '/home/me' });
  add(`${p}_partial_network_host`, platform, 'walk', ['/network/h', BASE]);
  add(`${p}_partial_servers`, platform, 'walk', ['/Network/Servers', BASE]);
}

// The macOS rule on its own, against several base directories.
for (const [name, e, t, probe] of [
  ['both_plain', '/Users/me/x', '/Users/me/repo', {}],
  ['network_complete_same', '/network/h/s/x', '/network/h/s/repo', {}],
  ['network_complete_other', '/network/h/t/x', '/network/h/s/repo', {}],
  ['network_partial', '/network/h', '/network/h/s/repo', {}],
  ['network_partial_prefix', '/network/', '/network/h/s/repo', {}],
  ['network_in_plain_base', '/network/h/s', '/Users/me/repo', {}],
  ['home_of_other', '/home/o/x', '/home/me/repo', { home: '/home/me', master: '/home x' }],
  ['home_own_base_other_mac', '/home/me/x', '/home/me/repo', { home: '/home/me', master: '/home x' }],
  ['home_without_listing', '/home/o/x', '/Users/me/repo', { home: '/home/me', master: '', stats: { '/home': { dev: 1 }, '/Users': { dev: 1 } } }],
  ['special_mount', '/.vol/1', '/Users/me/repo', {}],
  ['relative_path', 'x/y', '/Users/me/repo', {}],
  ['base_relative', '/network/h/s', 'repo', {}],
]) {
  add(`darwin_mac_${name}`, 'darwin', 'mac', [e, t], probe);
  add(`linux_mac_${name}`, 'linux', 'mac', [e, t], probe);
  add(`win32_mac_${name}`, 'win32', 'mac', [e, t], probe);
}

// Whether a step of a walk is allowed.
for (const platform of ['linux', 'darwin', 'win32']) {
  for (const [name, e, t] of [
    ['vol', '/.vol', BASE],
    ['plain', '/x/y', BASE],
    ['net_host', '/net/hosta', '/net/hosta/r'],
    ['net_other_host', '/net/hostb', '/net/hosta/r'],
    ['net_plain_base', '/net/hosta', BASE],
    ['net_bare', '/net', BASE],
    ['network_servers_host', '/network/servers/hosta', '/network/servers/hosta'],
    ['network_servers_partial', '/network/servers', '/network/servers/hosta'],
    ['relative', 'net/hosta', BASE],
  ]) {
    add(`${platform}_step_${name}`, platform, 'step', [e, t]);
  }
}

// Which host a share path names.
for (const platform of ['linux', 'darwin', 'win32']) {
  for (const [name, e] of [
    ['plain', '/x/y'],
    ['net_host', '/net/HostA/x'],
    ['net_bare', '/net'],
    ['net_dot_dot', '/net/a/../b'],
    ['net_dot_dot_back', '/net/../x/y'],
    ['servers_host', '/Network/Servers/HostA'],
    ['servers_partial', '/network/servers'],
    ['network_other', '/network/x/y'],
    ['relative', 'net/a'],
    ['format_characters', '/ne\u200et/a'],
    ['kelvin_host', '/net/\u212a'],
    ['upper_ascii_only', '/net/\u00c4x'],
    ['dotted_capital_i', '/net/\u0130x'],
    ['dotless_i', '/netw\u0131rk/x'],
  ]) {
    add(`${platform}_host_${name}`, platform, 'host', [e]);
  }
}

// Refusing link text.
for (const platform of ['linux', 'darwin', 'win32']) {
  for (const [name, e, t] of [
    ['plain', 'refs/x', BASE],
    ['share_syntax', '//h/s', BASE],
    ['backslash_share', '\\\\h\\s', BASE],
    ['device_path', '\\??\\C:\\x', BASE],
    ['network', '/network/h/s', BASE],
    ['network_through_dots', '../../network/h/s', '/x/y/z'],
    ['bare_net', '/net', BASE],
    ['bare_net_dots', '/net/x/..', BASE],
    ['net_host_same', '/net/hosta/x', '/net/hosta/r'],
    ['net_host_other', '/net/hostb/x', '/net/hosta/r'],
    ['net_host_relative_up', '../hostb', '/net/hosta'],
    ['net_host_resolved', '../../net/hosta/x', '/a/b/c'],
    ['vol', '/.vol/1', BASE],
    ['vol_relative', '.vol/1', '/'],
    ['file_mount', '/.file/id=1', BASE],
    ['mac_network_partial', '/network/h', '/Users/me/r'],
    ['mac_home', '/home/o/x', '/Users/me/r'],
  ]) {
    add(`${platform}_refuses_${name}`, platform, 'refuses', [e, t], { home: '/home/me', master: '/home x' });
  }
}

// The automount root check, with repeated calls because it keeps state.
const autofs = (name, platform, calls, statfs, extra = {}) => add(name, platform, 'autofs', [calls], { statfs, ...extra });
for (const platform of ['linux', 'darwin', 'win32']) {
  autofs(`${platform}_autofs_root_directory`, platform, ['/'], {});
  autofs(`${platform}_autofs_not_top_level`, platform, ['/a/b'], { '/a/b': [391] });
  autofs(`${platform}_autofs_map_root`, platform, ['/mnt', '/mnt'], { '/mnt': [391] });
  autofs(`${platform}_autofs_other_type`, platform, ['/mnt', '/mnt'], { '/mnt': [1] });
  autofs(`${platform}_autofs_flips`, platform, ['/mnt', '/mnt', '/mnt'], { '/mnt': [391, 1, 391] });
  autofs(`${platform}_autofs_expected_error`, platform, ['/mnt', '/mnt'], { '/mnt': [{ code: 'ENOENT' }, 391] });
  autofs(`${platform}_autofs_eacces`, platform, ['/mnt', '/mnt'], { '/mnt': [{ code: 'EACCES' }, 391] });
  autofs(`${platform}_autofs_eopnotsupp`, platform, ['/mnt', '/mnt'], { '/mnt': [{ code: 'EOPNOTSUPP' }, 391] });
  autofs(`${platform}_autofs_unexpected_once`, platform, ['/mnt', '/mnt'], { '/mnt': [{ code: 'EIO' }, 391] });
  autofs(`${platform}_autofs_unexpected_twice`, platform, ['/mnt', '/mnt', '/mnt'], { '/mnt': [{ code: 'EIO' }, { code: 'EIO' }, 391] });
  autofs(`${platform}_autofs_recovers_after_failure`, platform, ['/mnt', '/mnt', '/mnt'], { '/mnt': [{ code: 'EIO' }, 391, 391] });
  autofs(`${platform}_autofs_type_error`, platform, ['/mnt', '/mnt'], { '/mnt': [{ typeError: true }, 391] });
  autofs(`${platform}_autofs_two_roots`, platform, ['/mnt', '/net', '/mnt', '/net'], { '/mnt': [391, 1], '/net': [1, 391] });
}
autofs('wsl_autofs_map_root', 'linux', ['/mnt'], { '/mnt': [391] }, { wsl: true });
autofs('wsl_autofs_proc_version', 'linux', ['/mnt'], { '/mnt': [391] }, { procVersion: 'Linux version 5.15.0-microsoft-standard-WSL2' });
autofs('linux_autofs_proc_version_plain', 'linux', ['/mnt'], { '/mnt': [391] }, { procVersion: 'Linux version 6.1.0-generic' });
autofs('linux_autofs_proc_version_missing', 'linux', ['/mnt'], { '/mnt': [391] }, { procVersion: null });

// Finding the repository root around a directory.
for (const platform of ['linux', 'darwin']) {
  const root = (name, start, probe = {}) => add(`${platform}_root_${name}`, platform, 'root', [start], probe);
  root('found_here', '/a/b/c', kinds({ '/a/b/c/.git': 'dir' }));
  root('found_above', '/a/b/c', kinds({ '/a/.git': 'dir', '/a/b/c/.git': 'absent', '/a/b/.git': 'absent' }));
  root('git_file', '/a/b', kinds({ '/a/b/.git': 'file' }));
  root('special_marker', '/a/b', kinds({ '/a/b/.git': 'special', '/a/.git': 'dir' }));
  root('absent_everywhere', '/a/b', kinds({ '/a/b/.git': 'absent', '/a/.git': 'absent', '/.git': 'absent' }));
  root('at_filesystem_root', '/a', kinds({ '/a/.git': 'absent', '/.git': 'dir' }));
  root('start_is_root', '/', kinds({ '/.git': 'dir' }));
  root('start_with_dots', '/a/b/../c/./d', kinds({ '/a/c/d/.git': 'dir' }));
  root('marker_link_ok', '/a/b', { ...kinds({ '/a/b/.git': 'symlink' }), ...links({ '/a/b/.git': '/r/real' }), stats: { '/a/b/.git': { kind: 'dir' } } });
  root('marker_link_to_file', '/a/b', { ...kinds({ '/a/b/.git': 'symlink' }), ...links({ '/a/b/.git': '/r/real' }), stats: { '/a/b/.git': { kind: 'file' } } });
  root('marker_link_dangling', '/a/b', { ...kinds({ '/a/b/.git': 'symlink', '/a/.git': 'dir' }), ...links({ '/a/b/.git': '/r/real' }), stats: { '/a/b/.git': { code: 'ENOENT' } } });
  root('marker_link_to_special', '/a/b', { ...kinds({ '/a/b/.git': 'symlink', '/a/.git': 'dir' }), ...links({ '/a/b/.git': '/r/real' }), stats: { '/a/b/.git': { kind: 'special' } } });
  root('marker_link_unreadable', '/a/b', { ...kinds({ '/a/b/.git': 'symlink', '/a/.git': 'dir' }) });
  root('marker_link_to_network', '/a/b', { ...kinds({ '/a/b/.git': 'symlink', '/a/.git': 'dir' }), ...links({ '/a/b/.git': '/network/h/s' }) });
  root('marker_link_to_share', '/a/b', { ...kinds({ '/a/b/.git': 'symlink', '/a/.git': 'dir' }), ...links({ '/a/b/.git': '//h/s' }) });
  root('marker_link_target_walk_refused', '/a/b', { ...kinds({ '/a/b/.git': 'symlink', '/r/real': 'special', '/a/.git': 'dir' }), ...links({ '/a/b/.git': '/r/real' }) });
  root('marker_link_relative', '/a/b', { ...kinds({ '/a/b/.git': 'symlink' }), ...links({ '/a/b/.git': '../real' }), stats: { '/a/b/.git': { kind: 'dir' } } });
  root('autofs_skipped', '/mnt/proj', { ...kinds({ '/mnt/.git': 'dir', '/mnt/proj/.git': 'absent', '/.git': 'dir' }), statfs: { '/mnt': [391] } });
  root('autofs_not_skipped', '/mnt/proj', { ...kinds({ '/mnt/.git': 'dir', '/mnt/proj/.git': 'absent' }), statfs: { '/mnt': [1] } });
  root('autofs_leaf_skipped', '/mnt', { ...kinds({ '/mnt/.git': 'dir', '/.git': 'dir' }), statfs: { '/mnt': [391] } });
}

// Windows path syntax: drive letters, backslashes, UNC and device paths, recorded with the win32 path module loaded.
const R = String.raw;
const W = R`C:\r\repo\.git`;
const U = R`\\h\s\repo\.git`;
const wp = (...parts) => [W, ...parts].join('\\');
const up = (...parts) => [U, ...parts].join('\\');
const wlink = (at, text, extra = {}) => ({ kinds: { [at]: 'symlink', ...(extra.kinds ?? {}) }, links: { [at]: text, ...(extra.links ?? {}) } });
for (const [name, text, base, probe] of [
  ['plain', wp('HEAD'), W, {}],
  ['forward_slashes', 'C:/r/repo/.git/HEAD', W, {}],
  ['mixed_runs', R`C:\r\\repo//.git\.\HEAD`, W, {}],
  ['relative', R`refs\heads\main`, W, {}],
  ['rooted_no_drive', R`\r\repo\HEAD`, W, {}],
  ['rooted_forward', '/r/repo/HEAD', W, {}],
  ['dot_dot', R`C:\r\repo\..\x`, W, {}],
  ['dot_dot_past_root', R`C:\..\..\x`, W, {}],
  ['drive_relative', R`C:x\y`, W, {}],
  ['unc', R`\\h\s\repo\x`, W, {}],
  ['unc_forward', '//h/s/repo/x', W, {}],
  ['unc_root_only', R`\\h\s`, W, {}],
  ['unc_one_part', R`\\h`, W, {}],
  ['unc_trailing', R`\\h\s\\x`, W, {}],
  ['network_text', '/network/h/s/x', W, {}],
  ['vol_text', '/.vol/x', W, {}],
  ['absent_tail', wp('refs', 'x'), W, { kinds: { [wp('refs')]: 'absent' } }],
  ['file_midway', wp('HEAD', 'x'), W, { kinds: { [wp('HEAD')]: 'file' } }],
  ['special', wp('HEAD'), W, { kinds: { [wp('HEAD')]: 'special' } }],
  ['lookup_fails', wp('HEAD'), W, { kinds: { [wp('HEAD')]: 'other' } }],
  ['link_relative', wp('HEAD'), W, wlink(wp('HEAD'), R`refs\heads\x`)],
  ['link_forward_relative', wp('HEAD'), W, wlink(wp('HEAD'), 'refs/heads/x')],
  ['link_drive_absolute_with_tail', wp('refs', 'heads', 'x'), W, wlink(wp('refs'), R`D:\elsewhere`)],
  ['link_drive_absolute_trailing', wp('refs', 'heads', 'x'), W, wlink(wp('refs'), 'D:\\elsewhere\\')],
  ['link_parent', wp('refs', 'heads', 'x'), W, wlink(wp('refs'), '..\\shared\\')],
  ['link_rooted', wp('HEAD'), W, wlink(wp('HEAD'), R`\other`)],
  ['link_rooted_forward', wp('HEAD'), W, wlink(wp('HEAD'), '/other')],
  ['link_drive_relative', wp('HEAD'), W, wlink(wp('HEAD'), 'D:x')],
  ['link_unc_other_host', wp('HEAD'), W, wlink(wp('HEAD'), R`\\h\s\x`)],
  ['link_unc_same_host', up('HEAD'), U, wlink(up('HEAD'), R`\\h\s\other`)],
  ['link_unc_same_host_case', up('HEAD'), U, wlink(up('HEAD'), R`\\H\s\other`)],
  ['link_unc_other_share_same_host', up('HEAD'), U, wlink(up('HEAD'), R`\\h\t\other`)],
  ['link_unc_dot_dot', up('HEAD'), U, wlink(up('HEAD'), R`\\h\s\..\x`)],
  ['link_device_question', wp('HEAD'), W, wlink(wp('HEAD'), R`\\?\C:\x`)],
  ['link_device_dot', wp('HEAD'), W, wlink(wp('HEAD'), R`\\.\pipe\x`)],
  ['link_device_unc', up('HEAD'), U, wlink(up('HEAD'), R`\\?\UNC\h\s\x`)],
  ['link_device_unc_lower', up('HEAD'), U, wlink(up('HEAD'), R`\\?\unc\h\s\x`)],
  ['link_nt_prefix', wp('HEAD'), W, wlink(wp('HEAD'), R`\??\C:\x`)],
  ['link_nt_forward', wp('HEAD'), W, wlink(wp('HEAD'), '/??/x')],
  ['link_nt_after_dots', wp('HEAD'), W, wlink(wp('HEAD'), R`\a\..\??\x`)],
  ['link_question_marks_inside', wp('HEAD'), W, wlink(wp('HEAD'), R`a\??\x`)],
  ['link_loop', wp('a'), W, wlink(wp('a'), wp('a'))],
  ['link_nul', wp('HEAD'), W, wlink(wp('HEAD'), 'a\u0000b')],
  ['link_missing_text', wp('HEAD'), W, { kinds: { [wp('HEAD')]: 'symlink' } }],
  ['link_posix_network', wp('HEAD'), W, wlink(wp('HEAD'), '/network/h/s')],
  ['link_posix_vol', wp('HEAD'), W, wlink(wp('HEAD'), '/.vol/1/2')],
]) {
  add(`win32_walk_${name}`, 'win32', 'walk', [text, base], probe);
}

for (const [name, text, base] of [
  ['relative', R`refs\x`, W],
  ['drive_absolute', R`D:\other`, W],
  ['dots_up', R`..\..\x`, W],
  ['drive_relative', 'D:x', W],
  ['rooted', R`\x`, W],
  ['unc', R`\\h\s`, W],
  ['unc_forward', '//h/s', W],
  ['unc_same_host', R`\\h\s`, U],
  ['unc_same_host_upper', R`\\H\S`, U],
  ['unc_other_host', R`\\g\s`, U],
  ['unc_base_forward', R`\\h\s`, '//h/s/repo'],
  ['unc_four_separators', R`\\\\x`, U],
  ['unc_dots', R`\\h\s\..\x`, U],
  ['unc_dot_segment', R`\\h\s\a\.\x`, U],
  ['unc_dot_space_run', R`\\h\s\. .\x`, U],
  ['unc_triple_dot', R`\\h\s\...\x`, U],
  ['unc_dot_prefixed_name', R`\\h\s\.a\x`, U],
  ['unc_dot_suffixed_name', R`\\h\s\a.\x`, U],
  ['unc_dot_at_end', R`\\h\s\.`, U],
  ['unc_dot_dot_at_end', R`\\h\s\..`, U],
  ['device_question', R`\\?\C:\x`, W],
  ['device_dot', R`\\.\pipe\x`, W],
  ['device_forward', '//?/C:/x', W],
  ['device_unc', R`\\?\UNC\h\s\x`, U],
  ['device_unc_other', R`\\?\UNC\g\s\x`, U],
  ['device_unc_upper', R`\\?\unc\H\s`, U],
  ['device_unc_without_server', '\\\\?\\UNC\\', U],
  ['nt_prefix', R`\??\C:\x`, W],
  ['nt_forward', '/??/x', W],
  ['nt_after_dots', R`\a\..\??\x`, W],
  ['nt_inside', R`a\??\x`, W],
  ['posix_network', '/network/h/s', W],
  ['posix_net', '/net', W],
  ['posix_vol', '/.vol/1', W],
  ['base_unc_relative_text', R`x\y`, U],
  ['base_device', R`x`, R`\\?\C:\r`],
]) {
  add(`win32_refuses_${name}`, 'win32', 'refuses', [text, base], { home: '/home/me', master: '/home x' });
}

// The same checks on a posix host, where the text is not a Windows path but the share and device syntax still counts.
for (const platform of ['linux', 'darwin']) {
  for (const [name, text, base] of [
    ['unc_same_host', '//h/s', '//h/repo'],
    ['unc_same_host_case', '//H/s', '//h/repo'],
    ['unc_other_host', '//g/s', '//h/repo'],
    ['backslash_unc_same_host', R`\\h\s`, '//h/repo'],
    ['backslash_unc_plain_base', R`\\h\s`, BASE],
    ['unc_dots', '//h/../x', '//h/repo'],
    ['unc_dot_run', '//h/. /x', '//h/repo'],
    ['unc_triple_slash', '///h/x', '//h/repo'],
    ['nt_slash', '/??/x', BASE],
    ['nt_normalized', '/a/../??/x', BASE],
    ['nt_inside', '/a/??/x', BASE],
    ['device_slash', '//?/C:/x', BASE],
    ['device_dot', '//./x', BASE],
    ['device_unc', '//?/UNC/h/s', '//h/repo'],
    ['device_unc_backslash', R`\\?\UNC\h\s`, '//h/repo'],
    ['dots_text', '../x', BASE],
    ['dot_text', './x', BASE],
  ]) {
    add(`${platform}_refuses_${name}`, platform, 'refuses', [text, base], { home: '/home/me', master: '/home x' });
  }
}
add('linux_walk_link_unc_other_host', 'linux', 'walk', [`${BASE}/HEAD`, BASE], wlink(`${BASE}/HEAD`, '//h/other'));
add('linux_walk_link_nt_slash', 'linux', 'walk', [`${BASE}/HEAD`, BASE], wlink(`${BASE}/HEAD`, '/??/x'));
add('linux_walk_unc_base_link_same_host', 'linux', 'walk', ['/h/repo/.git/HEAD', '//h/repo/.git'], wlink('/h/repo/.git/HEAD', '//h/other'));

// Reaching text from a base directory: refused, unreachable, and where it leads.
for (const [name, text, base, probe] of [
  ['relative', R`refs\x`, W, {}],
  ['forward_relative', 'refs/x', W, {}],
  ['dots_up', R`..\x`, W, {}],
  ['dots_up_through_special', R`..\x`, W, { kinds: { [R`C:\r\repo\x`]: 'special' } }],
  ['dots_up_joined_special', R`..\x`, W, { kinds: { [wp('..')]: 'special' } }],
  ['drive_absolute', R`D:\x`, W, {}],
  ['drive_absolute_special', R`D:\x`, W, { kinds: { [R`D:\x`]: 'special' } }],
  ['rooted', R`\x`, W, {}],
  ['unc_refused', R`\\h\s\x`, W, {}],
  ['unc_same_host', R`\\h\s\x`, U, {}],
  ['through_link', R`refs\x`, W, wlink(wp('refs'), R`D:\real`)],
  ['through_refused_link', R`refs\x`, W, wlink(wp('refs'), R`\\g\s`)],
]) {
  add(`win32_reach_${name}`, 'win32', 'reach', [text, base], probe);
}
for (const platform of ['linux', 'darwin']) {
  for (const [name, text, base, probe] of [
    ['relative', 'refs/x', BASE, {}],
    ['dots_up', '../x', BASE, {}],
    ['dots_up_resolved_special', '../x', BASE, kinds({ '/r/repo/x': 'special' })],
    ['dots_up_joined_special', '../x', BASE, kinds({ [`${BASE}/..`]: 'special' })],
    ['absolute', '/abs/x', BASE, {}],
    ['absolute_special', '/abs/x', BASE, kinds({ '/abs': 'special' })],
    ['network', '/network/h/s', BASE, {}],
    ['same_host_share', '//h/s', '//h/repo', {}],
    ['through_link', 'refs/x', BASE, wlink(`${BASE}/refs`, '/real')],
  ]) {
    add(`${platform}_reach_${name}`, platform, 'reach', [text, base], probe);
  }
}

// Finding the repository root with Windows paths.
{
  const root = (name, start, probe = {}) => add(`win32_root_${name}`, 'win32', 'root', [start], probe);
  const git = (dir) => `${dir}\\.git`;
  root('found_here', R`C:\a\b\c`, kinds({ [git(R`C:\a\b\c`)]: 'dir' }));
  root('found_above', R`C:\a\b\c`, kinds({ [git(R`C:\a\b\c`)]: 'absent', [git(R`C:\a\b`)]: 'absent', [git(R`C:\a`)]: 'dir' }));
  root('git_file', R`C:\a\b`, kinds({ [git(R`C:\a\b`)]: 'file' }));
  root('special_marker', R`C:\a\b`, kinds({ [git(R`C:\a\b`)]: 'special', [git(R`C:\a`)]: 'dir' }));
  root('absent_everywhere', R`C:\a\b`, kinds({ [git(R`C:\a\b`)]: 'absent', [git(R`C:\a`)]: 'absent', [R`C:\.git`]: 'absent' }));
  root('at_drive_root', R`C:\a`, kinds({ [git(R`C:\a`)]: 'absent', [R`C:\.git`]: 'dir' }));
  root('start_is_drive_root', 'C:\\', kinds({ [R`C:\.git`]: 'dir' }));
  root('forward_slash_start', 'C:/a/b', kinds({ [git(R`C:\a\b`)]: 'dir' }));
  root('start_with_dots', R`C:\a\b\..\c\.\d`, kinds({ [git(R`C:\a\c\d`)]: 'dir' }));
  root('lower_case_drive', R`c:\a`, kinds({ [git(R`c:\a`)]: 'dir' }));
  root('rooted_no_drive', R`\a\b`, kinds({ [git(R`\a\b`)]: 'absent', [git(R`\a`)]: 'dir' }));
  root('unc_found_here', R`\\h\s\a\b`, kinds({ [git(R`\\h\s\a\b`)]: 'dir' }));
  root('unc_found_at_share_root', R`\\h\s\a`, kinds({ [git(R`\\h\s\a`)]: 'absent', [git(R`\\h\s`)]: 'dir' }));
  root('unc_absent_everywhere', R`\\h\s\a`, kinds({ [git(R`\\h\s\a`)]: 'absent', [git(R`\\h\s`)]: 'absent', [R`\.git`]: 'dir' }));
  root('marker_link_ok', R`C:\a\b`, { ...wlink(git(R`C:\a\b`), R`D:\real`), stats: { [git(R`C:\a\b`)]: { kind: 'dir' } } });
  root('marker_link_relative', R`C:\a\b`, { ...wlink(git(R`C:\a\b`), R`..\real`), stats: { [git(R`C:\a\b`)]: { kind: 'file' } } });
  root('marker_link_dangling', R`C:\a\b`, { ...wlink(git(R`C:\a\b`), R`D:\real`, { kinds: { [git(R`C:\a`)]: 'dir' } }), stats: { [git(R`C:\a\b`)]: { code: 'ENOENT' } } });
  root('marker_link_to_unc', R`C:\a\b`, wlink(git(R`C:\a\b`), R`\\h\s`, { kinds: { [git(R`C:\a`)]: 'dir' } }));
  root('marker_link_to_device', R`C:\a\b`, wlink(git(R`C:\a\b`), R`\\?\C:\x`, { kinds: { [git(R`C:\a`)]: 'dir' } }));
  root('marker_link_to_nt', R`C:\a\b`, wlink(git(R`C:\a\b`), R`\??\C:\x`, { kinds: { [git(R`C:\a`)]: 'dir' } }));
  root('marker_link_unreadable', R`C:\a\b`, { kinds: { [git(R`C:\a\b`)]: 'symlink', [git(R`C:\a`)]: 'dir' } });
}

// The helpers behind the path syntax, one process per platform: the bundle's own regular expressions and checks, and
// the path module of the platform where the bundle uses it.
const flavorScenarios = [];
{
  const singles = ['', '/', '\\', 'a', 'a/b', 'a\\b', 'a//b\\\\c', 'a\\', 'a/', 'C:', 'C:\\', 'C:/', 'C:\\a', 'c:/a\\b', 'C:a', '1:\\a', '\\a', '/a',
    '\\\\', '\\\\srv', '\\\\srv\\share', '\\\\srv\\share\\', '\\\\srv\\share\\x', '\\\\srv\\\\x', '\\\\\\share', '//srv/share/x', '//?/UNC/',
    '//?/UNC/Srv/x', '//./UNC/H', '//?/c:/x', '//?x', '//', '//?/', '\\??\\x', '/??/x', 'a??', '\\\\.\\..\\??\\x', '\\\\?\\..\\??\\x', '/a/./b', '/a/../b', 'a/.. /b',
    'a/.b', 'a./b', '..', '.', '...', '. ', '//a/./b', '//SRV/Share', '//srv/', '///', '/x/y', '//srv/share/..', '//Srv\\Share'];
  const bases = ['/', '//srv/share', '//SRV/other', '/x', 'C:\\a', '\\\\srv\\share\\x'];
  const pairs = [['C:\\a', 'b'], ['/a', 'b'], ['/a', '../b'], ['C:\\a', '..\\b'], ['a', 'b'], ['', 'b'], ['a', ''], ['C:\\', '..'], ['\\\\srv\\share\\x', '..\\..\\y'],
    ['C:\\a', 'D:\\b'], ['C:\\a', '/b'], ['/a/b', '../../..'], ['/', 'x']];
  const alone = ['/a/../b', 'C:\\a\\..\\b', 'a', '', '.', 'C:', '\\\\srv\\share', '/', '//a//b/'];
  for (const [platform, win] of [['linux', false], ['win32', true]]) {
    const calls = [['sep']];
    for (const text of singles) {
      for (const op of ['rooted', 'rootLen', 'split', 'ends', 'server', 'device', 'dots', 'nt', 'two', 'dirname']) calls.push([op, text]);
      for (const base of bases) calls.push(['share', text, base]);
    }
    for (const pair of pairs) for (const op of ['join', 'resolve']) calls.push([op, ...pair]);
    for (const text of alone) calls.push(['resolveAlone', text]);
    flavorScenarios.push({ name: `flavor_${platform}`, platform, fn: 'flavor', args: calls.filter((call) => win || !['resolve', 'resolveAlone'].includes(call[0]) || call.slice(1).every((part) => part.startsWith('/'))), probe: {} });
  }
}

const names = new Set();
for (const scenario of scenarios) {
  if (names.has(scenario.name)) throw new Error(`duplicate scenario ${scenario.name}`);
  names.add(scenario.name);
}
const cases = [];
const flavorCases = [];
for (const scenario of [...scenarios, ...flavorScenarios]) {
  const win32 = scenario.platform === 'win32';
  const run = spawnSync(process.execPath, [...(win32 ? ['--import', hookPath] : []), self, '--child', JSON.stringify({ bridgePath: patchedPath, ...scenario })], {
    encoding: 'utf8',
    timeout: 60000,
    env: { ...process.env, WIN32_PATH_PARENT: pathToFileURL(patchedPath).href },
  });
  if (run.status !== 0) throw new Error(`${scenario.name}: ${run.stderr.split('\n').map((line) => line.slice(0, 240)).join('\n')}`);
  (scenario.fn === 'flavor' ? flavorCases : cases).push({ ...scenario, expect: JSON.parse(run.stdout) });
}
fs.rmSync(patchedDir, { recursive: true, force: true });

for (const [file, list] of [['platform.json', cases], ['flavor.json', flavorCases]]) {
  const text = `[\n${list.map((entry) => JSON.stringify(entry)).join(',\n')}\n]\n`;
  const target = path.join(fixturesDir, file);
  if (check) {
    if (!fs.existsSync(target) || fs.readFileSync(target, 'utf8') !== text) {
      process.stderr.write(`${target} is stale\n`);
      process.exit(1);
    }
  } else {
    fs.mkdirSync(fixturesDir, { recursive: true });
    fs.writeFileSync(target, text);
  }
}

// Runs one scenario in its own process: the platform is read when the bundle loads and the checks keep state.
async function child({ bridgePath: bridge, platform, fn, args, probe }) {
  delete process.env.CLAUDE_CODE_ENTRYPOINT;
  delete process.env.WSL_DISTRO_NAME;
  delete process.env.WSL_INTEROP;
  globalThis.Bun = { which: () => null };
  process.env.PROBE_SESSION_CWD = '/nonexistent-session-directory';
  Object.defineProperty(process, 'platform', { value: platform });
  if (platform === 'win32') process.chdir('/');
  if (probe.wsl) process.env.WSL_DISTRO_NAME = 'Ubuntu';
  const module = await import(pathToFileURL(bridge).href).catch((error) => {
    process.stderr.write(`import failed: ${String(error?.message ?? error).slice(0, 300)}\n`);
    process.exit(3);
  });
  // Run one session creation so every lazily initialised piece of the bundle exists before the host is replaced.
  const server = http.createServer((req, res) => {
    req.resume();
    req.on('end', () => {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end('{"session":{"id":"cse_abc"}}');
    });
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const warm = fs.mkdtempSync(path.join(os.tmpdir(), 'bridge-platform-warm-'));
  await module.createCodeSession(`http://127.0.0.1:${server.address().port}`, 't', 'T', 5000, undefined, { gitRepoUrl: 'https://github.com/acme/widgets', branch: '' }, warm).catch(() => {});
  server.close();
  install(probe, platform);
  // The warm-up answered the home mount check against the real host and the bundle keeps that answer.
  module.__mountCache.value = undefined;
  const result = invoke(module, fn, args, platform);
  process.stdout.write(JSON.stringify(result));
}

function failure(code) {
  const error = new Error(code);
  error.code = code;
  return error;
}

function entry(kind, dev) {
  return {
    isSymbolicLink: () => kind === 'symlink',
    isFile: () => kind === 'file',
    isDirectory: () => kind === 'dir',
    dev,
  };
}

function install(probe, platform) {
  const kindMap = probe.kinds ?? {};
  const linkMap = probe.links ?? {};
  const stats = probe.stats ?? {};
  const statfsMap = Object.fromEntries(Object.entries(probe.statfs ?? {}).map(([key, list]) => [key, [...list]]));
  const real = { lstat: fs.lstatSync, readFile: fs.readFileSync };
  fs.lstatSync = (file, ...rest) => {
    if (typeof file !== 'string' || (platform !== 'win32' && !path.isAbsolute(file))) return real.lstat(file, ...rest);
    const kind = kindMap[file] ?? 'dir';
    if (kind === 'absent') throw failure('ENOENT');
    if (kind === 'other') throw failure('EIO');
    return entry(kind, 1);
  };
  fs.statSync = (file) => {
    const given = stats[file];
    if (given === undefined) return entry('dir', 1);
    if (given.code !== undefined) throw failure(given.code);
    return entry(given.kind ?? 'dir', given.dev ?? 1);
  };
  fs.readlinkSync = (file) => {
    if (linkMap[file] === undefined) throw failure('EINVAL');
    return Buffer.from(linkMap[file], 'utf8');
  };
  fs.statfsSync = (file) => {
    const list = statfsMap[file];
    if (list === undefined) throw failure('ENOENT');
    const next = list.length > 1 ? list.shift() : list[0];
    if (typeof next === 'number') return { type: next };
    if (next.typeError) throw new TypeError('statfs is not a function');
    throw failure(next.code);
  };
  fs.readFileSync = (file, ...rest) => {
    if (file === '/etc/auto_master') {
      if (probe.master === undefined) throw failure('ENOENT');
      if (typeof probe.master === 'object') throw failure(probe.master.code);
      return probe.master;
    }
    if (file === '/proc/version') {
      if (probe.procVersion === undefined || probe.procVersion === null) throw failure('ENOENT');
      return probe.procVersion;
    }
    return real.readFile(file, ...rest);
  };
  os.homedir = () => {
    if (probe.home === undefined || probe.home === null) throw failure('ENOENT');
    return probe.home;
  };
  syncBuiltinESMExports();
}

function invoke(module, fn, args, platform) {
  switch (fn) {
    case 'walk': return module.__walk(args[0], args[1]);
    case 'refuses': return module.__refuses(args[0], args[1]);
    case 'mac': return module.__mac(args[0], args[1]);
    case 'step': return module.__step(args[0], args[1]);
    case 'host': return module.__host(args[0]);
    case 'reach': {
      const resolve = platform === 'win32' ? win32Path.resolve : path.resolve;
      return { refused: module.__refuses(args[0], args[1]), unreachable: module.__reach(args[0], args[1]), resolved: resolve(args[1], args[0]) };
    }
    case 'autofs': return args[0].map((entryPath) => module.__autofs(entryPath));
    case 'root': {
      const found = module.__root(args[0]);
      return found === module.__none ? null : String(found);
    }
    case 'flavor': {
      const flavorPath = platform === 'win32' ? win32Path : path;
      const operations = {
        sep: () => flavorPath.sep,
        rooted: (text) => module.__rooted.test(text),
        rootLen: (text) => text.match(module.__rootRe)?.[0].length ?? 0,
        split: (text) => text.split(module.__splitRe),
        ends: (text) => module.__endRe.test(text),
        server: (text) => module.__server(text),
        device: (text) => module.__device(text),
        dots: (text) => module.__dots(text),
        nt: (text) => module.__nt(text),
        two: (text) => module.__two(text),
        share: (text, base) => module.__share(text, base),
        dirname: (text) => flavorPath.dirname(text),
        join: (left, right) => flavorPath.join(left, right),
        resolve: (left, right) => flavorPath.resolve(left, right),
        resolveAlone: (text) => flavorPath.resolve(text),
      };
      return args.map(([operation, ...inputs]) => operations[operation](...inputs));
    }
    default: throw new Error(`unknown function ${fn}`);
  }
}
