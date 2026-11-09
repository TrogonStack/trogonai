import { ProjectAccessPreset, ProjectLifecycle } from '@trogonai/proto';

export const projectLifecycleLabels: Record<ProjectLifecycle, string> = {
  [ProjectLifecycle.UNSPECIFIED]: 'Unspecified',
  [ProjectLifecycle.ACTIVE]: 'Active',
  [ProjectLifecycle.ARCHIVED]: 'Archived',
};

export const accessPresetLabels: Record<ProjectAccessPreset, string> = {
  [ProjectAccessPreset.UNSPECIFIED]: 'Unspecified',
  [ProjectAccessPreset.INHERITED]: 'Inherited',
  [ProjectAccessPreset.MEMBERS_ONLY]: 'Members only',
};

export const accessPresetDescriptions: Record<ProjectAccessPreset, string> = {
  [ProjectAccessPreset.UNSPECIFIED]: '',
  [ProjectAccessPreset.INHERITED]:
    'Whoever the parent level lets in can work here. Pick this when the project belongs to everyone who already has access above it.',
  [ProjectAccessPreset.MEMBERS_ONLY]:
    'Access stops at the project. Only people who hold a project membership get in, and you are added as the first member. Pick this for work you want to keep private.',
};

export const selectableAccessPresets = [ProjectAccessPreset.INHERITED, ProjectAccessPreset.MEMBERS_ONLY] as const;
