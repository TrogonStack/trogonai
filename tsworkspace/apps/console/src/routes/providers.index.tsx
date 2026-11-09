import { createFileRoute } from '@tanstack/react-router';

import { ProvidersScreen } from '@/screens/providers-screen';

export const Route = createFileRoute('/providers/')({ component: ProvidersScreen });
