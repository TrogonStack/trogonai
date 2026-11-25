import { useNavigate } from '@tanstack/react-router';

import { BackLink } from '@/components/back-link';
import { PageHeader } from '@/components/page-header';
import { ScheduleForm } from '@/features/schedules/components/schedule-form';
import { commandFromDraft } from '@/features/schedules/model';
import { useCreateSchedule } from '@/features/schedules/queries';

export function NewScheduleScreen() {
  const mutation = useCreateSchedule();
  const navigate = useNavigate();
  return (
    <div className="flex flex-col gap-6">
      <BackLink to="/schedules">All schedules</BackLink>
      <PageHeader
        eyebrow="Automation"
        title="Create schedule"
        description="Configure timing, delivery and message content. Definitions are fixed after creation; pause, resume or remove them later."
      />
      <ScheduleForm
        pending={mutation.isPending}
        error={mutation.error}
        onSubmit={async (draft) => {
          try {
            const row = await mutation.mutateAsync(commandFromDraft(draft));
            await navigate({ to: '/schedules/$scheduleId', params: { scheduleId: row.scheduleId } });
          } catch {
            /* Mutation owns the visible error state. */
          }
        }}
      />
    </div>
  );
}
