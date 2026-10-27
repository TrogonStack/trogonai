import { createFileRoute } from '@tanstack/react-router';

import { EnvironmentsScreen } from '@/screens/environments-screen';

export const Route = createFileRoute('/environments/')({ component: EnvironmentsScreen });
