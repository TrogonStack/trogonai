import type { ProjectAccessPreset } from '@trogonai/proto';

import { Chip } from '@/components/lifecycle-badge';

import { accessPresetDescriptions, accessPresetLabels } from '../domain/labels';

export function AccessPresetChip({ preset }: { preset: ProjectAccessPreset }) {
  return <Chip title={accessPresetDescriptions[preset]}>{accessPresetLabels[preset]}</Chip>;
}
