// Documents read as a managed policy source. The policy-tolerant read (`lE(text, file, true)`) is recorded
// over these, so the Rust port is compared with what the pinned code produced for each one.
import { KEYED } from './settings_read_battery.mjs';

const VALUES = [true, false, 'true', 'false', 'yes', 0, 1, null, [], {}, ['a'], { a: 1 }, 'disable', 'x'];

const nest = (path, value) => path.reduceRight((inner, key) => ({ [key]: inner }), value);

const MODES = ['default', 'acceptEdits', 'bypassPermissions', 'plan', 'dontAsk', 'auto', 'manual', 'bogus', 5, null];

// Documents built from every restrictive entry of the pinned table, then hand written shapes around the
// blocks that withhold grants (`permissions`, `autoMode`) and the objects that coerce or reject their value.
export function policyDocuments(restrictive) {
  const documents = [];
  const seenParents = new Set();
  for (const { path } of restrictive) {
    for (const value of VALUES) documents.push(nest(path, value));
    for (let length = 1; length < path.length; length += 1) {
      const parent = path.slice(0, length);
      const name = parent.join('.');
      if (seenParents.has(name)) continue;
      seenParents.add(name);
      for (const value of VALUES) documents.push(nest(parent, value));
      documents.push(nest(parent, { unknownKey: 1 }));
      documents.push(nest(parent, { ...nest(path.slice(length), 'bogus'), unknownKey: 1 }));
    }
  }
  for (const mode of MODES) {
    documents.push({ permissions: { defaultMode: mode } });
    documents.push({ permissions: { defaultMode: mode, deny: ['Bash(rm:*)'] } });
    documents.push({ permissions: { defaultMode: mode, deny: [5], allow: ['Read'] } });
    documents.push({ permissions: { defaultMode: mode, ask: 'x', additionalDirectories: ['/a'] } });
  }
  documents.push(
    { permissions: { deny: [1, 'Bash'], ask: ['Read'], allow: ['Read'], additionalDirectories: ['/a'] } },
    { permissions: { deny: 'Bash', allow: ['Read'], defaultMode: 'acceptEdits' } },
    { permissions: { deny: ['Bash'], ask: [{}], allow: ['Read'], defaultMode: 'plan' } },
    { permissions: { allow: [1, 2], deny: ['Bash'] } },
    { permissions: { allow: ['Read', 5], additionalDirectories: [1], defaultMode: 'auto' } },
    { permissions: { disableBypassPermissionsMode: 'disable', disableAutoMode: false, denyInsteadOfPrompting: 'true' } },
    { permissions: { blockReadsOutsideWorkingDirectories: 'x', blockWritesOutsideWorkingDirectories: true, extra: 1 } },
    { permissions: { deny: [], ask: [], allow: [], additionalDirectories: [], defaultMode: 'default' } },
    { permissions: { unknown: 1 } },
    { permissions: {} },
    { autoMode: { allow: ['a', 1], soft_deny: [1], hard_deny: ['x'], environment: ['e'], classifyAllShell: 'true' } },
    { autoMode: { soft_deny: ['a', 2], allow: ['b'], environment: ['c'] } },
    { autoMode: { hard_deny: 'x', allow: ['b'], environment: ['c'] } },
    { autoMode: { deny: [1], allow: ['b'] } },
    { autoMode: { allow: [1], environment: [2] } },
    { autoMode: { classifyAllShell: 5, allow: ['a'] } },
    { autoMode: { allow: ['a'], unknown: 1 } },
    { worktree: { bgIsolation: 'bogus', location: 5, baseRef: 'x', symlinkDirectories: ['a'], sparsePaths: [1] } },
    { worktree: { bgIsolation: 'worktree', location: 'x' } },
    { worktree: { symlinkDirectories: [1, 'a'], sparsePaths: 'x' } },
    { attribution: true },
    { attribution: false },
    { attribution: 'x' },
    { attribution: { sessionUrl: 'x', commit: 'c', pr: 'p' } },
    { attribution: { sessionUrl: true, commit: 5 } },
    { attribution: { sessionUrl: 'true' } },
    { remoteControl: { shareHostProfile: 'basic' } },
    { remoteControl: { shareHostProfile: 'bogus', other: 1 } },
    { remoteTools: { allowUnattendedServing: 'true' } },
    { remoteTools: { allowUnattendedServing: 5, other: 1 } },
    { prependPlugins: ['a'], appendPlugins: ['b', 'c'] },
    { prependPlugins: ['a', 1], appendPlugins: 'x' },
    { prependPlugins: null, appendPlugins: [] },
    { permissions: { allow: ['Read'], deny: ['Bash'] }, worktree: { bgIsolation: 'none' }, attribution: { sessionUrl: 'x' } },
    { crossSessionInbound: 'bogus' },
    { crossSessionInbound: 'hold' },
    { crossSessionInbound: 5 },
    { strictPluginOnlyCustomization: ['skills', 'bogus', 5] },
    { strictPluginOnlyCustomization: ['bogus'] },
    { strictPluginOnlyCustomization: [] },
    { strictPluginOnlyCustomization: 'x' },
    { strictPluginOnlyCustomization: true },
    { strictPluginOnlyCustomization: false },
    { strictPluginOnlyCustomization: 'true' },
    { strictPluginOnlyCustomization: null },
    { wslInheritsWindowsSettings: 'true' },
    { wslInheritsWindowsSettings: 'false' },
    { wslInheritsWindowsSettings: 'x' },
    { wslInheritsWindowsSettings: 5 },
    { wslInheritsWindowsSettings: null },
    { managedSourcesBehavior: 'x', wslInheritsWindowsSettings: true },
    { model: 'x', env: { A: '1' }, unknownTop: { a: 1 }, disableAllHooks: 'x', enforceAvailableModels: 5 },
    { disableAutoMode: false, disableDeepLinkRegistration: 'false', allowManagedHooksOnly: 'true' },
    { disableAutoMode: 'disable', allowManagedMcpServersOnly: null, enableAllProjectMcpServers: 'true' },
    { enableArtifact: 'x', autoUploadSessions: 1, channelsEnabled: true, remoteControlAtStartup: 'false' },
    { maxEffortLevel: 'x', feedbackDrafts: 5, availableModelsMatch: 'x', askUserQuestionTimeout: 1, dialogExpiry: true },
    { maxEffortLevel: 'high', feedbackDrafts: 'off', availableModelsMatch: 'prefix', modelProposedGoals: 'bogus' },
  );
  documents.push(...KEY_DOCUMENTS);
  documents.push(...SANDBOX_DOCUMENTS);
  documents.push(...CREDENTIALS.map((credentials) => ({ sandbox: { credentials } })));
  return [...documents, ...KEYED];
}

const FILE = { path: '/a/token', mode: 'mask' };
const ENV = { name: 'TOKEN', mode: 'mask' };
const PAIR = { accessKeyIdVar: 'KEY', secretAccessKeyVar: 'SECRET' };
const CONVENTIONAL = { accessKeyIdVar: 'AWS_ACCESS_KEY_ID', secretAccessKeyVar: 'AWS_SECRET_ACCESS_KEY' };

// Shapes around every fallback of the managed credentials reader: entries kept as a deny, pairs kept as
// suppressors, shapes denied, and blocks that read only as stand-ins.
const CREDENTIALS = [
  5,
  'x',
  [],
  [FILE],
  true,
  {},
  { other: 1 },
  ...[
    [FILE, { path: '/a/b/', mode: 'mask' }],
    [{ path: '/a/b', mode: 'bogus' }, FILE],
    [{ path: '', mode: 'mask' }, { path: 5, mode: 'deny' }],
    [{ path: '/a/x', mode: 'mask', decode: 'bogus' }],
    [5, 'x', null],
    [],
    FILE,
    { path: '/a/x', mode: 'mask', decode: 'bogus' },
    { path: '/a/x', mode: 'bogus' },
    { path: 5 },
    5,
    'x',
    null,
  ].map((files) => ({ files })),
  ...[
    [ENV, { name: 'BAD-NAME', mode: 'mask' }],
    [{ name: 'A', mode: 'mask', extract: 'x' }],
    [{ name: 'B', mode: 'mask', extract: 'x' }, { name: 'C', mode: 'deny' }],
    [{ name: 'D', mode: 5 }, { name: 6, mode: 'deny' }],
    [],
    ENV,
    { name: 'E', mode: 'mask', extract: 'x' },
    { name: 'BAD-NAME', mode: 'deny' },
    5,
    null,
  ].map((envVars) => ({ envVars })),
  ...[true, false, 'true', 'false', 'x', null, 5, []].map((allowPlaintextInject) => ({ allowPlaintextInject })),
  ...[
    [PAIR],
    [CONVENTIONAL],
    [{ ...PAIR, sessionTokenVar: 'TOKEN' }],
    [{ ...PAIR, sessionTokenVar: 5 }],
    [{ ...CONVENTIONAL, sessionTokenVar: 5 }],
    [{ ...CONVENTIONAL, sessionTokenVar: 'AWS_ACCESS_KEY_ID' }],
    [{ accessKeyIdVar: 'AWS_ACCESS_KEY_ID', secretAccessKeyVar: 'aws_access_key_id' }],
    [{ accessKeyIdVar: 'AWS_ACCESS_KEY_ID' }],
    [{ accessKeyIdVar: 'AWS_ACCESS_KEY_ID', secretAccessKeyVar: 5, sessionTokenVar: null }],
    [{ accessKeyIdVar: 'BAD-NAME', secretAccessKeyVar: 'AWS_SECRET_ACCESS_KEY', sessionTokenVar: 'BAD-TOKEN' }],
    [{ accessKeyIdVar: 'KEY', secretAccessKeyVar: 'AWS_SESSION_TOKEN', sessionTokenVar: 'KEY' }],
    [{ accessKeyIdVar: '_INVALID_PAIR_KEY', secretAccessKeyVar: 'AWS_SECRET_ACCESS_KEY', sessionTokenVar: 5 }],
    [{ accessKeyIdVar: 'BAD-NAME', secretAccessKeyVar: 'SECRET' }],
    [{ sessionTokenVar: 'AWS_SESSION_TOKEN' }, { sessionTokenVar: 'AWS_SESSION_TOKEN', other: 1 }],
    [PAIR, { accessKeyIdVar: 'KEY', secretAccessKeyVar: 'OTHER' }],
    [5, 'x', null],
    [{ accessKeyIdVar: 5 }],
    [],
    [{ ...CONVENTIONAL, sessionTokenVar: 5 }, PAIR, { accessKeyIdVar: 'AWS_ACCESS_KEY_ID', secretAccessKeyVar: 1 }],
    PAIR,
    CONVENTIONAL,
    { accessKeyIdVar: 'AWS_ACCESS_KEY_ID' },
    { ...CONVENTIONAL, sessionTokenVar: 'AWS_SESSION_TOKEN' },
    { accessKeyIdVar: 'BAD-NAME', secretAccessKeyVar: 'SECRET' },
    { other: 1 },
    5,
    'x',
    null,
  ].map((awsPairs) => ({ awsPairs })),
  ...[
    { streaming: 'deny', presigned: 'passthrough' },
    { streaming: 'bogus', presigned: 'passthrough' },
    { streaming: 'bogus', presigned: null, sigv4a: 5 },
    { streaming: 'bogus', other: 1 },
    { other: 1 },
    {},
    5,
    null,
    [],
  ].map((sigv4) => ({ sigv4 })),
  { allowPlaintextInject: 'x', sigv4: 5 },
  { allowPlaintextInject: 'x', sigv4: 5, files: 5, envVars: null, awsPairs: 'x' },
  { allowPlaintextInject: 'x', sigv4: { presigned: 'deny' } },
  { allowPlaintextInject: false, sigv4: 5 },
  { files: [{ path: '/a/b', mode: 'bogus' }], awsPairs: [5], other: 1 },
  { files: [FILE], envVars: [ENV], allowPlaintextInject: true, awsPairs: [PAIR], sigv4: { streaming: 'passthrough' } },
];

// Sandbox blocks around the locks that are never substituted or left out of the skeleton.
const SANDBOX_DOCUMENTS = [
  { sandbox: { failIfUnavailable: 'x' } },
  { sandbox: { failIfUnavailable: 'x', enabled: true } },
  { sandbox: { failIfUnavailable: 'true', enabled: 'x' } },
  { sandbox: { enabled: 'x' } },
  { sandbox: { enabled: 'x', credentials: 5 } },
  { sandbox: { credentials: 5, network: 'x' } },
  { sandbox: { credentials: 5, autoAllowBashIfSandboxed: true } },
  { sandbox: { credentials: null, enabled: null, filesystem: null } },
  { sandbox: { network: { allowManagedDomainsOnly: 'x' }, filesystem: { disabled: 'x' } } },
  { sandbox: { network: { allowManagedDomainsOnly: 'x', allowedDomains: ['a'] }, excludedCommands: [1, 'a'] } },
  { sandbox: { bwrapPath: 5, socatPath: '/x', ripgrep: { command: 5 }, ignoreViolations: 'x', unknown: 1 } },
  { sandbox: 5 },
  { sandbox: [] },
  { sandbox: 'x' },
  { sandbox: {} },
  { sandbox: null },
  { sandbox: { enabled: true }, isolation: { required: 'x' } },
  { isolation: { required: 'x', persistHome: 5 } },
  { isolation: 5 },
];


const MARKETPLACES = [
  { source: 'github', repo: 'owner/repo' },
  { source: 'github', repo: 'owner/*' },
  { source: 'github', repo: '-owner/*' },
  { source: 'github', repo: '../*' },
  { source: 'github', repo: 'own*er/repo' },
  { source: 'github', repo: 'owner/repo', ref: 'v*' },
  { source: 'git', url: 'https://github.com/owner/*' },
  { source: 'git', url: 'https://github.com/owner/repo.git' },
  { source: 'git', url: 'https://example.com/owner/*' },
  { source: 'git', url: 'git@github.com:owner/*.git' },
  { source: 'git', url: 'git@github.com:owner/%2A' },
  { source: 'git', url: 'git@github.com:owner/%E0%A4%A' },
  { source: 'git', url: 'git@example.com:owner/*' },
  { source: 'git', url: 'ssh://git@ssh.github.com:443/owner/*' },
  { source: 'git', url: 'https://user:pw@GitHub.com/a/./b/../*/.git/' },
  { source: 'git', url: 'https://github.com/owner/%2A' },
  { source: 'git', url: 'https://github.com\\owner/*' },
  { source: 'git', url: 'https://[::1]/owner/*' },
  { source: 'git', url: 'git+ssh://github.com/owner/*' },
  { source: 'git', url: 'https://www.github.com/owner/*' },
  { source: 'git', url: 'https://github.com/o%ner/*' },
  { source: 'git', url: 'https://github.com/owner/repo', ref: 'main*' },
  { source: 'hostPattern', hostPattern: '^github\\.com$' },
  { source: 'hostPattern', hostPattern: '(' },
  { source: 'pathPattern', pathPattern: '[a-' },
  { source: 'pathPattern', pathPattern: '^/srv/.*' },
  { source: 'npm', package: 'x' },
  { source: 'url', url: 'https://example.com/m.json' },
  { source: 'bogus' },
  'owner/repo',
  5,
  null,
  { source: 'git', url: 'file:///owner/*' },
  { source: 'git', url: 'git://github.com/owner/*' },
  { source: 'git', url: 'ssh://git@github.com/owner/repo.git' },
  { source: 'git', url: 'github.com/owner/*' },
  { source: 'git', url: 'git@GITHUB.COM.:owner/*' },
  { source: 'git', url: 'git@[::1]:owner/*' },
  { source: 'git', url: 'user:pw@github.com:owner/*' },
  { source: 'git', url: 'https://github.com:443/owner/%2a' },
  { source: 'git', url: 'git+https://github.com/owner/*.git' },
  { source: 'git', url: 'ssh://git@[::1]:22/owner/*' },
  { source: 'git', url: 'ssh://git@gith%75b.com/owner/*' },
  { source: 'git', url: 'ssh://git@github.com:22/owner/%2A' },
  { source: 'git', url: 'ssh://git@github.com:99999/owner/*' },
  { source: 'git', url: 'ssh://git@githüb.com/owner/*' },
  { source: 'github', repo: 'owner/*', ref: 'main' },
];

const SERVERS = [
  { serverName: 'a' },
  { serverName: 'a b' },
  { serverCommand: ['node', 'x'] },
  { serverCommand: [] },
  { serverUrl: 'https://x/*' },
  { serverName: 'a', extra: 1 },
  { serverName: '' },
  { serverName: ' ' },
  { serverName: ' a' },
  { serverName: 'a', serverUrl: 'https://x' },
  { serverCommand: ['node', 5] },
  {},
  'a',
  5,
  null,
];

// `policyHelpers` entries read under an OS key and under the `default` key, around the static payload an
// invalid entry keeps.
const HELPER_ENTRIES = [
  null,
  5,
  'x',
  [],
  {},
  { path: '/usr/local/bin/helper' },
  { path: null },
  { path: null, script: null, defaultSettings: null },
  { path: '/a/b', defaultSettings: 5 },
  { path: '/a/b', defaultSettings: { path: 'x' } },
  { defaultSettings: 5 },
  { defaultSettings: { model: 'x' } },
  { defaultSettings: null },
  { path: '/a/b', script: 'echo', interpreter: 'sh', defaultSettings: { model: 'x' } },
  { path: '/a/b', script: 'echo', interpreter: 'sh', defaultSettings: { model: 'x' }, outputBehavior: 'merge' },
  { path: '/a/b', script: 'echo', interpreter: 'sh', defaultSettings: { model: 'x' }, outputBehavior: 'bogus' },
  { path: '/a/b', script: 'echo', interpreter: 'sh', defaultSettings: { model: 'x' }, outputBehavior: null },
  { path: '/a/b', script: 'echo', interpreter: 'sh', defaultSettings: null },
  { path: 'rel', timeoutMs: -1, defaultSettings: { model: 'x' }, outputBehavior: 'replace' },
  { script: 'echo' },
  { script: 'echo', interpreter: 'sh', onFailure: 'x', retries: 2, refreshIntervalMs: 'x' },
  { path: '/a/b', onFailure: 'continue', outputBehavior: 'merge', unknown: 1 },
  { model: 'x', policyHelper: { path: '/a' } },
  { model: 'x' },
];

const KEY_DOCUMENTS = [
  ...MARKETPLACES.map((entry) => ({ strictKnownMarketplaces: [entry] })),
  ...MARKETPLACES.map((entry) => ({ blockedMarketplaces: [entry] })),
  { strictKnownMarketplaces: MARKETPLACES },
  { blockedMarketplaces: MARKETPLACES },
  ...[null, 5, 'x', [], {}, MARKETPLACES[0]].flatMap((value) => [{ strictKnownMarketplaces: value }, { blockedMarketplaces: value }]),
  ...SERVERS.map((entry) => ({ allowedMcpServers: [entry] })),
  ...SERVERS.map((entry) => ({ deniedMcpServers: [entry] })),
  { allowedMcpServers: SERVERS, deniedMcpServers: SERVERS },
  ...[null, 5, 'x', [], {}, SERVERS[0]].flatMap((value) => [{ allowedMcpServers: value }, { deniedMcpServers: value }]),
  { managedMcpServers: { good: { command: 'node' }, 'bad name': { command: 'node' }, broken: 5, http: { type: 'http', url: 'https://x' } } },
  { managedMcpServers: { __proto__x: { command: 'a' }, constructor: { command: 'a' }, ok_1: { type: 'sse', url: 'u' } } },
  { managedMcpServers: { bad: 5 } },
  {
    managedMcpServers: {
      sse: { type: 'sse', url: 'https://x/sse', headers: { A: 'b' } },
      plain: { type: 'http', url: 'http://x' },
      exec: { type: 'http', url: 'https://x', command: 'node' },
      vars: { type: 'http', url: 'https://x', headers: { A: '${TOKEN}' } },
      control: { type: 'http', url: 'https://x', headers: { A: 'b\u0007' } },
      stdio: { command: 'node', args: ['x'] },
      deep: { type: 'http', url: 'https://x', oauth: { a: { b: { c: { d: { e: { f: '${X}' } } } } } } },
      hidden: { type: 'http', url: 'https://x', 'key\u200b': 'v' },
      host: { type: 'http', url: 'https:///path' },
    },
  },
  ...[null, 5, 'x', [], {}, [{ command: 'a' }]].map((value) => ({ managedMcpServers: value })),
  { enabledPlugins: { 'a@m': true, 'b@m': ['x'], 'c@m': 5, 'd@m': [1] } },
  { enabledPlugins: { 'c@m': 5 } },
  { enabledPlugins: { 'c@m': 5, 'a@m': false } },
  ...[null, 5, 'x', [], ['a@m'], {}].map((value) => ({ enabledPlugins: value })),
  ...[null, 5, 'x', [], ['a', 1, 'b'], {}].flatMap((value) => [{ availableModels: value }, { deniedModels: value }]),
  ...[null, 5, 'x', [], ['https://a', 1, null, 'b'], {}].flatMap((value) => [{ allowedHttpHookUrls: value }, { httpHookAllowedEnvVars: value }]),
  ...[
    null,
    5,
    'x',
    [],
    {},
    ['plugin@market', 'a@b@c', 'bad', '-x@m', 'x@-m', 'p.1@m_2', 5, null],
    [{ marketplace: 'm', plugin: 'p' }, { marketplace: 'm', plugin: 'p', extra: 1 }, { marketplace: 5, plugin: 'p' }, { plugin: 'p' }],
    'plugin@market',
  ].map((value) => ({ allowedChannelPlugins: value })),
  ...[null, 5, 'x', [], {}, ['anthropic', 'bedrock', 'vertex', 'anthropicFoo', 'anthropicfoo', 'anthropicFoo-bar', 'bogus', 5], 'anthropic'].map(
    (value) => ({ allowedProviders: value }),
  ),
  ...[null, 5, [], {}, ['10.0.0.0/8', 5], 'x', '10.0.0.0/8'].map((value) => ({ gatewayInternalNetworks: value })),
  ...[null, 5, 'x', '', true, [], {}, ['org-a', 5], ['org-a', 'org-b'], [null]].map((value) => ({ forceLoginOrgUUID: value })),
  ...[null, 5, 'x', [], {}, { path: '/usr/local/bin/helper' }, { path: 5 }, { path: '/a/b', extra: 1, timeoutMs: 5 }].map(
    (value) => ({ policyHelper: value }),
  ),
  // The singular reader only notes these fields (statusOnly) before the helper schema reads the value, so each
  // shape pins what the schema itself keeps or drops.
  ...[
    { path: '/a/b', defaultSettings: { model: 'x' }, default: { model: 'y' } },
    { path: '/a/b', linux: { path: '/c' }, policyHelper: { path: '/d' }, policyHelpers: {} },
    { path: '/a/b', claudeMd: 'x', outputBehavior: 'merge', onFailure: 'continue', retries: 2 },
    { defaultSettings: { model: 'x' } },
    { script: 'echo hi', interpreter: 'sh' },
    { path: null },
  ].map((value) => ({ policyHelper: value })),
  ...[null, 5, 'x', true, [], {}, { bogus: 1 }].map((value) => ({ policyHelpers: value })),
  ...HELPER_ENTRIES.flatMap((entry) => [{ policyHelpers: { linux: entry } }, { policyHelpers: { default: entry } }]),
  { policyHelpers: { windows: { path: 'C:\\h\\helper.exe' }, wsl: { script: 'echo \u00e9', interpreter: 'sh' } } },
  { policyHelpers: { windows: { script: 'echo \u00e9', interpreter: 'sh', defaultSettings: { model: 'x' } } } },
  { policyHelpers: { default: { model: 'y' }, macos: { path: '/a/b' }, bogus: 1, linux: null, wsl: 5 } },
  { policyHelpers: 5, feedbackDrafts: 1 },
  { policyHelpers: { linux: { path: '/a/b' } }, feedbackDrafts: 1 },
  { feedbackDrafts: 1, unknownTop: 1 },
  { feedbackDrafts: 1, managedSourcesBehavior: 'x', wslInheritsWindowsSettings: true },
  { feedbackDrafts: 1, attribution: 5, remoteControl: 5 },
  { feedbackDrafts: 1, permissions: {} },
  { attribution: { sessionUrl: 5, commit: 'c' } },
  { permissions: { deny: [5], defaultMode: 'acceptEdits', allow: ['Read'] }, strictPluginOnlyCustomization: 'x' },
];
