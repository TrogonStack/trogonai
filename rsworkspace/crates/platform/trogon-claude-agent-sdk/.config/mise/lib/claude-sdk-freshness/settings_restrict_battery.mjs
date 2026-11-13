// Settings documents the generator runs through the pinned upstream restriction of a user-writable policy file and
// records, so a Rust test can replay each one. The hand-written documents cover every branch of the restriction;
// the seeded documents mix the same keys with values of every JSON shape.
const STRICT = { allowManagedHooksOnly: true };

export const DOCUMENTS = [
  {},
  { model: 'x' },
  STRICT,
  { ...STRICT, model: 'x' },
  { allowManagedHooksOnly: false },
  { allowManagedHooksOnly: 'false' },
  { allowManagedPermissionRulesOnly: true, allowManagedMcpServersOnly: 'true' },
  { disableAllHooks: true },
  { disableAllHooks: true, allowManagedHooksOnly: true },
  { disableAllHooks: true, allowManagedHooksOnly: false },
  { disableAllHooks: 'disable' },
  { disableReplyCheck: true },
  { disableReplyCheck: 'disable', model: 'x' },
  { disableAutoMode: 'disable' },
  { disableAutoMode: true },
  { disableAutoMode: false },
  { disableSomethingNew: true, disableOther: 'disable', disableThird: false, disableFourth: 'yes' },
  { enableSomethingNew: false, enableOther: true, enableThird: 'false' },
  { enabledPlugins: false },
  { enabledPlugins: true },
  { enabledPlugins: null },
  { enabledPlugins: 'abc' },
  { enabledPlugins: [false, true, false] },
  { enabledPlugins: { 'a@m': false, 'b@m': true, 'c@builtin': false, 'd@BuiltIn': false, e: false, 'f@x@builtin': false } },
  { enabledPlugins: { 'a@builtin@m': false, '@builtin': false, 'g@inline': false, 'h@': false, '10': false, '2': false } },
  { enabledPlugins: { 'a@m': true } },
  { enableAllProjectMcpServers: false },
  { enableAllProjectMcpServers: true },
  { enableAllProjectMcpServers: 'false' },
  { enableArtifact: false, enableWorkflows: 'false' },
  { maxEffortLevel: 'low' },
  { maxEffortLevel: 'max' },
  { maxEffortLevel: 'ultra' },
  { crossSessionInbound: 'refuse' },
  { crossSessionInbound: 'hold' },
  { crossSessionInbound: 'accept' },
  { modelProposedGoals: 'alwaysAsk' },
  { remoteControl: { shareHostProfile: 'basic', other: 1 } },
  { remoteControl: { shareHostProfile: 'full' } },
  { feedbackDrafts: 'off', askUserQuestionTimeout: 'never', dialogExpiry: 'never' },
  { availableModelsMatch: 'exact' },
  { availableModelsMatch: 'prefix' },
  { availableModelsMatch: 'other' },
  { availableModels: ['a'], enforceAvailableModels: true, availableModelsMatch: 'prefix' },
  { enforceAvailableModels: false },
  { enforceAvailableModels: 'false' },
  { worktree: { bgIsolation: 'worktree', other: 'x' } },
  { worktree: { bgIsolation: 'none' } },
  { worktree: 'x' },
  { autoMode: { classifyAllShell: true } },
  { autoMode: { classifyAllShell: 'true' } },
  { remoteTools: { allowUnattendedServing: false } },
  { attribution: false },
  { attribution: { sessionUrl: false } },
  { attribution: { sessionUrl: true, commit: 'x' } },
  { attribution: { sessionUrl: 'false' } },
  { isolation: { required: true, persistHome: false } },
  { isolation: { required: false, persistHome: true } },
  { isolation: { required: 'true' } },
  { isolation: true },
  { allowedHttpHookUrls: ['https://a'] },
  { httpHookAllowedEnvVars: [] },
  { httpHookAllowedEnvVars: ['A'] },
  { allowedMcpServers: [{ serverName: 'a' }], deniedMcpServers: [], disabledMcpjsonServers: ['x'] },
  { allowedMcpServers: null, deniedMcpServers: false, allowedProviders: 'x' },
  { strictKnownMarketplaces: [], allowedMarketplaces: ['a'], blockedMarketplaces: [{ source: 'github' }] },
  { strictPluginOnlyCustomization: true },
  { strictPluginOnlyCustomization: false },
  { strictPluginOnlyCustomization: 'false' },
  { strictPluginOnlyCustomization: 'true' },
  { strictPluginOnlyCustomization: ['skills', 'other'] },
  { strictPluginOnlyCustomization: ['other'] },
  { strictPluginOnlyCustomization: [] },
  { strictPluginOnlyCustomization: [1, null] },
  { deniedModels: ['a'], availableModelsMatch: 'exact', enforceAvailableModels: true },
  { permissions: { deny: ['Bash'], ask: ['Edit'], allow: ['Read'] } },
  { permissions: { deny: ['Read(!x)', 'Read(./!x)', 'Edit(!x)', 'Edit(./!x)', 'Write(!x)', 'Read(.!x)', 'read(!x)', 1] } },
  { permissions: { ask: ['Read(!x)'], deny: [] } },
  { permissions: { deny: 'Read(!x)' } },
  { permissions: { deny: null, ask: false } },
  { permissions: { disableBypassPermissionsMode: 'disable', disableAutoMode: 'disable' } },
  { permissions: { disableBypassPermissionsMode: false, disableAutoMode: 'false' } },
  { permissions: { disableBypassPermissionsMode: true } },
  { permissions: { defaultMode: 'plan', denyInsteadOfPrompting: true, blockReadsOutsideWorkingDirectories: true } },
  { permissions: { blockWritesOutsideWorkingDirectories: 'true' } },
  { permissions: 'x' },
  { permissions: [] },
  { permissions: null },
  { sandbox: { enabled: true, failIfUnavailable: true, autoAllowBashIfSandboxed: false } },
  { sandbox: { enabled: false, allowUnsandboxedCommands: false, enableWeakerNestedSandbox: 'false' } },
  { sandbox: { filesystem: { denyRead: ['a'], denyWrite: ['b'], allowWrite: ['c'], allowManagedReadPathsOnly: true } } },
  { sandbox: { filesystem: { disabled: false, allowManagedReadPathsOnly: false } } },
  { sandbox: { filesystem: { denyRead: null } } },
  { sandbox: { network: { deniedDomains: ['a'], allowedDomains: ['b'], strictAllowlist: true, allowManagedDomainsOnly: false } } },
  { sandbox: { network: { allowAllUnixSockets: false, allowLocalBinding: 'false', allowManagedDomainsOnly: true } } },
  { sandbox: { network: 'x', filesystem: [] } },
  { sandbox: { credentials: { files: [{ path: 'a', mode: 'allow' }, { path: 1 }, 'b', null] } } },
  { sandbox: { credentials: { files: { path: 'a' } } } },
  { sandbox: { credentials: { files: 'a', envVars: [{ name: 'A' }, { name: 'B', mode: 'allow' }, { other: 1 }] } } },
  { sandbox: { credentials: { envVars: [] } } },
  { sandbox: { credentials: { envVars: null } } },
  { sandbox: { credentials: { allowPlaintextInject: false } } },
  { sandbox: { credentials: { awsPairs: [{ accessKeyIdVar: 'AWS_ACCESS_KEY_ID', secretAccessKeyVar: 'AWS_SECRET_ACCESS_KEY' }] } } },
  { sandbox: { credentials: { awsPairs: [{ accessKeyIdVar: 'AWS_SESSION_TOKEN' }, { secretAccessKeyVar: 'MY' }, 'x'] } } },
  { sandbox: { credentials: { awsPairs: { sessionTokenVar: 'AWS_SESSION_TOKEN' } } } },
  { sandbox: { credentials: { awsPairs: { other: 'AWS_SESSION_TOKEN' } } } },
  { sandbox: { credentials: { awsPairs: {} } } },
  { sandbox: { credentials: { awsPairs: [] } } },
  { sandbox: { credentials: { awsPairs: null } } },
  { sandbox: { credentials: { awsPairs: 'x' } } },
  { sandbox: { credentials: { awsPairs: false } } },
  { sandbox: { credentials: { sigv4: { streaming: 'deny', presigned: 'allow', sigv4a: 'deny', other: 'deny' } } } },
  { sandbox: { credentials: { sigv4: { streaming: 'allow' } } } },
  { sandbox: { credentials: { sigv4: ['deny', 'allow'] } } },
  { sandbox: { credentials: { sigv4: 'deny' } } },
  { sandbox: { credentials: { sigv4: true } } },
  { sandbox: { credentials: { sigv4: null } } },
  { sandbox: { credentials: 'x' } },
  { sandbox: { credentials: [] } },
  { sandbox: { enabled: true, credentials: { files: [{ path: 'a' }], sigv4: { streaming: 'deny' } }, network: { deniedDomains: [] } } },
  { sandbox: 'x' },
  { sandbox: null },
  { parentSettingsBehavior: 'merge' },
  { parentSettingsBehavior: 'first' },
  { parentSettingsBehavior: 'merge', model: 'x' },
  { pluginConfigs: { 'cc-plugin-sec-default@builtin': { options: { allowManagedModsOnly: true } } } },
  { pluginConfigs: { 'cc-plugin-sec-default@builtin': { options: { allowManagedModsOnly: false } } } },
  { pluginConfigs: { 'cc-plugin-sec-default@builtin': { options: { allowManagedModsOnly: 'x', other: 1 } } } },
  { pluginConfigs: { 'cc-plugin-sec-default@builtin': { options: { allowManagedModsOnly: ['a', 'b'] } } } },
  { pluginConfigs: { 'cc-plugin-sec-default@builtin': { options: { allowManagedModsOnly: ['a', 1] } } } },
  { pluginConfigs: { 'cc-plugin-sec-default@builtin': { options: { allowManagedModsOnly: [] } } } },
  { pluginConfigs: { 'cc-plugin-sec-default@builtin': { options: { allowManagedModsOnly: null } } } },
  { pluginConfigs: { 'cc-plugin-sec-default@builtin': { options: { allowManagedModsOnly: { a: 1 } } } } },
  { pluginConfigs: { 'cc-plugin-sec-default@builtin': { options: { allowManagedModsOnly: 0 } } } },
  { pluginConfigs: { 'sec-default@builtin': { options: { allowManagedModsOnly: 'old' } } } },
  {
    pluginConfigs: {
      'cc-plugin-sec-default@builtin': { options: { allowManagedModsOnly: 'new' } },
      'sec-default@builtin': { options: { allowManagedModsOnly: 'old' } },
    },
  },
  {
    pluginConfigs: {
      'sec-default@builtin': { options: { allowManagedModsOnly: true } },
      'cc-plugin-sec-default@builtin': { options: { other: 1 } },
    },
  },
  { pluginConfigs: { 'cc-plugin-sec-default@builtin': { options: 'allowManagedModsOnly' } } },
  { pluginConfigs: { 'cc-plugin-sec-default@builtin': { options: null } } },
  { pluginConfigs: { 'cc-plugin-sec-default@builtin': 'x', other: { options: { allowManagedModsOnly: true } } } },
  { pluginConfigs: 'x' },
  { fastModePerSessionOptIn: true, isolatePeerMachines: true, syncClaudeAiSkills: false, channelsEnabled: false },
  { skipDangerousModePermissionPrompt: false, autoUploadSessions: false, remoteControlAtStartup: 'false' },
  { autoContinueAtUsageLimit: false, responseComparisonEnabled: false, skipWebFetchPreflight: false },
  {
    disableAllHooks: true,
    allowManagedHooksOnly: true,
    allowedHttpHookUrls: [],
    httpHookAllowedEnvVars: [],
    permissions: { deny: ['Bash'] },
    sandbox: { enabled: true, filesystem: { denyRead: ['a'] }, credentials: { files: [{ path: 'a', mode: 'deny' }] } },
    pluginConfigs: { 'cc-plugin-sec-default@builtin': { options: { allowManagedModsOnly: true } } },
    enabledPlugins: { 'a@m': false },
    parentSettingsBehavior: 'merge',
  },
  { allowManagedHooksOnly: 1 },
  { allowManagedHooksOnly: 1.5, enforceAvailableModels: 0 },
  { permissions: { deny: ['Bash'], ask: ['Edit'] }, model: 'x' },
  { permissions: { deny: ['Bash'], ask: ['Edit'] } },
  { sandbox: { network: { deniedDomains: ['a'], strictAllowlist: true } } },
  { sandbox: { credentials: { files: [{ path: 'a', mode: 'deny' }] } } },
  { sandbox: { credentials: { files: [{ mode: 'deny', path: 'a' }] } } },
  { sandbox: { credentials: { envVars: [{ name: 'A', mode: 'deny' }] } } },
  { enabledPlugins: { 'a@m': false } },
  { enabledPlugins: { 'a@m': false, 'b@m': true } },
  { attribution: { sessionUrl: false }, permissions: { disableBypassPermissionsMode: 'disable' } },
];

// The keys the seeded documents draw from: every key the restriction reads, a few it never keeps, and nested paths.
const TOP_KEYS = [
  'allowManagedPermissionRulesOnly', 'allowManagedHooksOnly', 'allowManagedMcpServersOnly', 'enforceAvailableModels',
  'disableAllHooks', 'disableReplyCheck', 'disableAutoMode', 'disableDeepLinkRegistration', 'disableNewThing',
  'enableArtifact', 'enableNewThing', 'enabledPlugins', 'enableAllProjectMcpServers', 'syncClaudeAiSkills',
  'strictPluginOnlyCustomization', 'availableModels', 'availableModelsMatch', 'deniedModels', 'allowedProviders',
  'allowedMcpServers', 'deniedMcpServers', 'disabledMcpjsonServers', 'allowedHttpHookUrls', 'httpHookAllowedEnvVars',
  'strictKnownMarketplaces', 'allowedMarketplaces', 'blockedMarketplaces', 'maxEffortLevel', 'crossSessionInbound',
  'feedbackDrafts', 'parentSettingsBehavior', 'model', 'env', 'attribution', 'isolation', 'worktree', 'autoMode',
  'remoteControl', 'remoteTools', 'permissions', 'sandbox', 'pluginConfigs',
];
const NESTED_KEYS = {
  permissions: ['deny', 'ask', 'allow', 'defaultMode', 'disableBypassPermissionsMode', 'disableAutoMode', 'denyInsteadOfPrompting'],
  sandbox: ['enabled', 'failIfUnavailable', 'autoAllowBashIfSandboxed', 'filesystem', 'network', 'credentials', 'other'],
  filesystem: ['denyRead', 'denyWrite', 'allowWrite', 'allowManagedReadPathsOnly', 'disabled'],
  network: ['deniedDomains', 'allowedDomains', 'strictAllowlist', 'allowManagedDomainsOnly', 'allowLocalBinding'],
  credentials: ['files', 'envVars', 'awsPairs', 'sigv4', 'allowPlaintextInject', 'other'],
  sigv4: ['streaming', 'presigned', 'sigv4a', 'other'],
  awsPairs: ['accessKeyIdVar', 'secretAccessKeyVar', 'sessionTokenVar', 'other'],
  files: ['path', 'mode', 'other'],
  envVars: ['name', 'mode', 'other'],
  attribution: ['sessionUrl', 'commit'],
  isolation: ['required', 'persistHome'],
  worktree: ['bgIsolation'],
  autoMode: ['classifyAllShell'],
  remoteControl: ['shareHostProfile'],
  remoteTools: ['allowUnattendedServing'],
  enabledPlugins: ['a@m', 'b@builtin', 'c@BUILTIN', 'd', 'e@inline', '7'],
  pluginConfigs: ['cc-plugin-sec-default@builtin', 'sec-default@builtin', 'other@m'],
  'cc-plugin-sec-default@builtin': ['options', 'other'],
  'sec-default@builtin': ['options', 'other'],
  options: ['allowManagedModsOnly', 'other'],
};
const SCALARS = [
  true, false, 'true', 'false', 'disable', 'deny', 'allow', 'merge', 'prefix', 'exact', 'never', 'off', 'low', 'refuse',
  'worktree', 'skills', 'other', 'Read(!x)', 'Edit(./!x)', 'Bash', 'AWS_ACCESS_KEY_ID', 'AWS_SESSION_TOKEN', '', 0, 1,
  2.5, null,
];

// mulberry32: a small deterministic generator so every run records the same documents.
function generator(seed) {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export function seededDocuments(count, seed) {
  const random = generator(seed);
  const pick = (list) => list[Math.floor(random() * list.length)];
  const value = (key, depth) => {
    const roll = random();
    const nested = NESTED_KEYS[key];
    if (nested && depth < 4 && roll < 0.55) {
      if (roll < 0.12) return Array.from({ length: Math.floor(random() * 3) }, () => value(pick(nested), depth + 1));
      return object(nested, depth + 1);
    }
    if (roll < 0.8) return pick(SCALARS);
    if (roll < 0.92) return Array.from({ length: Math.floor(random() * 3) }, () => pick(SCALARS));
    return {};
  };
  const object = (keys, depth) => {
    const out = {};
    const size = 1 + Math.floor(random() * Math.min(keys.length, 4));
    for (let index = 0; index < size; index += 1) {
      const key = pick(keys);
      out[key] = value(key, depth);
    }
    return out;
  };
  return Array.from({ length: count }, () => object(TOP_KEYS, 0));
}
