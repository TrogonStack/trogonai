import { createFileRoute } from '@tanstack/react-router';

import { NewProviderScreen } from '@/screens/new-provider-screen';

export const Route = createFileRoute('/providers/new')({ component: NewProviderScreen });
