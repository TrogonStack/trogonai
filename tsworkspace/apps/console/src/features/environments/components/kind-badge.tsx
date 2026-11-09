import { Chip } from '@/components/lifecycle-badge';

export function EnvironmentKindBadge({ builtIn }: { builtIn: boolean }) {
  return (
    <Chip title={builtIn ? 'Provided by the platform' : 'Created in this project'}>
      {builtIn ? 'Built-in' : 'Custom'}
    </Chip>
  );
}
