import { createFileRoute } from '@tanstack/react-router';

import { NewBackendScreen } from '@/screens/new-backend-screen';

export const Route = createFileRoute('/keys/backends/new')({ component: NewBackendScreen });
