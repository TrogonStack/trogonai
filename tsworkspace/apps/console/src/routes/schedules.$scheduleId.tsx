import { createFileRoute } from '@tanstack/react-router';

import { ScheduleDetailScreen } from '@/screens/schedule-detail-screen';
export const Route = createFileRoute('/schedules/$scheduleId')({ component: Screen });
function Screen() {
  const { scheduleId } = Route.useParams();
  return <ScheduleDetailScreen key={scheduleId} scheduleId={scheduleId} />;
}
