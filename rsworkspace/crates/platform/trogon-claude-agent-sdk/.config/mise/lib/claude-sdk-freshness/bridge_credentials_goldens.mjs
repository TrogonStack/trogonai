#!/usr/bin/env node
// Records what the pinned TypeScript package's `fetchRemoteCredentials` and `createCodeSession` (bridge.mjs) send
// to a local HTTP server and what each answer makes them return, so the Rust client is checked against the real
// functions instead of against a reading of them.
//
// usage: node bridge_credentials_goldens.mjs <path to bridge.mjs> <out-file> [--check]
//
// Each case holds the call arguments, the response the local server plays, the request it captured (method, path,
// raw header pairs with the host spelled "<host>", body text) and the value the function returned. A case whose
// server never answers or refuses the connection records no request.
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const [bridgeArg, outArg, ...flags] = process.argv.slice(2);
const bridgePath = bridgeArg ? path.resolve(bridgeArg) : bridgeArg;
const out = outArg ? path.resolve(outArg) : outArg;
if (!bridgePath || !out) {
  process.stderr.write('usage: bridge_credentials_goldens.mjs BRIDGE_MJS OUT_FILE [--check]\n');
  process.exit(2);
}
delete process.env.CLAUDE_CODE_ENTRYPOINT;
// The default session cwd is read from the process, so run in a throwaway directory whose path is masked as "<cwd>".
const cwdForGit = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'bridge-goldens-')));
process.chdir(cwdForGit);
const bridge = await import(pathToFileURL(bridgePath).href);

const ok = (extra = {}) => ({ worker_jwt: 'jwt', expires_in: 3600, api_base_url: 'https://ingress.example', worker_epoch: 7, ...extra });
const json = (status, value, headers = []) => ({ status, headers: [['content-type', 'application/json'], ...headers], body: JSON.stringify(value) });
const raw = (status, body, headers = []) => ({ status, headers, body });
const err403 = (error, headers = []) => json(403, { error }, headers);

const credentialCases = [];
const credential = (name, response, args = {}) => credentialCases.push({ name, response, args });
credential('ok_minimal', json(200, ok()));
credential('ok_with_trusted_device_token', json(200, ok()), { trustedDeviceToken: 'device-token' });
credential('ok_empty_trusted_device_token_is_not_sent', json(200, ok()), { trustedDeviceToken: '' });
credential('ok_epoch_string', json(200, ok({ worker_epoch: '7' })));
credential('ok_epoch_padded_string', json(200, ok({ worker_epoch: ' 8\n' })));
credential('ok_epoch_hex_string', json(200, ok({ worker_epoch: '0x10' })));
credential('ok_epoch_exponent_string', json(200, ok({ worker_epoch: '1e3' })));
credential('ok_epoch_empty_string_is_zero', json(200, ok({ worker_epoch: '' })));
credential('ok_epoch_negative', json(200, ok({ worker_epoch: -3 })));
credential('ok_epoch_whole_float', json(200, ok({ worker_epoch: 5.0 })));
credential('ok_epoch_binary_string', json(200, ok({ worker_epoch: '0b101' })));
credential('ok_epoch_octal_string', json(200, ok({ worker_epoch: '0o17' })));
credential('ok_epoch_signed_string', json(200, ok({ worker_epoch: '+12' })));
credential('ok_epoch_fraction_string', json(200, ok({ worker_epoch: '12.' })));
credential('ok_epoch_leading_dot_string', json(200, ok({ worker_epoch: '.5e1' })));
credential('ok_epoch_max_safe', json(200, ok({ worker_epoch: 9007199254740991 })));
credential('ok_expires_fractional', json(200, ok({ expires_in: 1.5 })));
credential('ok_extra_fields_ignored', json(200, ok({ extra: 1 })));
credential('ok_mcp_config', json(200, ok({ mcp_config: { content: '{"servers":{}}' } })));
credential('ok_mcp_config_empty_content', json(200, ok({ mcp_config: { content: '' } })));
credential('ok_mcp_config_number_content', json(200, ok({ mcp_config: { content: 1 } })));
credential('ok_mcp_config_null', json(200, ok({ mcp_config: null })));
credential('ok_mcp_config_string', json(200, ok({ mcp_config: 'x' })));
credential('ok_mcp_config_missing_content', json(200, ok({ mcp_config: {} })));
for (const epoch of ['abc', 1.5, 9007199254740992, '9007199254740993', null, true, [], {}, '1_0', 'Infinity', '-Infinity', '0x', '1e', '--1']) {
  credential(`malformed_epoch_${JSON.stringify(epoch)}`, json(200, ok({ worker_epoch: epoch })));
}
credential('malformed_epoch_overflow_token', raw(200, '{"worker_jwt":"j","expires_in":1,"api_base_url":"u","worker_epoch":1e999}'));
credential('ok_expires_overflow_token', raw(200, '{"worker_jwt":"j","expires_in":1e999,"api_base_url":"u","worker_epoch":2}'));
credential('malformed_missing_epoch_key', json(200, { worker_jwt: 'j', expires_in: 1, api_base_url: 'u' }));
credential('malformed_missing_jwt', json(200, { expires_in: 1, api_base_url: 'u', worker_epoch: 1 }));
credential('malformed_jwt_number', json(200, ok({ worker_jwt: 1 })));
credential('malformed_expires_string', json(200, ok({ expires_in: '5' })));
credential('malformed_api_base_url_number', json(200, ok({ api_base_url: 5 })));
credential('malformed_missing_api_base_url', json(200, { worker_jwt: 'j', expires_in: 1, worker_epoch: 1 }));
credential('malformed_missing_expires', json(200, { worker_jwt: 'j', api_base_url: 'u', worker_epoch: 1 }));
credential('malformed_array_body', raw(200, '[1,2]', [['content-type', 'application/json']]));
credential('malformed_null_body', raw(200, 'null'));
credential('malformed_text_body', raw(200, 'not json'));
credential('malformed_empty_body', raw(200, ''));
credential('malformed_bom_body_is_stripped', raw(200, `﻿${JSON.stringify(ok())}`));
credential('malformed_lone_number_body', raw(200, '7'));
for (const id of ['', 'a/b', 'ab cd', 'x\n', 'é', 'ok-id_1']) {
  credential(`session_id_${JSON.stringify(id)}`, json(200, ok()), { sessionId: id });
}
credential('status_401', json(401, { error: { message: 'bad token' } }));
credential('status_408', raw(408, ''));
credential('status_429', raw(429, ''));
credential('status_500', raw(500, 'boom'));
credential('status_503', raw(503, ''));
credential('status_404', json(404, { error: { message: 'gone' } }));
credential('status_400', raw(400, 'plain'));
credential('status_201_is_not_success', json(201, ok()));
credential('status_204', raw(204, ''));
credential('status_304', raw(304, ''));
credential('status_409', json(409, {}));
credential('status_403_untrusted_device', err403({ resource: 'untrusted_device' }));
credential('status_403_untrusted_device_sub_null', err403({ resource: 'untrusted_device', sub_reason: null }));
credential('status_403_untrusted_device_sub_empty', err403({ resource: 'untrusted_device', sub_reason: '' }));
credential('status_403_untrusted_device_bound_unattested', err403({ resource: 'untrusted_device', sub_reason: 'bound_unattested' }));
credential('status_403_untrusted_device_signature', err403({ resource: 'untrusted_device', sub_reason: 'served_call_unsigned' }));
credential('status_403_untrusted_device_other_sub', err403({ resource: 'untrusted_device', sub_reason: 'some_reason_9' }));
credential('status_403_untrusted_device_number_sub', err403({ resource: 'untrusted_device', sub_reason: 5 }));
credential('status_403_untrusted_device_object_sub', err403({ resource: 'untrusted_device', sub_reason: {} }));
credential('status_403_untrusted_device_bad_sub', err403({ resource: 'untrusted_device', sub_reason: 'Not-Snake' }));
credential('status_403_untrusted_device_long_sub', err403({ resource: 'untrusted_device', sub_reason: 'a'.repeat(65) }));
credential('status_403_untrusted_device_max_sub', err403({ resource: 'untrusted_device', sub_reason: 'a'.repeat(64) }));
credential('status_403_stale_relogin', err403({ resource: 'session_stale_relogin' }));
credential('status_403_tier_below_policy', err403({ resource: 'session_security_tier_below_org_policy' }));
credential('status_403_other_resource_origin', err403({ resource: 'other' }, [['request-id', 'req_abc123']]));
credential('status_403_origin_padded_request_id', err403({ resource: 'other' }, [['request-id', '  req_abc123  ']]));
credential('status_403_bad_request_id_falls_to_other', err403({ resource: 'other' }, [['request-id', 'abc123']]));
credential('status_403_long_request_id', err403({ resource: 'other' }, [['request-id', `req_${'a'.repeat(37)}`]]));
credential('status_403_cf_ray', err403({}, [['cf-ray', '8a1b2c3d-LHR']]));
credential('status_403_server_cloudflare', err403({}, [['server', 'Cloudflare']]));
credential('status_403_server_other', err403({}, [['server', 'nginx']]));
credential('status_403_empty_cf_ray_and_server', err403({}, [['cf-ray', ''], ['server', 'nginx']]));
credential('status_403_duplicate_request_id_joins', err403({}, [['request-id', 'req_a'], ['request-id', 'req_b']]));
credential('status_403_duplicate_server_keeps_first', err403({}, [['server', 'nginx'], ['server', 'cloudflare']]));
credential('status_403_no_body', raw(403, ''));
credential('status_403_error_not_object', json(403, { error: 'untrusted_device' }));
credential('status_403_error_array', json(403, { error: [] }));
credential('status_403_error_null', json(403, { error: null }));
credential('status_403_resource_number', json(403, { error: { resource: 1 } }));
credential('status_403_tier_with_padding_is_other', err403({ resource: 'session_security_tier_below_org_policy ' }));
for (const entrypoint of ['claude-vscode', 'remote', 'remote_baku', 'remote_cowork', 'remote_desktop', 'remote_mobile', 'remote_projects', 'claude-in-teams', 'sdk-cli', 'sdk-ts', 'sdk-py', 'mcp', 'claude-code-github-action', 'local-agent', 'claude_in_slack', 'claude-in-slack', 'cli', 'unknown-entrypoint', '']) {
  credential(`entrypoint_${entrypoint}`, json(200, ok()), { entrypoint });
}
credential('refused_connection', null);
credential('hangs_until_timeout', { hang: true }, { timeoutMs: 150 });
credential('redirect_307_keeps_post', { redirect: 307 });
credential('redirect_302_becomes_get', { redirect: 302 });
credential('status_500_after_redirect_is_null', { redirect: 307, final: raw(500, '') });

const createCases = [];
const create = (name, response, args = {}) => createCases.push({ name, response, args });
const session = (id = 'cse_abc123') => ({ session: { id } });
create('ok_minimal', json(200, session()));
create('ok_status_201', json(201, session()));
create('ok_tags', json(200, session()), { tags: ['a', 'b'] });
create('ok_empty_tags_omitted', json(200, session()), { tags: [] });
create('ok_grouping_id', json(200, session()), { sessionGroupingId: 'group-1' });
create('ok_empty_grouping_id_omitted', json(200, session()), { sessionGroupingId: '' });
create('ok_model', json(200, session()), { model: 'claude-x' });
create('ok_empty_model_omitted', json(200, session()), { model: '' });
create('ok_cwd', json(200, session()), { cwd: '/work/dir' });
create('ok_empty_cwd_is_kept', json(200, session()), { cwd: '' });
create('ok_default_cwd', json(200, session()));
create('ok_everything', json(200, session()), { title: 'T "q"   é', tags: ['x'], cwd: '/w', model: 'm', sessionGroupingId: 'g' });
create('ok_bare_prefix_id', json(200, session('cse_')));
create('ok_id_with_dashes', json(200, session('cse_a-b_c')));
for (const id of ['abc', 'session_abc', 'cse_a b', 'cse_a/b', 5, null, '', 'cse_é']) {
  create(`malformed_id_${JSON.stringify(id)}`, json(200, { session: { id } }));
}
create('malformed_no_session', json(200, {}));
create('malformed_session_null', json(200, { session: null }));
create('malformed_session_string', json(200, { session: 'cse_a' }));
create('malformed_array', raw(200, '[]'));
create('malformed_null', raw(200, 'null'));
create('malformed_text', raw(200, 'ok'));
create('malformed_201', json(201, {}));
create('status_401', json(401, { error: { message: 'bad token' } }));
create('status_408', raw(408, ''));
create('status_429', json(429, { error: { type: 'not_found_error', resource_type: 'session_grouping' } }));
create('status_500', raw(500, ''));
create('status_502', raw(502, ''));
create('status_404_plain_detail', json(404, { error: { message: 'no such repo' } }));
create('status_404_top_level_message', json(404, { message: 'top level' }));
create('status_404_top_level_message_wins', json(404, { message: 'top', error: { message: 'inner' } }));
create('status_404_non_string_message_uses_error', json(404, { message: 5, error: { message: 'inner' } }));
create('status_404_empty_top_level_message_blocks_error_message', json(404, { message: '', error: { message: 'inner' } }));
create('status_404_no_message', json(404, { error: {} }));
create('status_404_text_body', raw(404, 'plain text'));
create('status_400_array_body', raw(400, '[1]'));
create('status_400_grouping_by_type', json(400, { error: { type: 'not_found_error', resource_type: 'session_grouping', message: 'group missing' } }));
create('status_404_type_without_resource', json(404, { error: { type: 'not_found_error' } }));
create('status_404_resource_without_type', json(404, { error: { resource_type: 'session_grouping' } }));
create('status_400_grouping_public_hosted_only', json(400, { error: { reason: 'public_grouping_hosted_only' } }));
create('status_422_grouping_feature_disabled', json(422, { error: { reason: 'feature_disabled', message: 'off' } }));
create('status_403_grouping_reason', json(403, { error: { reason: 'feature_disabled' } }));
create('status_400_other_reason', json(400, { error: { reason: 'other' } }));
create('status_401_grouping_body_is_oauth', json(401, { error: { reason: 'feature_disabled' } }));
create('status_300_with_grouping_body_is_rejected', json(300, { error: { reason: 'feature_disabled' } }));
create('status_204', raw(204, ''));
create('status_error_not_object', json(400, { error: 'x' }));
create('status_error_null', json(400, { error: null }));
create('status_error_array', json(400, { error: [] }));
const detail = (message) => json(400, { error: { message } });
for (const [label, message] of [
  ['one_line', 'Repository not found'],
  ['backtick_kept', 'use `x` here'],
  ['newline', 'line1\nline2'],
  ['carriage_return', 'a\rb'],
  ['tab', 'a\tb'],
  ['double_space', 'a  b'],
  ['leading_space', ' a'],
  ['trailing_space', 'a '],
  ['only_space', ' '],
  ['empty', ''],
  ['nbsp', 'a b'],
  ['ideographic_space', 'a　b'],
  ['zero_width_space', 'a​b'],
  ['bidi_override', 'a‮b'],
  ['soft_hyphen', 'a­b'],
  ['variation_selector', 'a️b'],
  ['braille_blank', 'a⠀b'],
  ['bell', 'a\u0007b'],
  ['del', 'a\u007fb'],
  ['c1_control', 'a\u0085b'],
  ['line_separator', 'a b'],
  ['private_use', 'ab'],
  ['unassigned', 'a͸b'],
  ['fullwidth_grave', 'a｀b'],
  ['modifier_grave', 'aˋb'],
  ['greek_varia', 'a`b'],
  ['reversed_prime', 'a‵b'],
  ['leading_combining_mark', '́abc'],
  ['combining_mark_after_space', 'a ́b'],
  ['combining_mark_after_punctuation', 'a.́b'],
  ['combining_mark_after_letter', 'áb'],
  ['nfd_decomposed', 'é'],
  ['nfc_composed', 'é'],
  ['hangul_jamo_nfc', '가'],
  ['ansi_color', 'a\u001b[31mred\u001b[0m'],
  ['ansi_osc', 'a\u001b]0;title\u0007b'],
  ['emoji', 'ok 😀'],
  ['cjk', '日本語'],
  ['len_1024', 'a'.repeat(1024)],
  ['len_1025', 'a'.repeat(1025)],
  ['len_1024_units_with_pair', `${'a'.repeat(1022)}😀`],
  ['len_1025_units_with_pair', `${'a'.repeat(1023)}😀`],
  ['plain_punctuation', 'Bad request: "x" (code=3) [a] {b} <c>'],
]) {
  create(`detail_${label}`, detail(message));
}
const git = (gitRepoUrl, branch, defaultBranch) => ({ gitContext: { gitRepoUrl, branch, ...(defaultBranch === undefined ? {} : { defaultBranch }) } });
create('git_feature_branch', json(200, session()), git('https://github.com/acme/widgets.git', 'feat', 'main'));
create('git_default_branch_is_omitted', json(200, session()), git('https://github.com/acme/widgets', 'main', 'main'));
create('git_branch_without_default_evidence', json(200, session()), git('https://github.com/acme/widgets', 'feat'));
create('git_head_uses_default', json(200, session()), git('https://github.com/acme/widgets', 'HEAD', 'main'));
create('git_empty_branch_uses_default', json(200, session()), git('https://github.com/acme/widgets', '', 'main'));
create('git_scp_url', json(200, session()), git('git@github.com:acme/widgets.git', 'feat', 'main'));
create('git_shorthand_owner_name', json(200, session()), git('acme/widgets', 'feat', 'main'));
create('git_gitlab_host', json(200, session()), git('https://gitlab.com/acme/widgets', 'feat', 'main'));
create('git_gitlab_subgroup_is_dropped', json(200, session()), git('https://gitlab.com/group/sub/widgets', 'feat', 'main'));
create('git_unparseable_url', json(200, session()), git('not a url', 'feat', 'main'));
create('git_empty_url', json(200, session()), git('', 'feat', 'main'));
for (const [label, url] of [
  ['upper_host', 'https://GitHub.com/acme/widgets'],
  ['http_scheme', 'http://github.com/acme/widgets'],
  ['ssh_scheme', 'ssh://git@github.com/acme/widgets.git'],
  ['git_scheme', 'git://github.com/acme/widgets.git'],
  ['port', 'https://github.com:8443/acme/widgets'],
  ['userinfo', 'https://user:pw@github.com/acme/widgets'],
  ['trailing_slash', 'https://github.com/acme/widgets/'],
  ['trailing_git_slash', 'https://github.com/acme/widgets.git/'],
  ['owner_only', 'https://github.com/acme'],
  ['three_segments', 'https://github.com/acme/widgets/extra'],
  ['query_fragment', 'https://github.com/acme/widgets?x=1#f'],
  ['shorthand_git_suffix', 'acme/widgets.git'],
  ['shorthand_space', 'acme/wid gets'],
  ['leading_slash', '/acme/widgets'],
  ['scp_no_suffix', 'git@github.com:acme/widgets'],
  ['scp_other_host', 'git@gitlab.example.com:acme/widgets.git'],
  ['scp_other_user', 'deploy@github.com:acme/widgets.git'],
  ['padded', '  https://github.com/acme/widgets  '],
  ['dot_name', 'https://github.com/acme/wid.gets'],
  ['dotdot_owner', 'https://github.com/../widgets'],
  ['file_scheme', 'file:///acme/widgets'],
  ['ftp_scheme', 'ftp://github.com/acme/widgets'],
  ['name_dot_git_only', 'https://github.com/acme/.git'],
  ['shorthand_dots', 'a.b/c.d'],
  ['shorthand_three', 'a/b/c'],
]) {
  create(`git_url_${label}`, json(200, session()), git(url, 'feat', 'main'));
}
create('git_with_everything', json(200, session()), { ...git('https://github.com/acme/widgets', 'feat', 'main'), tags: ['t'], model: 'm', sessionGroupingId: 'g', cwd: '/w' });
create('git_dedup_absent_first_drop', json(200, session()), { ...git('https://github.com/acme/widgets', 'main', 'main'), dedup: {} });
create('git_dedup_same_key_stays', json(200, session()), { ...git('https://github.com/acme/widgets', 'main', 'main'), dedup: { lastKey: 'is_default:https://github.com/acme/widgets:main' } });
create('git_dedup_other_key_replaced', json(200, session()), { ...git('https://github.com/acme/widgets', 'main', 'main'), dedup: { lastKey: 'other' } });
create('git_dedup_no_evidence_key', json(200, session()), { ...git('https://github.com/acme/widgets', 'feat'), dedup: { lastKey: null } });
create('git_dedup_cleared_without_drop', json(200, session()), { ...git('https://github.com/acme/widgets', 'feat', 'main'), dedup: { lastKey: 'stale' } });
create('git_dedup_untouched_without_git_context', json(200, session()), { dedup: { lastKey: 'stale' } });
create('git_dedup_cleared_for_unparseable_url', json(200, session()), { ...git('not a url', 'main', 'main'), dedup: { lastKey: 'stale' } });
create('git_dedup_cleared_for_empty_url', json(200, session()), { ...git('', 'main', 'main'), dedup: { lastKey: 'stale' } });
create('git_context_without_url_key', json(200, session()), { gitContext: { branch: 'feat' } });
create('refused_connection', null);
create('hangs_until_timeout', { hang: true }, { timeoutMs: 150 });
create('redirect_307_keeps_post', { redirect: 307 });

function startServer() {
  const state = { response: null, captured: null, port: 0 };
  const server = http.createServer((req, res) => {
    const chunks = [];
    req.on('data', (chunk) => chunks.push(chunk));
    req.on('end', () => {
      const body = Buffer.concat(chunks).toString('utf8');
      const rawHeaders = [];
      for (let index = 0; index < req.rawHeaders.length; index += 2) {
        const name = req.rawHeaders[index];
        const value = name.toLowerCase() === 'host' ? '<host>' : req.rawHeaders[index + 1];
        if (name.toLowerCase() !== 'content-length') rawHeaders.push([name, value]);
      }
      if (req.url.startsWith('/final')) {
        state.captured = { ...(state.captured ?? {}), final: { method: req.method, path: req.url, headers: rawHeaders, body } };
        const final = state.response.final ?? { status: 200, headers: [], body: state.response.finalBody ?? '{}' };
        res.writeHead(final.status, final.headers);
        res.end(final.body);
        return;
      }
      state.captured = { method: req.method, path: req.url, headers: rawHeaders, body };
      const response = state.response;
      if (response.hang) return;
      if (response.redirect) {
        const target = req.url.startsWith('/v1/code/sessions/') && req.url.endsWith('/bridge') ? '/final/bridge' : '/final/sessions';
        res.writeHead(response.redirect, { location: target });
        res.end();
        return;
      }
      res.writeHead(response.status, response.headers);
      res.end(response.body);
    });
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => {
    state.port = server.address().port;
    resolve({ server, state });
  }));
}

const { server, state } = await startServer();

function jsonSafe(value) {
  return JSON.parse(JSON.stringify(value === undefined ? null : value));
}

async function run(kind, spec) {
  const { response, args } = spec;
  state.response = response;
  state.captured = null;
  let base = `http://127.0.0.1:${state.port}`;
  if (response === null) base = 'http://127.0.0.1:1';
  if (response?.redirect && response.final === undefined) state.response = { ...response, finalBody: kind === 'credentials' ? JSON.stringify(ok()) : JSON.stringify(session()) };
  const timeoutMs = args.timeoutMs ?? 5000;
  if (args.entrypoint !== undefined) process.env.CLAUDE_CODE_ENTRYPOINT = args.entrypoint;
  let result;
  let dedupAfter;
  if (kind === 'credentials') {
    result = await bridge.fetchRemoteCredentials(args.sessionId ?? 'cse_abc123', base, 'access-token', timeoutMs, args.trustedDeviceToken);
  } else {
    const dedup = args.dedup === undefined ? undefined : { ...args.dedup };
    result = await bridge.createCodeSession(base, 'access-token', args.title ?? 'My session', timeoutMs, args.tags, args.gitContext, args.cwd, args.model, args.sessionGroupingId, dedup);
    if (dedup !== undefined) dedupAfter = 'lastKey' in dedup ? dedup.lastKey : '<absent>';
  }
  delete process.env.CLAUDE_CODE_ENTRYPOINT;
  const captured = state.captured;
  return {
    name: spec.name,
    args,
    response: response === null ? 'refused' : response.hang ? 'hang' : response,
    request: captured === null ? null : captured,
    result: jsonSafe(result),
    resultIsNull: result === null,
    ...(dedupAfter === undefined ? {} : { dedupAfter }),
  };
}

const goldens = { credentials: [], create: [] };
for (const spec of credentialCases) goldens.credentials.push(await run('credentials', spec));
for (const spec of createCases) goldens.create.push(await run('create', spec));
server.close();
fs.rmSync(cwdForGit, { recursive: true, force: true });

const section = (cases) => '[\n' + cases.map((c) => JSON.stringify(c)).join(',\n') + '\n]';
let text = '{\n' + Object.entries(goldens).map(([key, cases]) => JSON.stringify(key) + ': ' + section(cases)).join(',\n') + '\n}';
text = text.split(JSON.stringify(cwdForGit).slice(1, -1)).join('<cwd>');
text += '\n';
if (flags.includes('--check')) {
  if (fs.readFileSync(out, 'utf8') !== text) {
    process.stderr.write('bridge credentials goldens are stale\n');
    process.exit(1);
  }
} else {
  fs.writeFileSync(out, text);
}
