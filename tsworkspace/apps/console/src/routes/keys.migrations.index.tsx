import { createFileRoute } from '@tanstack/react-router';

import { MigrationsScreen } from '@/screens/migrations-screen';

export const Route = createFileRoute('/keys/migrations/')({ component: MigrationsScreen });
