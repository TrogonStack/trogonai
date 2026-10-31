import { createFileRoute } from '@tanstack/react-router';

import { SourcesScreen } from '@/screens/sources-screen';

export const Route = createFileRoute('/sources/')({ component: SourcesScreen });
