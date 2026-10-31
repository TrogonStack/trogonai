import { createFileRoute } from '@tanstack/react-router';

import { KeysScreen } from '@/screens/keys-screen';

export const Route = createFileRoute('/keys/')({ component: KeysScreen });
