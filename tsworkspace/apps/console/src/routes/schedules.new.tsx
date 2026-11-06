import { createFileRoute } from '@tanstack/react-router';

import { NewScheduleScreen } from '@/screens/new-schedule-screen';
export const Route = createFileRoute('/schedules/new')({ component: NewScheduleScreen });
