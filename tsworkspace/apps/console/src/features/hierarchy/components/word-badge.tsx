import { Chip } from '@/components/lifecycle-badge';

import { nodeWordLabels, type NodeWord } from '../domain/tree';

const titles: Record<NodeWord, string> = {
  organization: 'The root of the tree',
  workspace: 'A direct child of the organization',
  folder: 'Deeper in the tree than a workspace',
};

export function NodeWordBadge({ word }: { word: NodeWord }) {
  return <Chip title={titles[word]}>{nodeWordLabels[word]}</Chip>;
}
