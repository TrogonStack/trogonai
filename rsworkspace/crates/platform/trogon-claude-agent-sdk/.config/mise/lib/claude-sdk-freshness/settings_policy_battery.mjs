// Documents read as a managed policy source. The policy-tolerant read (`lE(text, file, true)`) is recorded
// over these, so the Rust port is compared with what the pinned code produced for each one.
import { KEYED } from './settings_read_battery.mjs';

// Keys whose policy-tolerant handling lives in a later stage of the port; documents holding one are left out here.
export const LATER_STAGE_KEYS = new Set([
  'sandbox',
  'isolation',
  'policyHelper',
  'policyHelpers',
  'strictKnownMarketplaces',
  'blockedMarketplaces',
  'allowedMcpServers',
  'deniedMcpServers',
  'managedMcpServers',
  'enabledPlugins',
  'availableModels',
  'deniedModels',
  'allowedHttpHookUrls',
  'httpHookAllowedEnvVars',
  'allowedChannelPlugins',
  'gatewayInternalNetworks',
  'forceLoginOrgUUID',
  'allowedProviders',
]);

const VALUES = [true, false, 'true', 'false', 'yes', 0, 1, null, [], {}, ['a'], { a: 1 }, 'disable', 'x'];

const nest = (path, value) => path.reduceRight((inner, key) => ({ [key]: inner }), value);

const MODES = ['default', 'acceptEdits', 'bypassPermissions', 'plan', 'dontAsk', 'auto', 'manual', 'bogus', 5, null];

// Documents built from every restrictive entry of the pinned table, then hand written shapes around the
// blocks that withhold grants (`permissions`, `autoMode`) and the objects that coerce or reject their value.
export function policyDocuments(restrictive) {
  const documents = [];
  const seenParents = new Set();
  for (const { path } of restrictive) {
    if (LATER_STAGE_KEYS.has(path[0])) continue;
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
  const clean = KEYED.filter((document) => !Object.keys(document).some((key) => LATER_STAGE_KEYS.has(key)));
  return [...documents, ...clean];
}
