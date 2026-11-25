import { createFileRoute } from '@tanstack/react-router';

import { SchedulesScreen } from '@/screens/schedules-screen';
export const Route = createFileRoute('/schedules/')({ component: SchedulesScreen });
