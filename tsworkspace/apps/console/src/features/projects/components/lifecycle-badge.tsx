import { ProjectLifecycle } from '@trogonai/proto';

import { StatusPill, type Tone } from '@/components/lifecycle-badge';

import { projectLifecycleLabels } from '../domain/labels';

const tones: Record<ProjectLifecycle, Tone> = {
  [ProjectLifecycle.UNSPECIFIED]: 'neutral',
  [ProjectLifecycle.ACTIVE]: 'success',
  [ProjectLifecycle.ARCHIVED]: 'neutral',
};

export function ProjectLifecycleBadge({ lifecycle }: { lifecycle: ProjectLifecycle }) {
  return (
    <StatusPill
      tone={tones[lifecycle]}
      struck={lifecycle === ProjectLifecycle.ARCHIVED}
      label={projectLifecycleLabels[lifecycle]}
    />
  );
}
