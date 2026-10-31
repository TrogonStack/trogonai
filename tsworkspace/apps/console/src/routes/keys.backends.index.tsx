import { createFileRoute } from '@tanstack/react-router';

import { BackendsScreen } from '@/screens/backends-screen';

export const Route = createFileRoute('/keys/backends/')({ component: BackendsScreen });
