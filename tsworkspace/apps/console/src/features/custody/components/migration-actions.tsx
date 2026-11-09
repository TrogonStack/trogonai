import { PauseIcon, PlayIcon } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { useNotify } from '@/components/ui/toaster';

import type { MigrationView } from '../domain/view';
import { usePauseMigration, useResumeMigration } from '../queries';

export function MigrationActions({ view }: { view: MigrationView }) {
  const pause = usePauseMigration();
  const resume = useResumeMigration();
  const notify = useNotify();
  const { migration } = view;

  return (
    <div className="flex flex-wrap items-center gap-2">
      {migration.status === 'running' ? (
        <Button
          type="button"
          variant="outline"
          size="lg"
          disabled={pause.isPending}
          onClick={() =>
            pause.mutate(migration.id, {
              onSuccess: () => notify.success('Migration paused', migration.id),
              onError: (error) => notify.failure('Could not pause', error.message),
            })
          }
        >
          <PauseIcon data-icon="inline-start" />
          Pause
        </Button>
      ) : null}
      {migration.status === 'paused' ? (
        <Button
          type="button"
          size="lg"
          disabled={resume.isPending}
          onClick={() =>
            resume.mutate(migration.id, {
              onSuccess: () => notify.success('Migration resumed', migration.id),
              onError: (error) => notify.failure('Could not resume', error.message),
            })
          }
        >
          <PlayIcon data-icon="inline-start" />
          Resume
        </Button>
      ) : null}
    </div>
  );
}
